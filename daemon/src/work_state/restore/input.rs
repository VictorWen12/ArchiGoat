//! Work input recovery keeps immutable launch evidence private and stable across format upgrades.

use std::path::{Path, PathBuf};

/// Restore accepts the current control path and migrates one exact legacy input when necessary.
pub(super) fn restore(session: &Path, path: PathBuf, legacy: String) -> Result<PathBuf, String> {
    let current = session.join(".app").join("input.json");
    let prior = session.join("input.json");
    if path == current {
        crate::work::runtime::read_input(&current)?;
        return Ok(current);
    }
    if !path.as_os_str().is_empty() && path != prior {
        return Err("Stored Work input path changed".to_owned());
    }
    let bytes = if prior.exists() {
        crate::work::runtime::read_input(&prior)?
    } else if !legacy.is_empty() {
        legacy.clone()
    } else if current.exists() {
        crate::work::runtime::read_input(&current)?
    } else {
        return Err("Stored Work input is invalid".to_owned());
    };
    if !legacy.is_empty() && bytes != legacy {
        return Err("Stored Work input changed during legacy recovery".to_owned());
    }
    migrate(&prior, &current, bytes.as_bytes())?;
    Ok(current)
}

/// Migrate hides legacy evidence from deliverable workspace bytes exactly once.
fn migrate(prior: &Path, current: &Path, bytes: &[u8]) -> Result<(), String> {
    let control = current
        .parent()
        .ok_or_else(|| "Stored Work input path is invalid".to_owned())?;
    crate::host::create_private_dir(control)?;
    if current.exists() {
        if crate::work::runtime::read_input(current)?.as_bytes() != bytes {
            return Err("Stored Work input changed during migration".to_owned());
        }
    } else {
        crate::work::runtime::preserve_input(current, bytes)?;
    }
    if prior.exists() {
        crate::host::make_writable(prior)?;
        std::fs::remove_file(prior)
            .map_err(|error| format!("Could not remove legacy Work input: {error}"))?;
    }
    crate::work::runtime::read_input(current)?;
    Ok(())
}
