//! WorkStore performs idempotent lifecycle transitions for independent Work identities.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use crate::{
    delivery::Harvested,
    state::{OwnerStop, RunProgress, RunSnapshot},
    work::{ResultKind, RuntimeRecovery, RuntimeWork},
};

use super::model::{DoneWork, Entry, Running, StoppedWork, snapshot};

// RunningNeedsPower names the durable entries that can still execute or verify bytes.
fn running_needs_power(entry: &Entry) -> bool {
    matches!(entry, Entry::Running(work) if !work.attention)
        || matches!(entry, Entry::ArtifactPending(_))
}

/// WorkStore gives each Work ID one lifecycle without a global concurrency ceiling.
pub(crate) struct WorkStore {
    pub(super) entries: HashMap<String, Entry>,
    pub(super) native_owned: HashSet<String>,
}

// This store durably owns the latest state for every local frozen Work.
impl WorkStore {
    /// Load restores the exact durable map without launching or replaying Work.
    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        let store = Self {
            entries: super::persist::load(path)?,
            native_owned: HashSet::new(),
        };
        for (work_id, entry) in &store.entries {
            if running_needs_power(entry) {
                crate::keepalive::work_started(work_id);
            }
        }
        Ok(store)
    }

    /// ActiveIds restores only native Running owners into Provider-switch exclusion.
    pub(crate) fn active_ids(&self) -> HashSet<String> {
        self.entries
            .iter()
            .filter(|(_, entry)| matches!(entry, Entry::Running(_)))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// DeliveryCandidates returns durable Done truth in stable order until Account acknowledges it.
    pub(crate) fn remote_delivery_candidates(&self) -> Vec<(String, RunSnapshot)> {
        let mut candidates = self
            .entries
            .iter()
            .filter(|(_, entry)| matches!(entry, Entry::Done(work) if work.remote))
            .map(|(work_id, entry)| (work_id.clone(), snapshot(entry)))
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.0.cmp(&right.0));
        candidates
    }

    /// RemotePendingArtifacts names only Account-owned results awaiting byte revalidation.
    pub(crate) fn remote_pending_artifacts(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter_map(|(work, entry)| {
                matches!(entry, Entry::ArtifactPending(pending) if pending.remote)
                    .then(|| work.clone())
            })
            .collect()
    }

    /// Snapshots publishes only physically owned Running or durable non-live attention truth.
    pub(crate) fn remote_snapshots(&self) -> Vec<(String, RunSnapshot)> {
        let mut snapshots = self
            .entries
            .iter()
            .filter(|(work_id, entry)| match entry {
                Entry::Running(work) => {
                    work.remote && (work.attention || self.native_owned.contains(work_id.as_str()))
                }
                Entry::ArtifactPending(work) => {
                    work.remote && self.native_owned.contains(work_id.as_str())
                }
                Entry::Done(work) => work.remote,
                Entry::Stopped(work) => work.remote,
            })
            .map(|(work_id, entry)| (work_id.clone(), snapshot(entry)))
            .collect::<Vec<_>>();
        snapshots.sort_by(|left, right| left.0.cmp(&right.0));
        snapshots
    }

    /// Save commits every transition atomically before terminal cleanup.
    pub(crate) fn save(&self, path: &Path) -> Result<(), String> {
        super::persist::save(&self.entries, path)
    }

    /// Contains protects Start idempotency before any new native runner can launch.
    pub(super) fn contains(&self, work_id: &str) -> bool {
        self.entries.contains_key(work_id)
    }

    /// IsRemote distinguishes Account-owned Work from desktop-local Work across retries.
    pub(super) fn is_remote(&self, work_id: &str) -> Option<bool> {
        self.entries.get(work_id).map(|entry| match entry {
            Entry::Running(work) => work.remote,
            Entry::ArtifactPending(work) => work.remote,
            Entry::Done(work) => work.remote,
            Entry::Stopped(work) => work.remote,
        })
    }

    /// MarkRemote upgrades only an Account-proven legacy Work; every unrelated local Work stays local.
    pub(super) fn mark_remote(&mut self, work_id: &str) -> bool {
        let Some(entry) = self.entries.get_mut(work_id) else {
            return false;
        };
        match entry {
            Entry::Running(work) => work.remote = true,
            Entry::ArtifactPending(work) => work.remote = true,
            Entry::Done(work) => work.remote = true,
            Entry::Stopped(work) => work.remote = true,
        }
        true
    }

    /// InsertRunning publishes one owner only after its workspace and identity are complete.
    pub(super) fn insert_running(&mut self, running: Running) {
        let work_id = running.work_id.clone();
        let active = !running.attention;
        self.entries
            .insert(work_id.clone(), Entry::Running(running));
        if active {
            crate::keepalive::work_started(&work_id);
        }
    }

    /// RemoveRunning rolls back an admission whose durable write failed before launch.
    pub(super) fn remove_running(&mut self, work_id: &str) {
        if matches!(self.entries.get(work_id), Some(Entry::Running(_))) {
            self.entries.remove(work_id);
            crate::keepalive::work_stopped(work_id);
        }
    }

    /// NativeOwned changes only the ephemeral proof that this process holds one AgentRun.
    pub(super) fn set_native_owned(&mut self, work_id: &str, owned: bool) -> bool {
        if owned {
            self.native_owned.insert(work_id.to_owned())
        } else {
            self.native_owned.remove(work_id)
        }
    }

    /// Snapshot retries artifact verification but never lets public Done outrun durable truth.
    pub(super) fn snapshot(
        &mut self,
        work_id: &str,
        state_path: &Path,
    ) -> Result<Option<RunSnapshot>, String> {
        self.promote_artifact(work_id, state_path)?;
        Ok(self.entries.get(work_id).map(snapshot))
    }

    /// Running exposes exactly the durable facts required for restart reattachment.
    pub(super) fn running(&self) -> Vec<RuntimeRecovery> {
        self.entries
            .values()
            .filter_map(|entry| match entry {
                Entry::Running(work) if !work.attention => recovery(work),
                _ => None,
            })
            .collect()
    }

    /// Binding exposes one durable Running owner, parked or executing, so parked bytes can still deliver.
    pub(super) fn binding(&self, work_id: &str) -> Option<RuntimeRecovery> {
        match self.entries.get(work_id) {
            Some(Entry::Running(work)) => recovery(work),
            _ => None,
        }
    }

    /// ParkedIds names every Work waiting on its owner, so a restart can look for undelivered bytes.
    pub(super) fn parked_ids(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter_map(|(work_id, entry)| {
                matches!(entry, Entry::Running(work) if work.attention).then(|| work_id.clone())
            })
            .collect()
    }

    /// SessionLeased reports a live Work other than the excluded one already owning this exact
    /// Provider conversation.
    pub(super) fn session_leased(
        &self,
        provider: crate::provider::Provider,
        native_session: &str,
        except: Option<&str>,
    ) -> bool {
        !native_session.is_empty()
            && self.entries.values().any(|entry| {
                matches!(entry, Entry::Running(work)
                    if work.provider == provider
                        && work.native_session == native_session
                        && except != Some(work.work_id.as_str()))
            })
    }

    /// LiveConversationWork names the Running Work already executing this conversation, or the one
    /// holding the native session it is bound to, so a later brief reaches that Work as its own turn
    /// instead of forking a second Agent onto the same conversation.
    pub(super) fn live_conversation_work(
        &self,
        provider: crate::provider::Provider,
        conversation: &str,
        bound_session: Option<&str>,
    ) -> Option<String> {
        self.entries.values().find_map(|entry| {
            let Entry::Running(work) = entry else {
                return None;
            };
            if work.provider != provider {
                return None;
            }
            let holds_session = bound_session
                .is_some_and(|session| !session.is_empty() && work.native_session == session);
            // A Work that has not yet bound its conversation still owns it: its own stored input says so.
            let holds_conversation =
                crate::work::runtime::stored_runtime(&work.input_path, &work.work_id)
                    .is_ok_and(|(held, _)| held == conversation);
            (holds_session || holds_conversation).then(|| work.work_id.clone())
        })
    }

    /// ConversationLease names the binding a Running Work holds, so its end can release it.
    pub(super) fn conversation_lease(
        &self,
        work_id: &str,
    ) -> Option<(crate::provider::Provider, Option<String>, String)> {
        let Some(Entry::Running(work)) = self.entries.get(work_id) else {
            return None;
        };
        let (conversation, _) =
            crate::work::runtime::stored_runtime(&work.input_path, &work.work_id).ok()?;
        Some((work.provider, work.model_selection.clone(), conversation))
    }

    /// StopAuthority returns only the signal owned by the addressed native Running Work.
    pub(super) fn stop_authority(&self, work_id: &str) -> Option<OwnerStop> {
        match self.entries.get(work_id) {
            Some(Entry::Running(work)) => Some(work.stop.clone()),
            _ => None,
        }
    }

    /// NeedsAttention distinguishes a parked Running owner from an executing turn.
    pub(super) fn needs_attention(&self, work_id: &str) -> bool {
        matches!(self.entries.get(work_id), Some(Entry::Running(work)) if work.attention)
    }

    /// ProgressSequence resumes a new native continuation after the last public Provider action.
    pub(super) fn progress_sequence(&self, work_id: &str) -> usize {
        match self.entries.get(work_id) {
            Some(Entry::Running(work)) => work
                .progress
                .as_ref()
                .map(|progress| progress.sequence as usize)
                .unwrap_or_default(),
            _ => 0,
        }
    }

    /// Progress returns the one durable fact needed to roll back a failed replacement.
    pub(super) fn progress(&self, work_id: &str) -> Option<RunProgress> {
        match self.entries.get(work_id) {
            Some(Entry::Running(work)) => work.progress.clone(),
            _ => None,
        }
    }

    /// ReplaceProgress keeps the latest actionable Provider fact and its monotonic replay proof.
    pub(super) fn replace_progress(
        &mut self,
        work_id: &str,
        index: usize,
        update: String,
    ) -> Result<bool, String> {
        let Some(Entry::Running(work)) = self.entries.get_mut(work_id) else {
            return Ok(false);
        };
        let previous = work.progress.as_ref();
        let sequence = previous
            .map(|progress| progress.sequence as usize)
            .unwrap_or_default();
        if index + 1 < sequence {
            return Ok(false);
        }
        if index + 1 == sequence {
            return if previous.is_some_and(|progress| progress.text == update) {
                Ok(false)
            } else {
                Err("Provider progress changed during recovery".to_owned())
            };
        }
        if index != sequence {
            return Err("Provider progress order changed during recovery".to_owned());
        }
        work.progress = Some(RunProgress {
            sequence: (index + 1) as u64,
            text: update,
        });
        Ok(true)
    }

    /// RollbackProgress restores the one prior fact when its durable replacement failed.
    pub(super) fn rollback_progress(&mut self, work_id: &str, progress: Option<RunProgress>) {
        if let Some(Entry::Running(work)) = self.entries.get_mut(work_id) {
            work.progress = progress;
        }
    }

    /// Tokens returns the one durable count needed to roll back a failed replacement.
    pub(super) fn tokens(&self, work_id: &str) -> Option<u64> {
        match self.entries.get(work_id) {
            Some(Entry::Running(work)) => work.tokens,
            _ => None,
        }
    }

    /// ReplaceTokens keeps the public count monotonic so journal replay can never regress it.
    pub(super) fn replace_tokens(&mut self, work_id: &str, total: u64) -> bool {
        let Some(Entry::Running(work)) = self.entries.get_mut(work_id) else {
            return false;
        };
        if work.tokens.is_some_and(|current| current >= total) {
            return false;
        }
        work.tokens = Some(total);
        true
    }

    /// RollbackTokens restores the prior count when its durable replacement failed.
    pub(super) fn rollback_tokens(&mut self, work_id: &str, tokens: Option<u64>) {
        if let Some(Entry::Running(work)) = self.entries.get_mut(work_id) {
            work.tokens = tokens;
        }
    }

    /// Model returns the one durable identity needed to roll back a failed replacement.
    pub(super) fn model(&self, work_id: &str) -> Option<String> {
        match self.entries.get(work_id) {
            Some(Entry::Running(work)) => work.model.clone(),
            _ => None,
        }
    }

    /// ReplaceModel records the Provider-reported executing model exactly as announced.
    pub(super) fn replace_model(&mut self, work_id: &str, model: String) -> bool {
        let Some(Entry::Running(work)) = self.entries.get_mut(work_id) else {
            return false;
        };
        if work.model.as_deref() == Some(model.as_str()) {
            return false;
        }
        work.model = Some(model);
        true
    }

    /// RollbackModel restores the prior identity when its durable replacement failed.
    pub(super) fn rollback_model(&mut self, work_id: &str, model: Option<String>) {
        if let Some(Entry::Running(work)) = self.entries.get_mut(work_id) {
            work.model = model;
        }
    }

    /// MarkLaunched durably closes launch admission before recovery becomes reattach-only. This
    /// launch is also where an armed follow-up's exact words reach a runner, so it is recorded.
    pub(super) fn mark_launched(&mut self, work_id: &str) -> bool {
        let Some(Entry::Running(work)) = self.entries.get_mut(work_id) else {
            return false;
        };
        if work.launched {
            return false;
        }
        work.launched = true;
        work.steer_delivered |= work.steering;
        true
    }

    /// RestoreUnlaunched keeps memory aligned with disk when launch evidence cannot commit. Only the
    /// admission is taken back: the launch itself already happened, so the follow-up it carried stays
    /// delivered and no later runner is asked to do that same thing again.
    pub(super) fn restore_unlaunched(&mut self, work_id: &str) {
        if let Some(Entry::Running(work)) = self.entries.get_mut(work_id) {
            work.launched = false;
        }
    }

    /// Finish replaces Running with one verified result but does not publish before durable save.
    pub(super) fn finish(
        &mut self,
        work_id: &str,
        answer: String,
        kind: ResultKind,
        run: Option<String>,
        native_session: String,
        harvested: Harvested,
    ) -> Option<Running> {
        let Entry::Running(running) = self.entries.remove(work_id)? else {
            return None;
        };
        crate::keepalive::work_stopped(work_id);
        let manifest = harvested.manifest.clone();
        let freeze_root = (kind == ResultKind::Artifact).then(|| running.freeze_root.clone());
        self.entries.insert(
            work_id.to_owned(),
            Entry::Done(DoneWork {
                remote: running.remote,
                work_id: running.work_id.clone(),
                session: Some(running.session.clone()),
                answer,
                kind,
                run,
                native_session,
                // Account acknowledgement owns cleanup even when delivery contains only an answer.
                freeze_root,
                manifest,
                harvested: Some(harvested),
                started_at: running.started_at,
                ended_at: crate::work::runtime::now_ms().ok(),
            }),
        );
        Some(running)
    }

    /// RestoreRunning reverses only an uncommitted Done so success cannot outrun disk.
    pub(super) fn restore_running(&mut self, running: Running) {
        let work_id = running.work_id.clone();
        let active = !running.attention;
        self.entries
            .insert(work_id.clone(), Entry::Running(running));
        if active {
            crate::keepalive::work_started(&work_id);
        }
    }

    /// MarkStopped prepares one terminal end while retaining the previous entry for save rollback; only a runner that already ended may skip owner authority, and its end is durably recorded as not the owner's.
    pub(super) fn mark_stopped(
        &mut self,
        work_id: &str,
        owner: bool,
        reason: String,
    ) -> Option<Entry> {
        let entry = self.entries.remove(work_id)?;
        let (remote, session, freeze_root, started_at) = match &entry {
            Entry::Running(work) if work.stop.requested() || !owner => (
                work.remote,
                Some(work.session.clone()),
                Some(work.freeze_root.clone()),
                work.started_at,
            ),
            Entry::ArtifactPending(work) => (
                work.remote,
                work.session.clone(),
                Some(work.freeze_root.clone()),
                work.started_at,
            ),
            _ => {
                self.entries.insert(work_id.to_owned(), entry);
                return None;
            }
        };
        crate::keepalive::work_stopped(work_id);
        self.entries.insert(
            work_id.to_owned(),
            Entry::Stopped(StoppedWork {
                remote,
                work_id: work_id.to_owned(),
                session,
                freeze_root,
                started_at,
                ended_at: crate::work::runtime::now_ms().ok(),
                owner,
                reason,
            }),
        );
        Some(entry)
    }

    /// RestoreEntry reverses an uncommitted terminal transition before public release.
    pub(super) fn restore_entry(&mut self, work_id: &str, entry: Entry) {
        let active = running_needs_power(&entry);
        self.entries.insert(work_id.to_owned(), entry);
        if active {
            crate::keepalive::work_started(work_id);
        }
    }

    /// TerminalPaths exposes only private trees whose Done, owner Stopped, or Failed truth Account already owns.
    pub(super) fn terminal_paths(
        &self,
        work_id: &str,
    ) -> Option<(Option<PathBuf>, Option<PathBuf>)> {
        match self.entries.get(work_id) {
            Some(Entry::Done(work)) => Some((work.session.clone(), work.freeze_root.clone())),
            Some(Entry::Stopped(work)) => Some((work.session.clone(), work.freeze_root.clone())),
            _ => None,
        }
    }

    /// CanSwapRelease admits a binary swap only while no Work is running or reverifying its artifact.
    pub(crate) fn can_swap_release(&self) -> bool {
        !self
            .entries
            .values()
            .any(|entry| matches!(entry, Entry::Running(_) | Entry::ArtifactPending(_)))
    }

    /// TerminalSessions maps every session tree to its persisted end, so retention never mistakes a live Work for garbage.
    pub(crate) fn terminal_sessions(&self) -> Vec<(PathBuf, Option<u64>)> {
        self.entries
            .values()
            .filter_map(|entry| match entry {
                // A live or reverifying Work carries no end, which retention reads as untouchable.
                Entry::Running(work) => Some((work.session.clone(), None)),
                Entry::ArtifactPending(work) => work.session.clone().map(|path| (path, None)),
                Entry::Done(work) => work.session.clone().map(|path| (path, work.ended_at)),
                Entry::Stopped(work) => work.session.clone().map(|path| (path, work.ended_at)),
            })
            .collect()
    }

    /// DeliveryRoots maps every frozen tree to its persisted end, so retention never sweeps live bytes.
    pub(crate) fn delivery_roots(&self) -> Vec<(PathBuf, Option<u64>)> {
        self.entries
            .values()
            .filter_map(|entry| match entry {
                // A Work that can still deliver carries no end, which retention reads as untouchable.
                Entry::Running(work) => Some((work.freeze_root.clone(), None)),
                Entry::ArtifactPending(work) => Some((work.freeze_root.clone(), None)),
                Entry::Done(work) => work.freeze_root.clone().map(|path| (path, work.ended_at)),
                Entry::Stopped(work) => work.freeze_root.clone().map(|path| (path, work.ended_at)),
            })
            .collect()
    }

    /// RetireReaped drops terminal entries whose expired session trees retention already removed, so the store never regrows around deleted workspaces.
    pub(crate) fn retire_reaped(&mut self, reaped: &HashSet<PathBuf>) -> bool {
        let before = self.entries.len();
        self.entries.retain(|_, entry| match entry {
            Entry::Done(work) => work
                .session
                .as_ref()
                .is_none_or(|path| !reaped.contains(path)),
            Entry::Stopped(work) => work
                .session
                .as_ref()
                .is_none_or(|path| !reaped.contains(path)),
            _ => true,
        });
        before != self.entries.len()
    }

    /// TakeTerminal removes only an Account-acknowledged terminal Work while retaining rollback truth.
    pub(super) fn take_terminal(&mut self, work_id: &str) -> Option<Entry> {
        if !matches!(
            self.entries.get(work_id),
            Some(Entry::Done(_) | Entry::Stopped(_))
        ) {
            return None;
        }
        self.entries.remove(work_id)
    }
}

/// Recovery projects one durable Running record onto the facts every resume and delivery path needs.
fn recovery(work: &Running) -> Option<RuntimeRecovery> {
    let (conversation, resume) =
        crate::work::runtime::stored_runtime(&work.input_path, &work.work_id).ok()?;
    Some(RuntimeRecovery {
        conversation,
        work_id: work.work_id.clone(),
        session: work.session.clone(),
        freeze_root: work.freeze_root.clone(),
        native_session: work.native_session.clone(),
        model_selection: work.model_selection.clone(),
        effort_selection: work.effort_selection.clone(),
        resume,
        runner_id: work.runner_id.clone(),
        input_path: work.input_path.clone(),
        started_at: work.started_at,
        launched: work.launched,
        repair: work.repair,
        failure: work.failure.clone(),
        steer: work.steering.then(|| work.steer.clone()).flatten(),
        steer_delivered: work.steer_delivered,
        rotating: work.rotating,
        provider: work.provider,
    })
}

/// RunningFrom builds the only durable native owner representation.
pub(super) fn running_from(runtime: &RuntimeWork, remote: bool) -> Running {
    Running::from_runtime(runtime, remote)
}
