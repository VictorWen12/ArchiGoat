//! Windows CLI support receives Provider commands and returns native discovery, login, health, and install results.

use super::job::{self, Job};
use crate::provider::{LocalCli, Provider};
use std::{
    env,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt},
    process::Command,
    time::timeout,
};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

const DIAGNOSTIC_BYTES: usize = 1024 * 1024;
const TRUNCATED: &[u8] = b"\n[ArchiGoat: diagnostic output truncated]\n";

/// Returns a native command's status and untouched output.
pub(crate) struct Output {
    /// Reports whether the command completed successfully.
    pub success: bool,
    /// Preserves standard output.
    pub stdout: String,
    /// Preserves diagnostic output.
    pub stderr: String,
}

/// Finds an installed Provider CLI without changing the machine.
pub(crate) fn find_cli(provider: Provider, extra: &[PathBuf]) -> Option<LocalCli> {
    let mut dirs = extra.to_vec();
    if let Some(path) = env::var_os("PATH") {
        dirs.extend(env::split_paths(&path));
    }
    dirs.extend(known_dirs());
    dirs.into_iter()
        .find_map(|dir| executable_in(&dir, provider))
}

/// Runs a Provider check and captures its complete result.
pub(crate) async fn capture_cli(
    program: &LocalCli,
    args: &[String],
    input: Option<&str>,
    seconds: u64,
) -> Result<Output, String> {
    let job = Job::new()?;
    let mut command = Command::new(program.program());
    command
        .args(program.prefix())
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    job::hidden_suspended(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start {}: {error}", program.program().display()))?;
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Err(cleanup_error(
            "Could not open CLI output streams".to_owned(),
            job::reap_unowned(&mut child).await,
        ));
    };
    let mut stdin = child.stdin.take();
    if input.is_some() && stdin.is_none() {
        return Err(cleanup_error(
            "Could not open CLI input stream".to_owned(),
            job::reap_unowned(&mut child).await,
        ));
    }
    if let Err(error) = job.assign(&child) {
        let cleanup = job::reap_unowned(&mut child).await.err();
        return Err(cleanup.map_or(error.clone(), |cleanup| format!("{error}; {cleanup}")));
    }
    if let Err(error) = job::resume(&child) {
        let cleanup = job.finish(&mut child).await.err();
        return Err(cleanup.map_or(error.clone(), |cleanup| format!("{error}; {cleanup}")));
    }
    let work = async {
        let write = async {
            if let (Some(mut stdin), Some(input)) = (stdin.take(), input) {
                stdin
                    .write_all(input.as_bytes())
                    .await
                    .map_err(|error| format!("Could not write CLI input: {error}"))?;
                stdin
                    .shutdown()
                    .await
                    .map_err(|error| format!("Could not close CLI input: {error}"))?;
            }
            Ok::<(), String>(())
        };
        let (status, stdout, stderr, write) = tokio::join!(
            child.wait(),
            collect_output(stdout),
            collect_output(stderr),
            write,
        );
        (status, stdout, stderr, write)
    };
    // PHYSICS: bounds one setup subprocess that reads a model catalog. No Work is running behind it.
    let (status, stdout, stderr, write) = match timeout(Duration::from_secs(seconds), work).await {
        Ok(result) => result,
        Err(_) => {
            job.finish(&mut child).await?;
            return Err("CLI health check did not finish".to_owned());
        }
    };
    job.finish(&mut child).await?;
    let status = status.map_err(|error| format!("Could not wait for CLI: {error}"))?;
    write?;
    Ok(Output {
        success: status.success(),
        stdout: String::from_utf8_lossy(&stdout?).into_owned(),
        stderr: String::from_utf8_lossy(&stderr?).into_owned(),
    })
}

