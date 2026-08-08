//! Work observation exposes only durable public changes through one event-backed wait.

use std::time::Duration;

use crate::state::{DaemonState, RunPhase, RunSnapshot};

use super::model::Entry;

/// The relay executes one Account command at a time, so a held read must end well inside the
/// forty-five second presence window: a quiet Work may never stop this desktop from polling for
/// the next brief a phone queued, nor from proving it is awake.
const OBSERVE_HOLD: Duration = Duration::from_secs(20);

// This observer lets clients wait for real Work changes without polling.
impl DaemonState {
    /// AppendWorkAnswer durably exposes one monotonic Provider text update while its Work runs.
    pub(crate) fn append_work_answer(&self, work_id: &str, candidate: &str) -> Result<(), String> {
        let context = {
            let works = self
                .works
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match works.entries.get(work_id) {
                Some(Entry::Running(work)) => Some((
                    work.runner_id.clone(),
                    work.session.clone(),
                    work.input_path.clone(),
                    work.freeze_root.clone(),
                )),
                _ => None,
            }
        };
        let Some((runner, session, input, freeze)) = context else {
            return Ok(());
        };
        let mut candidate = candidate.to_owned();
        crate::work::redact_answer(&runner, &session, &input, &freeze, &mut candidate);
        let mut works = self
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = match works.entries.get(work_id) {
            Some(Entry::Running(work)) => Some(work.answer.clone()),
            _ => None,
        };
        let changed = match works.entries.get_mut(work_id) {
            Some(Entry::Running(work)) => work.append_answer(&candidate),
            _ => false,
        };
        if !changed {
            return Ok(());
        }
        if let Err(error) = works.save(self.work_state_path()) {
            if let (Some(previous), Some(Entry::Running(work))) =
                (previous, works.entries.get_mut(work_id))
            {
                work.answer = previous;
            }
            return Err(error);
        }
        drop(works);
        self.work_events.notify_waiters();
        self.relay_events.notify_one();
        Ok(())
    }

    /// DeliveryCandidates exposes only durable Done entries awaiting Account acknowledgement.
    pub(crate) fn remote_delivery_candidates(&self) -> Vec<(String, RunSnapshot)> {
        let mut works = self
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for work in works.remote_pending_artifacts() {
            if let Err(error) = works.promote_artifact(&work, self.work_state_path()) {
                eprintln!("Product remote artifact promotion retry: {error}");
            }
        }
        works.remote_delivery_candidates()
    }

    /// RunSnapshots exposes all durable public facts to the outbound Account status worker.
    pub(crate) fn remote_snapshots(&self) -> Vec<(String, RunSnapshot)> {
        self.works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remote_snapshots()
    }

    /// RunSnapshot keeps an uncommitted artifact promotion publicly Running.
    pub(crate) fn run_snapshot(&self, work_id: &str) -> Option<RunSnapshot> {
        let mut works = self
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match works.snapshot(work_id, self.work_state_path()) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                eprintln!("Product artifact promotion remains Running: {error}");
                works.entries.get(work_id).map(super::model::snapshot)
            }
        }
    }

    /// ObserveWork holds one relay request until this Work publishes newer public truth.
    pub(crate) async fn observe_work(&self, work_id: &str) -> Option<RunSnapshot> {
        let observed = self.run_snapshot(work_id)?;
        if observed.phase != RunPhase::Running {
            return Some(observed);
        }
        let deadline = tokio::time::Instant::now() + OBSERVE_HOLD;
        loop {
            // Registration before the read prevents a concurrent Work transition from being missed.
            let changed = self.work_events.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let current = self.run_snapshot(work_id)?;
            if current != observed {
                return Some(current);
            }
            // Unchanged truth is still truth: its reader re-reads, and this desktop stays reachable.
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                return Some(observed);
            }
        }
    }
}
