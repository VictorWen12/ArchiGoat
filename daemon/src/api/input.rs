//! This module preserves exact attachment bytes and binds their receipt to one Work.

use axum::body::Body;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_stream::StreamExt as _;

use super::work::StagedInput;
use crate::{proof, state::DaemonState};

/// Upload describes one browser attachment without interpreting its content.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Upload {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) media: String,
    pub(crate) bytes: u64,
}

/// Receipt proves that ArchiGoat preserved one exact attachment for one Work.
#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Receipt {
    session: String,
    nonce: String,
    id: String,
    name: String,
    media: String,
    bytes: u64,
    sha256: String,
    proof: String,
}

// Receipt access exposes the verified digest needed to confirm frozen Account input.
impl Receipt {
    // The digest lets callers confirm preserved bytes without exposing receipt internals.
    pub(crate) fn digest(&self) -> &str {
        &self.sha256
    }
}

/// stage streams one attachment into ArchiGoat-private storage and signs its exact facts.
pub(crate) async fn stage(
    state: &DaemonState,
    work_id: String,
    nonce: String,
    upload: Upload,
    body: Body,
) -> Result<Receipt, String> {
    if !proof::valid_nonce(&nonce) {
        return Err("Attachment command identity is invalid".to_owned());
    }
    let path = state.staged_input_path(&work_id, &nonce)?;
    let receipt_path = state.staged_receipt_path(&work_id, &nonce)?;
    if tokio::fs::symlink_metadata(&path).await.is_ok() {
        let sha256 = verify_exact(&path, upload.bytes).await?;
        if let Some(saved) = read_receipt(&receipt_path)? {
            let mut expected = unsigned_receipt(work_id, nonce, upload, sha256);
            expected.proof = saved.proof.clone();
            if saved != expected || !state.verify_host_work(&payload(&saved)?, &saved.proof) {
                return Err("Attachment receipt changed after staging".to_owned());
            }
            return Ok(saved);
        }
        // A file-only crash recovers only when Account resends the exact original body.
        let recovery = path.with_extension("recovery");
        let recovered = write_exact(body, &recovery, upload.bytes).await;
        let cleanup = discard_temporary(&recovery).await;
        let recovered = match (recovered, cleanup) {
            (Ok(value), Ok(())) => value,
            (Err(primary), Ok(())) => return Err(primary),
            (Ok(_), Err(cleanup)) => return Err(cleanup),
            (Err(primary), Err(cleanup)) => return Err(format!("{primary}; {cleanup}")),
        };
        if recovered != sha256 {
            return Err("Attachment retry bytes changed after staging".to_owned());
        }
        let receipt = signed_receipt(state, unsigned_receipt(work_id, nonce, upload, sha256))?;
        persist_receipt(&receipt_path, &receipt)?;
        return Ok(receipt);
    }
    let temporary = path.with_extension("part");
    match tokio::fs::remove_file(&temporary).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Could not clear incomplete attachment: {error}")),
    }
    let staged = async {
        let sha256 = write_exact(body, &temporary, upload.bytes).await?;
        let mut permissions = tokio::fs::metadata(&temporary)
            .await
            .map_err(|error| format!("Could not inspect preserved attachment: {error}"))?
            .permissions();
        permissions.set_readonly(true);
        tokio::fs::set_permissions(&temporary, permissions)
            .await
            .map_err(|error| format!("Could not protect preserved attachment: {error}"))?;
        tokio::fs::rename(&temporary, &path)
            .await
            .map_err(|error| format!("Could not finalize preserved attachment: {error}"))?;
        Ok::<_, String>(sha256)
    }
    .await;
    let sha256 = match staged {
        Ok(value) => value,
        Err(primary) => {
            return match discard_temporary(&temporary).await {
                Ok(()) => Err(primary),
                Err(cleanup) => Err(format!("{primary}; {cleanup}")),
            };
        }
    };
    let receipt = signed_receipt(state, unsigned_receipt(work_id, nonce, upload, sha256))?;
    persist_receipt(&receipt_path, &receipt)?;
    Ok(receipt)
}

/// DiscardTemporary removes only an uncommitted attachment and makes cleanup failure observable.
async fn discard_temporary(path: &std::path::Path) -> Result<(), String> {
    crate::host::make_writable(path)?;
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Could not remove incomplete attachment: {error}")),
    }
}

/// UnsignedReceipt fixes Account metadata and preserved bytes before installation signing.
fn unsigned_receipt(work_id: String, nonce: String, upload: Upload, sha256: String) -> Receipt {
    Receipt {
        session: work_id,
        nonce,
        id: upload.id,
        name: upload.name,
        media: upload.media,
        bytes: upload.bytes,
        sha256,
        proof: String::new(),
    }
}

