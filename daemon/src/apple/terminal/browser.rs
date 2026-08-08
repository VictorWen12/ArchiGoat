//! A Work's browser automation dies with its Work, so the user's own Chrome can quit afterwards.

use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Read,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    process::Command,
    task::JoinSet,
};

const MAX_SESSIONS: usize = 128;
const MAX_SESSION_ENTRIES: usize = 512;
const MAX_SESSION_BYTES: u64 = 64 * 1024;
const RPC_DEADLINE: Duration = Duration::from_millis(500);
const PROCESS_DEADLINE: Duration = Duration::from_secs(3);
const STOP_REQUEST: &[u8] = b"{\"id\":1,\"method\":\"stop\",\"params\":{}}\n";

// BrowserScope owns the private paths and immutable identity needed for safe teardown.
pub(super) struct BrowserScope {
    sessions: PathBuf,
    temp: PathBuf,
    uid: u32,
    dev: u64,
    ino: u64,
}

// One session file names the socket that can ask its browser to close.
#[derive(Deserialize)]
struct Session {
    #[serde(rename = "socketPath")]
    socket_path: PathBuf,
}

// This scope confines a Work's browser at launch so the same Work can release it at the end.
impl BrowserScope {
    // Prepare binds browser state to the Work nonce and owner-only directories.
    pub(super) fn prepare(root: &Path) -> Result<Self, String> {
        let nonce = root
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| crate::proof::valid_nonce(value))
            .ok_or_else(|| "Apple browser Work nonce is invalid".to_owned())?;
        let root_metadata = fs::symlink_metadata(root)
            .map_err(|error| format!("Could not inspect Apple Work: {error}"))?;
        if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
            return Err("Apple Work is not a directory".to_owned());
        }
        let uid = root_metadata.uid();
        let sessions = root.join("browser-sessions");
        // A short owner temp path leaves enough bytes for Playwright's macOS Unix socket.
        let temp = std::env::temp_dir().join(format!("ag-{}", &nonce[..16]));
        private_directory(&sessions, uid)?;
        let metadata = private_directory(&temp, uid)?;
        Ok(Self {
            sessions,
            temp,
            uid,
            dev: metadata.dev(),
            ino: metadata.ino(),
        })
    }

    // Apply keeps every browser session and socket this Work opens inside paths the Work owns.
    pub(super) fn apply(&self, command: &mut Command) {
        command
            .env("PWTEST_DAEMON_SESSION_DIR", &self.sessions)
            .env("TMPDIR", &self.temp)
            .env("PWTEST_SOCKETS_DIR", &self.temp);
    }

    // Stop asks known sessions to close, kills only proven private-tree holders, then removes that tree.
    pub(super) async fn stop(&self) -> Result<(), String> {
        self.revalidate_temp()?;
        let mut stops = JoinSet::new();
        for socket in session_sockets(&self.sessions, &self.temp).unwrap_or_default() {
            stops.spawn(async move {
                // PHYSICS: a close request to a socket nobody answers; the deadline ends the request
                // and never the Work, whose turn has already finished by the time this runs.
                let _ = tokio::time::timeout(RPC_DEADLINE, stop_socket(&socket)).await;
            });
        }
        let asked = !stops.is_empty();
        while stops.join_next().await.is_some() {}
        // The wait is for a browser that was asked to close to finish closing. A turn that opened no
        // browser asked nobody, and waits for nobody.
        if asked {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        self.revalidate_temp()?;
        // A browser that ignored the close request still holds this tree, and holding it holds the user's Chrome.
        let live_holders = holders(self.uid, &self.temp).await?;
        if !live_holders.is_empty() {
            self.revalidate_temp()?;
            let mut kill = Command::new("/bin/kill");
            kill.arg("-KILL");
            for pid in live_holders {
                kill.arg(pid.to_string());
            }
            // PHYSICS: a browser still holding this Work's private tree holds the user's Chrome.
            let _ = tokio::time::timeout(PROCESS_DEADLINE, kill.status()).await;
            // The wait is for a killed process to leave the table before it is counted again.
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        self.revalidate_temp()?;
        if !holders(self.uid, &self.temp).await?.is_empty() {
            return Err("Apple browser processes survived cleanup".to_owned());
        }

        self.revalidate_temp()?;
        fs::remove_dir_all(&self.temp)
            .map_err(|error| format!("Could not remove Apple browser state: {error}"))
    }

    // Revalidation prevents a replaced path from becoming the cleanup target.
    fn revalidate_temp(&self) -> Result<(), String> {
        let metadata = fs::symlink_metadata(&self.temp)
            .map_err(|error| format!("Could not revalidate Apple browser state: {error}"))?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != self.uid
            || metadata.dev() != self.dev
            || metadata.ino() != self.ino
            || metadata.permissions().mode() & 0o7777 != 0o700
        {
            return Err("Apple browser state identity changed".to_owned());
        }
        Ok(())
    }
}

// Private directories must remain real, owner-matched, and inaccessible to other users.
fn private_directory(path: &Path, uid: u32) -> Result<fs::Metadata, String> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(format!(
                "Could not create Apple browser directory {}: {error}",
                path.display()
            ));
        }
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect Apple browser directory: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || metadata.uid() != uid {
        return Err("Apple browser directory ownership is invalid".to_owned());
    }
    let directory = File::open(path)
        .map_err(|error| format!("Could not open Apple browser directory: {error}"))?;
    let opened = directory
        .metadata()
        .map_err(|error| format!("Could not verify Apple browser directory: {error}"))?;
    if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
        return Err("Apple browser directory identity changed".to_owned());
    }
    directory
        .set_permissions(fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("Could not protect Apple browser directory: {error}"))?;
    let opened = directory
        .metadata()
        .map_err(|error| format!("Could not verify Apple browser directory: {error}"))?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Could not revalidate Apple browser directory: {error}"))?;
    if !opened.is_dir()
        || opened.uid() != uid
        || opened.permissions().mode() & 0o7777 != 0o700
        || metadata.file_type().is_symlink()
        || metadata.dev() != opened.dev()
        || metadata.ino() != opened.ino()
    {
        return Err("Apple browser directory protection is invalid".to_owned());
    }
    Ok(opened)
}

