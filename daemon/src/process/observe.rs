//! Observe validates ordered native output and exposes only Provider-public facts.

use super::{
    provider_events::{Stage, climb, provider_event_with_activity},
    turn::TurnOutcome,
};
use crate::{
    execution::AgentEvent,
    host::AgentRun,
    provider::Provider,
    state::{OwnerStop, TurnStop},
    work::{MAX_PROTECTED_BYTES, MAX_PROTECTED_ITEMS},
};

/// One incomplete untrusted JSONL record may not exhaust ArchiGoat memory. Past this the record is
/// released and decoding resumes at the next line; the turn producing it keeps running.
const MAX_JSONL_LINE_BYTES: usize = 1024 * 1024;

/// A turn remembers this many message identities, enough for any turn a person reads in one sitting.
const MAX_ANSWER_IDS: usize = 256;

/// A successful native turn carries only the session, answer, and terminal proof needed for Done.
pub(crate) struct ObservedWork {
    /// Session binding prevents another native conversation from satisfying this Work.
    pub(crate) native_session: String,
    /// Answer remains Provider-native until the egress gate accepts one bounded delivery line.
    pub(crate) answer: Option<String>,
    /// The exact terminal event gates runner cleanup after durable delivery.
    pub(crate) terminal_sequence: u64,
}

/// Observer retains decoded session, answer, and progress while a runner is safely reattached.
pub(crate) struct Observer {
    provider: Provider,
    decoder: Decoder,
    sequence: u64,
    terminal_failure: bool,
    stopped: bool,
    rotated: bool,
}

// This observer streams native process facts without retaining an unbounded history.
impl Observer {
    /// New starts at the first durable journal frame for one Provider-owned Work.
    pub(crate) fn new(provider: Provider, progress_sequence: usize) -> Self {
        Self {
            provider,
            decoder: Decoder::new(progress_sequence),
            sequence: 0,
            terminal_failure: false,
            stopped: false,
            rotated: false,
        }
    }

    /// Sequence lets recovery back off only when the same journal position keeps failing.
    pub(crate) fn sequence(&self) -> u64 {
        self.sequence
    }

    /// TerminalFailure prevents pointless replay after the native journal has physically ended.
    pub(crate) fn terminal_failure(&self) -> bool {
        self.terminal_failure
    }

    /// Stopped reports a runner that ended itself so liveness can repair the Work.
    pub(crate) fn stopped(&self) -> bool {
        self.stopped
    }

    /// Rotated reports the requested turn boundary without borrowing owner Stop.
    pub(crate) fn rotated(&self) -> bool {
        self.rotated
    }

    /// NativeSession exposes only Provider-emitted identity for safe repair continuation.
    pub(crate) fn native_session(&self) -> Option<&str> {
        self.decoder.native_session.as_deref()
    }

    /// PrivateOutputs exposes bounded evidence only to durable Work state.
    pub(crate) fn private_outputs(&self) -> &[String] {
        &self.decoder.private_outputs
    }

