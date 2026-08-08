//! One conversation's latest delivered app survives Work cleanup and seeds its next Work.

use std::{
    fs, io,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Replace one conversation's kept files with exactly the files in its verified delivery.
pub(crate) fn replace(store: &Path, freeze_root: &Path, names: &[&str]) -> Result<(), String> {
    let metadata = fs::symlink_metadata(freeze_root)
        .map_err(|error| format!("Could not inspect delivered app: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("Delivered app root is invalid".to_owned());
    }
    let parent = store
        .parent()
        .ok_or_else(|| "Delivered app store is invalid".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not prepare delivered app store: {error}"))?;
    let temporary = temporary_path(store)?;
    fs::create_dir(&temporary)
        .map_err(|error| format!("Could not prepare delivered app store: {error}"))?;
    let result =
        copy_names(&temporary, freeze_root, names).and_then(|()| commit(&temporary, store));
    if let Err(error) = result {
        let _ = remove_tree(&temporary);
        return Err(error);
    }
    Ok(())
}

/// Seed a fresh Work with the kept files and return paths whose write access must be restored.
pub(crate) fn seed(store: &Path, workspace: &Path) -> Result<Vec<PathBuf>, String> {
    let metadata = match fs::symlink_metadata(store) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("Could not inspect kept app: {error}")),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("Kept app store is invalid".to_owned());
    }
    fs::create_dir_all(workspace)
        .map_err(|error| format!("Could not prepare Work workspace: {error}"))?;
    let mut copied = Vec::new();
    for entry in fs::read_dir(store).map_err(|error| format!("Could not read kept app: {error}"))? {
        let entry = entry.map_err(|error| format!("Could not read kept app: {error}"))?;
        let file_name = entry.file_name();
        let name = file_name
            .to_str()
            .ok_or_else(|| "Kept app file name is invalid".to_owned())?;
        safe_name(name)?;
        let source = entry.path();
        let metadata = fs::symlink_metadata(&source)
            .map_err(|error| format!("Could not inspect kept app: {error}"))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("Kept app contains a non-file".to_owned());
        }
        let destination = workspace.join(name);
        fs::copy(&source, &destination)
            .map_err(|error| format!("Could not seed kept app: {error}"))?;
        copied.push(destination);
    }
    Ok(copied)
}

/// Copy only the manifest names from immutable verified bytes into a new store.
fn copy_names(temporary: &Path, freeze_root: &Path, names: &[&str]) -> Result<(), String> {
    for name in names {
        safe_name(name)?;
        let source = freeze_root.join(name);
        let metadata = fs::symlink_metadata(&source)
            .map_err(|error| format!("Could not inspect delivered app file: {error}"))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("Delivered app file is invalid".to_owned());
        }
        let destination = temporary.join(name);
        fs::copy(&source, &destination)
            .map_err(|error| format!("Could not keep delivered app file: {error}"))?;
        make_writable(&destination)?;
    }
    Ok(())
}

/// Swap a complete new store into place while retaining the previous store until the swap succeeds.
fn commit(temporary: &Path, store: &Path) -> Result<(), String> {
    let existing = match fs::symlink_metadata(store) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err("Delivered app store is invalid".to_owned());
            }
            true
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(format!("Could not inspect delivered app store: {error}")),
    };
    let backup = existing.then(|| {
        store.with_file_name(format!(
            ".{}-old-{}",
            store
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("app"),
            unique_suffix(),
        ))
    });
    if let Some(backup) = &backup {
        fs::rename(store, backup)
            .map_err(|error| format!("Could not stage previous app store: {error}"))?;
    }
    if let Err(error) = fs::rename(temporary, store) {
        if let Some(backup) = &backup {
            let _ = fs::rename(backup, store);
        }
        return Err(format!("Could not commit delivered app store: {error}"));
    }
    if let Some(backup) = backup {
        remove_tree(&backup)?;
    }
    Ok(())
}

/// Generate a private sibling path without adding a dependency to the standalone test module.
fn temporary_path(store: &Path) -> Result<PathBuf, String> {
    let name = store
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "Delivered app store is invalid".to_owned())?;
    for attempt in 0..16_u128 {
        let candidate = store.with_file_name(format!(".{name}-new-{}", unique_suffix() + attempt));
        match fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(candidate),
            Ok(_) => continue,
            Err(error) => return Err(format!("Could not prepare delivered app store: {error}")),
        }
    }
    Err("Could not reserve delivered app store".to_owned())
}

/// The store is private and writeable so a later replacement can remove its previous files.
fn make_writable(path: &Path) -> Result<(), String> {
    let mut permissions = fs::metadata(path)
        .map_err(|error| format!("Could not inspect delivered app file: {error}"))?
        .permissions();
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions)
        .map_err(|error| format!("Could not prepare delivered app file: {error}"))
}

/// Reject every path shape except one ordinary file name from the verified Work manifest.
fn safe_name(name: &str) -> Result<&Path, String> {
    let path = Path::new(name);
    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Ok(path),
        _ => Err("Delivered app file name is invalid".to_owned()),
    }
}

/// Remove one private directory while treating an already-clean path as success.
fn remove_tree(path: &Path) -> Result<(), String> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Could not remove delivered app store: {error}")),
    }
}

/// A process-local timestamp makes concurrent store swaps use different sibling paths.
fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_nanos())
        .saturating_add(u128::from(std::process::id()))
}
