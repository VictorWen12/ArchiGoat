//! Freeze verification prevents links, replacement, or cleanup mistakes from corrupting user delivery.

use super::model::DeliveryFile;
use sha2::{Digest, Sha256};
use std::{
    fs,
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
};

/// artifact_id binds one public receipt to its Work, relative name, and immutable digest.
pub(super) fn artifact_id(receipt: &DeliveryFile) -> String {
    let mut identity = Sha256::new();
    identity.update(receipt.work_id.as_bytes());
    identity.update([0]);
    identity.update(receipt.name.as_bytes());
    identity.update([0]);
    identity.update(receipt.sha256.as_bytes());
    format!("{:x}", identity.finalize())
}

/// safe_name accepts only portable relative paths contained by the private freeze root.
pub(super) fn safe_name(name: &str) -> Result<PathBuf, String> {
    let path = Path::new(name);
    let parts = path.components().collect::<Vec<_>>();
    if parts.is_empty()
        || parts
            .iter()
            .any(|part| !matches!(part, Component::Normal(_)))
        || name.chars().any(char::is_control)
    {
        return Err("Stored artifact name is invalid".to_owned());
    }
    Ok(path.to_path_buf())
}

/// path_metadata_without_links proves every existing component stays inside the physical freeze tree.
pub(super) fn path_metadata_without_links(
    root: &Path,
    relative: &Path,
) -> Result<fs::Metadata, String> {
    let mut path = root.to_path_buf();
    for component in relative.components() {
        path.push(component.as_os_str());
        let metadata =
            fs::symlink_metadata(&path).map_err(|_| "Stored artifact is unavailable".to_owned())?;
        if linked(&metadata) {
            return Err("Stored artifact path contains a link".to_owned());
        }
    }
    fs::symlink_metadata(path).map_err(|_| "Stored artifact is unavailable".to_owned())
}

/// digest_file reauthorizes a restarted delivery only after hashing every frozen byte.
pub(super) fn digest_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|_| "Stored artifact is unavailable".to_owned())?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| "Stored artifact is unavailable".to_owned())?;
        if read == 0 {
            return Ok(format!("{:x}", digest.finalize()));
        }
        digest.update(&buffer[..read]);
    }
}

/// remove_freeze removes only one verified delivery tree after making its immutable files deletable.
pub(super) fn remove_freeze(root: &Path) -> Result<(), String> {
    make_writable_tree(root)?;
    match fs::remove_dir_all(root) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Could not remove artifact freeze: {error}")),
    }
}

/// discard_private_tree cleans one exact Work-owned tree after its durable owner releases it.
pub(crate) fn discard_private_tree(root: &Path) -> Result<(), String> {
    remove_freeze(root)
}

/// make_writable_tree enables exact cleanup while unlinking links without following them.
fn make_writable_tree(path: &Path) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("Could not inspect artifact freeze: {error}")),
    };
    if linked(&metadata) {
        return fs::remove_file(path)
            .or_else(|error| {
                (error.kind() == std::io::ErrorKind::NotFound)
                    .then_some(())
                    .ok_or(error)
            })
            .map_err(|error| format!("Could not unlink private tree link: {error}"));
    }
    if metadata.is_dir() {
        crate::host::make_writable(path)?;
        for entry in fs::read_dir(path)
            .map_err(|error| format!("Could not inspect artifact freeze: {error}"))?
        {
            make_writable_tree(&entry.map_err(|error| error.to_string())?.path())?;
        }
    } else {
        crate::host::make_writable(path)?;
    }
    Ok(())
}

/// linked centralizes the host-specific link boundary used by read and cleanup paths.
pub(super) fn linked(metadata: &fs::Metadata) -> bool {
    crate::host::linked(metadata)
}