    /// Observe consumes one already-owned journal until verified completion, owner Stop, or repair.
    pub(crate) async fn observe(
        &mut self,
        run: &mut AgentRun,
        stop: OwnerStop,
        turn: TurnStop,
        mut on_update: impl FnMut(usize, String) -> Result<(), String>,
        mut on_session: impl FnMut(String) -> Result<(), String>,
        mut on_tokens: impl FnMut(u64) -> Result<(), String>,
        mut on_model: impl FnMut(String) -> Result<(), String>,
        mut on_private: impl FnMut(&str) -> Result<(), String>,
        mut on_answer: impl FnMut(Option<&str>, &str) -> Result<(), String>,
    ) -> Result<Option<ObservedWork>, String> {
        // Reattached journals replay from frame one; decoded frames remain immutable and are skipped.
        loop {
            let frame = run.next(stop.clone(), turn.clone()).await?;
            if frame.sequence <= self.sequence {
                continue;
            }
            // Frames missing between the last one and this one are frames this journal lost, never a
            // Work that ended. The gap is named once and the turn keeps being observed.
            if frame.sequence > self.sequence + 1 {
                crate::trace::line(&format!(
                    "{} lost {} journal frame(s) before frame {}; this turn keeps running",
                    self.provider,
                    frame.sequence - self.sequence - 1,
                    frame.sequence
                ));
            }
            self.sequence = frame.sequence;
            match frame.event {
                AgentEvent::Stdout(bytes) => {
                    let session = self.decoder.native_session.clone();
                    self.decoder.push(
                        self.provider,
                        &bytes,
                        &mut on_update,
                        &mut on_tokens,
                        &mut on_model,
                        &mut on_private,
                        &mut on_answer,
                    );
                    if self.decoder.native_session != session
                        && let Some(native_session) = self.decoder.native_session.clone()
                    {
                        commit("the Provider's own session", on_session(native_session));
                    }
                }
                // Diagnostics stay local so machine details and secrets never reach the result view.
                AgentEvent::Stderr => {}
                // A quiet report from a previous generation of the runner says a command was quiet,
                // and quiet is not death: it names nothing about whether this turn is alive.
                AgentEvent::Stalled => {}
                AgentEvent::Done => {
                    // A process exit alone cannot claim user success without Provider-native completion.
                    self.terminal_failure = true;
                    self.decoder.finish(
                        self.provider,
                        &mut on_update,
                        &mut on_tokens,
                        &mut on_model,
                        &mut on_private,
                        &mut on_answer,
                    );
                    if self.decoder.turn.failed() {
                        // PHYSICS: the Provider process is gone and reported its own cause for it.
                        return Err(self
                            .decoder
                            .turn
                            .failure()
                            .unwrap_or("Local Agent could not complete this turn")
                            .to_owned());
                    }
                    if !self.decoder.turn.completed() {
                        // PHYSICS: the Provider process is gone without finishing its turn.
                        return Err("Local Agent ended without native completion".to_owned());
                    }
                    self.terminal_failure = false;
                    // A turn the Provider finished delivers. A stream that carried the answer but
                    // not the session header binds nothing new: the session this Work is already
                    // bound to is the session that produced this, and the product goes out.
                    return Ok(Some(ObservedWork {
                        native_session: self.decoder.native_session.clone().unwrap_or_default(),
                        answer: self.decoder.answer.clone(),
                        terminal_sequence: frame.sequence,
                    }));
                }
                // Owner authority turns this runner's stop into the publicly requested Stopped state.
                AgentEvent::Stopped if stop.requested() => return Ok(None),
                // A queued follow-up ends only this native turn; the Work stays Running.
                AgentEvent::Stopped if turn.requested() => {
                    self.rotated = true;
                    return Ok(None);
                }
                // A runner that stopped itself ended this turn; the Work decides how it continues,
                // and a stop this machine imposed says so in its own words.
                AgentEvent::Stopped => {
                    self.stopped = true;
                    return Ok(None);
                }
            }
        }
    }
}

/// Decoder preserves one Provider turn across arbitrary OS frame boundaries without inventing events.
struct Decoder {
    pending: Vec<u8>,
    native_session: Option<String>,
    next_progress: usize,
    answer: Option<String>,
    private_outputs: Vec<String>,
    private_output_bytes: usize,
    /// ReleasedOutput reports once that this turn produced more native output than is retained.
    released_output: bool,
    tokens: u64,
    /// The furthest step this turn reached keeps its public stages moving forward only.
    reached: Stage,
    /// The step this turn last named stays unrepeated however many frames report it again.
    named: Option<String>,
    /// Every message identity this turn already published, so a resent item is still one message.
    published: Vec<String>,
    /// Turn is the single settled outcome of this native turn, however its stream behaved.
    turn: TurnOutcome,
    /// Releasing skips the tail of one record too large to hold, up to its own newline.
    releasing: bool,
}

