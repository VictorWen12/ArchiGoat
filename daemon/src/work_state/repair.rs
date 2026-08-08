//! Repair atomically advances one Work through sequential Provider-native continuations.

use super::{
    model::{Entry, MAX_REPAIRS},
    store::WorkStore,
};
use crate::{
    state::{DaemonState, RunProgress},
    work::RuntimeWork,
};

/// RepairRollback restores the exact runner binding if durable rotation cannot commit.
pub(super) struct RepairRollback {
    native_session: String,
    launched: bool,
    repair: bool,
    repairs: u32,
    attention: bool,
    failure: Option<String>,
}

/// AttentionRollback restores an exhausted Work if its resumable park cannot commit.
struct AttentionRollback {
    attention: bool,
    failure: Option<String>,
    progress: Option<RunProgress>,
}

/// AttentionResumeRollback restores a parked Work if wake recovery cannot commit durably.
pub(super) struct AttentionResumeRollback {
    work_id: String,
    attention: bool,
    repair: bool,
    repairs: u32,
    launched: bool,
    failure: Option<String>,
    progress: Option<RunProgress>,
}

/// TransportShaped accepts only failures that can be repaired by reconnecting the same session.
pub(super) fn transport_shaped(reason: Option<&str>) -> bool {
    let reason = reason.unwrap_or_default().to_ascii_lowercase();
    [
        "network",
        "transport",
        "timed out",
        "timeout",
        "connection reset",
        "connection refused",
        "broken pipe",
        "disconnected",
        "unreachable",
        "eof",
    ]
    .iter()
    .any(|marker| reason.contains(marker))
}

// This repair path removes only corrupt persisted Work state before recovery.
impl WorkStore {
    /// BindSession captures the Provider identity required for safe continuation; a Provider that
    /// answers a resume with a new conversation rebinds here instead of stranding the Work.
    pub(super) fn bind_session(
        &mut self,
        work_id: &str,
        session: String,
    ) -> Result<Option<String>, String> {
        let Some(Entry::Running(work)) = self.entries.get_mut(work_id) else {
            return Err("Running Work binding is unavailable".to_owned());
        };
        if work.native_session == session {
            return Ok(None);
        }
        Ok(Some(std::mem::replace(&mut work.native_session, session)))
    }

    /// RestoreSession keeps memory aligned with disk after a failed identity commit.
    pub(super) fn restore_session(&mut self, work_id: &str, session: String) {
        if let Some(Entry::Running(work)) = self.entries.get_mut(work_id) {
            work.native_session = session;
        }
    }

    /// RotateRepair reopens the same Work runner identity only after its previous process is physically gone.
    pub(super) fn rotate_repair(
        &mut self,
        work_id: &str,
        native_session: String,
        reason: String,
    ) -> Result<RepairRollback, String> {
        let Some(Entry::Running(work)) = self.entries.get_mut(work_id) else {
            return Err("Running Work binding is unavailable".to_owned());
        };
        let rollback = RepairRollback {
            native_session: std::mem::replace(&mut work.native_session, native_session),
            launched: work.launched,
            repair: work.repair,
            repairs: work.repairs,
            attention: work.attention,
            failure: work.failure.clone(),
        };
        work.launched = false;
        work.repair = true;
        work.attention = false;
        // The durable count is what keeps a restart from re-arming an exhausted Work's budget.
        work.repairs += 1;
        work.failure = Some(reason);
        Ok(rollback)
    }

    /// RestoreRepair prevents an uncommitted runner identity from becoming executable.
    pub(super) fn restore_repair(&mut self, work_id: &str, rollback: RepairRollback) {
        if let Some(Entry::Running(work)) = self.entries.get_mut(work_id) {
            work.native_session = rollback.native_session;
            work.launched = rollback.launched;
            work.repair = rollback.repair;
            work.repairs = rollback.repairs;
            work.attention = rollback.attention;
            work.failure = rollback.failure;
        }
    }

    /// ParkAttention preserves the same native Work after self-repair reaches its bound.
    fn park_attention(&mut self, work_id: &str, reason: &str) -> Result<AttentionRollback, String> {
        let Some(Entry::Running(work)) = self.entries.get_mut(work_id) else {
            return Err("Running Work binding is unavailable".to_owned());
        };
        let rollback = AttentionRollback {
            attention: work.attention,
            failure: work.failure.clone(),
            progress: work.progress.clone(),
        };
        let attention = super::model::attention_text(work.provider, reason);
        work.attention = true;
        work.failure = Some(reason.to_owned());
        work.progress = Some(RunProgress {
            sequence: work
                .progress
                .as_ref()
                .map(|progress| progress.sequence.saturating_add(1))
                .unwrap_or(1),
            text: attention,
        });
        crate::keepalive::work_stopped(work_id);
        Ok(rollback)
    }

    /// RestoreAttention keeps memory aligned when a resumable park cannot persist.
    fn restore_attention(&mut self, work_id: &str, rollback: AttentionRollback) {
        if let Some(Entry::Running(work)) = self.entries.get_mut(work_id) {
            work.attention = rollback.attention;
            work.failure = rollback.failure;
            work.progress = rollback.progress;
            if !work.attention {
                crate::keepalive::work_started(work_id);
            }
        }
    }

