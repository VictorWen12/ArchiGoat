//! The janitor delivers what a restart parked and expires only trees released by durable lifecycle truth.

// Standard storage and time tools inspect exact Work-owned paths.
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

// DaemonState supplies the locked Work snapshot and private storage root.
use crate::state::DaemonState;

/// Terminal sessions remain recoverable for one week after their persisted end.
const TERMINAL_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Unknown session directories remain recoverable for three months.
const ORPHAN_RETENTION: Duration = Duration::from_secs(90 * 24 * 60 * 60);

/// Startup is the one moment a Work parked by a dead runner delivers without its owner asking.
static REHYDRATED: std::sync::Once = std::sync::Once::new();

/// Sweep delivers parked products once, then removes only expired terminal or long-orphaned trees.
pub(super) fn sweep(state: &DaemonState) -> Result<(), String> {
    REHYDRATED.call_once(|| state.rehydrate_parked_deliveries());
    let (sessions, deliveries) = {
        let works = state
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (works.terminal_sessions(), works.delivery_roots())
    };
    let private_root = state.private_root()?;
    let works_root = private_root.join("Works");
    let deliveries_root = private_root.join("Deliveries");
    let now = SystemTime::now();
    let now_ms: u64 = now
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("System time is invalid: {error}"))?
        .as_millis()
        .try_into()
        .map_err(|_| "System time is out of range".to_owned())?;

    let reaped = expire(&works_root, known(&works_root, sessions)?, now, now_ms)?;
    // A reaped workspace retires its store entry before anything else may fail, so the durable map
    // never regrows around deleted trees.
    if !reaped.is_empty() {
        let mut works = state
            .works
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if works.retire_reaped(&reaped) {
            works.save(state.work_state_path())?;
        }
    }
    // A frozen delivery no Work owns is the same garbage as an orphaned workspace.
    expire(
        &deliveries_root,
        known(&deliveries_root, deliveries)?,
        now,
        now_ms,
    )?;
    Ok(())
}

/// Known indexes every durable tree under one root; a path outside it can never authorize deletion.
fn known(
    root: &Path,
    stored: Vec<(PathBuf, Option<u64>)>,
) -> Result<HashMap<PathBuf, Option<u64>>, String> {
    let mut trees = HashMap::new();
    for (path, ended_at) in stored {
        let valid = path.parent() == Some(root)
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(crate::proof::valid_nonce);
        if !valid || trees.insert(path, ended_at).is_some() {
            return Err("Stored Work session path is invalid".to_owned());
        }
    }
    Ok(trees)
}

/// Expire removes only nonce trees whose durable owner ended or never existed, and names what it removed.
fn expire(
    root: &Path,
    known: HashMap<PathBuf, Option<u64>>,
    now: SystemTime,
    now_ms: u64,
) -> Result<HashSet<PathBuf>, String> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(error) => return Err(format!("Could not inspect Work sessions: {error}")),
    };
    let mut reaped = HashSet::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("Could not inspect Work session: {error}"))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !crate::proof::valid_nonce(name) {
            continue;
        }
        let path = entry.path();
        match known.get(&path) {
            Some(None) => continue,
            Some(Some(ended_at))
                if Duration::from_millis(now_ms.saturating_sub(*ended_at)) < TERMINAL_RETENTION =>
            {
                continue;
            }
            Some(Some(_)) => {}
            None => {
                let metadata = match fs::symlink_metadata(&path) {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == ErrorKind::NotFound => continue,
                    Err(error) => {
                        return Err(format!("Could not inspect orphaned Work session: {error}"));
                    }
                };
                let Ok(modified) = metadata.modified() else {
                    continue;
                };
                let Ok(age) = now.duration_since(modified) else {
                    continue;
                };
                if !metadata.is_dir() || age < ORPHAN_RETENTION {
                    continue;
                }
            }
        }
        crate::delivery::discard_private_tree(&path)?;
        reaped.insert(path);
    }
    Ok(reaped)
}