// This decoder extracts provider events incrementally from native process output.
impl Decoder {
    /// New starts progress after a continuation or from frame one during journal reattachment.
    fn new(next_progress: usize) -> Self {
        Self {
            pending: Vec::new(),
            native_session: None,
            next_progress,
            answer: None,
            private_outputs: Vec::new(),
            private_output_bytes: 0,
            released_output: false,
            tokens: 0,
            reached: Stage::Designing,
            named: None,
            published: Vec::new(),
            turn: TurnOutcome::new(),
            releasing: false,
        }
    }

    /// Preserve JSONL records split across OS frames so Provider meaning is never truncated. One
    /// record too large to hold is released at its own newline: memory stays bounded, and what the
    /// Provider says next still reaches the person who asked.
    fn push(
        &mut self,
        provider: Provider,
        bytes: &[u8],
        on_update: &mut impl FnMut(usize, String) -> Result<(), String>,
        on_tokens: &mut impl FnMut(u64) -> Result<(), String>,
        on_model: &mut impl FnMut(String) -> Result<(), String>,
        on_private: &mut impl FnMut(&str) -> Result<(), String>,
        on_answer: &mut impl FnMut(Option<&str>, &str) -> Result<(), String>,
    ) {
        self.pending.extend_from_slice(bytes);
        while let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line = self.pending.drain(..=end).collect::<Vec<_>>();
            if self.releasing {
                self.releasing = false;
                continue;
            }
            self.line(
                provider,
                trim_line(&line),
                on_update,
                on_tokens,
                on_model,
                on_private,
                on_answer,
            );
        }
        if self.pending.len() > MAX_JSONL_LINE_BYTES {
            crate::trace::line(&format!(
                "released one {provider} record too large to hold; this turn keeps decoding"
            ));
            self.pending.clear();
            self.releasing = true;
        }
    }

    /// Read the final record a runner journaled without a newline; bytes that are not a record at
    /// all are the tail of a stream that stopped mid-word, so they are released, not held against
    /// the turn that produced them. A complete trailing record still carries its full meaning.
    fn finish(
        &mut self,
        provider: Provider,
        on_update: &mut impl FnMut(usize, String) -> Result<(), String>,
        on_tokens: &mut impl FnMut(u64) -> Result<(), String>,
        on_model: &mut impl FnMut(String) -> Result<(), String>,
        on_private: &mut impl FnMut(&str) -> Result<(), String>,
        on_answer: &mut impl FnMut(Option<&str>, &str) -> Result<(), String>,
    ) {
        if self.pending.is_empty() || self.releasing {
            return;
        }
        let line = std::mem::take(&mut self.pending);
        let line = trim_line(&line);
        self.line(
            provider, line, on_update, on_tokens, on_model, on_private, on_answer,
        );
    }

    /// Project only authenticated runner output through the Provider's public event grammar.
    fn line(
        &mut self,
        provider: Provider,
        line: &[u8],
        on_update: &mut impl FnMut(usize, String) -> Result<(), String>,
        on_tokens: &mut impl FnMut(u64) -> Result<(), String>,
        on_model: &mut impl FnMut(String) -> Result<(), String>,
        on_private: &mut impl FnMut(&str) -> Result<(), String>,
        on_answer: &mut impl FnMut(Option<&str>, &str) -> Result<(), String>,
    ) {
        if line.is_empty() {
            return;
        }
        // Bytes that are not a record are one lost line, not a defect in the turn that wrote them:
        // the rest of this frame still carries everything the Agent said.
        let Ok(value) = serde_json::from_slice(line) else {
            crate::trace::line(&format!(
                "released {} unreadable {provider} bytes; this turn keeps its own meaning",
                line.len()
            ));
            return;
        };
        let event = provider_event_with_activity(provider, &value, self.reached.building());
        // A Provider that answers with a conversation of its own naming rebinds to it; the durable
        // binding is the one authority on which session this Work holds.
        if let Some(session) = event.native_session {
            self.native_session = Some(session);
        }
        // A turn shows the work it is doing now, so its steps only move forward and each is named once.
        if let Some(stage) = event.stage {
            let update = climb(stage, &mut self.reached).label();
            if self.named.as_deref() != Some(update.as_str()) {
                commit(
                    "this turn's step",
                    on_update(self.next_progress, update.clone()),
                );
                self.next_progress += 1;
                self.named = Some(update);
            }
        }
        // Cumulative tokens replay deterministically, so recovery re-derives the same public count.
        if let Some(delta) = event.tokens {
            self.tokens += delta;
            commit("this turn's token count", on_tokens(self.tokens));
        }
        // The executing model is a Provider-public identity fact, published as soon as it is known.
        if let Some(model) = event.model {
            commit("the executing model", on_model(model));
        }
        for output in event.private_output {
            commit("this turn's native output", on_private(&output));
            if self.private_outputs.contains(&output) {
                continue;
            }
            if self.private_outputs.len() < MAX_PROTECTED_ITEMS
                && self.private_output_bytes.saturating_add(output.len()) <= MAX_PROTECTED_BYTES
            {
                self.private_output_bytes += output.len();
                self.private_outputs.push(output);
            } else if !self.released_output {
                self.released_output = true;
                eprintln!(
                    "Product kept {} bytes of this turn's native output and released the rest",
                    self.private_output_bytes
                );
            }
        }
        // Every Provider answer frame becomes durable public text before terminal completion.
        if let Some(answer) = event.answer {
            // One message is one message. Its closing frame carries what already streamed, and a
            // Provider that reopened its stream resends the very item it had already sent under
            // that item's own identity; neither is a second thing the Agent said.
            let resent = event
                .answer_id
                .as_deref()
                .is_some_and(|id| self.published.iter().any(|seen| seen == id));
            let repeat = (event.completed || resent)
                && self
                    .answer
                    .as_deref()
                    .is_some_and(|current| current.trim_end().ends_with(answer.trim_end()));
            if !repeat {
                commit(
                    "what the Agent said",
                    on_answer(event.answer_id.as_deref(), &answer),
                );
                append_answer(self.answer.get_or_insert_with(String::new), &answer);
                if let Some(id) = event.answer_id.filter(|_| !resent) {
                    if self.published.len() == MAX_ANSWER_IDS {
                        self.published.remove(0);
                    }
                    self.published.push(id);
                }
            }
        }
        // Native failure is settled only after the full durable journal is consumed, and this
        // turn's own completion outranks any stream failure it recovered from along the way.
        self.turn
            .observe(event.completed, event.failed, event.failure);
    }
}

