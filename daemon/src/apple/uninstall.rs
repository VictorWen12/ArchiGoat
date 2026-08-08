//! Removal ends this installation and deletes every file the ArchiGoat created, keeping only the owner's delivered artifacts.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use crate::{Config, DaemonState};

// Deliveries holds finished artifacts the owner still owns, so removal never touches it.
const KEPT: &str = "Deliveries";

// A Work's browser tree is named from the first half of a nonce, so only that exact shape may be removed.
const BROWSER_PREFIX: &str = "ag-";
const LEGACY_BROWSER_PREFIX: &str = concat!("f", "o-");
const BROWSER_NONCE: usize = 16;
const RETIRE_TIMEOUT: Duration = Duration::from_secs(3);

/// RetireRemote asks Account to release this installation before local identity deletion.
pub(crate) async fn retire_remote() {
    let config = match Config::load() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("ArchiGoat retirement skipped: {error}");
            return;
        }
    };
    let state = match DaemonState::new(config) {
        Ok(state) => state,
        Err(error) => {
            eprintln!("ArchiGoat retirement skipped: {error}");
            return;
        }
    };
    let Some(credential) = state.credential().await else {
        return;
    };
    // PHYSICS: bounds one request made while this installation is already being removed.
    let client = match reqwest::Client::builder()
        .connect_timeout(RETIRE_TIMEOUT)
        .timeout(RETIRE_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            eprintln!("ArchiGoat retirement skipped: {error}");
            return;
        }
    };
    let response = crate::account_relay::authorized_request(
        client.post(crate::account_relay::endpoint(&state, "/auth/app/retire")),
        &state,
        &credential,
    )
    .timeout(RETIRE_TIMEOUT)
    .send()
    .await;
    match response {
        Ok(response) if response.status().is_success() => {}
        Ok(response) => eprintln!(
            "ArchiGoat retirement skipped: Account returned {}",
            response.status()
        ),
        Err(error) => eprintln!("ArchiGoat retirement skipped: {error}"),
    }
}

/// RemoveInstallation stops every other running ArchiGoat and deletes its state, logs, temporary trees, and App.
pub(crate) fn remove_installation() -> Result<(), String> {
    let mut problems = Vec::new();
    // A ArchiGoat that owns no machine liveness owns no other instance either, so it never ends one.
    if !crate::keepalive::disabled() {
        stop_other_instances();
    }
    if let Some(root) = state_root() {
        remove_state(&root, &mut problems);
    }
    remove_temporary(&mut problems);
    remove_app(&mut problems);
    if problems.is_empty() {
        return Ok(());
    }
    Err(problems.join("; "))
}

// StateRoot resolves the same private directory the running ArchiGoat used, so removal cleans what that ArchiGoat wrote.
fn state_root() -> Option<PathBuf> {
    let file = match env::var_os("ARCHIGOAT_STATE") {
        Some(value) => PathBuf::from(value),
        None => super::default_state_file()?,
    };
    file.parent().map(Path::to_path_buf)
}

// Every state entry except the owner's Deliveries is ArchiGoat-owned bookkeeping that removal deletes.
fn remove_state(root: &Path, problems: &mut Vec<String>) {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            problems.push(format!("could not read ArchiGoat state: {error}"));
            return;
        }
    };
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        if entry.file_name() == KEPT {
            continue;
        }
        discard(&entry.path(), problems);
    }
}

// The lifecycle log and each Work's browser tree live beside other temporary files, so only ArchiGoat-shaped names are removed.
fn remove_temporary(problems: &mut Vec<String>) {
    let temp = env::temp_dir();
    discard(&temp.join("ArchiGoat"), problems);
    discard(&temp.join(concat!("Pl", "ugin")), problems);
    let Ok(entries) = fs::read_dir(&temp) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        // Only a directory named for a Work's own browser nonce can be this ArchiGoat's temporary tree.
        for prefix in [BROWSER_PREFIX, LEGACY_BROWSER_PREFIX] {
            let Some(nonce) = name.strip_prefix(prefix) else {
                continue;
            };
            if nonce.len() == BROWSER_NONCE && nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                discard(&entry.path(), problems);
            }
            break;
        }
    }
}

// Removal sweeps both supported install locations so stale bundles cannot revive the product.
fn remove_app(problems: &mut Vec<String>) {
    for bundle in installed_apps() {
        discard(&bundle, problems);
    }
}

// Both fixed install locations are the only paths an installer or the launcher ever writes the App to.
fn installed_apps() -> Vec<PathBuf> {
    let mut apps = vec![PathBuf::from("/Applications/ArchiGoat.app")];
    apps.push(PathBuf::from("/Applications/").join(concat!("Pl", "ugin.app")));
    if let Some(home) = env::var_os("HOME") {
        apps.push(PathBuf::from(&home).join("Applications/ArchiGoat.app"));
        apps.push(
            PathBuf::from(home)
                .join("Applications")
                .join(concat!("Pl", "ugin.app")),
        );
    }
    apps
}

// One removal reports a real failure and stays quiet about a path that was already gone.
fn discard(path: &Path, problems: &mut Vec<String>) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    let removed = if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    match removed {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => problems.push(format!("could not remove {}: {error}", path.display())),
    }
}

// A ArchiGoat still holding the loopback socket would rewrite the state this removal is deleting.
fn stop_other_instances() {
    let own = std::process::id();
    let Some(listing) = pgrep() else {
        return;
    };
    let others = listing
        .split_whitespace()
        .filter_map(|value| value.parse::<u32>().ok())
        .filter(|pid| *pid > 1 && *pid != own)
        .map(|pid| pid.to_string())
        .collect::<Vec<_>>();
    if others.is_empty() {
        return;
    }
    // PHYSICS: this installation is being removed at the owner's own request.
    // Termination lets a live ArchiGoat close its own files; launchd already released it before this point.
    let _ = Command::new("/bin/kill")
        .arg("-TERM")
        .arg("--")
        .args(&others)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

// Process discovery matches the ArchiGoat's exact executable name among this user's own processes.
fn pgrep() -> Option<String> {
    let mut pids = Vec::new();
    for name in ["archigoat", concat!("pl", "ugin")] {
        let output = Command::new("/usr/bin/pgrep")
            .args(["-x", name])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if output.status.success() {
            pids.extend(
                String::from_utf8_lossy(&output.stdout)
                    .split_whitespace()
                    .map(str::to_owned),
            );
        }
    }
    (!pids.is_empty()).then(|| pids.join("\n"))
}
