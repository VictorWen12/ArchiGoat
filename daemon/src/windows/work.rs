//! Windows Work binds one public Work to one signed runner and durable observer.

mod admission;
mod console;
mod model;
mod observer;

pub(super) use model::{SignedWork, WorkRequest};
pub(crate) use observer::AgentRun;

use crate::{
    provider::{LocalCli, Provider},
    state::DaemonState,
};
use std::path::{Path, PathBuf};

/// Starts one headless runner only after its immutable command is durably admitted.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn launch(
    work_id: &str,
    runner_id: &str,
    provider: Provider,
    program: &LocalCli,
    args: Vec<String>,
    input: String,
    cwd: PathBuf,
    desktop_root: PathBuf,
    freeze_root: PathBuf,
    state: &DaemonState,
) -> Result<AgentRun, String> {
    if work_id.trim().is_empty() || work_id.chars().any(char::is_control) {
        return Err("Work identity is invalid".to_owned());
    }
    let record = state.host.begin(work_id, runner_id)?;
    let stop_proof = admission::prepare(
        &record,
        provider,
        program,
        args,
        input,
        cwd,
        desktop_root,
        freeze_root,
        state,
    )?;
    let mut supervisor = console::open(&record)?;
    if let Err(error) = super::job::resume(&supervisor) {
        let cleanup = super::job::reap_unowned(&mut supervisor).await;
        return Err(join_errors(error, cleanup));
    }
    Ok(AgentRun::new(record, Some(supervisor), None, stop_proof))
}

/// Reattaches exactly one claimed runner without taking Provider ownership again.
pub(crate) fn reattach(
    work_id: &str,
    _session: &Path,
    state: &DaemonState,
) -> Result<Option<AgentRun>, String> {
    find_record(work_id, state, true)
}

/// Rejects ambiguous duplicate records so one Work never observes two native runners.
fn find_record(
    work_id: &str,
    state: &DaemonState,
    require_claim: bool,
) -> Result<Option<AgentRun>, String> {
    let mut matched = None;
    for identity in state.host.record_identities()? {
        let record = state.host.record(work_id, &identity)?;
        let Some(run) = admission::verify(work_id, record, &identity, state, require_claim)? else {
            continue;
        };
        if matched.is_some() {
            return Err("Windows Work owns more than one runner".to_owned());
        }
        matched = Some(run);
    }
    Ok(matched)
}

/// Preserves the launch cause while adding only actionable process cleanup evidence.
fn join_errors(error: String, process: Result<(), String>) -> String {
    [Some(error), process.err()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("; ")
}
