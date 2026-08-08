//! Protected output evidence stays durable for one Work's complete native history.

use super::{model::Entry, store::WorkStore};
use crate::{
    state::DaemonState,
    work::{MAX_PROTECTED_BYTES, MAX_PROTECTED_ITEM_BYTES, MAX_PROTECTED_ITEMS},
};
use sha2::{Digest, Sha256};

const MIN_PROTECTED_LITERAL_BYTES: usize = 8;

struct Rollback {
    outputs: Vec<String>,
}

impl WorkStore {
    fn absorb_protected(
        &mut self,
        work_id: &str,
        outputs: &[String],
    ) -> Result<Option<Rollback>, String> {
        let Some(Entry::Running(work)) = self.entries.get_mut(work_id) else {
            return Err("Running Work binding is unavailable".to_owned());
        };
        let rollback = Rollback {
            outputs: work.protected_outputs.clone(),
        };
        let mut bytes = work
            .protected_outputs
            .iter()
            .map(String::len)
            .sum::<usize>();
        for output in outputs {
            if output.trim().len() < MIN_PROTECTED_LITERAL_BYTES {
                continue;
            }
            if work.protected_outputs.contains(output) {
                continue;
            }
            if output.len() <= MAX_PROTECTED_ITEM_BYTES
                && work.protected_outputs.len() < MAX_PROTECTED_ITEMS
                && bytes.saturating_add(output.len()) <= MAX_PROTECTED_BYTES
            {
                bytes += output.len();
                work.protected_outputs.push(output.clone());
            }
        }
        let changed = work.protected_outputs != rollback.outputs;
        Ok(changed.then_some(rollback))
    }

    fn restore_protected(&mut self, work_id: &str, rollback: Rollback) {
        if let Some(Entry::Running(work)) = self.entries.get_mut(work_id) {
            work.protected_outputs = rollback.outputs;
        }
    }
}

impl DaemonState {
    pub(super) fn record_protected_literal(
        &self,
        work_id: &str,
        output: &str,
    ) -> Result<(), String> {
        if output.trim().len() < MIN_PROTECTED_LITERAL_BYTES {
            return Ok(());
        }
        self.record_protected_outputs(work_id, &[output.to_owned()])?;
        let session = {
            let works = self
                .works
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match works.entries.get(work_id) {
                Some(Entry::Running(work)) => work.session.clone(),
                _ => return Err("Running Work binding is unavailable".to_owned()),
            }
        };
        let root = session.join(".app").join("protected-output");
        crate::host::create_private_dir(&root)?;
        let path = root.join(format!("{:x}", Sha256::digest(output.as_bytes())));
        if !path.exists() {
            crate::host::replace_private(&path, output.as_bytes())?;
        }
        Ok(())
    }

    /// RecordProtectedOutputs commits exact evidence before a physical turn may be released.
    pub(super) fn record_protected_outputs(
        &self,
        work_id: &str,
        outputs: &[String],
    ) -> Result<(), String> {
        let mut works = self
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(rollback) = works.absorb_protected(work_id, outputs)? else {
            return Ok(());
        };
        if let Err(error) = works.save(self.work_state_path()) {
            works.restore_protected(work_id, rollback);
            return Err(error);
        }
        Ok(())
    }
}
