//! Apple execution owns one native Provider process until success or owner Stop.

use super::{
    browser::BrowserScope,
    journal::append_event,
    model::{DONE, Output, Request, STDERR, STDOUT, STOP, STOPPED},
    sweep,
};
use crate::{proof, work::freeze_delivery_receipt};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    signal::unix::{SignalKind, signal},
    sync::mpsc,
};

/// Stop checks slow down while idle, preserving responsive owner control without hot polling.
const MAX_IDLE_WAIT: Duration = Duration::from_secs(1);
const INITIAL_IDLE_WAIT: Duration = Duration::from_millis(25);
/// One unterminated Provider record may not exhaust local memory; past this it is the record that is
/// released, and the turn streams on.
const MAX_PROVIDER_RECORD_BYTES: usize = 1024 * 1024;

// Execute streams native output durably and freezes every deliverable byte before the turn ends.
pub(super) async fn execute(mut request: Request, root: &Path, secret: &str) -> Result<(), String> {
    let mut terminate_signal = signal(SignalKind::terminate())
        .map_err(|error| format!("Could not own Terminal termination: {error}"))?;
    let mut hangup_signal = signal(SignalKind::hangup())
        .map_err(|error| format!("Could not own Terminal closure: {error}"))?;
    // Confinement is what makes this Work's browser findable later; losing it must never cost the user the Work.
    let browser = BrowserScope::prepare(root)
        .inspect_err(|error| {
            crate::trace::line(&format!("browser scope unavailable: {error}"));
        })
        .ok();
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
        .kill_on_drop(true)
        .process_group(0);
    if let Some(browser) = &browser {
        browser.apply(&mut command);
    }
    // PHYSICS: nothing was ever started, so there is no turn here to preserve.
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start Local Agent: {error}"))?;
    let group = child
        .id()
        .ok_or_else(|| "Local Agent identity is unavailable".to_owned())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Could not read Local Agent output".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Could not read Local Agent errors".to_owned())?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Could not write Local Agent input".to_owned())?;
    // Input is delivered unchanged once, then closed so the Provider can execute.
    let input = std::mem::take(&mut request.input);
    let mut input_task = tokio::spawn(async move {
        stdin.write_all(input.as_bytes()).await?;
        stdin.shutdown().await
    });
    let (sender, mut receiver) = mpsc::channel(16);
    let stdout_task = tokio::spawn(copy_lines(stdout, STDOUT, sender.clone()));
    let stderr_task = tokio::spawn(copy_bytes(stderr, STDERR, sender));
    let (mut sequence, mut closed, mut status, mut stopped) = (1, 0, None, false);
    let mut input_open = true;
    let mut stop_wait = INITIAL_IDLE_WAIT;
    // A Provider with unreliable exits proves completion through its own journaled result event.
    let mut native_completed = false;
    // Deliverables are committed once, at the moment they verify; no later process end may replace them.
    let mut frozen = false;
    // The machine event that ended this runner, so a logout is never read as a Provider that died.
    let mut machine_stopped = false;
    // Provider exit ends execution; descendants die before their inherited pipes are drained.
    while status.is_none() {
        tokio::select! {
            // PHYSICS: the Provider process itself is gone, the one fact that can end its turn.
            waited = child.wait(), if status.is_none() => {
                let waited = waited.map_err(|error| format!("Could not wait for Local Agent: {error}"))?;
                status = Some(waited);
                terminate(&mut child, group, browser.as_ref()).await?;
            }
            delivered = &mut input_task, if input_open => {
                input_open = false;
                // A Provider that stopped reading its own input decides its own outcome; the broken
                // pipe is journaled as the diagnostic it is and takes no turn with it.
                if !stopped && !matches!(delivered, Ok(Ok(()))) {
                    append_event(root, sequence, STDERR, b"Local Agent stopped reading its input")?;
                    sequence += 1;
                }
            }
            output = receiver.recv(), if closed < 2 => {
                if !native_completed && proves_native_completion(&output, request.provider) {
                    native_completed = true;
                    // The Provider's own completion is when this turn's deliverables verify.
                    freeze_now(&request, root, &mut sequence)?;
                    frozen = true;
                }
                match record_output(output, root, &mut sequence, &mut closed) {
                    Ok(Some(_)) => stop_wait = INITIAL_IDLE_WAIT,
                    Ok(None) => {}
                    // PHYSICS: this Work's journal cannot be written, so no further fact about the
                    // turn can be recorded at all. The observer settles it on what it already froze.
                    Err(error) => {
                        return Err(cleanup_error(
                            error,
                            terminate(&mut child, group, browser.as_ref()).await.map(|_| ()),
                        ));
                    }
                }
            }
            _ = tokio::time::sleep(stop_wait), if !stopped => {
                // PHYSICS: the owner pressed Stop, proved for this exact runner.
                if owner_stopped(root, &request.nonce, secret) {
                    stopped = true;
                    status = Some(terminate(&mut child, group, browser.as_ref()).await?);
                }
                stop_wait = (stop_wait * 2).min(MAX_IDLE_WAIT);
            }
            // PHYSICS: this machine is taking the runner down — a restart, a logout, a reload.
            _ = terminate_signal.recv() => {
                machine_stopped = true;
                terminate(&mut child, group, browser.as_ref()).await?;
                break;
            }
            // PHYSICS: the session that owns this runner is closing.
            _ = hangup_signal.recv() => {
                machine_stopped = true;
                terminate(&mut child, group, browser.as_ref()).await?;
                break;
            }
        }
    }
    if machine_stopped {
        // PHYSICS: this machine is going down, so these readers have nothing left to read.
        input_task.abort();
        stdout_task.abort();
        stderr_task.abort();
        // An end this machine imposed keeps whatever the Agent had already built.
        if !frozen {
            freeze_now(&request, root, &mut sequence)?;
        }
        // A runner this machine deliberately ended is a stop, never a crash the Work must repair.
        return append_event(root, sequence, STOPPED, &[]);
    }
    if !stopped && owner_stopped(root, &request.nonce, secret) {
        stopped = true;
    }
    // PHYSICS: the writers are dead processes, so every pipe reaches its own end. Draining to that
    // end is what keeps the line proving this turn finished — and the answer beside it — out of the
    // bytes a deadline would have thrown away.
    while input_open || closed < 2 {
        tokio::select! {
            delivered = &mut input_task, if input_open => {
                input_open = false;
                if !stopped && !matches!(delivered, Ok(Ok(()))) {
                    append_event(root, sequence, STDERR, b"Local Agent stopped reading its input")?;
                    sequence += 1;
                }
            }
            output = receiver.recv(), if closed < 2 => {
                record_output(output, root, &mut sequence, &mut closed)?;
            }
        }
    }
    stdout_task
        .await
        .map_err(|error| format!("Local Agent stdout observer failed: {error}"))?;
    stderr_task
        .await
        .map_err(|error| format!("Local Agent stderr observer failed: {error}"))?;
    // A turn that ended without proving completion still hands over whatever it had already built.
    if !frozen {
        freeze_now(&request, root, &mut sequence)?;
    }
    if stopped {
        append_event(root, sequence, STOPPED, &[])
    } else {
        // Done is a physical turn terminator; Provider events separately prove user success.
        append_event(root, sequence, DONE, &[])
    }
}

