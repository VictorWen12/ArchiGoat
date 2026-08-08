//! Work state is the single bridge from accepted identity to native execution and verified delivery.

mod artifact;
mod completion;
mod drive;
mod model;
mod observe;
mod persist;
mod protected;
mod repair;
mod restore;
mod saved;
mod steer;
mod store;
mod terminal;

use std::{fs, fs::File, path::PathBuf, time::Duration};

/// A delayed state-file commit must retry without burning CPU or delaying all owner Stop recovery.
const STOP_COMMIT_MAX_WAIT: Duration = Duration::from_secs(1);
const STOP_COMMIT_INITIAL_WAIT: Duration = Duration::from_millis(25);
/// A brief held for a connection change rechecks this often, close enough that the person who sent it
/// sees the Agent take it up as the change ends.
const CONNECTION_SETTLE_WAIT: Duration = Duration::from_millis(50);

use crate::{
    api::work::StagedInput,
    delivery::DeliveryFile,
    state::{DaemonState, OwnerStop, RunPhase},
    work::{RuntimeRecovery, RuntimeSteer, RuntimeWork, WorkRequest},
};

pub(crate) use model::TurnStop;
pub(crate) use store::WorkStore;

/// StartRefusal separates a brief that belongs to a Work already running from a machine that could
/// not admit it at all, so the creator's words reach the live Agent instead of a second one.
pub(crate) enum StartRefusal {
    /// Busy names the Work already running this brief's conversation.
    Busy(String),
    /// Unavailable carries the local diagnostic for a brief this machine could not start.
    Unavailable(String),
}

// This refusal stays a product fact, so every caller can act on it without reading diagnostics.
impl std::fmt::Display for StartRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy(work_id) => {
                write!(
                    formatter,
                    "This conversation is already running as {work_id}"
                )
            }
            Self::Unavailable(reason) => formatter.write_str(reason),
        }
    }
}

// Every durable failure inside admission is a machine fact, not a live conversation.
impl From<String> for StartRefusal {
    fn from(reason: String) -> Self {
        Self::Unavailable(reason)
    }
}

/// NativeOwnership publishes Running only for the lifetime of one physically owned AgentRun.
struct NativeOwnership {
    state: DaemonState,
    work_id: String,
}

impl Drop for NativeOwnership {
    fn drop(&mut self) {
        let changed = self
            .state
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .set_native_owned(&self.work_id, false);
        if changed {
            self.state.work_events.notify_waiters();
        }
    }
}

/// ResumeAdmitted consumes the same stable launch right when restart happened before launch proof.
pub(crate) async fn resume_admitted(state: DaemonState, work: RuntimeRecovery) {
    let runtime = RuntimeWork::recover(work);
    drive::run(state, runtime).await;
}

