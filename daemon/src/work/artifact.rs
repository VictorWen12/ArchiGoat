//! Artifact freezing turns one Work workspace's top-level deliverable files into immutable, receipt-verified delivery bytes.

mod receipt;
mod tree;

use receipt::Receipt;
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub(crate) use receipt::load_delivery_receipt;

/// Freeze every top-level Work output file without classifying format or size.
pub(crate) fn freeze_delivery_receipt(
    work_id: &str,
    workspace: &Path,
    delivery_root: &Path,
    freeze_root: &Path,
) -> Result<(), String> {
    receipt::valid_work(work_id)?;
    let workspace = fs::canonicalize(workspace)
        .map_err(|error| format!("Work workspace is unavailable: {error}"))?;
    let delivery_root = fs::canonicalize(delivery_root)
        .map_err(|error| format!("Delivery root is unavailable: {error}"))?;
    if !workspace.starts_with(&delivery_root) {
        return Err("Work delivery boundary is invalid".to_owned());
    }
    let parent = freeze_root
        .parent()
        .ok_or_else(|| "Freeze destination is invalid".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not prepare delivery storage: {error}"))?;
    let temporary = parent.join(format!(".freeze-{}", crate::proof::nonce()?));
    crate::host::create_private_dir(&temporary)?;

    // Build and protect a complete sibling so restart sees either no delivery or exact truth.
    let result = build_freeze(work_id, &workspace, &temporary)
        .and_then(|receipts| receipt::commit(&temporary, &receipts))
        .and_then(|()| commit_freeze(&temporary, freeze_root));
    match result {
        Ok(()) => Ok(()),
        Err(primary) => match cleanup_failed_freeze(&temporary) {
            Ok(()) => Err(primary),
            Err(cleanup) => Err(format!("{primary}; {cleanup}")),
        },
    }
}

/// CleanupFailedFreeze reports uncommitted byte leaks while preserving the original delivery error.
fn cleanup_failed_freeze(temporary: &Path) -> Result<(), String> {
    tree::make_writable(temporary)?;
    match fs::remove_dir_all(temporary) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Could not remove failed delivery freeze: {error}")),
    }
}

/// Publish protected contents before locking the final root; any interrupted root lock fails closed.
fn commit_freeze(temporary: &Path, freeze_root: &Path) -> Result<(), String> {
    tree::make_contents_readonly(temporary)?;
    fs::rename(temporary, freeze_root)
        .map_err(|error| format!("Could not commit frozen delivery: {error}"))?;
    if let Err(primary) = tree::make_root_readonly(freeze_root) {
        return match cleanup_failed_freeze(freeze_root) {
            Ok(()) => Err(primary),
            Err(cleanup) => Err(format!("{primary}; {cleanup}")),
        };
    }
    Ok(())
}

/// Deliverables are the workspace's top-level regular files; directories and links stay build scratch and never enter delivery.
fn build_freeze(work_id: &str, workspace: &Path, target: &Path) -> Result<Vec<Receipt>, String> {
    let mut receipts = Vec::new();
    for entry in fs::read_dir(workspace)
        .map_err(|error| format!("Could not inspect Work output: {error}"))?
    {
        let entry = entry.map_err(|error| format!("Could not inspect Work output: {error}"))?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| format!("Could not inspect Work output: {error}"))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            continue;
        }
        receipts.push(copy_open_file(
            work_id,
            workspace,
            target,
            &PathBuf::from(entry.file_name()),
        )?);
    }
    receipts.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(receipts)
}

/// Copy an already-opened, identity-checked source so path replacement cannot redirect bytes.
fn copy_open_file(
    work_id: &str,
    workspace: &Path,
    target: &Path,
    relative: &Path,
) -> Result<Receipt, String> {
    let name = receipt::portable_name(relative)?;
    let source_path = workspace.join(relative);
    let mut source =
        File::open(&source_path).map_err(|error| format!("Could not open Work output: {error}"))?;
    let opened = source
        .metadata()
        .map_err(|error| format!("Could not inspect opened Work output: {error}"))?;
    // The currently addressed route must contain no link even when its final target stays local.
    tree::verify_source_route(workspace, relative)?;
    let canonical = fs::canonicalize(&source_path)
        .map_err(|error| format!("Could not resolve Work output: {error}"))?;
    let current = File::open(&canonical)
        .map_err(|error| format!("Could not inspect Work output identity: {error}"))?;
    if !opened.is_file() || !canonical.starts_with(workspace) || !tree::same_file(&source, &current)
    {
        return Err("Work output changed outside its workspace boundary".to_owned());
    }
    let destination = target.join(relative);
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Could not prepare frozen output: {error}"))?;
    }
    let mut frozen = File::create(&destination)
        .map_err(|error| format!("Could not create frozen output: {error}"))?;
    let (bytes, sha256) = copy_and_hash(&mut source, &mut frozen)?;
    frozen
        .sync_all()
        .map_err(|error| format!("Could not commit frozen output: {error}"))?;
    // The path must still identify the opened handle after copying; replacement fails the Work closed.
    let after =
        File::open(&source_path).map_err(|_| "Work output changed while freezing".to_owned())?;
    if !tree::same_file(&source, &after) {
        return Err("Work output changed while freezing".to_owned());
    }
    Ok(receipt::from_frozen(
        work_id,
        name,
        relative,
        destination,
        bytes,
        sha256,
    ))
}

/// Stream directly between handles so large products stay bounded and hashes prove copied bytes.
fn copy_and_hash(source: &mut File, destination: &mut File) -> Result<(u64, String), String> {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = source
            .read(&mut buffer)
            .map_err(|_| "Could not read Work output".to_owned())?;
        if read == 0 {
            return Ok((bytes, format!("{:x}", hash.finalize())));
        }
        destination
            .write_all(&buffer[..read])
            .map_err(|_| "Could not write frozen output".to_owned())?;
        hash.update(&buffer[..read]);
        bytes += read as u64;
    }
}
