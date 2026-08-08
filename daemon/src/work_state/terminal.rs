//! Terminal acknowledgement deletes private Work data only after Account owns Done or owner Stopped.

use crate::state::DaemonState;

// This terminal path starts exactly one native session for an admitted Work.
impl DaemonState {
    /// SettleRefusedWork records a delivery Account refused while its terminal entry, frozen bytes and
    /// workspace file all keep addressing the same finished product, so a later retry still finds it.
    pub(crate) fn settle_refused_work(&self, work_id: &str) -> Result<(), String> {
        let works = self
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // BOUNDARY: only Account's own terminal truth may be settled; a Running Work keeps executing.
        if works.terminal_paths(work_id).is_none() && works.contains(work_id) {
            return Err("Account refused a Work that is still Running".to_owned());
        }
        drop(works);
        // A server status pauses one delivery and proves nothing about the product it carried, so
        // nothing here is deleted; only Account acknowledgement removes a finished Work.
        eprintln!("Product Work delivery was refused and its finished result stays: {work_id}");
        Ok(())
    }

    /// AcknowledgeWork settles terminal truth even when private bookkeeping needs later repair.
    pub(crate) fn acknowledge_work(&self, work_id: &str) -> Result<(), String> {
        let checkpoint = {
            let mut works = self
                .works
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let checkpoint = works.settle_checkpoint(work_id);
            if checkpoint.as_ref().is_some_and(|(_, changed)| *changed)
                && let Err(error) = works.save(self.work_state_path())
            {
                works.rollback_checkpoint(work_id);
                return Err(error);
            }
            checkpoint
        };
        if let Some((freeze_root, _)) = checkpoint {
            // Account owns this turn's bytes. The Work session remains for Build/Edit/Draft.
            if let Err(error) = crate::delivery::discard_private_tree(&freeze_root) {
                eprintln!("Product could not discard checkpoint delivery: {error}");
            }
            self.work_events.notify_waiters();
            return Ok(());
        }
        let paths = {
            let works = self
                .works
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match works.terminal_paths(work_id) {
                Some(paths) => Some(paths),
                // BOUNDARY: only Account's own terminal truth may be acknowledged and deleted.
                None if works.contains(work_id) => {
                    return Err("Account acknowledged a Work that is still Running".to_owned());
                }
                None => None,
            }
        };

        // Account ownership permits deleting only bytes bound to this terminal Work.
        if let Some((session, freeze_root)) = &paths {
            if let Some(root) = freeze_root {
                if let Err(error) = crate::delivery::discard_private_tree(root) {
                    eprintln!("Product could not discard acknowledged delivery: {error}");
                }
            }
            if let Some(root) = session {
                if let Err(error) = crate::work::input_view::InputView::discard_session(root) {
                    eprintln!("Product could not discard Cursor input view: {error}");
                }
                if let Err(error) = crate::delivery::discard_private_tree(root) {
                    eprintln!("Product could not discard acknowledged session: {error}");
                }
            }
        }
        if let Err(error) = self.discard_work_inputs(work_id) {
            eprintln!("Product could not discard acknowledged inputs: {error}");
        }

        // Durable removal is last so a failed cleanup or save remains safe to retry.
        let Some(_) = paths else {
            return Ok(());
        };
        let mut works = self
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(entry) = works.take_terminal(work_id) else {
            return Ok(());
        };
        if let Err(error) = works.save(self.work_state_path()) {
            works.restore_entry(work_id, entry);
            eprintln!("Product could not settle acknowledged Work state: {error}");
            return Ok(());
        }
        self.work_events.notify_waiters();
        Ok(())
    }

    /// PublishWork is the only Account order that deletes a delivered creator Work and its runtime.
    pub(crate) fn publish_work(&self, work_id: &str) -> Result<(), String> {
        let paths = self
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .publish_paths(work_id);
        let Some((session, freeze_root)) = paths else {
            if self
                .works
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .contains(work_id)
            {
                return Err("Publish requires a delivered Work".to_owned());
            }
            return Ok(());
        };
        if let Err(error) = crate::delivery::discard_private_tree(&freeze_root) {
            eprintln!("Product could not discard published delivery: {error}");
        }
        if let Err(error) = crate::work::input_view::InputView::discard_session(&session) {
            eprintln!("Product could not discard published input view: {error}");
        }
        if let Err(error) = crate::delivery::discard_private_tree(&session) {
            eprintln!("Product could not discard published session: {error}");
        }
        if let Err(error) = self.discard_work_inputs(work_id) {
            eprintln!("Product could not discard published inputs: {error}");
        }
        let mut works = self
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(entry) = works.take_published(work_id) else {
            return Ok(());
        };
        if let Err(error) = works.save(self.work_state_path()) {
            works.restore_entry(work_id, entry);
            return Err(error);
        }
        drop(works);
        self.work_events.notify_waiters();
        Ok(())
    }
}
