//! Apple hosts local CLIs and one headless runner for each Work.

// Agent discovery, setup, and login stay separate from active Work.
mod cli;
// Terminal gives each Work one headless runner and one stoppable process family.
mod terminal;
// Removal deletes everything this installation created except the owner's delivered artifacts.
mod uninstall;

pub(crate) use cli::{
    Login, Output, capture_cli, capture_dialogue, default_state_file, find_cli, install_cli,
    spawn_login,
};
pub(crate) use terminal::{AgentRun, launch, reattach};
pub(crate) use uninstall::{remove_installation, retire_remote};

// Random names keep unrelated private files separate.
use crate::proof;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};

// The Mac reports the exact hostname its owner already sees on their network.
pub(crate) fn machine_name() -> Option<String> {
    let output = std::process::Command::new("/bin/hostname").output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).ok())
        .flatten()
}

// Private files are created once so other local users cannot read or replace them.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("Could not create private state: {error}"))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("Could not save private state: {error}"))
}

// Replacement makes every saved ArchiGoat fact appear complete or not at all.
pub(crate) fn replace_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Private state path is invalid".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create private state directory: {error}"))?;
    let next = parent.join(format!(".next-{}", proof::nonce()?));
    write_private(&next, bytes)?;
    if let Err(error) = fs::rename(&next, path) {
        let primary = format!("Could not replace private state: {error}");
        return match fs::remove_file(&next) {
            Ok(()) => Err(primary),
            Err(cleanup) if cleanup.kind() == std::io::ErrorKind::NotFound => Err(primary),
            Err(cleanup) => Err(format!(
                "{primary}; could not remove staged state: {cleanup}"
            )),
        };
    }
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("Could not commit private state: {error}"))
}

// Owner write access lets Product remove only state it owns.
pub(crate) fn make_writable(path: &Path) -> Result<(), String> {
    let mut permissions = match fs::metadata(path) {
        Ok(metadata) => metadata.permissions(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("Could not inspect local file permissions: {error}")),
    };
    permissions.set_mode(permissions.mode() | 0o200);
    match fs::set_permissions(path, permissions) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Could not restore local file ownership: {error}")),
    }
}

// A private directory keeps Work data away from other local users.
pub(crate) fn create_private_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path)
        .map_err(|error| format!("Could not create private directory: {error}"))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("Could not protect private directory: {error}"))
}

// Link checks prevent a private path from being redirected elsewhere.
pub(crate) fn linked(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

// The helper route starts signed Terminal Work without adding a public command.
pub(crate) async fn run_helper(args: &[String]) -> Result<bool, String> {
    match args.first().map(String::as_str) {
        Some("--terminal-work") => {
            if args.len() != 2 {
                return Err("Terminal Work requires one signed job".to_owned());
            }
            terminal::run_terminal_work(Path::new(&args[1])).await?;
            Ok(true)
        }
        _ => Ok(false),
    }
}
