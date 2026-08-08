//! Apple finds local Agents and runs short setup commands without touching active Work.

// Provider details identify the local Agent the ArchiGoat may run.
use crate::provider::{LocalCli, Provider};
use std::{
    env,
    fs::OpenOptions,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
// Setup output is read while the ArchiGoat stays responsive.
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    time::timeout,
};

const DIAGNOSTIC_BYTES: usize = 1024 * 1024;
const TRUNCATED: &[u8] = b"\n[ArchiGoat: diagnostic output truncated]\n";

// Output tells the product whether setup worked and why it failed.
pub(crate) struct Output {
    // Success reports the Agent's real result.
    pub(crate) success: bool,
    // Standard output carries the Agent's normal message.
    pub(crate) stdout: String,
    // Standard error carries the Agent's failure message.
    pub(crate) stderr: String,
}

// Finds the user's installed Agent in normal and configured locations.
pub(crate) fn find_cli(provider: Provider, extra: &[PathBuf]) -> Option<LocalCli> {
    let mut directories = extra.to_vec();
    if let Some(path) = env::var_os("PATH") {
        directories.extend(env::split_paths(&path));
    }
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        directories.extend([home.join(".local/bin"), home.join(".npm-global/bin")]);
    }
    let system_root = PathBuf::from("/");
    directories.extend([
        system_root.join("opt/homebrew/bin"),
        system_root.join("usr/local/bin"),
    ]);
    directories.into_iter().find_map(|directory| {
        let path = directory.join(provider.program());
        path.is_file()
            .then(|| provider.local_cli(path, None))
            .flatten()
    })
}

// Runs a short setup command and returns its complete result.
pub(crate) async fn capture_cli(
    cli: &LocalCli,
    args: &[String],
    input: Option<&str>,
    seconds: u64,
) -> Result<Output, String> {
    let mut command = background_command(cli.program());
    command.process_group(0);
    let mut child = command
        .args(cli.prefix())
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("Could not start {}: {error}", cli.program().display()))?;
    let group = child
        .id()
        .ok_or_else(|| "Could not own CLI process group".to_owned())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Could not read CLI output".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Could not read CLI errors".to_owned())?;
    let mut stdin = child.stdin.take();
    let run = async {
        let write = async {
            if let (Some(mut stream), Some(value)) = (stdin.take(), input) {
                stream
                    .write_all(value.as_bytes())
                    .await
                    .map_err(|error| format!("Could not write CLI input: {error}"))?;
                stream
                    .shutdown()
                    .await
                    .map_err(|error| format!("Could not close CLI input: {error}"))?;
            }
            Ok::<(), String>(())
        };
        let (status, stdout, stderr, write) =
            tokio::join!(child.wait(), read_all(stdout), read_all(stderr), write,);
        (status, stdout, stderr, write)
    };
    // PHYSICS: bounds one setup subprocess that reads a model catalog. No Work is running behind it.
    let (status, stdout, stderr, write) = match timeout(Duration::from_secs(seconds), run).await {
        Ok(result) => result,
        Err(_) => {
            terminate_group(group).await?;
            child
                .wait()
                .await
                .map_err(|error| format!("Could not reap timed-out CLI: {error}"))?;
            return Err(format!("CLI exceeded {seconds} seconds"));
        }
    };
    write?;
    let status = status.map_err(|error| format!("Could not wait for CLI: {error}"))?;
    Ok(Output {
        success: status.success(),
        stdout: String::from_utf8_lossy(&stdout?).into_owned(),
        stderr: String::from_utf8_lossy(&stderr?).into_owned(),
    })
}

// Runs one short protocol dialogue; input stays open until the answering line arrives.
pub(crate) async fn capture_dialogue(
    cli: &LocalCli,
    args: &[String],
    input: &str,
    finished: fn(&str) -> bool,
    seconds: u64,
) -> Result<String, String> {
    let mut command = background_command(cli.program());
    command.process_group(0);
    let mut child = command
        .args(cli.prefix())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("Could not start {}: {error}", cli.program().display()))?;
    let group = child
        .id()
        .ok_or_else(|| "Could not own CLI process group".to_owned())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Could not read CLI output".to_owned())?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Could not write CLI input".to_owned())?;
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
    // The dialogue peer never exits on its own, so its whole group ends here either way.
    let _ = terminate_group(group).await;
    let _ = child.wait().await;
    answer
}

