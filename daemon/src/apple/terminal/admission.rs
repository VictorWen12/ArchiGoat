//! Apple admission creates and verifies one signed host command per Work.

use super::{
    execute,
    model::{CLAIM, JOB, Job, LIVENESS, Request},
    observer::AgentRun,
};
use crate::apple::write_private;
use crate::{
    DaemonState, proof,
    provider::{LocalCli, Provider},
};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::process::Command;

#[allow(clippy::too_many_arguments)]
// Launch durably admits the exact Work before starting its one headless runner.
pub(crate) async fn launch(
    work_id: &str,
    runner_id: &str,
    provider: Provider,
    cli: &LocalCli,
    args: Vec<String>,
    input: String,
    cwd: PathBuf,
    desktop_root: PathBuf,
    freeze_root: PathBuf,
    state: &DaemonState,
) -> Result<AgentRun, String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("Could not locate ArchiGoat: {error}"))?;
    let state_file = state
        .config
        .state_file
        .clone()
        .ok_or_else(|| "ArchiGoat state is unavailable".to_owned())?;
    if work_id.trim().is_empty() || !executable.is_file() || !cli.program().is_file() {
        return Err("Apple Work launch is invalid".to_owned());
    }
    if !proof::valid_nonce(runner_id) {
        return Err("Apple Work runner identity is invalid".to_owned());
    }
    // A private runner directory isolates this Work's admission and event truth.
    let root = desktop_root.join(".app/terminal").join(runner_id);
    fs::create_dir_all(&root).map_err(|error| format!("Could not prepare Apple Work: {error}"))?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("Could not protect Apple Work: {error}"))?;
    // Exact Provider arguments preserve native capability without ArchiGoat interpretation.
    let prefix = cli
        .prefix()
        .iter()
        .map(|value| {
            value
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| "Provider entry is not valid UTF-8".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let request = Request {
        work_id: work_id.to_owned(),
        nonce: runner_id.to_owned(),
        state: state_file,
        program: cli.program().to_path_buf(),
        prefix,
        args,
        input,
        cwd,
        provider: Some(provider),
        desktop_root,
        freeze_root,
    };
    let payload = serde_json::to_vec(&request)
        .map_err(|error| format!("Could not encode Apple Work: {error}"))?;
    let job = Job {
        proof: state.sign_host_work(&payload)?,
        request,
    };
    let job_path = root.join(JOB);
    let job_bytes = serde_json::to_vec(&job)
        .map_err(|error| format!("Could not encode Apple Work: {error}"))?;
    let stop_proof = state.sign_host_work(format!("stop:{runner_id}").as_bytes())?;
    // An admission already on disk owns this runner. Its bytes are never replaced, and one that no
    // longer reads back the same is still that runner's own journal to observe: this launch attaches
    // to it and starts nothing second.
    match fs::read(&job_path) {
        Ok(saved) if saved == job_bytes => {}
        Ok(_) => {
            crate::trace::line("attached to an Apple Work admission this launch did not write");
            return Ok(AgentRun::new(root, None, stop_proof));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write_private(&job_path, &job_bytes)?
        }
        Err(error) => return Err(format!("Could not read Apple Work admission: {error}")),
    }
    // The runner executes headlessly; its journal, not a window, is this Work's progress.
    // A login shell loads the owner's full environment exactly as Terminal did, then exec replaces it with the runner.
    let runner = Command::new("/bin/zsh")
        .arg("-lc")
        .arg("exec \"$0\" --terminal-work \"$1\"")
        .arg(&executable)
        .arg(&job_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Its own process group keeps the runner owning its Work across ArchiGoat signals and restarts.
        .process_group(0)
        .spawn()
        .map_err(|error| format!("Could not start Apple Work: {error}"))?;
    Ok(AgentRun::new(root, Some(runner), stop_proof))
}

// Reattach resumes a claimed runner without starting another native Agent.
pub(crate) fn reattach(
    work_id: &str,
    session: &Path,
    state: &DaemonState,
) -> Result<Option<AgentRun>, String> {
    let Some(root) = find_admission(work_id, session, state)? else {
        return Ok(None);
    };
    if !root.join(CLAIM).is_file() {
        return Ok(None);
    }
    let runner_id = root
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "Apple Work runner identity is invalid".to_owned())?;
    let stop_proof = state.sign_host_work(format!("stop:{runner_id}").as_bytes())?;
    Ok(Some(AgentRun::new(root, None, stop_proof)))
}

// FindAdmission returns exactly one verified stable command for this Work.
fn find_admission(
    work_id: &str,
    session: &Path,
    state: &DaemonState,
) -> Result<Option<PathBuf>, String> {
    let entries = match fs::read_dir(session.join(".app/terminal")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Could not inspect Apple Work: {error}")),
    };
    let mut matched = None;
    for entry in entries {
        let root = entry
            .map_err(|error| format!("Could not inspect Apple Work: {error}"))?
            .path();
        let bytes = match fs::read(root.join(JOB)) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        // An admission written in another shape than this build reads is one admission this Work
        // cannot attach to, never a reason to stop looking for the one it can.
        let Ok(job) = serde_json::from_slice::<Job>(&bytes) else {
            crate::trace::line("skipped an Apple Work admission this build cannot read");
            continue;
        };
        if job.request.work_id != work_id {
            continue;
        }
        let payload = serde_json::to_vec(&job.request)
            .map_err(|error| format!("Could not verify Apple Work: {error}"))?;
        // BOUNDARY: authentication — an admission this installation did not sign, or one sitting
        // under another runner's name, is never attached to. It is skipped, not fatal.
        if root.file_name().and_then(|value| value.to_str()) != Some(&job.request.nonce)
            || !state.verify_host_work(&payload, &job.proof)
        {
            crate::trace::line("skipped an Apple Work admission this installation did not sign");
            continue;
        }
        if matched.is_some() {
            // The first verified admission owns this Work; a second is a leftover, not a conflict.
            crate::trace::line("kept the first verified Apple Work admission for this Work");
            continue;
        }
        matched = Some(root);
    }
    Ok(matched)
}

// RunTerminalWork verifies this installation's proof and atomically claims native execution.
pub(super) async fn run_terminal_work(job_path: &Path) -> Result<(), String> {
    let bytes =
        fs::read(job_path).map_err(|error| format!("Could not read Apple Work: {error}"))?;
    let job: Job =
        serde_json::from_slice(&bytes).map_err(|_| "Apple Work is invalid".to_owned())?;
    let root = job_path
        .parent()
        .ok_or_else(|| "Apple Work path is invalid".to_owned())?;
    let payload = serde_json::to_vec(&job.request)
        .map_err(|error| format!("Could not verify Apple Work: {error}"))?;
    let secret = DaemonState::load_instance_secret(&job.request.state)?;
    if job_path.file_name().and_then(|value| value.to_str()) != Some(JOB)
        || root.file_name().and_then(|value| value.to_str()) != Some(&job.request.nonce)
        || !proof::verify_host(&secret, &payload, &job.proof)
    {
        return Err("Apple Work proof is invalid".to_owned());
    }
    // Create-new claim makes repeated runner launches converge on one execution.
    let claim = match OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(root.join(CLAIM))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(format!("Could not claim Apple Work: {error}")),
    };
    // The lock is this runner's kernel-backed liveness; the marker is written only while it is held.
    claim
        .try_lock()
        .map_err(|_| "Could not hold the Apple Work claim".to_owned())?;
    (&claim)
        .write_all(LIVENESS)
        .and_then(|()| claim.sync_all())
        .map_err(|error| format!("Could not commit Apple Work claim: {error}"))?;
    // Helper errors preserve the signed admission and journal for recovery; they never invent Stop.
    let result = execute::execute(job.request, root, &secret).await;
    // The claim lock outlives execution so observers never see it free while this runner lives.
    drop(claim);
    result
}
