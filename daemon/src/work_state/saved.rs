//! Saved Work encoding keeps only facts that restore ownership or prove a terminal result.

use super::{model::Entry, persist::SavedWork};

/// SaveEntry converts live synchronization state into restart-safe product facts.
pub(super) fn save_entry(entry: &Entry) -> SavedWork {
    match entry {
        // Running retains the one native owner and immutable launch input.
        Entry::Running(work) => SavedWork::Running {
            remote: work.remote,
            work_id: work.work_id.clone(),
            provider: work.provider,
            model_selection: work.model_selection.clone(),
            effort_selection: work.effort_selection.clone(),
            session: work.session.clone(),
            freeze_root: work.freeze_root.clone(),
            native_session: work.native_session.clone(),
            runner_id: work.runner_id.clone(),
            input_path: work.input_path.clone(),
            input: String::new(),
            launched: work.launched,
            repair: work.repair,
            steer: work.steer.clone(),
            steers: work.steers.clone(),
            steering: work.steering,
            steer_delivered: work.steer_delivered,
            rotating: work.rotating,
            stopping: work.stopping,
            repairs: work.repairs,
            attention: work.attention,
            failure: work.failure.clone(),
            started_at: work.started_at,
            answer: work.answer.clone(),
            progress: work.progress.clone(),
            tokens: work.tokens,
            model: work.model.clone(),
            protected_outputs: work.protected_outputs.clone(),
        },
        // Checkpoint retains the same native owner plus one Account-deliverable turn.
        Entry::Checkpoint(checkpoint) => {
            let work = &checkpoint.running;
            SavedWork::Checkpoint {
                remote: work.remote,
                work_id: work.work_id.clone(),
                provider: work.provider,
                model_selection: work.model_selection.clone(),
                effort_selection: work.effort_selection.clone(),
                session: work.session.clone(),
                freeze_root: work.freeze_root.clone(),
                native_session: work.native_session.clone(),
                runner_id: work.runner_id.clone(),
                input_path: work.input_path.clone(),
                input: String::new(),
                launched: work.launched,
                repair: work.repair,
                steer: work.steer.clone(),
                steers: work.steers.clone(),
                steering: work.steering,
                steer_delivered: work.steer_delivered,
                rotating: work.rotating,
                stopping: work.stopping,
                repairs: work.repairs,
                attention: work.attention,
                failure: work.failure.clone(),
                started_at: work.started_at,
                answer: checkpoint.answer.clone(),
                progress: work.progress.clone(),
                tokens: work.tokens,
                model: work.model.clone(),
                protected_outputs: work.protected_outputs.clone(),
                kind: checkpoint.kind,
                run: checkpoint.run.clone(),
                manifest: checkpoint.manifest.clone(),
                settled: checkpoint.settled,
                ended_at: checkpoint.ended_at,
            }
        }
        // Pending artifacts retain evidence needed to revalidate bytes after restart.
        Entry::ArtifactPending(work) => SavedWork::ArtifactPending {
            remote: work.remote,
            work_id: work.work_id.clone(),
            session: work.session.clone(),
            answer: work.answer.clone(),
            kind: work.kind,
            run: work.run.clone(),
            native_session: work.native_session.clone(),
            manifest: work.manifest.clone(),
            freeze_root: work.freeze_root.clone(),
            started_at: work.started_at,
        },
        // Done retains the exact Account-deliverable answer and artifact receipts.
        Entry::Done(work) => SavedWork::Done {
            remote: work.remote,
            work_id: work.work_id.clone(),
            session: work.session.clone(),
            answer: work.answer.clone(),
            kind: work.kind,
            run: work.run.clone(),
            native_session: work.native_session.clone(),
            manifest: work.manifest.clone(),
            freeze_root: work.freeze_root.clone(),
            started_at: work.started_at,
            ended_at: work.ended_at,
        },
        // Stopped retains the terminal fact, whether the owner caused it, and cleanup paths.
        Entry::Stopped(work) => SavedWork::Stopped {
            remote: work.remote,
            work_id: work.work_id.clone(),
            session: work.session.clone(),
            freeze_root: work.freeze_root.clone(),
            started_at: work.started_at,
            ended_at: work.ended_at,
            owner: work.owner,
            reason: work.reason.clone(),
        },
    }
}
