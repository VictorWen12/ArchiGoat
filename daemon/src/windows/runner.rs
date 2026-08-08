//! Windows runner receives one signed Work, owns its Provider Job, and appends raw ordered journal frames.

use super::{
    job::{self, Job},
    journal,
    work::{SignedWork, WorkRequest},
};
use crate::{Config, DaemonState, proof, work::freeze_delivery_receipt};
use std::{path::Path, process::Stdio, time::Duration};

const CLAIM: &str = "claimed";
/// Stop checks slow down while idle, preserving responsive owner control without hot polling.
const MAX_IDLE_WAIT: Duration = Duration::from_secs(1);
const INITIAL_IDLE_WAIT: Duration = Duration::from_millis(25);
/// One unterminated Provider record may not exhaust local memory; past this it is the record that is
/// released, and the turn streams on.
const MAX_PROVIDER_RECORD_BYTES: usize = 1024 * 1024;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::mpsc,
};

/// Carries one output chunk or one closed stream into the ordered journal.
enum Output {
    Bytes(u8, Vec<u8>),
    Closed(Result<(), String>),
}

/// Verifies and executes one signed Work without converting helper failure into public Stop.
pub(super) async fn run(job_path: &Path) -> Result<(), String> {
    let (request, root, secret) = load(job_path)?;
    // The locked create-new claim lets repeated Console openings consume one stable launch right once.
    let Some(_liveness) = super::liveness::RunnerLiveness::claim(&root.join(CLAIM))? else {
        return Ok(());
    };
    execute(&request, &root, &secret).await
}

/// Accepts only a Work file inside the current ArchiGoat's private state root.
fn load(job_path: &Path) -> Result<(WorkRequest, std::path::PathBuf, String), String> {
    let bytes =
        std::fs::read(job_path).map_err(|error| format!("Could not read Windows Work: {error}"))?;
    let job: SignedWork =
        serde_json::from_slice(&bytes).map_err(|_| "Windows Work is invalid".to_owned())?;
    let root = job_path
        .parent()
        .ok_or_else(|| "Windows Work path is invalid".to_owned())?;
    let config = Config::load()?;
    let state = config
        .state_file
        .clone()
        .ok_or_else(|| "ArchiGoat state is unavailable".to_owned())?;
    let expected_root = state
        .parent()
        .ok_or_else(|| "ArchiGoat state boundary is invalid".to_owned())?
        .join("WindowsWork")
        .join(&job.request.nonce);
    if job_path.file_name().and_then(|value| value.to_str()) != Some("work.json")
        || root.file_name().and_then(|value| value.to_str()) != Some(&job.request.nonce)
        || !proof::valid_nonce(&job.request.nonce)
        || job.request.state != state
        || root != expected_root
    {
        return Err("Windows Work identity is invalid".to_owned());
    }
    let payload = serde_json::to_vec(&job.request)
        .map_err(|error| format!("Could not verify Windows Work: {error}"))?;
    let secret = DaemonState::load_instance_secret(&state)?;
    if !proof::verify_host(&secret, &payload, &job.proof) {
        return Err("Windows Work proof is invalid".to_owned());
    }
    Ok((job.request, root.to_path_buf(), secret))
}