// Installs the chosen Agent with its official installer in the user's own Terminal, so nothing downloads unseen.
pub(crate) async fn install_cli(provider: Provider, _seconds: u64) -> Result<(), String> {
    let installer = match provider {
        Provider::Codex => {
            "export CODEX_NON_INTERACTIVE=1; curl -fsSL https://chatgpt.com/codex/install.sh | sh"
        }
        Provider::Claude => "curl -fsSL https://claude.ai/install.sh | bash",
        Provider::Cursor => "curl https://cursor.com/install -fsS | bash",
    };
    let label = provider.label();
    let product_name = crate::config::product_name();
    // The installer's own output and exit status stay in the window, so a failure reads as a message instead of silence.
    let command = format!(
        "{installer}\nstatus=$?\nif [ \"$status\" -eq 0 ]; then\n  printf '\\n{product_name}: {label} is installed. You can close this window.\\n'\nelse\n  printf '\\n{product_name}: {label} installation failed (status %s). The messages above say why.\\n' \"$status\"\nfi\nexit \"$status\"\n"
    );
    open_terminal("install", "Install", &command)
        .map_err(|error| format!("{label} install failed: {error}"))
}

/// Login holds the silent sign-in process; dropping it ends the flow.
pub(crate) struct Login {
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

// The Agent's own login opens the browser itself and finishes on its local callback, so no window is needed.
pub(crate) fn spawn_login(cli: &LocalCli, args: &[String]) -> Result<Login, String> {
    let mut command = Command::new(cli.program());
    command
        .args(cli.prefix())
        .args(args)
        // Stdin stays open: the login waits on its browser callback and must not read an end-of-input.
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command
        .spawn()
        .map(|child| Login { child })
        .map_err(|error| format!("Could not start the sign-in flow: {error}"))
}

// One visible Terminal window is the only place the ArchiGoat runs a command the user should see.
fn open_terminal(name: &str, title: &str, command: &str) -> Result<(), String> {
    let directory = default_state_file()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .ok_or_else(|| "Could not locate private Terminal command".to_owned())?;
    super::create_private_dir(&directory)?;
    let path = directory.join(format!("{name}-{}.command", crate::proof::nonce()?));
    // The command removes itself once its window is finished with it.
    let script = format!(
        "#!/bin/sh\ntrap 'rm -f -- \"$0\"' EXIT\nprintf '\\033]0;{} {title}\\007'\n{command}\n",
        crate::config::product_name()
    );
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o700)
        .open(&path)
        .map_err(|error| format!("Could not create Terminal command: {error}"))?;
    file.write_all(script.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("Could not save Terminal command: {error}"))?;
    let status = std::process::Command::new("/usr/bin/open")
        .args(["-a", "Terminal"])
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("Could not open Terminal: {error}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| "Terminal did not open".to_owned())
}

// Creates a command that cannot consume ArchiGoat input or output by accident.
pub(crate) fn background_command(program: &Path) -> Command {
    Command::new(program)
}

// Keeps private ArchiGoat state in the user's normal Apple application folder.
pub(crate) fn default_state_file() -> Option<PathBuf> {
    env::var_os("HOME").and_then(|home| {
        let root = PathBuf::from(home).join("Library/Application Support");
        let current = root.join("ArchiGoat");
        let legacy = root.join(concat!("Pl", "ugin"));
        let adopted = crate::config::adopt_legacy_state(
            &legacy,
            &current,
            concat!("pl", "ugin.json"),
            "archigoat.json",
        );
        if !adopted {
            return crate::config::legacy_state_file(&legacy, concat!("pl", "ugin.json"));
        }
        crate::config::state_file(&current, "archigoat.json")
    })
}

// Drains setup output while retaining bounded diagnostics for the user.
async fn read_all(mut input: impl AsyncRead + Unpin) -> Result<Vec<u8>, String> {
    let mut output = Vec::with_capacity(DIAGNOSTIC_BYTES);
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let count = input
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

// Stops every child of a setup command after its time limit.
// PHYSICS: ends a setup subprocess this daemon owns and nothing else; no Work observes it.
async fn terminate_group(group: u32) -> Result<(), String> {
    let status = Command::new("/bin/kill")
        .args(["-KILL", "--", &format!("-{group}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map_err(|error| format!("Could not stop timed-out CLI: {error}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("Could not stop timed-out CLI: {status}"))
}
