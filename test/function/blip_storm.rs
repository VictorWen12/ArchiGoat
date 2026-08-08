//! A stream blip must cost seconds, never the Work. This drives the shipped observer, the shipped
//! Provider grammar, and the shipped drive-loop recovery shape over hostile native journals: frames
//! that arrive after a turn already finished, journals cut off mid-record, retries that repeat a
//! message the Work already published, and the same journal replayed from frame one.

// The production sources under test compile here unchanged; only their platform seams are scripted.
#![allow(dead_code)]

#[path = "../../daemon/src/execution.rs"]
mod execution;
#[path = "../../daemon/src/provider/mod.rs"]
mod provider;
#[path = "../../daemon/src/trace.rs"]
mod trace;

// Process keeps the real module shape, so the observer resolves its own event grammar.
#[path = "."]
mod process {
    #[path = "../../daemon/src/process/observe.rs"]
    pub(crate) mod observe;
    #[path = "../../daemon/src/process/provider_events.rs"]
    pub(crate) mod provider_events;
    #[path = "../../daemon/src/process/turn.rs"]
    pub(crate) mod turn;
}

// Work supplies only the evidence bounds the decoder reads; no bound is under test here.
mod work {
    pub(crate) const MAX_PROTECTED_ITEMS: usize = 64;
    pub(crate) const MAX_PROTECTED_BYTES: usize = 256 * 1024;
    pub(crate) const MAX_PROTECTED_ITEM_BYTES: usize = 64 * 1024;
    pub(crate) mod runtime {
        pub(crate) const NETWORK: bool = true;
    }
}

// Proof and version exist for Provider code this check does not exercise.
mod proof {
    pub(crate) fn valid_nonce(_name: &str) -> bool {
        true
    }
}

fn version() -> &'static str {
    "test"
}

// State scripts owner and follow-up authority, so a turn boundary can be driven from a test.
mod state {
    use std::sync::{Arc, atomic::AtomicBool, atomic::Ordering};

    #[derive(Clone, Default)]
    pub(crate) struct OwnerStop(Arc<AtomicBool>);

    impl OwnerStop {
        pub(crate) fn requested(&self) -> bool {
            self.0.load(Ordering::Acquire)
        }
    }

    #[derive(Clone, Default)]
    pub(crate) struct TurnStop(Arc<AtomicBool>);

    impl TurnStop {
        pub(crate) fn queued() -> Self {
            Self(Arc::new(AtomicBool::new(true)))
        }

        pub(crate) fn requested(&self) -> bool {
            self.0.load(Ordering::Acquire)
        }
    }
}

// Host replays one already-durable journal, exactly as the runner would have written it.
mod host {
    use crate::{
        execution::AgentFrame,
        state::{OwnerStop, TurnStop},
    };

    pub(crate) struct AgentRun(std::vec::IntoIter<AgentFrame>);

    impl AgentRun {
        pub(crate) fn scripted(frames: Vec<AgentFrame>) -> Self {
            Self(frames.into_iter())
        }

        /// No machine took these runners down; every turn here ends on its own native evidence.
        pub(crate) fn machine_stop_cause(&self) -> Option<String> {
            None
        }

        pub(crate) async fn next(
            &mut self,
            _stop: OwnerStop,
            _turn: TurnStop,
        ) -> Result<AgentFrame, String> {
            self.0
                .next()
                .ok_or_else(|| "the scripted journal ended without a terminal frame".to_owned())
        }
    }
}

use execution::{AgentEvent, AgentFrame};
use process::observe::{ObservedWork, Observer};
use provider::Provider;