/// Runs the Provider, preserves its output, and freezes files only after success.
async fn execute(request: &WorkRequest, root: &Path, secret: &str) -> Result<(), String> {
    let job = Job::new()?;
    let mut command = Command::new(&request.program);
    command
        .args(&request.prefix)
        .args(&request.args)
        .current_dir(&request.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // PHYSICS: nothing was started yet — an early return before the first wait must take the
        // Provider process with it rather than leave an unobserved one behind.
        .kill_on_drop(true);
    job::console_suspended(&mut command);
    // PHYSICS: nothing was ever started, so there is no turn here to preserve.
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start Local Agent: {error}"))?;
    let (Some(stdout), Some(stderr), Some(mut stdin)) =
        (child.stdout.take(), child.stderr.take(), child.stdin.take())
    else {
        return Err(cleanup_error(
            "Could not open Local Agent streams".to_owned(),
            job::reap_unowned(&mut child).await,
        ));
    };
    if let Err(error) = job.assign(&child) {
        return Err(cleanup_error(error, job::reap_unowned(&mut child).await));
    }
    if let Err(error) = job::resume(&child) {
        return Err(cleanup_error(error, job.finish(&mut child).await));
    }
    let input_bytes = request.input.clone();
    let mut input = tokio::spawn(async move {
        stdin.write_all(input_bytes.as_bytes()).await?;
        stdin.shutdown().await
    });
    let (sender, mut output) = mpsc::channel(16);
    let stdout_task = tokio::spawn(copy_lines(stdout, journal::STDOUT, sender.clone()));
    let stderr_task = tokio::spawn(copy_bytes(stderr, journal::STDERR, sender));
    let mut sequence = 1_u64;
    let mut status = None;
    let mut input_open = true;
    let mut outputs = 2_u8;
    let mut stopped = false;
    let mut stop_wait = INITIAL_IDLE_WAIT;
    // A Provider with unreliable exits proves completion through its own journaled result event.
    let mut native_completed = false;
    // Provider exit ends execution; descendants die before their inherited pipes are drained.
    while status.is_none() {
        tokio::select! {
            // PHYSICS: the Provider process itself is gone, the one fact that can end its turn.
            result = child.wait(), if status.is_none() => {
                match result {
                    Ok(result) => {
                        status = Some(result);
                        job.finish(&mut child).await?;
                    }
                    Err(error) => {
                        return Err(cleanup_error(
                            format!("Could not wait for Local Agent: {error}"),
                            job.finish(&mut child).await,
                        ));
                    }
                }
            }
            result = &mut input, if input_open => {
                input_open = false;
                // A Provider that stopped reading its own input decides its own outcome; the broken
                // pipe is journaled as the diagnostic it is and takes no turn with it.
                if !stopped && !matches!(result, Ok(Ok(()))) {
                    journal::append(root, sequence, journal::STDERR, b"Local Agent stopped reading its input")?;
                    sequence += 1;
                }
            }
            event = output.recv(), if outputs > 0 => {
                if !native_completed && proves_native_completion(&event, request.provider) {
                    native_completed = true;
                }
                match record_output(event, root, &mut sequence, &mut outputs) {
                    Ok(Some(_)) => stop_wait = INITIAL_IDLE_WAIT,
                    Ok(None) => {}
                    // PHYSICS: this Work's journal cannot be written, so no further fact about the
                    // turn can be recorded at all. The observer settles it on what it already froze.
                    Err(error) => {
                        return Err(cleanup_error(error, job.finish(&mut child).await));
                    }
                }
            }
            _ = tokio::time::sleep(stop_wait), if !stopped => {
                // PHYSICS: the owner pressed Stop, proved for this exact runner.
                if valid_signal(root, "stop", "stop", &request.nonce, secret) {
                    stopped = true;
                    job.finish(&mut child).await?;
                    status = Some(
                        child
                            .wait()
                            .await
                        .map_err(|error| format!("Could not confirm stopped Agent: {error}"))?,
                    );
                }
                stop_wait = (stop_wait * 2).min(MAX_IDLE_WAIT);
            }
        }
    }
    if !stopped && valid_signal(root, "stop", "stop", &request.nonce, secret) {
        stopped = true;
    }
    // PHYSICS: the writers are dead processes, so every pipe reaches its own end. Draining to that
    // end is what keeps the line proving this turn finished — and the answer beside it — out of the
    // bytes a deadline would have thrown away.
    while input_open || outputs > 0 {
        tokio::select! {
            result = &mut input, if input_open => {
                input_open = false;
                if !stopped && !matches!(result, Ok(Ok(()))) {
                    journal::append(root, sequence, journal::STDERR, b"Local Agent stopped reading its input")?;
                    sequence += 1;
                }
            }
            event = output.recv(), if outputs > 0 => {
                // A Provider that exits before its own result line is consumed still proves completion here.
                native_completed = native_completed || proves_native_completion(&event, request.provider);
                record_output(event, root, &mut sequence, &mut outputs)?;
            }
        }
    }
    stdout_task
        .await
        .map_err(|error| format!("Local Agent stdout observer failed: {error}"))?;
    stderr_task
        .await
        .map_err(|error| format!("Local Agent stderr observer failed: {error}"))?;
    if !stopped
        && (status
            .as_ref()
            .is_some_and(std::process::ExitStatus::success)
            || native_completed)
    {
        if let Err(error) = freeze_delivery_receipt(
            &request.work_id,
            &request.cwd,
            &request.desktop_root,
            &request.freeze_root,
        ) {
            // The observer keeps a failed freeze Running and resumes the same native session.
            journal::append(root, sequence, journal::STDERR, error.as_bytes())?;
            sequence += 1;
        }
    }
    if stopped {
        journal::append(root, sequence, journal::STOPPED, &[])
    } else {
        // Done proves the process ended; Provider completion still gates public success.
        journal::append(root, sequence, journal::DONE, &[])
    }
}

/// ProvesNativeCompletion reads the Provider's own result grammar, so completion is recognized on
/// every path that carries its stdout — including bytes drained after the process already exited.
fn proves_native_completion(event: &Option<Output>, provider: crate::provider::Provider) -> bool {
    let Some(Output::Bytes(journal::STDOUT, bytes)) = event else {
        return false;
    };
    provider.native_completion(&String::from_utf8_lossy(bytes))
}

/// Records one complete output event and reports which stream carried Provider bytes. A stream that
/// ends badly is journaled as the diagnostic it is: the Provider's own outcome decides this turn,
/// never the state of the pipe carrying it.
fn record_output(
    event: Option<Output>,
    root: &Path,
    sequence: &mut u64,
    outputs: &mut u8,
) -> Result<Option<u8>, String> {
    match event {
        Some(Output::Bytes(kind, bytes)) => {
            journal::append(root, *sequence, kind, &bytes)?;
            *sequence += 1;
            Ok(Some(kind))
        }
        Some(Output::Closed(Ok(()))) => {
            *outputs -= 1;
            Ok(None)
        }
        Some(Output::Closed(Err(error))) => {
            journal::append(root, *sequence, journal::STDERR, error.as_bytes())?;
            *sequence += 1;
            *outputs -= 1;
            Ok(None)
        }
        None => {
            // Both readers are gone, so nothing more can arrive; the journal already holds the turn.
            *outputs = 0;
            Ok(None)
        }
    }
}

/// CleanupError preserves both the Work failure and any failure to end its process tree.
fn cleanup_error(primary: String, cleanup: Result<(), String>) -> String {
    match cleanup {
        Ok(()) => primary,
        Err(error) => format!("{primary}; cleanup failed: {error}"),
    }
}

/// Accepts Stop only when it is signed for this exact runner.
fn valid_signal(root: &Path, file: &str, purpose: &str, nonce: &str, secret: &str) -> bool {
    let Ok(value) = std::fs::read_to_string(root.join(file)) else {
        return false;
    };
    proof::verify_host(
        secret,
        format!("{purpose}:{nonce}").as_bytes(),
        value.trim(),
    )
}

/// Preserves stdout lines in Provider order. One record too large to hold is released at its own
/// newline, so a frame this machine cannot carry costs one frame and never the Agent's work.
async fn copy_lines(mut reader: impl AsyncRead + Unpin, kind: u8, sender: mpsc::Sender<Output>) {
    let mut buffer = [0_u8; 8192];
    let mut pending = Vec::new();
    let mut releasing = false;
    let result = loop {
        match reader.read(&mut buffer).await {
            Ok(0) => {
                if !pending.is_empty()
                    && !releasing
                    && sender.send(Output::Bytes(kind, pending)).await.is_err()
                {
                    return;
                }
                break Ok(());
            }
            Ok(count) => {
                pending.extend_from_slice(&buffer[..count]);
                while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                    let line = pending.drain(..=end).collect();
                    // The tail of a released record ends at its own newline and goes no further.
                    if releasing {
                        releasing = false;
                        continue;
                    }
                    if sender.send(Output::Bytes(kind, line)).await.is_err() {
                        return;
                    }
                }
                if pending.len() > MAX_PROVIDER_RECORD_BYTES {
                    crate::trace::line(
                        "released one Local Agent record too large to carry; this turn streams on",
                    );
                    pending.clear();
                    releasing = true;
                }
            }
            Err(error) => break Err(format!("Could not read Local Agent output: {error}")),
        }
    };
    if sender.send(Output::Closed(result)).await.is_err() {
        return;
    }
}

/// Preserves diagnostic bytes without changing their content.
async fn copy_bytes(mut reader: impl AsyncRead + Unpin, kind: u8, sender: mpsc::Sender<Output>) {
    let mut buffer = [0_u8; 8192];
    let result = loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break Ok(()),
            Ok(count) => {
                if sender
                    .send(Output::Bytes(kind, buffer[..count].to_vec()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Err(error) => break Err(format!("Could not read Local Agent errors: {error}")),
        }
    };
    if sender.send(Output::Closed(result)).await.is_err() {
        return;
    }
}
