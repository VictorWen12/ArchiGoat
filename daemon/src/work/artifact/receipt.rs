//! Receipts bind immutable frozen files to one Work and reject every changed delivery byte.

use super::super::ArtifactFact;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    fs::File,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

pub(super) const MANIFEST: &str = ".manifest.json";

/// Receipt contains only facts that prove and help deliver one frozen output.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Receipt {
    pub(super) work_id: String,
    pub(super) name: String,
    pub(super) sha256: String,
    pub(super) bytes: u64,
    pub(super) format: String,
    pub(super) width: Option<u32>,
    pub(super) height: Option<u32>,
}

/// Build public file facts only from the destination bytes just written by the verified handle.
pub(super) fn from_frozen(
    work_id: &str,
    name: String,
    relative: &Path,
    path: PathBuf,
    bytes: u64,
    sha256: String,
) -> Receipt {
    let (width, height) = image::image_dimensions(path)
        .map(|(w, h)| (Some(w), Some(h)))
        .unwrap_or((None, None));
    Receipt {
        work_id: work_id.to_owned(),
        name,
        sha256,
        bytes,
        format: mime(relative),
        width,
        height,
    }
}

/// Commit a complete manifest before its directory becomes immutable delivery truth.
pub(super) fn commit(root: &Path, receipts: &[Receipt]) -> Result<(), String> {
    let bytes = serde_json::to_vec(receipts)
        .map_err(|error| format!("Could not encode delivery manifest: {error}"))?;
    let next = root.join(".manifest.next");
    let mut file = File::create(&next)
        .map_err(|error| format!("Could not create delivery manifest: {error}"))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("Could not save delivery manifest: {error}"))?;
    fs::rename(next, root.join(MANIFEST))
        .map_err(|error| format!("Could not commit delivery manifest: {error}"))
}

/// Load only receipts whose immutable files still match Work ownership, size, and digest.
pub(crate) fn load_delivery_receipt(
    work_id: &str,
    freeze_root: &Path,
) -> Result<Vec<ArtifactFact>, String> {
    valid_work(work_id)?;
    verify_regular(freeze_root, true)?;
    let manifest = freeze_root.join(MANIFEST);
    verify_regular(&manifest, false)?;
    let receipts: Vec<Receipt> = serde_json::from_slice(
        &fs::read(manifest).map_err(|_| "Frozen delivery manifest is unavailable".to_owned())?,
    )
    .map_err(|_| "Frozen delivery manifest is invalid".to_owned())?;
    let mut names = HashSet::with_capacity(receipts.len());
    receipts
        .into_iter()
        .map(|receipt| {
            if receipt.work_id != work_id || !names.insert(receipt.name.clone()) {
                return Err("Frozen delivery ownership is invalid".to_owned());
            }
            let relative = safe_relative(&receipt.name)?;
            let path = freeze_root.join(&relative);
            verify_components(freeze_root, &relative)?;
            let (bytes, sha256) = digest(&path)?;
            if bytes != receipt.bytes || sha256 != receipt.sha256 {
                return Err("Frozen delivery bytes changed".to_owned());
            }
            Ok(ArtifactFact {
                work_id: receipt.work_id,
                name: receipt.name,
                title: String::new(),
                tags: Vec::new(),
                sha256: receipt.sha256,
                bytes: receipt.bytes,
                format: receipt.format,
                width: receipt.width,
                height: receipt.height,
                freeze_root: freeze_root.to_path_buf(),
                frozen_path: path,
            })
        })
        .collect()
}

/// Portable names preserve top-level files while rejecting traversal and control semantics.
pub(super) fn portable_name(path: &Path) -> Result<String, String> {
    let name = path
        .to_str()
        .ok_or_else(|| "Work output name is not UTF-8".to_owned())?
        .replace('\\', "/");
    safe_relative(&name)?;
    Ok(name)
}

/// Work identity binds every private receipt to the accepted public execution.
pub(super) fn valid_work(work_id: &str) -> Result<(), String> {
    if work_id.trim().is_empty() || work_id.chars().any(char::is_control) {
        Err("Work identity is invalid".to_owned())
    } else {
        Ok(())
    }
}

/// Safe relative paths allow one top-level file but never escape the frozen root.
fn safe_relative(name: &str) -> Result<PathBuf, String> {
    let path = Path::new(name);
    if name.is_empty()
        || name.chars().any(char::is_control)
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        Err("Work output name is unsafe".to_owned())
    } else {
        Ok(path.to_path_buf())
    }
}

/// Every path component remains a non-link and the final file remains immutable.
fn verify_components(root: &Path, relative: &Path) -> Result<(), String> {
    let mut path = root.to_path_buf();
    for part in relative.components() {
        path.push(part.as_os_str());
        if fs::symlink_metadata(&path)
            .map_err(|_| "Frozen output is unavailable".to_owned())?
            .file_type()
            .is_symlink()
        {
            return Err("Frozen output path contains a link".to_owned());
        }
    }
    verify_regular(&path, false)
}

/// Frozen roots and files must be the expected type, non-link, and read-only.
fn verify_regular(path: &Path, directory: bool) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| "Frozen delivery is unavailable".to_owned())?;
    if metadata.file_type().is_symlink()
        || directory != metadata.is_dir()
        || !metadata.permissions().readonly()
    {
        return Err("Frozen delivery path is invalid".to_owned());
    }
    Ok(())
}

/// Stream hashing re-proves large frozen bytes without a product size gate.
fn digest(path: &Path) -> Result<(u64, String), String> {
    let mut file = File::open(path).map_err(|_| "Frozen output is unavailable".to_owned())?;
    let mut hash = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| "Could not verify frozen output".to_owned())?;
        if read == 0 {
            return Ok((bytes, format!("{:x}", hash.finalize())));
        }
        bytes += read as u64;
        hash.update(&buffer[..read]);
    }
}

/// MIME improves product handling without rejecting unfamiliar Provider-native formats.
fn mime(path: &Path) -> String {
    match path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "json" => "application/json",
        "csv" => "text/csv",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "html" => "text/html",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
    .to_owned()
}
