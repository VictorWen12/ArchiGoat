//! Work restoration validates durable identity and bytes without launching any native execution.

mod input;

use std::{
    collections::{HashSet, VecDeque},
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use crate::{
    delivery::{DeliveryFile, Harvested},
    state::OwnerStop,
    work::{ResultKind, RuntimeSteer, valid_work_id},
};

use super::{
    model::{
        ArtifactPending, DoneWork, Entry, MAX_REPAIRS, RUNNER_END, Running, StoppedWork, TurnStop,
    },
    persist::SavedWork,
};

/// Restore validates terminal shape and never trusts artifact Done without matching frozen bytes.
pub(super) fn restore(saved: SavedWork) -> Result<(String, Entry), String> {
    match saved {
        // Running restores one native owner without launching it here.
        SavedWork::Running {
            remote,
            work_id,
            provider,
            model_selection,
            effort_selection,
            session,
            freeze_root,
            native_session,
            runner_id,
            input_path,
            input,
            launched,
            repair,
            steer,
            steers,
            steering,
            steer_delivered,
            rotating,
            stopping,
            repairs,
            attention,
            failure,
            started_at,
            answer,
            progress,
            tokens,
            model,
            protected_outputs,
        } => {
            valid_running_identity(&work_id, provider, &native_session)?;
            let (steer, steers, steering, rotating) =
                sanitized_steers(&work_id, &session, steer, steers, steering, rotating);
            if steering && rotating {
                return Err("Stored steer state is invalid".to_owned());
            }
            // Older exhausted Running records upgrade to resumable attention instead of terminal failure.
            let attention =
                !stopping && (attention || (repairs >= MAX_REPAIRS && failure.is_some()));
            // A parked Work restores under the wording this build renders, so a Provider's own
            // transport prose stored by an earlier build never returns to the screen.
            let parked = failure
                .as_deref()
                .map(|reason| super::model::attention_text(provider, reason))
                .unwrap_or_else(|| super::model::ATTENTION.to_owned());
            let progress = if attention
                && progress
                    .as_ref()
                    .is_none_or(|progress| progress.text != parked)
            {
                Some(crate::state::RunProgress {
                    sequence: progress
                        .as_ref()
                        .map(|progress| progress.sequence.saturating_add(1))
                        .unwrap_or(1),
                    text: parked,
                })
            } else {
                progress
            };
            let input_path = input::restore(&session, input_path, input)?;
            valid_runner(&runner_id, &input_path)?;
            let (steer, steers, steering, rotating) = if stopping {
                (None, VecDeque::new(), false, false)
            } else {
                (steer, steers, steering, rotating)
            };
            let stop = OwnerStop::new();
            if stopping {
                stop.request();
            }
            let turn_stop = TurnStop::new();
            if rotating {
                turn_stop.request();
            }
            let protected_outputs = protected_outputs_prefix(protected_outputs);
            let id = work_id.clone();
            Ok((
                id,
                Entry::Running(Running {
                    remote,
                    work_id,
                    provider,
                    model_selection,
                    effort_selection,
                    session,
                    freeze_root,
                    native_session,
                    runner_id,
                    input_path,
                    launched,
                    repair,
                    steer,
                    steers,
                    // Only the head that owns the live turn can already have been delivered to it.
                    steer_delivered: steering && steer_delivered,
                    steering,
                    rotating,
                    stopping,
                    repairs,
                    attention,
                    failure: if attention && failure.is_none() {
                        Some(RUNNER_END.to_owned())
                    } else {
                        failure
                    },
                    started_at,
                    answer,
                    progress,
                    tokens,
                    model,
                    protected_outputs,
                    stop,
                    turn_stop,
                }),
            ))
        }
        // Pending artifacts remain private until frozen bytes verify again.
        SavedWork::ArtifactPending {
            remote,
            work_id,
            session,
            answer,
            kind,
            run,
            native_session,
            manifest,
            freeze_root,
            started_at,
        } => {
            valid_artifact(&work_id, kind, &run, &native_session, &manifest)?;
            Ok(restore_artifact(
                work_id,
                session,
                answer,
                kind,
                run,
                native_session,
                manifest,
                freeze_root,
                started_at,
                None,
                false,
                remote,
            ))
        }
        // Done is exposed only when its answer or artifact evidence is structurally complete.
        SavedWork::Done {
            remote,
            work_id,
            session,
            answer,
            kind,
            run,
            native_session,
            manifest,
            freeze_root,
            started_at,
            ended_at,
        } => {
            valid_identity(&work_id, &native_session)?;
            match kind {
                ResultKind::Artifact => {
                    let run = run.ok_or_else(|| "Stored artifact run is invalid".to_owned())?;
                    valid_artifact(&work_id, kind, &run, &native_session, &manifest)?;
                    Ok(match freeze_root {
                        Some(root) => restore_artifact(
                            work_id,
                            session,
                            answer,
                            kind,
                            run,
                            native_session,
                            manifest,
                            root,
                            started_at,
                            ended_at,
                            true,
                            remote,
                        ),
                        None => {
                            let id = work_id.clone();
                            (
                                id,
                                Entry::Done(DoneWork {
                                    remote,
                                    work_id,
                                    session,
                                    answer,
                                    kind,
                                    run: Some(run),
                                    native_session,
                                    manifest,
                                    freeze_root: None,
                                    harvested: None,
                                    started_at,
                                    ended_at,
                                }),
                            )
                        }
                    })
                }
                ResultKind::Answer => {
                    if run.is_some() || !manifest.is_empty() || answer.trim().is_empty() {
                        return Err("Stored answer result is invalid".to_owned());
                    }
                    let id = work_id.clone();
                    Ok((
                        id,
                        Entry::Done(DoneWork {
                            remote,
                            work_id,
                            session,
                            answer,
                            kind,
                            run: None,
                            native_session,
                            manifest,
                            freeze_root: None,
                            harvested: None,
                            started_at,
                            ended_at,
                        }),
                    ))
                }
            }
        }
        // Stopped restores the recorded terminal cause and cleanup ownership without reinterpreting either.
        SavedWork::Stopped {
            remote,
            work_id,
            session,
            freeze_root,
            started_at,
            ended_at,
            owner,
            reason,
        } => {
            valid_work_id(&work_id)?;
            let id = work_id.clone();
            Ok((
                id,
                Entry::Stopped(StoppedWork {
                    remote,
                    work_id,
                    session,
                    freeze_root,
                    started_at,
                    ended_at,
                    owner,
                    reason: if owner || !reason.trim().is_empty() {
                        reason
                    } else {
                        RUNNER_END.to_owned()
                    },
                }),
            ))
        }
    }
}

/// ProtectedOutputsPrefix restores only the bounded in-memory prefix; spill files retain later literals.
fn protected_outputs_prefix(outputs: Vec<String>) -> Vec<String> {
    let mut kept = Vec::new();
    let mut bytes = 0_usize;
    for output in outputs {
        if kept.contains(&output) {
            continue;
        }
        if output.len() <= crate::work::MAX_PROTECTED_ITEM_BYTES
            && kept.len() < crate::work::MAX_PROTECTED_ITEMS
            && bytes.saturating_add(output.len()) <= crate::work::MAX_PROTECTED_BYTES
        {
            bytes += output.len();
            kept.push(output);
        }
    }
    kept
}

/// SanitizedSteers drops only the queued follow-ups that no longer reauthorize, so a lost message never erases its Running Work.
fn sanitized_steers(
    work_id: &str,
    session: &Path,
    steer: Option<RuntimeSteer>,
    steers: VecDeque<RuntimeSteer>,
    steering: bool,
    rotating: bool,
) -> (Option<RuntimeSteer>, VecDeque<RuntimeSteer>, bool, bool) {
    let head_id = steer.as_ref().map(|item| item.id.clone());
    let mut ids = HashSet::new();
    let mut kept: VecDeque<RuntimeSteer> = VecDeque::new();
    for item in steer.into_iter().chain(steers) {
        if let Err(reason) = item.validate(work_id, session) {
            eprintln!("Product dropped an unrestorable follow-up on Work {work_id}: {reason}");
            continue;
        }
        if !ids.insert(item.id.clone()) {
            eprintln!("Product dropped a duplicated follow-up on Work {work_id}");
            continue;
        }
        kept.push_back(item);
    }
    let head = kept.pop_front();
    // A dropped head ends any rotation in flight; the next survivor arms through the ordinary queue path.
    let head_survived =
        matches!((&head, &head_id), (Some(kept), Some(original)) if kept.id == *original);
    if head_survived {
        (head, kept, steering, rotating)
    } else {
        (head, kept, false, false)
    }
}

/// RestoreArtifact exposes Done only when every frozen byte still matches its verified manifest.
#[allow(clippy::too_many_arguments)]
fn restore_artifact(
    work_id: String,
    session: Option<PathBuf>,
    answer: String,
    kind: ResultKind,
    run: String,
    native_session: String,
    manifest: Vec<DeliveryFile>,
    freeze_root: PathBuf,
    started_at: u64,
    ended_at: Option<u64>,
    settle_missing: bool,
    remote: bool,
) -> (String, Entry) {
    let id = work_id.clone();
    match Harvested::rehydrate(answer.clone(), kind, manifest.clone(), freeze_root.clone()) {
        Ok(harvested) => (
            id,
            Entry::Done(DoneWork {
                remote,
                work_id,
                session,
                answer,
                kind,
                run: Some(run),
                native_session,
                manifest,
                freeze_root: Some(freeze_root),
                harvested: Some(harvested),
                started_at,
                ended_at,
            }),
        ),
        Err(_)
            if settle_missing
                && matches!(
                    fs::symlink_metadata(&freeze_root),
                    Err(error) if error.kind() == ErrorKind::NotFound
                ) =>
        {
            (
                id,
                Entry::Done(DoneWork {
                    remote,
                    work_id,
                    session,
                    answer,
                    kind,
                    run: Some(run),
                    native_session,
                    manifest,
                    freeze_root: None,
                    harvested: None,
                    started_at,
                    ended_at,
                }),
            )
        }
        Err(_) => (
            id,
            Entry::ArtifactPending(ArtifactPending {
                remote,
                work_id,
                session,
                answer,
                kind,
                run,
                native_session,
                manifest,
                freeze_root,
                started_at,
            }),
        ),
    }
}

/// ValidArtifact rejects receipt sets that cannot represent one verified product delivery.
fn valid_artifact(
    work_id: &str,
    kind: ResultKind,
    run: &str,
    native_session: &str,
    manifest: &[DeliveryFile],
) -> Result<(), String> {
    valid_identity(work_id, native_session)?;
    if kind != ResultKind::Artifact
        || run.is_empty()
        || manifest.is_empty()
        || manifest.iter().any(|file| file.work_id != work_id)
    {
        return Err("Stored artifact result is invalid".to_owned());
    }
    Ok(())
}

/// ValidIdentity prevents malformed Work or native session handles from controlling recovery.
fn valid_identity(work_id: &str, native_session: &str) -> Result<(), String> {
    valid_work_id(work_id)?;
    if native_session.is_empty() || native_session.chars().any(char::is_control) {
        return Err("Stored native session is invalid".to_owned());
    }
    Ok(())
}

/// ValidRunningIdentity accepts Codex before its first thread event while rejecting malformed ownership.
fn valid_running_identity(
    work_id: &str,
    provider: crate::provider::Provider,
    native_session: &str,
) -> Result<(), String> {
    valid_work_id(work_id)?;
    if native_session.chars().any(char::is_control)
        || (provider == crate::provider::Provider::Claude && native_session.is_empty())
    {
        return Err("Stored native session is invalid".to_owned());
    }
    Ok(())
}

/// ValidRunner keeps restart launch ownership bounded to one private nonce and preserved request.
fn valid_runner(runner_id: &str, input_path: &std::path::Path) -> Result<(), String> {
    if !crate::proof::valid_nonce(runner_id) {
        return Err("Stored runner admission is invalid".to_owned());
    }
    crate::work::runtime::read_input(input_path)?;
    Ok(())
}