/// SignedReceipt creates the installation proof exactly once for durable replay.
fn signed_receipt(state: &DaemonState, mut receipt: Receipt) -> Result<Receipt, String> {
    receipt.proof = state.sign_host_work(&payload(&receipt)?)?;
    Ok(receipt)
}

/// PersistReceipt atomically saves the first signed receipt beside its immutable bytes.
fn persist_receipt(path: &std::path::Path, receipt: &Receipt) -> Result<(), String> {
    let bytes = serde_json::to_vec(receipt)
        .map_err(|error| format!("Could not encode attachment receipt: {error}"))?;
    crate::host::replace_private(path, &bytes)
}

/// ReadReceipt accepts only the durable regular sidecar or a genuinely absent crash gap.
fn read_receipt(path: &std::path::Path) -> Result<Option<Receipt>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Could not inspect attachment receipt: {error}")),
    };
    if !metadata.is_file() || crate::host::linked(&metadata) {
        return Err("Attachment receipt changed after staging".to_owned());
    }
    let bytes = std::fs::read(path)
        .map_err(|error| format!("Could not read attachment receipt: {error}"))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "Attachment receipt changed after staging".to_owned())
}

/// bind verifies each receipt and returns its exact private file once for this Work.
pub(crate) async fn bind(
    state: &DaemonState,
    work_id: &str,
    receipts: Vec<Receipt>,
) -> Result<Vec<StagedInput>, String> {
    let mut nonces = HashSet::new();
    let mut ids = HashSet::new();
    let mut inputs = Vec::with_capacity(receipts.len());
    for receipt in receipts {
        if receipt.session != work_id
            || !proof::valid_nonce(&receipt.nonce)
            || !nonces.insert(receipt.nonce.clone())
            || !ids.insert(receipt.id.clone())
            || !state.verify_host_work(&payload(&receipt)?, &receipt.proof)
        {
            return Err("Attachment receipt ownership changed".to_owned());
        }
        let path = state.staged_input_path(work_id, &receipt.nonce)?;
        let receipt_path = state.staged_receipt_path(work_id, &receipt.nonce)?;
        if read_receipt(&receipt_path)?.as_ref() != Some(&receipt) {
            return Err("Attachment receipt changed after staging".to_owned());
        }
        if verify_exact(&path, receipt.bytes).await? != receipt.sha256 {
            return Err("Attachment bytes changed after staging".to_owned());
        }
        inputs.push(StagedInput {
            id: receipt.id,
            name: receipt.name,
            media: receipt.media,
            bytes: receipt.bytes,
            sha256: receipt.sha256,
            path,
        });
    }
    Ok(inputs)
}

/// VerifyExact rejects links, non-files, truncation, and same-length substitution before Agent access.
async fn verify_exact(path: &std::path::Path, expected: u64) -> Result<String, String> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|error| format!("Could not read preserved attachment: {error}"))?;
    if !metadata.is_file() || crate::host::linked(&metadata) || metadata.len() != expected {
        return Err("Attachment bytes changed after staging".to_owned());
    }
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| format!("Could not read preserved attachment: {error}"))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| format!("Could not read preserved attachment: {error}"))?;
        if read == 0 {
            return Ok(format!("{:x}", digest.finalize()));
        }
        digest.update(&buffer[..read]);
    }
}

/// payload is the exact receipt identity signed by this ArchiGoat process.
fn payload(receipt: &Receipt) -> Result<Vec<u8>, String> {
    serde_json::to_vec(&(
        &receipt.session,
        &receipt.nonce,
        &receipt.id,
        &receipt.name,
        &receipt.media,
        receipt.bytes,
        &receipt.sha256,
    ))
    .map_err(|error| format!("Could not encode attachment receipt: {error}"))
}

/// write_exact preserves the complete stream and returns its SHA-256 digest.
async fn write_exact(body: Body, path: &std::path::Path, expected: u64) -> Result<String, String> {
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .await
        .map_err(|error| format!("Could not create preserved attachment: {error}"))?;
    let mut stream = body.into_data_stream();
    let mut hasher = Sha256::new();
    let mut received = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|error| format!("Could not receive attachment bytes: {error}"))?;
        received = received
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| "Attachment byte count overflowed".to_owned())?;
        if received > expected {
            return Err("Attachment contains more bytes than declared".to_owned());
        }
        file.write_all(&chunk)
            .await
            .map_err(|error| format!("Could not preserve attachment bytes: {error}"))?;
        hasher.update(&chunk);
    }
    file.sync_all()
        .await
        .map_err(|error| format!("Could not persist attachment bytes: {error}"))?;
    if received != expected {
        return Err("Attachment contains fewer bytes than declared".to_owned());
    }
    Ok(format!("{:x}", hasher.finalize()))
}
