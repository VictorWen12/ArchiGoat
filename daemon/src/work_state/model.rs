//! Work models hold only facts that change public progress, verified delivery, or owner Stop,
//! and record the conversation every screen renders.

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use sha2::{Digest, Sha256};

use crate::{
    delivery::{DeliveryFile, Harvested},
    provider::Provider,
    state::{DaemonState, OwnerStop, RunPhase, RunProgress, RunSnapshot, WorkEventKind},
    work::{ResultKind, RuntimeSteer, RuntimeWork},
};

/// A Work may replace its dead runner only this many times before it asks the creator.
pub(super) const MAX_REPAIRS: u32 = 3;

/// TurnStop interrupts one physical turn without claiming the owner stopped its Work.
#[derive(Clone)]
pub(crate) struct TurnStop(Arc<AtomicBool>);

// This internal signal rotates one native turn while preserving the public Running Work.
impl TurnStop {
    /// New starts one physical turn without an internal interruption.
    pub(super) fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    /// Request ends only the current physical turn.
    pub(crate) fn request(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Requested separates internal rotation from owner Stop.
    pub(crate) fn requested(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Only the terminal the user chose may claim the user chose it.
pub(super) const OWNER_STOP: &str = "Stopped by you";
/// A legacy terminal runner failure delivered nothing and must say exactly that.
pub(super) const RUNNER_END: &str = "Your Agent stopped without delivering";
/// Attention is the one public stage that asks the creator without ending the Work.
pub(super) const ATTENTION: &str = "Needs attention";

/// AttentionText keeps the stored recovery cause visible without inventing another control state.
pub(super) fn attention_text(provider: Provider, reason: &str) -> String {
    let reason = reason.trim();
    // A Provider losing the connection to its own service is that Provider's fact. Its transport
    // prose names a socket the creator cannot see, so the park names whose service went quiet.
    if super::repair::transport_shaped(Some(reason)) {
        return format!(
            "{ATTENTION} — {} lost its connection to its own service",
            provider.label()
        );
    }
    if reason.is_empty() {
        ATTENTION.to_owned()
    } else {
        format!("{ATTENTION} — {reason}")
    }
}

/// Entry adds artifact recovery internally while its one terminal Stop carries its own public cause.
pub(super) enum Entry {
    Running(Running),
    ArtifactPending(ArtifactPending),
    Done(DoneWork),
    Stopped(StoppedWork),
}

/// Running binds one Provider runner, native session, Stop authority, and public progress.
pub(super) struct Running {
    pub(super) remote: bool,
    pub(super) work_id: String,
    pub(super) provider: Provider,
    /// ModelSelection keeps the connection's exact native tier bound to this Work.
    pub(super) model_selection: Option<String>,
    /// EffortSelection keeps the connection's exact native reasoning tier bound to this Work.
    pub(super) effort_selection: Option<String>,
    pub(super) session: PathBuf,
    pub(super) freeze_root: PathBuf,
    pub(super) native_session: String,
    pub(super) runner_id: String,
    pub(super) input_path: PathBuf,
    pub(super) launched: bool,
    /// Repair records that the next runner must resume the bound native session.
    pub(super) repair: bool,
    /// Steer is the durable head message awaiting or driving one native continuation.
    pub(super) steer: Option<RuntimeSteer>,
    /// Steers preserves later messages in their accepted order.
    pub(super) steers: VecDeque<RuntimeSteer>,
    /// Steering records that the head owns the current physical turn.
    pub(super) steering: bool,
    /// SteerDelivered records that a runner already received the head's exact words, so replacing a
    /// dead runner continues that turn instead of asking the Agent to do the same thing twice.
    pub(super) steer_delivered: bool,
    /// Rotating records that the prior physical turn ended before its private cleanup.
    pub(super) rotating: bool,
    /// Stopping makes owner Stop dominate every queued continuation across restart.
    pub(super) stopping: bool,
    /// Repairs counts every runner this Work has already replaced, bounding self-repair across restarts.
    pub(super) repairs: u32,
    /// Attention parks an exhausted Work while preserving same-session steering and owner Stop.
    pub(super) attention: bool,
    /// Failure preserves the latest repair cause if the bounded continuation cannot recover.
    pub(super) failure: Option<String>,
    pub(super) started_at: u64,
    /// Answer is the cumulative safe Provider text visible while the native turn is Running.
    pub(super) answer: String,
    pub(super) progress: Option<RunProgress>,
    /// Cumulative Provider-reported output tokens, public exactly as the native CLI shows them.
    pub(super) tokens: Option<u64>,
    /// The Provider-reported executing model, public so answer quality stays attributable.
    pub(super) model: Option<String>,
    /// ProtectedOutputs is the bounded exact tool-output output set for this Work's whole lifetime.
    pub(super) protected_outputs: Vec<String>,
    pub(super) stop: OwnerStop,
    pub(super) turn_stop: TurnStop,
}

/// ArtifactPending keeps native completion Running until every frozen byte verifies again.
pub(super) struct ArtifactPending {
    pub(super) remote: bool,
    pub(super) work_id: String,
    pub(super) session: Option<PathBuf>,
    pub(super) answer: String,
    pub(super) kind: ResultKind,
    pub(super) run: String,
    pub(super) native_session: String,
    pub(super) manifest: Vec<DeliveryFile>,
    pub(super) freeze_root: PathBuf,
    pub(super) started_at: u64,
}

/// DoneWork owns one verified result and optional frozen bytes awaiting Account acknowledgement.
pub(super) struct DoneWork {
    pub(super) remote: bool,
    pub(super) work_id: String,
    pub(super) session: Option<PathBuf>,
    pub(super) answer: String,
    pub(super) kind: ResultKind,
    pub(super) run: Option<String>,
    pub(super) native_session: String,
    pub(super) manifest: Vec<DeliveryFile>,
    pub(super) freeze_root: Option<PathBuf>,
    pub(super) harvested: Option<Harvested>,
    pub(super) started_at: u64,
    /// EndedAt lets retention measure from the terminal moment instead of guessing from launch.
    pub(super) ended_at: Option<u64>,
}

/// StoppedWork records one terminal end and original public history.
pub(super) struct StoppedWork {
    pub(super) remote: bool,
    pub(super) work_id: String,
    pub(super) session: Option<PathBuf>,
    pub(super) freeze_root: Option<PathBuf>,
    pub(super) started_at: u64,
    /// EndedAt lets retention measure from the terminal moment instead of guessing from launch.
    pub(super) ended_at: Option<u64>,
    /// Owner separates the user's explicit Stop from a runner that ended or spent its repair budget.
    pub(super) owner: bool,
    /// Reason preserves the exact runner failure shown to the user.
    pub(super) reason: String,
}

// This running record preserves the minimum facts needed to resume one Work.
impl Running {
    /// FromRuntime transfers an admitted runtime into durable ownership before native launch.
    pub(super) fn from_runtime(runtime: &RuntimeWork, remote: bool) -> Self {
        Self {
            remote,
            work_id: runtime.work_id.clone(),
            provider: runtime.provider,
            model_selection: runtime.model_selection.clone(),
            effort_selection: runtime.effort_selection.clone(),
            session: runtime.session.clone(),
            freeze_root: runtime.freeze_root.clone(),
            native_session: runtime.native_session.clone(),
            runner_id: runtime.runner_id.clone(),
            input_path: runtime.input_path.clone(),
            launched: false,
            repair: false,
            steer: runtime.steer.clone(),
            steers: VecDeque::new(),
            steering: runtime.steer.is_some(),
            steer_delivered: false,
            rotating: runtime.rotating,
            stopping: false,
            repairs: 0,
            attention: false,
            failure: None,
            started_at: runtime.started_at,
            answer: String::new(),
            progress: None,
            tokens: None,
            model: None,
            protected_outputs: Vec::new(),
            stop: OwnerStop::new(),
            turn_stop: TurnStop::new(),
        }
    }
}

/// Snapshot projects every internal state onto Running, Done, owner Stopped, or Failed only.
pub(super) fn snapshot(entry: &Entry) -> RunSnapshot {
    match entry {
        Entry::Running(work) if work.attention => failed_snapshot(work),
        Entry::Running(work) => running_snapshot(
            work.started_at,
            work.answer.clone(),
            work.progress.clone(),
            work.tokens,
            work.model.clone(),
        ),
        Entry::ArtifactPending(work) => {
            running_snapshot(work.started_at, work.answer.clone(), None, None, None)
        }
        Entry::Done(work) => RunSnapshot {
            phase: RunPhase::Done,
            text: work.answer.clone(),
            started_at: work.started_at,
            progress: None,
            tokens: None,
            model: None,
            kind: Some(work.kind),
            run: work.run.clone(),
            files: work.manifest.clone(),
            events: Vec::new(),
            // A turn that ended with words and no files leaves the ball with the creator:
            // the Agent's own completion evidence is the waiting signal, never text heuristics.
            awaiting: work.manifest.is_empty() && !work.answer.trim().is_empty(),
        },
        Entry::Stopped(work) => RunSnapshot {
            phase: if work.owner {
                RunPhase::Stopped
            } else {
                RunPhase::Failed
            },
            text: if work.owner { OWNER_STOP } else { &work.reason }.to_owned(),
            started_at: work.started_at,
            progress: None,
            tokens: None,
            model: None,
            kind: None,
            run: None,
            files: Vec::new(),
            events: Vec::new(),
            awaiting: false,
        },
    }
}

/// FailedSnapshot exposes a parked durable conversation as failure, never as live execution.
fn failed_snapshot(work: &Running) -> RunSnapshot {
    RunSnapshot {
        phase: RunPhase::Failed,
        text: attention_text(work.provider, work.failure.as_deref().unwrap_or_default()),
        started_at: work.started_at,
        progress: None,
        tokens: None,
        model: None,
        kind: None,
        run: None,
        files: Vec::new(),
        events: Vec::new(),
        awaiting: false,
    }
}

/// RunningSnapshot exposes Provider-public progress without leaking recovery anomalies.
fn running_snapshot(
    started_at: u64,
    answer: String,
    progress: Option<RunProgress>,
    tokens: Option<u64>,
    model: Option<String>,
) -> RunSnapshot {
    RunSnapshot {
        phase: RunPhase::Running,
        text: answer,
        started_at,
        // Only the Provider's own waiting stage parks a turn on the creator.
        awaiting: progress
            .as_ref()
            .is_some_and(|progress| progress.text == crate::process::WAITING),
        // An empty internal action retains only the replay cursor between physical turns.
        progress: progress.filter(|progress| !progress.text.is_empty()),
        tokens,
        model,
        kind: None,
        run: None,
        files: Vec::new(),
        events: Vec::new(),
    }
}

// AppendAnswer keeps one monotonic answer stream safe across Provider replay and full-message frames.
impl Running {
    pub(super) fn append_answer(&mut self, candidate: &str) -> bool {
        crate::process::append_answer(&mut self.answer, candidate)
    }
}

// These writers put every turn of the conversation where each screen reads it.
impl DaemonState {
    /// RecordAgentMessage puts one complete Agent message in the conversation and the delivered text.
    pub(crate) fn record_agent_message(
        &self,
        work_id: &str,
        id: Option<&str>,
        text: &str,
    ) -> Result<(), String> {
        let Some(published) = self.work_answer(work_id).map(|answer| answer.len()) else {
            eprintln!(
                "Product kept an Agent message out of Work {work_id}: it is no longer running"
            );
            return Ok(());
        };
        self.append_work_answer(work_id, text)?;
        let Some(answer) = self.work_answer(work_id) else {
            eprintln!(
                "Product kept an Agent message out of Work {work_id}: it is no longer running"
            );
            return Ok(());
        };
        // Redaction already struck inside these words, and the message itself is never withheld.
        // A frame that only repeats published prose is that same message, so it opens no bubble.
        if published >= answer.len() {
            return Ok(());
        }
        let message = &answer[published..];
        match id {
            // One Provider message is one bubble: its later frames extend the message they belong to.
            Some(id) => self.extend_agent_message(work_id, id, message),
            None => self.push_work_event(
                work_id,
                WorkEventKind::AgentMessage {
                    id: message_identity(message),
                    text: message.to_owned(),
                },
            ),
        }
        Ok(())
    }

    /// WorkAnswer reads the Agent prose one Running Work has already published.
    fn work_answer(&self, work_id: &str) -> Option<String> {
        match self
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entries
            .get(work_id)
        {
            Some(Entry::Running(work)) => Some(work.answer.clone()),
            _ => None,
        }
    }

    /// RecordUserMessage puts the creator's own words in the conversation under the turn's own identity.
    pub(crate) fn record_user_message(
        &self,
        work_id: &str,
        steer_id: &str,
        text: &str,
        attachments: Vec<String>,
    ) {
        self.push_work_event(
            work_id,
            WorkEventKind::UserMessage {
                steer_id: steer_id.to_owned(),
                text: text.to_owned(),
                attachments,
            },
        );
    }

    /// RecordWorkStage keeps one Provider action in the conversation and as the current label.
    pub(crate) fn record_work_stage(
        &self,
        work_id: &str,
        index: usize,
        label: String,
    ) -> Result<(), String> {
        // A turn names each action it takes once; a replayed frame repeats no step already recorded.
        if self.replace_work_progress(work_id, index, label.clone())? {
            self.push_work_stage(work_id, label);
        }
        Ok(())
    }

    /// RecordArtifacts names every delivered file in the conversation that produced it.
    pub(crate) fn record_artifacts(&self, work_id: &str, files: &[DeliveryFile]) {
        for file in files {
            self.push_work_event(
                work_id,
                WorkEventKind::Artifact {
                    name: file.title.clone(),
                },
            );
        }
    }
}

/// MessageIdentity gives a Provider message without its own identity one stable, content-exact name.
fn message_identity(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
