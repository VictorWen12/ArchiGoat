//! Verified completion commits native evidence before any Work can become publicly Done.

use std::path::{Path, PathBuf};

use crate::{
    process::ObservedWork,
    state::DaemonState,
    work::{DeliveredWork, RuntimeRecovery},
};

use super::model::Entry;

/// CompletionError separates repairable user-value gaps from transient durable commit failures.
pub(super) enum CompletionError {
    /// Repair continues the same native Work because its terminal output cannot become truthful Done.
    Repair(String),
    /// Retry preserves the verified result while local durable state is temporarily unavailable.
    Retry(String),
}

// This completion path makes Done visible only after verified native delivery.
impl DaemonState {
    /// CompleteObserved accepts native completion only when answer or every artifact byte verifies.
    pub(super) fn complete_observed(
        &self,
        work_id: &str,
        observed: &ObservedWork,
    ) -> Result<(), CompletionError> {
        let binding = self.work_binding(work_id).ok_or_else(|| {
            CompletionError::Retry("Running Work binding is unavailable".to_owned())
        })?;
        let freeze_root = binding.freeze_root.clone();
        if binding.native_session != observed.native_session {
            return Err(CompletionError::Repair(
                "Provider native session does not match this Work".to_owned(),
            ));
        }
        // Answer and artifacts must agree with the Work-owned frozen receipt before Done is prepared.
        let artifacts = crate::work::load_delivery_receipt(work_id, &freeze_root)
            .map_err(CompletionError::Repair)?;
        let durable_answer = {
            let works = self
                .works
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match works.entries.get(work_id) {
                Some(Entry::Running(work)) => work.answer.clone(),
                _ => String::new(),
            }
        };
        let mut answer = if durable_answer.trim().is_empty() {
            observed.answer.clone()
        } else {
            Some(durable_answer)
        };
        binding.validate_inputs().map_err(CompletionError::Repair)?;
        crate::work::validate_egress(
            &binding.runner_id,
            &binding.session,
            &binding.input_path,
            &binding.freeze_root,
            &mut answer,
            &artifacts,
        )
        .map_err(CompletionError::Repair)?;
        let delivered = DeliveredWork::verified(work_id.to_owned(), answer, artifacts)
            .map_err(CompletionError::Repair)?;
        // A Work that ends before its session binds names no conversation, and a Work that forked its
        // own session never takes the conversation from the Work still holding it.
        if !observed.native_session.is_empty()
            && !self.conversation_held_elsewhere(&binding, work_id, &observed.native_session)
        {
            self.bind_conversation(
                binding.provider,
                binding.model_selection.as_deref(),
                &binding.conversation,
                &observed.native_session,
            )
            .map_err(CompletionError::Retry)?;
        }
        let harvested = crate::delivery::harvest(delivered);
        if !harvested.manifest.is_empty() {
            let names = harvested
                .manifest
                .iter()
                .map(|file| file.name.as_str())
                .collect::<Vec<_>>();
            if let Err(error) = self
                .conversation_app_root(&binding.conversation)
                .and_then(|store| crate::work::app::replace(&store, &freeze_root, &names))
            {
                eprintln!("Product could not keep delivered app: {error}");
            }
        }
        let answer = harvested.answer.clone();
        let kind = harvested.kind;
        let run_id = (!harvested.manifest.is_empty()).then(|| {
            freeze_root
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or(work_id)
                .to_owned()
        });
        let rollback = self
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .finish(
                work_id,
                answer,
                kind,
                run_id,
                observed.native_session.clone(),
                harvested,
            )
            .ok_or_else(|| CompletionError::Retry("Work is no longer Running".to_owned()))?;
        // Disk failure rolls the tentative Done back to Running, preventing false public success.
        if let Err(error) = self.persist_works_result() {
            self.works
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .restore_finished(work_id, rollback);
            return Err(CompletionError::Retry(error));
        }
        if kind == crate::work::ResultKind::Answer
            && let Err(error) = crate::delivery::discard_private_tree(&freeze_root)
        {
            eprintln!("Product could not discard empty answer freeze: {error}");
        }
        self.release_work(work_id);
        self.work_events.notify_waiters();
        self.relay_events.notify_one();
        Ok(())
    }

    /// ConversationHeldElsewhere reports another live Work owning the session this conversation names,
    /// so a Work that forked its own session never redirects the conversation away from its holder.
    fn conversation_held_elsewhere(
        &self,
        binding: &RuntimeRecovery,
        work_id: &str,
        native_session: &str,
    ) -> bool {
        let Ok(Some(bound)) = self.conversation_session(
            binding.provider,
            binding.model_selection.as_deref(),
            &binding.conversation,
        ) else {
            return false;
        };
        bound != native_session
            && self
                .works
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .session_leased(binding.provider, &bound, Some(work_id))
    }

    /// RehydrateParkedDeliveries hands every parked Work's finished product to its owner after a restart.
    pub(crate) fn rehydrate_parked_deliveries(&self) {
        let parked = self
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .parked_ids();
        for work_id in parked {
            if let Err(reason) = self.rehydrate_parked(&work_id) {
                eprintln!("Product could not deliver a parked Work: {reason}");
            }
        }
    }

    /// RehydrateParked delivers a parked Work's frozen bytes, freezing its workspace first when they are gone.
    fn rehydrate_parked(&self, work_id: &str) -> Result<(), String> {
        let binding = self
            .work_binding(work_id)
            .ok_or_else(|| "Parked Work binding is unavailable".to_owned())?;
        // Bytes that still verify deliver exactly as they are; only their absence rebuilds a freeze.
        if !deliverable(work_id, &binding.freeze_root) {
            crate::delivery::discard_private_tree(&binding.freeze_root)?;
            crate::work::freeze_delivery_receipt(
                work_id,
                &workspace_root(&binding.session),
                &binding.session,
                &binding.freeze_root,
            )?;
            if !deliverable(work_id, &binding.freeze_root) {
                // An empty workspace holds no product; this Work stays parked for its owner.
                return crate::delivery::discard_private_tree(&binding.freeze_root);
            }
        }
        let observed = ObservedWork {
            native_session: binding.native_session.clone(),
            answer: None,
            terminal_sequence: 0,
        };
        self.complete_observed(work_id, &observed)
            .map_err(|error| match error {
                CompletionError::Repair(reason) | CompletionError::Retry(reason) => reason,
            })
    }
}

/// Deliverable reports frozen bytes that still match their receipt and carry at least one product file.
fn deliverable(work_id: &str, freeze_root: &Path) -> bool {
    crate::work::load_delivery_receipt(work_id, freeze_root)
        .is_ok_and(|artifacts| !artifacts.is_empty())
}

/// WorkspaceRoot resolves the same deliverable tree the runner freezes from: the Work subtree when it
/// exists, the session itself for Work admitted before that split.
fn workspace_root(session: &Path) -> PathBuf {
    let workspace = session.join("Work");
    workspace
        .is_dir()
        .then_some(workspace)
        .unwrap_or_else(|| session.to_path_buf())
}