    /// ResumeTransportAttention reopens only parked transport failures, preserving terminal attention causes.
    pub(super) fn resume_transport_attention(
        &mut self,
    ) -> Result<(Vec<RuntimeWork>, Vec<AttentionResumeRollback>), ()> {
        let ids = self
            .entries
            .iter()
            .filter_map(|(work_id, entry)| match entry {
                Entry::Running(work)
                    if work.attention && transport_shaped(work.failure.as_deref()) =>
                {
                    Some(work_id.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let mut rollback = Vec::new();
        for work_id in &ids {
            let Some(Entry::Running(work)) = self.entries.get_mut(work_id) else {
                continue;
            };
            rollback.push(AttentionResumeRollback {
                work_id: work_id.clone(),
                attention: work.attention,
                repair: work.repair,
                repairs: work.repairs,
                launched: work.launched,
                failure: work.failure.clone(),
                progress: work.progress.clone(),
            });
            work.attention = false;
            work.repair = true;
            work.repairs = 0;
            work.launched = false;
            let sequence = work
                .progress
                .as_ref()
                .map(|progress| progress.sequence.saturating_add(1))
                .unwrap_or(1);
            work.progress = Some(RunProgress {
                sequence,
                text: "Running".to_owned(),
            });
            crate::keepalive::work_started(work_id);
        }
        let recoveries = self
            .running()
            .into_iter()
            .filter(|runtime| ids.iter().any(|work_id| work_id == &runtime.work_id))
            .map(RuntimeWork::recover)
            .collect::<Vec<_>>();
        if recoveries.len() != ids.len() {
            self.restore_transport_attention(rollback);
            return Err(());
        }
        Ok((recoveries, rollback))
    }

    /// RestoreTransportAttention returns wake-mutated records to their parked state after a failed save.
    pub(super) fn restore_transport_attention(&mut self, rollback: Vec<AttentionResumeRollback>) {
        for prior in rollback {
            if let Some(Entry::Running(work)) = self.entries.get_mut(&prior.work_id) {
                work.attention = prior.attention;
                work.repair = prior.repair;
                work.repairs = prior.repairs;
                work.launched = prior.launched;
                work.failure = prior.failure;
                work.progress = prior.progress;
                if work.attention {
                    crate::keepalive::work_stopped(&prior.work_id);
                }
            }
        }
    }

    /// RepairBudgetSpent reports that this Work has already replaced as many runners as it may.
    pub(super) fn repair_budget_spent(&self, work_id: &str) -> bool {
        matches!(
            self.entries.get(work_id),
            Some(Entry::Running(work)) if work.repairs >= MAX_REPAIRS
        )
    }
}

// This repair path resumes valid Running Work after local interruption.
impl DaemonState {
    /// ResumeTransportAttentionAfterWake reopens every transport-shaped parked Work once per wake edge.
    pub(crate) fn resume_transport_attention_after_wake(&self) -> bool {
        let recoveries = {
            let mut works = self
                .works
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Ok((recoveries, rollback)) = works.resume_transport_attention() else {
                return false;
            };
            if recoveries.is_empty() {
                return true;
            }
            if let Err(error) = works.save(self.work_state_path()) {
                works.restore_transport_attention(rollback);
                eprintln!("Product wake recovery commit retry: {error}");
                return false;
            }
            recoveries
        };
        self.work_events.notify_waiters();
        self.relay_events.notify_one();
        for runtime in recoveries {
            let state = self.clone();
            tokio::spawn(async move { super::drive::run(state, runtime).await });
        }
        true
    }

    /// BindNativeSession commits the Provider conversation before any repair can be admitted.
    pub(crate) fn bind_native_session(&self, work_id: &str, session: String) -> Result<(), String> {
        let rotation = {
            let mut works = self
                .works
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(previous) = works.bind_session(work_id, session)?
                && let Err(error) = works.save(self.work_state_path())
            {
                works.restore_session(work_id, previous);
                return Err(error);
            }
            works.rotation_authority(work_id)
        };
        if let Some(rotation) = rotation {
            rotation.request();
        }
        Ok(())
    }

    /// PrepareAttention parks an evidence-overflow Work without spending or looping its repair budget.
    pub(crate) fn prepare_attention(&self, work_id: &str, reason: String) -> Result<(), String> {
        let mut works = self
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let rollback = works.park_attention(work_id, &reason)?;
        if let Err(error) = works.save(self.work_state_path()) {
            works.restore_attention(work_id, rollback);
            return Err(error);
        }
        drop(works);
        self.work_events.notify_waiters();
        self.relay_events.notify_one();
        Ok(())
    }

    /// PrepareRepair replaces only the dead turn; exhaustion parks this same Work for user continuation.
    pub(crate) fn prepare_repair(
        &self,
        work_id: &str,
        native_session: String,
        reason: String,
    ) -> Result<Option<RuntimeWork>, String> {
        // Frozen bytes belong to their owner from the moment they verify: the next verified freeze
        // replaces them, and nothing else in this Work's life may destroy them.
        // Exhaustion stays owned and steerable; it never borrows terminal failure.
        let spent = self
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .repair_budget_spent(work_id);
        if spent {
            let mut works = self
                .works
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let rollback = works.park_attention(work_id, &reason)?;
            if let Err(error) = works.save(self.work_state_path()) {
                works.restore_attention(work_id, rollback);
                return Err(error);
            }
            drop(works);
            self.work_events.notify_waiters();
            self.relay_events.notify_one();
            return Ok(None);
        }
        let mut works = self
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let rollback = works.rotate_repair(work_id, native_session, reason)?;
        if let Err(error) = works.save(self.work_state_path()) {
            works.restore_repair(work_id, rollback);
            return Err(error);
        }
        let work = works
            .running()
            .into_iter()
            .find(|work| work.work_id == work_id)
            .ok_or_else(|| "Running Work binding is unavailable".to_owned())?;
        Ok(Some(RuntimeWork::recover(work)))
    }
}
