//! Artifact state serves, acknowledges, and re-verifies only receipts owned by one Done Work.

use std::{fs::File, path::Path};

use crate::delivery::{DeliveryFile, Harvested};

use super::{
    model::{DoneWork, Entry},
    store::WorkStore,
};

// This store records verified artifacts required for a truthful Done Work.
impl WorkStore {
    /// OpenArtifact serves bytes only from a verified Done run awaiting Account acknowledgement.
    pub(super) fn open_artifact(
        &self,
        run: &str,
        name: &str,
    ) -> Result<(File, DeliveryFile), String> {
        self.entries
            .values()
            .find_map(|entry| match entry {
                Entry::Checkpoint(work) if work.run.as_deref() == Some(run) => work
                    .harvested
                    .as_ref()
                    .map(|harvested| harvested.open(name)),
                Entry::Done(done) if done.run.as_deref() == Some(run) => done
                    .harvested
                    .as_ref()
                    .map(|harvested| harvested.open(name)),
                _ => None,
            })
            .unwrap_or_else(|| Err("Artifact run is unavailable".to_owned()))
    }

    /// PromoteArtifact exposes Done only after verified bytes and the promotion commit together.
    pub(super) fn promote_artifact(
        &mut self,
        work_id: &str,
        state_path: &Path,
    ) -> Result<(), String> {
        let Some(entry) = self.entries.remove(work_id) else {
            return Ok(());
        };
        let Entry::ArtifactPending(pending) = entry else {
            self.entries.insert(work_id.to_owned(), entry);
            return Ok(());
        };
        match Harvested::rehydrate(
            pending.answer.clone(),
            pending.kind,
            pending.manifest.clone(),
            pending.freeze_root.clone(),
        ) {
            Ok(harvested) => {
                self.entries.insert(
                    work_id.to_owned(),
                    Entry::Done(DoneWork {
                        remote: pending.remote,
                        work_id: pending.work_id.clone(),
                        session: pending.session.clone(),
                        answer: pending.answer.clone(),
                        kind: pending.kind,
                        run: Some(pending.run.clone()),
                        native_session: pending.native_session.clone(),
                        manifest: pending.manifest.clone(),
                        freeze_root: Some(pending.freeze_root.clone()),
                        harvested: Some(harvested),
                        started_at: pending.started_at,
                        ended_at: crate::work::runtime::now_ms().ok(),
                    }),
                );
                if let Err(error) = self.save(state_path) {
                    self.entries
                        .insert(work_id.to_owned(), Entry::ArtifactPending(pending));
                    return Err(error);
                }
                crate::keepalive::work_stopped(work_id);
                Ok(())
            }
            Err(_) => {
                self.entries
                    .insert(work_id.to_owned(), Entry::ArtifactPending(pending));
                Ok(())
            }
        }
    }
}