/// Runs one short protocol dialogue; input stays open until the answering line arrives.
pub(crate) async fn capture_dialogue(
    program: &LocalCli,
    args: &[String],
    input: &str,
    finished: fn(&str) -> bool,
    seconds: u64,
) -> Result<String, String> {
    let job = Job::new()?;
    let mut command = Command::new(program.program());
    command
        .args(program.prefix())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    job::hidden_suspended(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start {}: {error}", program.program().display()))?;
    let (Some(stdout), Some(mut stdin)) = (child.stdout.take(), child.stdin.take()) else {
        return Err(cleanup_error(
            "Could not open CLI dialogue streams".to_owned(),
            job::reap_unowned(&mut child).await,
        ));
    };
    if let Err(error) = job.assign(&child) {
        let cleanup = job::reap_unowned(&mut child).await.err();
        return Err(cleanup.map_or(error.clone(), |cleanup| format!("{error}; {cleanup}")));
    }
    if let Err(error) = job::resume(&child) {
        let cleanup = job.finish(&mut child).await.err();
        return Err(cleanup.map_or(error.clone(), |cleanup| format!("{error}; {cleanup}")));
    }
    let exchange = async {
        stdin
            .write_all(input.as_bytes())
            .await
            .map_err(|error| format!("Could not write CLI input: {error}"))?;
        stdin
            .flush()
            .await
            .map_err(|error| format!("Could not send CLI input: {error}"))?;
        // Stdin stays open because the peer stops answering after end-of-input.
        let mut lines = tokio::io::BufReader::new(stdout).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) if finished(&line) => return Ok(line),
                Ok(Some(_)) => continue,
                Ok(None) => return Err("CLI ended before answering".to_owned()),
                Err(error) => return Err(format!("Could not read CLI output: {error}")),
            }
        }
    };
    // PHYSICS: bounds one setup dialogue whose peer never exits on its own. No Work is behind it.
    let answer = match timeout(Duration::from_secs(seconds), exchange).await {
        Ok(answer) => answer,
        Err(_) => Err(format!("CLI exceeded {seconds} seconds")),
    };
    // The dialogue peer never exits on its own, so its whole job ends here either way.
    let cleanup = job.finish(&mut child).await;
    answer.and_then(|line| cleanup.map(|()| line))
}

/// CleanupError preserves the command failure when process cleanup also fails.
fn cleanup_error(primary: String, cleanup: Result<(), String>) -> String {
    match cleanup {
        Ok(()) => primary,
        Err(error) => format!("{primary}; cleanup failed: {error}"),
    }
}

/// Installs the official Provider CLI with its native installer.
pub(crate) async fn install_cli(provider: Provider, seconds: u64) -> Result<(), String> {
    let script = match provider {
        Provider::Codex => {
            "$env:CODEX_NON_INTERACTIVE='1'; irm https://chatgpt.com/codex/install.ps1 | iex"
        }
        Provider::Claude => "irm https://claude.ai/install.ps1 | iex",
        Provider::Cursor => "irm 'https://cursor.com/install?win32=true' | iex",
    };
    let args = [
        "-NoLogo".to_owned(),
        "-NoProfile".to_owned(),
        "-ExecutionPolicy".to_owned(),
        "Bypass".to_owned(),
        "-Command".to_owned(),
        script.to_owned(),
    ];
    install_provider(&system_program("powershell.exe")?, &args, seconds)
        .await
        .map_err(|error| format!("{} install failed: {error}", provider.label()))
}

/// Login holds the background sign-in process; dropping it ends the flow.
pub(crate) struct Login {
    _job: Job,
    child: tokio::process::Child,
}

impl Login {
    /// Running reports whether this sign-in is still waiting on the person.
    pub(crate) fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Submit hands the browser's one-time code to the waiting sign-in flow.
    pub(crate) async fn submit(&mut self, code: &str) -> Result<(), String> {
        let stdin = self
            .child
            .stdin
            .as_mut()
            .ok_or_else(|| "The sign-in flow is not waiting for a code".to_owned())?;
        stdin
            .write_all(format!("{code}\n").as_bytes())
            .await
            .map_err(|error| format!("Could not deliver the code: {error}"))?;
        stdin
            .flush()
            .await
            .map_err(|error| format!("Could not deliver the code: {error}"))
    }
}

/// The Agent's own login opens the browser itself and finishes on its local callback, so no console is needed.
pub(crate) fn spawn_login(program: &LocalCli, args: &[String]) -> Result<Login, String> {
    let job = Job::new()?;
    let mut command = Command::new(program.program());
    command
        .args(program.prefix())
        .args(args)
        // Stdin stays open: the login waits on its browser callback and must not read an end-of-input.
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    job::hidden_suspended(&mut command);
    let child = command
        .spawn()
        .map_err(|error| format!("Could not start the sign-in flow: {error}"))?;
    job.assign(&child)?;
    job::resume(&child)?;
    Ok(Login { _job: job, child })
}

