//! Windows admission creates once and verifies the exact signed native Work command.

use super::{AgentRun, SignedWork, WorkRequest};
use crate::{
    provider::{LocalCli, Provider},
    state::DaemonState,
};
use std::path::PathBuf;

/// Names the immutable signed command consumed by the helper.
pub(super) const JOB: &str = "work.json";
/// Names the create-new execution claim that prevents duplicate Providers.
pub(super) const CLAIM: &str = "claimed";

/// Writes an immutable signed command before any process can claim execution.
#[allow(clippy::too_many_arguments)]
pub(super) fn prepare(
    record: &super::super::active::Record,
    provider: Provider,
    program: &LocalCli,
    args: Vec<String>,
    input: String,
    cwd: PathBuf,
    desktop_root: PathBuf,
    freeze_root: PathBuf,
    state: &DaemonState,
) -> Result<String, String> {
    if !program.program().is_file() || !cwd.is_dir() {
        return Err("Windows Work preflight could not verify its command".to_owned());
    }
    let prefix = program
        .prefix()
        .iter()
        .map(|value| {
            value
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| "Provider entry is not valid UTF-8".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let request = WorkRequest {
        work_id: record.work_id.clone(),
        nonce: record.identity.clone(),
        state: state
            .config
            .state_file
            .clone()
            .ok_or_else(|| "ArchiGoat state is unavailable".to_owned())?,
        program: program.program().to_path_buf(),
        prefix,
        args,
        input,
        cwd,
        provider,
        desktop_root,
        freeze_root,
    };
    let payload = serde_json::to_vec(&request)
        .map_err(|error| format!("Could not encode Windows Work: {error}"))?;
    let signed = SignedWork {
        proof: state.sign_host_work(&payload)?,
        request,
    };
    let bytes = serde_json::to_vec(&signed)
        .map_err(|error| format!("Could not encode signed Windows Work: {error}"))?;
    create_once(&record.root.join(JOB), &bytes)?;
    state.sign_host_work(format!("stop:{}", record.identity).as_bytes())
}

/// Accepts only the exact persisted command, making duplicate launch unable to replace it.
fn create_once(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    match std::fs::read(path) {
        Ok(saved) if saved == bytes => Ok(()),
        Ok(_) => Err("Windows Work admission changed".to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            super::super::file::write_private(path, bytes)
        }
        Err(error) => Err(format!("Could not read Windows Work admission: {error}")),
    }
}

/// Verifies identity, state ownership, and host signature before observation.
pub(super) fn verify(
    work_id: &str,
    record: super::super::active::Record,
    identity: &str,
    state: &DaemonState,
    require_claim: bool,
) -> Result<Option<AgentRun>, String> {
    if require_claim && !record.root.join(CLAIM).is_file() {
        return Ok(None);
    }
    let bytes = std::fs::read(record.root.join(JOB))
        .map_err(|error| format!("Could not read Windows Work record: {error}"))?;
    let signed: SignedWork =
        serde_json::from_slice(&bytes).map_err(|_| "Windows Work record is invalid".to_owned())?;
    if signed.request.work_id != work_id {
        return Ok(None);
    }
    if signed.request.nonce != identity {
        return Err("Windows Work record does not match Work identity".to_owned());
    }
    let state_file = state
        .config
        .state_file
        .as_ref()
        .ok_or_else(|| "ArchiGoat state is unavailable".to_owned())?;
    if &signed.request.state != state_file {
        return Err("Windows Work record state is invalid".to_owned());
    }
    let payload = serde_json::to_vec(&signed.request)
        .map_err(|error| format!("Could not verify Windows Work record: {error}"))?;
    let secret = DaemonState::load_instance_secret(state_file)?;
    if !crate::proof::verify_host(&secret, &payload, &signed.proof) {
        return Err("Windows Work record proof is invalid".to_owned());
    }
    let Some(liveness) =
        super::super::liveness::RunnerLiveness::reattach(&record.root.join(CLAIM))?
    else {
        return Ok(None);
    };
    let stop = state.sign_host_work(format!("stop:{identity}").as_bytes())?;
    Ok(Some(AgentRun::new(record, None, Some(liveness), stop)))
}