/// Commit records a display or evidence write that did not land. Bookkeeping never outranks the Work:
/// the journal is replayed from its first frame on every reattachment, so the write gets another
/// chance there, and nothing a person is waiting on ends because a bookkeeping write needed one.
fn commit(what: &str, written: Result<(), String>) {
    if let Err(error) = written {
        crate::trace::line(&format!("{what} is not committed yet: {error}"));
    }
}

/// AppendAnswer is the one rule every surface folds a Provider answer frame by, and it reports what
/// that frame actually added. A frame is monotonic when it extends what is already there, and it adds
/// nothing when this answer already holds it — wherever in this answer it already sits, because a
/// journal replayed from frame one says every message of the turn again, not only its last. So a
/// restart mid-turn and a rotation resumed after one never say the Agent's own words twice.
pub(crate) fn append_answer(answer: &mut String, candidate: &str) -> bool {
    if candidate.is_empty() {
        return false;
    }
    let delta = if candidate.starts_with(answer.as_str()) {
        &candidate[answer.len()..]
    } else if answer.contains(candidate) {
        return false;
    } else {
        candidate
    };
    if delta.is_empty() {
        return false;
    }
    answer.push_str(delta);
    true
}

/// Remove transport delimiters only; Provider content bytes otherwise remain unchanged.
fn trim_line(mut bytes: &[u8]) -> &[u8] {
    while bytes
        .last()
        .is_some_and(|byte| matches!(byte, b'\r' | b'\n'))
    {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}