// This orchestration keeps each frozen Work on its single durable native path.
impl DaemonState {
    fn conversation_session(
        &self,
        provider: crate::provider::Provider,
        model: Option<&str>,
        conversation: &str,
    ) -> Result<Option<String>, String> {
        let path = self.conversation_path(provider, model, conversation)?;
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("Could not read conversation binding: {error}")),
        };
        let native_session =
            String::from_utf8(bytes).map_err(|_| "Conversation binding is invalid".to_owned())?;
        if native_session.is_empty() || native_session.chars().any(char::is_control) {
            return Err("Conversation binding is invalid".to_owned());
        }
        Ok(Some(native_session))
    }

    fn bind_conversation(
        &self,
        provider: crate::provider::Provider,
        model: Option<&str>,
        conversation: &str,
        native_session: &str,
    ) -> Result<(), String> {
        let path = self.conversation_path(provider, model, conversation)?;
        crate::host::create_private_dir(
            path.parent()
                .ok_or_else(|| "Conversation storage is invalid".to_owned())?,
        )?;
        crate::host::replace_private(&path, native_session.as_bytes())
    }

    /// DiscardConversation ends this binding, so the next Work in the conversation opens its own session.
    fn discard_conversation(
        &self,
        provider: crate::provider::Provider,
        model: Option<&str>,
        conversation: &str,
    ) {
        let Ok(path) = self.conversation_path(provider, model, conversation) else {
            return;
        };
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => eprintln!("Product could not release a conversation binding: {error}"),
        }
    }

    // The model is part of the key, so a Provider or model change leaves the old binding unreachable.
    fn conversation_path(
        &self,
        provider: crate::provider::Provider,
        model: Option<&str>,
        conversation: &str,
    ) -> Result<PathBuf, String> {
        use sha2::{Digest, Sha256};
        let key = format!(
            "{}:{}:{conversation}",
            provider.program(),
            model.unwrap_or_default()
        );
        Ok(self
            .private_root()?
            .join("Conversations")
            .join(format!("{:x}", Sha256::digest(key.as_bytes()))))
    }

    /// OwnNativeRun ties Account-visible Running to one live native observer lifetime.
    fn own_native_run(&self, work_id: &str, physically_live: bool) -> Option<NativeOwnership> {
        if !physically_live {
            return None;
        }
        let changed = self
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .set_native_owned(work_id, true);
        if changed {
            self.work_events.notify_waiters();
        }
        Some(NativeOwnership {
            state: self.clone(),
            work_id: work_id.to_owned(),
        })
    }

    /// ConnectionSettled waits out a Provider connection change, which always ends on its own: the
    /// attempt either connects an Agent or releases its own admission. Nothing here ends a brief.
    async fn connection_settled(&self) {
        loop {
            let changing = self
                .run_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .connecting
                .is_some();
            if !changing {
                return;
            }
            tokio::time::sleep(CONNECTION_SETTLE_WAIT).await;
        }
    }

    /// StartWork admits one identity once, persists it before launch, and attaches duplicate Starts to that truth.
    pub(crate) async fn start_work(
        &self,
        work_id: String,
        request: WorkRequest,
        inputs: Vec<StagedInput>,
    ) -> Result<(), StartRefusal> {
        self.admit_work(work_id, request, inputs, false).await
    }

    /// StartRemoteWork persists Account provenance before native execution begins.
    pub(crate) async fn start_remote_work(
        &self,
        work_id: String,
        request: WorkRequest,
        inputs: Vec<StagedInput>,
    ) -> Result<(), String> {
        self.admit_work(work_id, request, inputs, true)
            .await
            .map_err(|refusal| refusal.to_string())
    }

    /// AdmitWork gives local and Account Work one durable engine with explicit provenance.
    async fn admit_work(
        &self,
        work_id: String,
        request: WorkRequest,
        inputs: Vec<StagedInput>,
        remote: bool,
    ) -> Result<(), StartRefusal> {
        if let Some(existing) = self.work_provenance(&work_id) {
            if existing == remote {
                return Ok(());
            }
            // A re-leased Account Start is the exact upgrade proof for Work admitted by the old relay.
            if remote {
                let mut works = self
                    .works
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if !works.mark_remote(&work_id) {
                    return Err(StartRefusal::Unavailable(
                        "Work identity is unavailable".to_owned(),
                    ));
                }
                if let Err(error) = works.save(self.work_state_path()) {
                    if let Some(entry) = works.entries.get_mut(&work_id) {
                        match entry {
                            model::Entry::Running(work) => work.remote = false,
                            model::Entry::ArtifactPending(work) => work.remote = false,
                            model::Entry::Done(work) => work.remote = false,
                            model::Entry::Stopped(work) => work.remote = false,
                        }
                    }
                    return Err(error.into());
                }
                self.work_events.notify_waiters();
                self.relay_events.notify_waiters();
                return Ok(());
            }
            return Err(StartRefusal::Unavailable(
                "Work identity belongs to another control plane".to_owned(),
            ));
        }
        // BOUNDARY: with no Agent connected there is nothing on this machine that can run a brief.
        let (provider, model_selection, effort_selection) = self
            .connected_selection()
            .await
            .ok_or_else(|| "Connect a local Agent before starting Work".to_owned())?;
        // A creator who named no tier still gets one: the published fast preset, which is the tier a
        // build loop is waiting on. A tier they did name is theirs and passes through untouched.
        let fast = self.presets(Some(provider)).await.unwrap_or_default().fast;
        let model_selection = model_selection.or(fast.model);
        let effort_selection = effort_selection.or(fast.effort);
        let conversation = request.conversation_id(&work_id)?;
        let bound_session =
            self.conversation_session(provider, model_selection.as_deref(), &conversation)?;
        // One conversation runs one Work at a time. A brief that arrives while its conversation is
        // live belongs to that Work, so it is named back to its sender instead of starting a second
        // Agent that rebuilds the same product beside the first one.
        if let Some(live) = self
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .live_conversation_work(provider, &conversation, bound_session.as_deref())
        {
            return Err(StartRefusal::Busy(live));
        }
        // A connection change moves when this brief starts, never whether it starts.
        self.connection_settled().await;
        let runtime = RuntimeWork::prepare(
            self,
            work_id.clone(),
            provider,
            model_selection,
            effort_selection,
            request,
            inputs,
            bound_session,
        )?;
        let mut duplicate = false;
        loop {
            // A change that arrived while this runtime was prepared keeps every staged byte of it.
            let changing = {
                let mut slot = self
                    .run_slot
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if slot.connecting.is_some() {
                    true
                } else {
                    let mut works = self
                        .works
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if let Some(existing) = works.is_remote(&work_id) {
                        // BOUNDARY: one Work identity answers to one control plane.
                        if existing != remote {
                            runtime.discard()?;
                            return Err(StartRefusal::Unavailable(
                                "Work identity belongs to another control plane".to_owned(),
                            ));
                        }
                        duplicate = true;
                    } else {
                        slot.active.insert(work_id.clone());
                        works.insert_running(store::running_from(&runtime, remote));
                        if let Err(error) = works.save(self.work_state_path()) {
                            works.remove_running(&work_id);
                            slot.active.remove(&work_id);
                            if let Err(cleanup) = runtime.discard() {
                                return Err(StartRefusal::Unavailable(format!(
                                    "{error}; {cleanup}"
                                )));
                            }
                            return Err(error.into());
                        }
                    }
                    false
                }
            };
            if !changing {
                break;
            }
            self.connection_settled().await;
        }
        if duplicate {
            runtime.discard()?;
            return Ok(());
        }
        if remote {
            self.work_events.notify_waiters();
        }
        let state = self.clone();
        tokio::spawn(async move { drive::run(state, runtime).await });
        Ok(())
    }

    /// SteerWork durably queues one exact follow-up on the existing Running owner.
    pub(crate) fn steer_work(
        &self,
        work_id: &str,
        steer_id: String,
        request: WorkRequest,
        inputs: Vec<StagedInput>,
    ) -> Result<bool, String> {
        let steer = RuntimeSteer::prepare(steer_id, request, inputs)?;
        let (rotation, resumed) = {
            let mut works = self
                .works
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let queued = match works.queue_steer(work_id, steer) {
                Ok(queued) => queued,
                Err(reason)
                    if matches!(reason.as_str(), "Work is not Running" | "Work is stopping") =>
                {
                    return Ok(false);
                }
                Err(reason) => return Err(reason),
            };
            let restart = queued.as_ref().is_some_and(|(_, restart)| *restart);
            if let Some((rollback, _)) = queued
                && let Err(error) = works.save(self.work_state_path())
            {
                works.rollback_steering(work_id, rollback);
                return Err(error);
            }
            let resumed = restart
                .then(|| {
                    works
                        .running()
                        .into_iter()
                        .find(|work| work.work_id == work_id)
                        .map(RuntimeWork::recover)
                })
                .flatten();
            (works.rotation_authority(work_id), resumed)
        };
        if let Some(rotation) = rotation {
            rotation.request();
        }
        if let Some(runtime) = resumed {
            let state = self.clone();
            self.work_events.notify_waiters();
            self.relay_events.notify_one();
            tokio::spawn(async move { drive::run(state, runtime).await });
        }
        Ok(true)
    }

    /// StopWork exercises owner authority once and waits until the addressed Work is no longer Running.
    pub(crate) async fn stop_work(&self, work_id: &str) {
        let mut retry = STOP_COMMIT_INITIAL_WAIT;
        let authority = loop {
            let result = {
                let mut works = self
                    .works
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let Some(authority) = works.stop_authority(work_id) else {
                    break None;
                };
                let turn = works.turn_stop_authority(work_id);
                let committed = match works.begin_stop(work_id) {
                    Some(rollback) => match works.save(self.work_state_path()) {
                        Ok(()) => true,
                        Err(error) => {
                            works.rollback_steering(work_id, rollback);
                            eprintln!("Product could not persist Work Stop: {error}");
                            false
                        }
                    },
                    None => true,
                };
                if committed {
                    // Publish authority while the state lock still excludes a competing Done commit.
                    authority.request();
                    if let Some(turn) = &turn {
                        turn.request();
                    }
                    Some((authority, turn))
                } else {
                    None
                }
            };
            if let Some(result) = result {
                break Some(result);
            }
            tokio::time::sleep(retry).await;
            retry = (retry * 2).min(STOP_COMMIT_MAX_WAIT);
        };
        let Some(_) = authority else {
            if self
                .run_snapshot(work_id)
                .is_some_and(|snapshot| snapshot.phase == RunPhase::Running)
            {
                while !self.mark_owner_stopped(work_id) {
                    tokio::time::sleep(retry).await;
                    retry = (retry * 2).min(STOP_COMMIT_MAX_WAIT);
                }
            }
            return;
        };
        if self
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .needs_attention(work_id)
        {
            while !self.mark_owner_stopped(work_id) {
                tokio::time::sleep(retry).await;
                retry = (retry * 2).min(STOP_COMMIT_MAX_WAIT);
            }
            return;
        }
        loop {
            // Register before reading so a terminal transition cannot strand the owner's Stop.
            let changed = self.work_events.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self
                .run_snapshot(work_id)
                .is_none_or(|snapshot| snapshot.phase != RunPhase::Running)
            {
                return;
            }
            changed.await;
        }
    }

    /// WorkBinding resolves one durable Running owner, parked or executing, without launching it.
    pub(super) fn work_binding(&self, work_id: &str) -> Option<RuntimeRecovery> {
        self.works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .binding(work_id)
    }

    /// RecoveryCandidates exposes each native Running owner once; Done and Stopped can never launch.
    pub(crate) fn recovery_candidates(&self) -> Vec<RuntimeRecovery> {
        self.works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .running()
    }

    /// OwnerStop returns the exact signal shared by API, live runner, and restart recovery.
    pub(crate) fn owner_stop(&self, work_id: &str) -> Option<OwnerStop> {
        self.works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .stop_authority(work_id)
    }

    /// TurnStop returns the current runner-only interruption authority.
    pub(crate) fn turn_stop(&self, work_id: &str) -> Option<TurnStop> {
        self.works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .turn_stop_authority(work_id)
    }

    /// RequestPendingRotation wakes an eligible runner after admission, binding, or recovery.
    pub(crate) fn request_pending_rotation(&self, work_id: &str) {
        let rotation = self
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .rotation_authority(work_id);
        if let Some(rotation) = rotation {
            rotation.request();
        }
    }

    /// RecordSteerRotation commits the terminal boundary before native runner cleanup.
    pub(crate) fn record_steer_rotation(&self, work_id: &str) -> Result<bool, String> {
        let mut works = self
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(rollback) = works.begin_rotation(work_id) else {
            return Ok(false);
        };
        if let Err(error) = works.save(self.work_state_path()) {
            works.rollback_steering(work_id, rollback);
            return Err(error);
        }
        Ok(true)
    }

    /// PrepareSteer arms the selected follow-up only after prior native cleanup.
    pub(crate) fn prepare_steer(&self, work_id: &str) -> Result<RuntimeWork, String> {
        let mut works = self
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let rollback = works
            .arm_head(work_id)
            .ok_or_else(|| "Queued follow-up is unavailable".to_owned())?;
        if let Err(error) = works.save(self.work_state_path()) {
            works.rollback_steering(work_id, rollback);
            return Err(error);
        }
        let work = works
            .running()
            .into_iter()
            .find(|work| work.work_id == work_id)
            .ok_or_else(|| "Running Work binding is unavailable".to_owned())?;
        Ok(RuntimeWork::recover(work))
    }

    /// ReplaceWorkProgress durably publishes one current Provider action without retaining history.
    pub(crate) fn replace_work_progress(
        &self,
        work_id: &str,
        index: usize,
        update: String,
    ) -> Result<bool, String> {
        let mut works = self
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = works.progress(work_id);
        let changed = works.replace_progress(work_id, index, update)?;
        if changed {
            if let Err(error) = works.save(self.work_state_path()) {
                works.rollback_progress(work_id, previous);
                return Err(error);
            }
            drop(works);
            self.work_events.notify_waiters();
        }
        Ok(changed)
    }

    /// ReplaceWorkTokens durably publishes one cumulative Provider token count for a Running Work.
    pub(crate) fn replace_work_tokens(&self, work_id: &str, total: u64) -> Result<(), String> {
        let mut works = self
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = works.tokens(work_id);
        let changed = works.replace_tokens(work_id, total);
        if changed {
            if let Err(error) = works.save(self.work_state_path()) {
                works.rollback_tokens(work_id, previous);
                return Err(error);
            }
            drop(works);
            self.work_events.notify_waiters();
        }
        Ok(())
    }

    /// ReplaceWorkModel durably publishes the Provider-reported executing model for a Running Work.
    pub(crate) fn replace_work_model(&self, work_id: &str, model: String) -> Result<(), String> {
        let mut works = self
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = works.model(work_id);
        let changed = works.replace_model(work_id, model);
        if changed {
            if let Err(error) = works.save(self.work_state_path()) {
                works.rollback_model(work_id, previous);
                return Err(error);
            }
            drop(works);
            self.work_events.notify_waiters();
        }
        Ok(())
    }

    /// WorkProgressSequence keeps a continuation ordered after the latest native action.
    pub(crate) fn work_progress_sequence(&self, work_id: &str) -> usize {
        self.works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .progress_sequence(work_id)
    }

    /// MarkLaunched commits the point after which recovery may only reattach native execution.
    pub(crate) fn mark_launched(&self, work_id: &str) -> Result<(), String> {
        let rotation = {
            let mut works = self
                .works
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if works.mark_launched(work_id)
                && let Err(error) = works.save(self.work_state_path())
            {
                works.restore_unlaunched(work_id);
                return Err(error);
            }
            works.rotation_authority(work_id)
        };
        if let Some(rotation) = rotation {
            rotation.request();
        }
        Ok(())
    }

    /// MarkOwnerStopped commits only explicit owner Stop and releases only that Work's Provider binding.
    pub(crate) fn mark_owner_stopped(&self, work_id: &str) -> bool {
        self.commit_stopped(work_id, true, model::OWNER_STOP.to_owned())
    }

    /// CommitStopped durably publishes the one terminal end, its cause, and frees the Work's Provider binding.
    fn commit_stopped(&self, work_id: &str, owner: bool, reason: String) -> bool {
        let mut lease = None;
        let committed = {
            let mut works = self
                .works
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if owner {
                lease = works.conversation_lease(work_id);
            }
            let Some(previous) = works.mark_stopped(work_id, owner, reason) else {
                return false;
            };
            if let Err(error) = works.save(self.work_state_path()) {
                works.restore_entry(work_id, previous);
                eprintln!("Product could not persist Work Stop: {error}");
                false
            } else {
                true
            }
        };
        if !committed {
            return false;
        }
        // The owner's Stop ends this Work's claim on its Provider conversation.
        if let Some((provider, model, conversation)) = lease {
            self.discard_conversation(provider, model.as_deref(), &conversation);
        }
        self.release_work(work_id);
        self.work_events.notify_waiters();
        true
    }

    /// OpenArtifact returns exact frozen bytes only from an evidence-backed Done run.
    pub(crate) fn open_artifact(
        &self,
        run: &str,
        name: &str,
    ) -> Result<(File, DeliveryFile), String> {
        self.works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .open_artifact(run, name)
    }

    /// WorkProvenance reads the durable control plane without changing Work state.
    fn work_provenance(&self, work_id: &str) -> Option<bool> {
        self.works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_remote(work_id)
    }

    /// PersistWorksResult lets terminal transitions refuse cleanup when durable truth is unavailable.
    pub(super) fn persist_works_result(&self) -> Result<(), String> {
        self.works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .save(self.work_state_path())
    }

    /// ReleaseWork frees only the finished identity so distinct Works remain parallel.
    pub(super) fn release_work(&self, work_id: &str) {
        self.run_slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .active
            .remove(work_id);
        self.work_notify.notify_waiters();
    }
}