// FreezeNow commits this Work's deliverables and journals a failed freeze without ending the turn:
// the observer keeps a Work whose freeze failed Running and resumes the same native session.
fn freeze_now(request: &Request, root: &Path, sequence: &mut u64) -> Result<(), String> {
    if let Err(error) = freeze_workspace(request) {
        append_event(root, *sequence, STDERR, error.as_bytes())?;
        *sequence += 1;
    }
    Ok(())
}

// FreezeWorkspace verifies and commits this Work's deliverables whenever it holds any, so a killed,
// stopped, or failed turn still leaves exact frozen bytes to deliver.
fn freeze_workspace(request: &Request) -> Result<(), String> {
    if holds_deliverable(&request.cwd)? {
        // A newly verified freeze replaces the previous one; the same Work never keeps two deliveries.
        crate::delivery::discard_private_tree(&request.freeze_root)?;
    } else if std::fs::symlink_metadata(&request.freeze_root).is_ok() {
        // An empty workspace adds nothing, so bytes this Work already froze stay exactly as they are.
        return Ok(());
    }
    freeze_delivery_receipt(
        &request.work_id,
        &request.cwd,
        &request.desktop_root,
        &request.freeze_root,
    )
}

// HoldsDeliverable reads exactly the top-level regular files the freeze itself would copy.
fn holds_deliverable(workspace: &Path) -> Result<bool, String> {
    for entry in std::fs::read_dir(workspace)
        .map_err(|error| format!("Could not inspect Work output: {error}"))?
    {
        let entry = entry.map_err(|error| format!("Could not inspect Work output: {error}"))?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let metadata = std::fs::symlink_metadata(entry.path())
            .map_err(|error| format!("Could not inspect Work output: {error}"))?;
        if metadata.is_file() && !metadata.file_type().is_symlink() {
            return Ok(true);
        }
    }
    Ok(false)
}