/// The exact notice the Codex CLI prints while it reopens its own model stream.
const RECONNECT: &str = r#"{"type":"error","message":"Reconnecting... 5/5 (request timed out)"}"#;
const STARTED: &str = r#"{"type":"thread.started","thread_id":"thread-1","model":"gpt-5"}"#;
const BUILDING: &str = r#"{"type":"item.started","item":{"id":"i1","type":"file_change"}}"#;
const SHELL: &str = r#"{"type":"item.completed","item":{"id":"c1","type":"command_execution","aggregated_output":"ok"}}"#;
const FIRST: &str = r#"{"type":"item.completed","item":{"type":"agent_message","id":"m1","text":"Adding the jump."}}"#;
const ANSWER: &str = r#"{"type":"item.completed","item":{"type":"agent_message","id":"m2","text":"Your runner game is ready."}}"#;
const CLOSING: &str = r#"{"type":"item.completed","item":{"type":"agent_message","id":"m3","text":"Tell me if the jump feels too high."}}"#;
const COMPLETED: &str = r#"{"type":"turn.completed","usage":{"output_tokens":42}}"#;
const USAGE_LIMIT: &str = r#"{"type":"error","message":"You've hit your usage limit."}"#;

/// Piece is one durable journal frame, so a test can cut a record off exactly where a death would.
#[derive(Clone)]
enum Piece {
    /// One complete Provider record, newline terminated, as the runner journals it.
    Line(&'static str),
    /// Bytes the runner journaled with no terminating newline: the shape of a stream cut short.
    Cut(&'static str),
    /// A report that a command produced nothing, as an earlier generation of the runner wrote it.
    Quiet(&'static str),
    /// The physical end of the native process.
    Done,
    /// A runner that ended itself.
    Stopped,
}

/// Settled is the drive loop's own vocabulary for how one observation epoch ended.
#[derive(Debug, PartialEq)]
enum Settled {
    /// The Work delivered: the answer text and native session it committed.
    Delivered(String),
    /// The Work threw the turn away and asked for another runner with this cause.
    Repaired(String),
    /// The Work ended this native turn to arm a queued follow-up.
    Rotated,
    /// The driver never settled within a bounded number of reattachments.
    Wedged,
}

/// Watched records every public change one observation published, so nothing may double unseen.
#[derive(Default)]
struct Watched {
    stages: Vec<(usize, String)>,
    answers: Vec<(Option<String>, String)>,
    sessions: Vec<String>,
}

/// Drive runs the shipped recovery shape: a non-terminal decode fault reattaches the same observer
/// to the same journal from frame one, exactly as work_state::drive::observe_until_terminal does.
async fn drive(pieces: &[Piece]) -> (Settled, Watched) {
    drive_with(pieces, state::TurnStop::default()).await
}

async fn drive_with(pieces: &[Piece], turn: state::TurnStop) -> (Settled, Watched) {
    let stop = state::OwnerStop::default();
    let mut observer = Observer::new(Provider::Codex, 0);
    let mut watched = Watched::default();
    // The real loop reattaches without a bound; eight epochs is far past any healthy journal.
    for _ in 0..8 {
        let mut run = host::AgentRun::scripted(frames(pieces));
        let outcome = observe(&mut observer, &mut run, &stop, &turn, &mut watched).await;
        match outcome {
            Ok(Some(observed)) => {
                return (
                    Settled::Delivered(observed.answer.unwrap_or_default()),
                    watched,
                );
            }
            Ok(None) if observer.rotated() => return (Settled::Rotated, watched),
            Ok(None) if observer.stopped() => {
                return (
                    Settled::Repaired("Your Agent stopped without delivering".to_owned()),
                    watched,
                );
            }
            Ok(None) => {}
            Err(cause) => {
                // A terminal frame proves the old runner is gone, so this Work stops replaying.
                if observer.terminal_failure() {
                    return (Settled::Repaired(cause), watched);
                }
            }
        }
    }
    (Settled::Wedged, watched)
}

/// Observe runs one epoch of the shipped observer and records every public change it published.
async fn observe(
    observer: &mut Observer,
    run: &mut host::AgentRun,
    stop: &state::OwnerStop,
    turn: &state::TurnStop,
    watched: &mut Watched,
) -> Result<Option<ObservedWork>, String> {
    let stages = std::cell::RefCell::new(Vec::new());
    let answers = std::cell::RefCell::new(Vec::new());
    let sessions = std::cell::RefCell::new(Vec::new());
    let settled = observer
        .observe(
            run,
            stop.clone(),
            turn.clone(),
            |index, label| {
                stages.borrow_mut().push((index, label));
                Ok(())
            },
            |session| {
                sessions.borrow_mut().push(session);
                Ok(())
            },
            |_| Ok(()),
            |_| Ok(()),
            |_| Ok(()),
            |id, answer| {
                answers
                    .borrow_mut()
                    .push((id.map(str::to_owned), answer.to_owned()));
                Ok(())
            },
        )
        .await;
    watched.stages.extend(stages.into_inner());
    watched.answers.extend(answers.into_inner());
    watched.sessions.extend(sessions.into_inner());
    settled
}

/// Frames turns one script into the ordered durable journal a runner would have written.
fn frames(pieces: &[Piece]) -> Vec<AgentFrame> {
    pieces
        .iter()
        .enumerate()
        .map(|(index, piece)| AgentFrame {
            sequence: index as u64 + 1,
            event: match piece {
                Piece::Line(line) => AgentEvent::Stdout(format!("{line}\n").into_bytes()),
                Piece::Cut(bytes) => AgentEvent::Stdout((*bytes).to_owned().into_bytes()),
                Piece::Quiet(_) => AgentEvent::Stalled,
                Piece::Done => AgentEvent::Done,
                Piece::Stopped => AgentEvent::Stopped,
            },
        })
        .collect()
}

/// A retry is not an ending, so tool traffic and completion after it still deliver one product.
#[tokio::test]
async fn a_retry_notice_survives_the_rest_of_its_own_turn() {
    let (settled, watched) = drive(&[
        Piece::Line(STARTED),
        Piece::Line(RECONNECT),
        Piece::Line(BUILDING),
        Piece::Line(SHELL),
        Piece::Line(RECONNECT),
        Piece::Line(ANSWER),
        Piece::Line(COMPLETED),
        Piece::Done,
    ])
    .await;
    assert_eq!(
        settled,
        Settled::Delivered("Your runner game is ready.".to_owned()),
        "a retry notice cost this turn the product it went on to finish",
    );
    let labels = watched
        .stages
        .iter()
        .map(|(_, label)| label.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        labels,
        ["Building", "Delivering"],
        "a retried turn's steps went backwards, repeated, or vanished",
    );
    assert_eq!(
        watched.sessions,
        ["thread-1"],
        "a retried turn rebound its native conversation",
    );
}

/// A stray non-JSON line inside a live journal costs one reattachment, never the finished product.
#[tokio::test]
async fn a_stray_non_json_line_costs_no_product() {
    let (settled, watched) = drive(&[
        Piece::Line(STARTED),
        Piece::Line("Reconnecting... 1/5 (request timed out)"),
        Piece::Line(ANSWER),
        Piece::Line(COMPLETED),
        Piece::Done,
    ])
    .await;
    assert_eq!(
        settled,
        Settled::Delivered("Your runner game is ready.".to_owned()),
        "one unreadable line cost this turn the product the Agent finished",
    );
    assert_eq!(
        watched.answers.len(),
        1,
        "replaying past an unreadable line published the same message twice",
    );
}

/// A turn the Provider finished delivers, even when its journal was cut off mid-record afterwards.
#[tokio::test]
async fn a_finished_turn_delivers_though_its_journal_was_cut_off() {
    let (settled, _) = drive(&[
        Piece::Line(STARTED),
        Piece::Line(ANSWER),
        Piece::Line(COMPLETED),
        // The Provider died partway through its next record: complete bytes, no terminating newline.
        Piece::Cut(r#"{"type":"item.completed","item":{"type":"comm"#),
        Piece::Done,
    ])
    .await;
    assert_eq!(
        settled,
        Settled::Delivered("Your runner game is ready.".to_owned()),
        "a turn the Agent finished lost its whole product to bytes that arrived after it",
    );
}

/// The same truth for a trailing byte that was never a record at all.
#[tokio::test]
async fn a_finished_turn_delivers_though_one_stray_byte_followed_it() {
    // The last tail is JSON the runner could read and no Provider record at all: it decodes, and
    // still says nothing about a turn that already finished.
    for tail in [" ", "\u{1b}[2K", "npm install 45%\r", "\0", "42"] {
        let (settled, _) = drive(&[
            Piece::Line(STARTED),
            Piece::Line(ANSWER),
            Piece::Line(COMPLETED),
            Piece::Cut(tail),
            Piece::Done,
        ])
        .await;
        assert_eq!(
            settled,
            Settled::Delivered("Your runner game is ready.".to_owned()),
            "a finished turn lost its product to trailing bytes {tail:?}",
        );
    }
}

/// A retry that the Provider answers by repeating an already-published message publishes it once.
#[tokio::test]
async fn a_repeated_message_after_a_retry_is_still_one_message() {
    let (settled, watched) = drive(&[
        Piece::Line(STARTED),
        Piece::Line(FIRST),
        Piece::Line(ANSWER),
        Piece::Line(RECONNECT),
        // The reopened stream resends the item it had already streamed, under its own same identity.
        Piece::Line(ANSWER),
        Piece::Line(COMPLETED),
        Piece::Done,
    ])
    .await;
    let resent = watched
        .answers
        .iter()
        .filter(|(id, _)| id.as_deref() == Some("m2"))
        .count();
    assert_eq!(
        (settled, resent),
        (
            Settled::Delivered("Adding the jump.Your runner game is ready.".to_owned()),
            1,
        ),
        "a message the Provider resent after its own retry reached the creator twice",
    );
}

/// One message that is still being written is not a resent message: a Provider that carries the same
/// message identity through longer and longer text is one message the creator watches grow.
#[tokio::test]
async fn a_message_still_being_written_keeps_growing() {
    let growing = r#"{"type":"item.completed","item":{"type":"agent_message","id":"m1","text":"Adding the jump. Then the double jump."}}"#;
    let (settled, watched) = drive(&[
        Piece::Line(STARTED),
        Piece::Line(FIRST),
        Piece::Line(growing),
        Piece::Line(COMPLETED),
        Piece::Done,
    ])
    .await;
    assert_eq!(
        settled,
        Settled::Delivered("Adding the jump. Then the double jump.".to_owned()),
        "a message that grew under its own identity was frozen at its first frame",
    );
    assert_eq!(
        watched.answers.last().map(|(_, text)| text.as_str()),
        Some("Adding the jump. Then the double jump."),
        "the creator never saw the rest of the message the Agent was still writing",
    );
}

/// Nothing that arrives after the Provider's own completion may take that turn's product away.
#[tokio::test]
async fn nothing_after_a_completion_unsettles_it() {
    let (settled, _) = drive(&[
        Piece::Line(STARTED),
        Piece::Line(ANSWER),
        Piece::Line(COMPLETED),
        Piece::Line(RECONNECT),
        Piece::Line(USAGE_LIMIT),
        Piece::Line(r#"{"type":"turn.failed","error":{"message":"The model refused this turn."}}"#),
        Piece::Line(r#"{"type":"error","message":"Reconnecting... 5/5"#),
        Piece::Done,
    ])
    .await;
    assert_eq!(
        settled,
        Settled::Delivered("Your runner game is ready.".to_owned()),
        "a frame that arrived after this turn finished took its product away",
    );
}

/// A real cause still reaches the creator when it arrives after a retry notice.
#[tokio::test]
async fn a_real_cause_after_a_retry_still_reaches_the_creator() {
    let (settled, _) = drive(&[
        Piece::Line(STARTED),
        Piece::Line(RECONNECT),
        Piece::Line(USAGE_LIMIT),
        Piece::Done,
    ])
    .await;
    assert_eq!(
        settled,
        Settled::Repaired("You've hit your usage limit.".to_owned()),
        "a cause the creator can act on lost its own words behind a retry",
    );
}

/// A retry notice that is the last thing a dead runner said reports no transport prose.
#[tokio::test]
async fn a_retry_notice_that_ends_a_runner_reports_the_runner_end() {
    let (settled, _) = drive(&[
        Piece::Line(STARTED),
        Piece::Line(BUILDING),
        Piece::Line(RECONNECT),
        Piece::Stopped,
    ])
    .await;
    assert_eq!(
        settled,
        Settled::Repaired("Your Agent stopped without delivering".to_owned()),
        "a runner that died inside its own retry window reported the retry as the cause",
    );
}

/// Quiet is not death. A command that produced nothing for as long as it took, inside a turn that was
/// also retrying its own stream, still delivers the product the Agent went on to finish.
#[tokio::test]
async fn a_quiet_command_inside_a_retried_turn_still_delivers() {
    let (settled, _) = drive(&[
        Piece::Line(STARTED),
        Piece::Line(RECONNECT),
        Piece::Quiet("codex"),
        Piece::Line(ANSWER),
        Piece::Line(COMPLETED),
        Piece::Done,
    ])
    .await;
    assert_eq!(
        settled,
        Settled::Delivered("Your runner game is ready.".to_owned()),
        "a command that went quiet inside a retry window cost the creator the finished product",
    );

    // And when the runner really did end, the cause is how it ended — never a verdict about quiet.
    let (settled, _) = drive(&[
        Piece::Line(STARTED),
        Piece::Line(RECONNECT),
        Piece::Quiet("codex"),
        Piece::Done,
    ])
    .await;
    assert_eq!(
        settled,
        Settled::Repaired("Local Agent ended without native completion".to_owned()),
        "a quiet command became this Work's reported cause",
    );
}

/// A turn boundary requested while the Provider is retrying ends that turn, not the Work.
#[tokio::test]
async fn a_turn_boundary_inside_a_retry_window_rotates() {
    let (settled, watched) = drive_with(
        &[
            Piece::Line(STARTED),
            Piece::Line(RECONNECT),
            Piece::Line(FIRST),
            Piece::Stopped,
        ],
        state::TurnStop::queued(),
    )
    .await;
    assert_eq!(
        settled,
        Settled::Rotated,
        "a queued follow-up inside a retry window ended the Work instead of the turn",
    );
    assert_eq!(
        watched.answers.len(),
        1,
        "a rotating turn published its own message more than once",
    );
}

/// A journal replayed into a fresh observer — every reattachment that does not carry the previous
/// observer, which is every restart of this daemon mid-turn and every rotation resumed after one —
/// republishes this turn's messages, and folding them into the Work's own answer adds nothing.
#[tokio::test]
async fn a_replayed_journal_republishes_nothing() {
    // Three messages, because a turn of two can only prove its first and its last: a message with
    // messages on both sides of it is the one a replay is most likely to say twice.
    let ended = [
        Piece::Line(STARTED),
        Piece::Line(FIRST),
        Piece::Line(ANSWER),
        Piece::Line(CLOSING),
        Piece::Stopped,
    ];
    // The live turn published its messages, exactly as record_agent_message would have.
    let (_, live) = drive_with(&ended, state::TurnStop::queued()).await;
    let published = live
        .answers
        .iter()
        .map(|(_, text)| text.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        published,
        [
            "Adding the jump.",
            "Your runner game is ready.",
            "Tell me if the jump feels too high.",
        ],
        "the live turn did not publish its own messages once each",
    );
    // A restarted driver reattaches a fresh observer to the same journal, exactly as
    // observe_until_terminal does for a launched Work and resume_steer_rotation does for a rotation.
    let mut observer = Observer::new(Provider::Codex, 0);
    let mut replay = Watched::default();
    let mut run = host::AgentRun::scripted(frames(&ended));
    let _ = observe(
        &mut observer,
        &mut run,
        &state::OwnerStop::default(),
        &state::TurnStop::queued(),
        &mut replay,
    )
    .await;
    // The Work's own monotonic answer is what the replayed text is folded into, by the one shipped
    // rule every surface folds by: work_state::model::Running::append_answer calls exactly this.
    let mut answer = published.concat();
    for (_, text) in &replay.answers {
        process::observe::append_answer(&mut answer, text);
    }
    assert_eq!(
        answer, "Adding the jump.Your runner game is ready.Tell me if the jump feels too high.",
        "replaying an ended turn's journal duplicated a message it already delivered",
    );
}