/// Hides short ArchiGoat-owned checks from the desktop.
pub(crate) fn background_command(program: &Path) -> Command {
    let mut command = Command::new(program);
    command.as_std_mut().creation_flags(CREATE_NO_WINDOW);
    command
}

/// Runs one official installer and confirms success.
pub(crate) async fn install_provider(
    program: &Path,
    args: &[String],
    seconds: u64,
) -> Result<(), String> {
    let job = Job::new()?;
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    job::hidden_suspended(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start install: {error}"))?;
    if let Err(error) = job.assign(&child) {
        let cleanup = job::reap_unowned(&mut child).await.err();
        return Err(cleanup.map_or(error.clone(), |cleanup| format!("{error}; {cleanup}")));
    }
    if let Err(error) = job::resume(&child) {
        let cleanup = job.finish(&mut child).await.err();
        return Err(cleanup.map_or(error.clone(), |cleanup| format!("{error}; {cleanup}")));
    }
    // PHYSICS: bounds one Provider installer the owner started. No Work is running behind it.
    let status = match timeout(Duration::from_secs(seconds), child.wait()).await {
        Ok(status) => status.map_err(|error| format!("Could not wait for install: {error}")),
        Err(_) => {
            job.finish(&mut child).await?;
            return Err("Provider install did not finish".to_owned());
        }
    };
    job.finish(&mut child).await?;
    let status = status?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("Provider install failed with {status}"))
}

/// Drains process output while retaining bounded diagnostics for the user.
async fn collect_output(mut reader: impl tokio::io::AsyncRead + Unpin) -> Result<Vec<u8>, String> {
    let mut output = Vec::with_capacity(DIAGNOSTIC_BYTES);
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let count = reader
            .read(&mut buffer)
            .await
            .map_err(|error| format!("Could not read CLI output: {error}"))?;
        if count == 0 {
            break;
        }
        let keep = count.min(DIAGNOSTIC_BYTES.saturating_sub(output.len()));
        output.extend_from_slice(&buffer[..keep]);
        truncated |= keep < count;
    }
    if truncated {
        output.extend_from_slice(TRUNCATED);
    }
    Ok(output)
}

/// Lists standard Windows locations where Provider CLIs may exist.
fn known_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(local) = env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        dirs.push(local.join("Programs/OpenAI/Codex/bin"));
        dirs.push(local.join("Programs/Claude"));
        dirs.push(local.join("Programs/Codex"));
    }
    if let Some(home) = env::var_os("USERPROFILE").map(PathBuf::from) {
        dirs.push(home.join(".local/bin"));
    }
    if let Some(roaming) = env::var_os("APPDATA").map(PathBuf::from) {
        dirs.push(roaming.join("npm"));
    }
    dirs
}

/// Accepts only a real Provider executable in one directory.
fn executable_in(dir: &Path, provider: Provider) -> Option<LocalCli> {
    let name = provider.program();
    let powershell = env::var_os("SystemRoot")
        .map(PathBuf::from)
        .map(|root| root.join("System32/WindowsPowerShell/v1.0/powershell.exe"));
    let command = env::var_os("SystemRoot")
        .map(PathBuf::from)
        .map(|root| root.join("System32/cmd.exe"));
    [
        format!("{name}.exe"),
        format!("{name}.ps1"),
        format!("{name}.cmd"),
    ]
    .into_iter()
    .map(|name| dir.join(name))
    .find_map(|path| {
        path.is_file()
            .then(|| {
                let wrapper = match path.extension().and_then(|value| value.to_str()) {
                    Some(value) if value.eq_ignore_ascii_case("ps1") => powershell.clone(),
                    Some(value) if value.eq_ignore_ascii_case("cmd") => command.clone(),
                    _ => None,
                };
                provider.local_cli(path, wrapper)
            })
            .flatten()
    })
}

/// Resolves a Windows system executable by name.
pub(super) fn system_program(name: &str) -> Result<PathBuf, String> {
    let path = env::var_os("SystemRoot")
        .map(PathBuf::from)
        .map(|root| {
            if name.eq_ignore_ascii_case("powershell.exe") {
                root.join("System32/WindowsPowerShell/v1.0/powershell.exe")
            } else {
                root.join("System32").join(name)
            }
        })
        .filter(|path| path.is_file())
        .ok_or_else(|| format!("Windows system program is unavailable: {name}"))?;
    Ok(path)
}