// ProvesNativeCompletion reads the Provider's own result grammar, so completion is recognized on
// every path that carries its stdout — including bytes drained after the process already exited.
fn proves_native_completion(
    output: &Option<Output>,
    provider: Option<crate::provider::Provider>,
) -> bool {
    let (Some(Output::Bytes(STDOUT, bytes)), Some(provider)) = (output, provider) else {
        return false;
    };
    provider.native_completion(&String::from_utf8_lossy(bytes))
}

// RecordOutput commits one complete output event and reports which stream carried Provider bytes. A
// stream that ends badly is journaled as the diagnostic it is: the Provider's own outcome decides
// this turn, never the state of the pipe carrying it.
fn record_output(
    output: Option<Output>,
    root: &Path,
    sequence: &mut u64,
    closed: &mut u8,
) -> Result<Option<u8>, String> {
    match output {
        Some(Output::Bytes(kind, bytes)) => {
            append_event(root, *sequence, kind, &bytes)?;
            *sequence += 1;
            Ok(Some(kind))
        }
        Some(Output::Closed(Ok(()))) => {
            *closed += 1;
            Ok(None)
        }
        Some(Output::Closed(Err(error))) => {
            append_event(root, *sequence, STDERR, error.as_bytes())?;
            *sequence += 1;
            *closed += 1;
            Ok(None)
        }
        None => {
            // Both readers are gone, so nothing more can arrive; the journal already holds the turn.
            *closed = 2;
            Ok(None)
        }
    }
}

// CleanupError preserves both the Work failure and any failure to end its process group.
fn cleanup_error(primary: String, cleanup: Result<(), String>) -> String {
    match cleanup {
        Ok(()) => primary,
        Err(error) => format!("{primary}; cleanup failed: {error}"),
    }
}

// OwnerStopped accepts only this installation's proof for this exact runner.
fn owner_stopped(root: &Path, runner_id: &str, secret: &str) -> bool {
    let Ok(value) = std::fs::read_to_string(root.join(STOP)) else {
        return false;
    };
    proof::verify_host(secret, format!("stop:{runner_id}").as_bytes(), value.trim())
}

// CopyLines preserves every complete line's bytes. One record too large to hold is the record that is
// released — the stream continues at the next line, because a Provider frame this machine cannot
// carry is one lost frame and never the end of the Agent's work.
async fn copy_lines(mut input: impl AsyncRead + Unpin, kind: u8, sender: mpsc::Sender<Output>) {
    let mut buffer = [0; 8192];
    let mut pending = Vec::new();
    let mut releasing = false;
    let result = loop {
        match input.read(&mut buffer).await {
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
    let _ = sender.send(Output::Closed(result)).await;
}

// CopyBytes journals diagnostic bytes without promoting them to public progress.
async fn copy_bytes(mut input: impl AsyncRead + Unpin, kind: u8, sender: mpsc::Sender<Output>) {
    let mut buffer = [0; 8192];
    loop {
        match input.read(&mut buffer).await {
            Ok(0) => {
                if sender.send(Output::Closed(Ok(()))).await.is_err() {
                    return;
                }
                return;
            }
            Err(error) => {
                if sender
                    .send(Output::Closed(Err(format!(
                        "Could not read Local Agent errors: {error}"
                    ))))
                    .await
                    .is_err()
                {
                    return;
                }
                return;
            }
            Ok(count)
                if sender
                    .send(Output::Bytes(kind, buffer[..count].to_vec()))
                    .await
                    .is_err() =>
            {
                return;
            }
            Ok(_) => {}
        }
    }
}

// Terminate kills the Work's process group and every descendant observed before its parent exits.
// PHYSICS: reached only once the turn has really ended — the Provider exited, the owner pressed Stop,
// or this machine is taking the runner down. It ends processes, never a turn.
async fn terminate(
    child: &mut Child,
    group: u32,
    browser: Option<&BrowserScope>,
) -> Result<std::process::ExitStatus, String> {
    // Escapees are collected while the parent chain still stands, then reaped after the group ends.
    let survivors = sweep::tree_survivors(group).await;
    let kill = Command::new("/bin/kill")
        .args(["-KILL", "--", &format!("-{group}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map_err(|error| format!("Could not signal Local Agent process group: {error}"))?;
    if !kill.success() {
        let live = Command::new("/bin/kill")
            .args(["-0", "--", &format!("-{group}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map_err(|error| format!("Could not inspect Local Agent process group: {error}"))?;
        if live.success() {
            return Err("Could not signal Local Agent process group".to_owned());
        }
    }
    // A delivered Work never fails over cleanup: a browser that outlived it is logged, never raised.
    if let Some(browser) = browser
        && let Err(error) = browser.stop().await
    {
        crate::trace::line(&format!("browser cleanup incomplete: {error}"));
    }
    sweep::end_processes(&survivors).await;
    child
        .wait()
        .await
        .map_err(|error| format!("Could not stop Local Agent: {error}"))
}