// Session discovery accepts only bounded regular files one directory below the private root.
fn session_sockets(sessions: &Path, temp: &Path) -> Result<Vec<PathBuf>, String> {
    let mut sockets = Vec::new();
    let mut entries = 0;
    let mut sessions_seen = 0;
    for directory in fs::read_dir(sessions)
        .map_err(|error| format!("Could not read Apple browser sessions: {error}"))?
    {
        entries += 1;
        if entries > MAX_SESSION_ENTRIES {
            return Err("Apple browser session entries exceed their bound".to_owned());
        }
        let directory =
            directory.map_err(|error| format!("Could not read Apple browser session: {error}"))?;
        if !directory
            .file_type()
            .map_err(|error| format!("Could not inspect Apple browser session: {error}"))?
            .is_dir()
        {
            continue;
        }
        for entry in fs::read_dir(directory.path())
            .map_err(|error| format!("Could not read Apple browser session: {error}"))?
        {
            entries += 1;
            if entries > MAX_SESSION_ENTRIES {
                return Err("Apple browser session entries exceed their bound".to_owned());
            }
            let entry =
                entry.map_err(|error| format!("Could not read Apple browser session: {error}"))?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("session")
                || !entry
                    .file_type()
                    .map_err(|error| {
                        format!("Could not inspect Apple browser session file: {error}")
                    })?
                    .is_file()
            {
                continue;
            }
            sessions_seen += 1;
            if sessions_seen > MAX_SESSIONS {
                return Err("Apple browser session count exceeds its bound".to_owned());
            }
            if let Some(socket) = session_socket(&entry.path(), temp)? {
                sockets.push(socket);
            }
        }
    }
    Ok(sockets)
}

// One bounded session file can name only an existing Unix socket inside the private temp tree.
fn session_socket(path: &Path, temp: &Path) -> Result<Option<PathBuf>, String> {
    let file = File::open(path)
        .map_err(|error| format!("Could not open Apple browser session: {error}"))?;
    if !file
        .metadata()
        .map_err(|error| format!("Could not inspect Apple browser session: {error}"))?
        .is_file()
    {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    file.take(MAX_SESSION_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read Apple browser session: {error}"))?;
    if bytes.len() as u64 > MAX_SESSION_BYTES {
        return Err("Apple browser session exceeds its byte bound".to_owned());
    }
    let Ok(session) = serde_json::from_slice::<Session>(&bytes) else {
        return Ok(None);
    };
    let Ok(relative) = session.socket_path.strip_prefix(temp) else {
        return Ok(None);
    };
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Ok(None);
    }
    let Ok(metadata) = fs::symlink_metadata(&session.socket_path) else {
        return Ok(None);
    };
    Ok(metadata
        .file_type()
        .is_socket()
        .then_some(session.socket_path))
}

// The browser session receives exactly one close request and its bounded acknowledgement is consumed.
async fn stop_socket(path: &Path) -> Result<(), String> {
    let mut stream = UnixStream::connect(path)
        .await
        .map_err(|error| format!("Could not contact Apple browser session: {error}"))?;
    stream
        .write_all(STOP_REQUEST)
        .await
        .map_err(|error| format!("Could not stop Apple browser session: {error}"))?;
    let mut consumed = 0;
    let mut buffer = [0_u8; 1024];
    loop {
        let read = stream
            .read(&mut buffer)
            .await
            .map_err(|error| format!("Could not read Apple browser session reply: {error}"))?;
        if read == 0 || buffer[..read].contains(&b'\n') {
            return Ok(());
        }
        consumed += read;
        if consumed > MAX_SESSION_BYTES as usize {
            return Err("Apple browser session reply exceeds its byte bound".to_owned());
        }
    }
}

// lsof identifies only processes of the Work owner that physically hold the private tree.
async fn holders(uid: u32, temp: &Path) -> Result<BTreeSet<u32>, String> {
    let mut command = Command::new("/usr/sbin/lsof");
    command
        .args(["-w", "-nP", "-t", "-a", "-u"])
        .arg(uid.to_string())
        .arg("+D")
        .arg(temp)
        .kill_on_drop(true);
    // PHYSICS: bounds one cleanup probe of this machine, outside any live turn.
    let output = tokio::time::timeout(PROCESS_DEADLINE, command.output())
        .await
        .map_err(|_| "Apple browser process proof timed out".to_owned())?
        .map_err(|error| format!("Could not inspect Apple browser processes: {error}"))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !matches!(output.status.code(), Some(0 | 1)) || !stderr.trim().is_empty() {
        return Err(format!(
            "Could not prove Apple browser processes stopped ({:?}): {}",
            output.status.code(),
            stderr.trim()
        ));
    }
    let own_pid = std::process::id();
    let mut pids = BTreeSet::new();
    for line in output.stdout.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let pid = std::str::from_utf8(line)
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .ok_or_else(|| "Apple browser process proof is invalid".to_owned())?;
        if pid > 1 && pid != own_pid {
            pids.insert(pid);
        }
    }
    Ok(pids)
}
