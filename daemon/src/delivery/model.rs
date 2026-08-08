//! Delivery models bind public receipts to exact private frozen files until Account confirms persistence.

use super::freeze::{artifact_id, digest_file, linked, path_metadata_without_links, safe_name};
use crate::work::ResultKind;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, fs::File, path::PathBuf};

/// DeliveryFile reveals only the identity and verified physical facts needed to deliver one artifact.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub(crate) struct DeliveryFile {
    /// artifact_id lets Account request one exact immutable artifact.
    #[serde(rename = "artifactId")]
    pub(crate) artifact_id: String,
    /// work_id prevents receipts from crossing Work ownership.
    #[serde(rename = "workId")]
    pub(crate) work_id: String,
    /// name preserves the user-visible relative product path.
    pub(crate) name: String,
    /// title preserves optional human identity carried by the artifact.
    #[serde(default)]
    pub(crate) title: String,
    /// tags preserves optional human labels carried by the artifact.
    #[serde(default)]
    pub(crate) tags: Vec<String>,
    /// bytes lets delivery prove completeness before publication.
    pub(crate) bytes: u64,
    /// sha256 proves Account received the frozen bytes unchanged.
    pub(crate) sha256: String,
    /// format helps Mine present the delivered product correctly.
    pub(crate) format: String,
    /// width preserves verified image presentation when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) width: Option<u32>,
    /// height preserves verified image presentation when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) height: Option<u32>,
}

/// FrozenFile keeps the private path beside its public receipt for exact streamed reads.
#[derive(Debug)]
pub(super) struct FrozenFile {
    /// path stays private and identifies the exact streamed file on this device.
    pub(super) path: PathBuf,
    /// receipt is rechecked before the file crosses the device boundary.
    pub(super) receipt: DeliveryFile,
}

/// Harvested owns one verified terminal result until durable Account acknowledgement.
#[derive(Debug)]
pub(crate) struct Harvested {
    /// answer is the Provider-native user response.
    pub(crate) answer: String,
    /// kind distinguishes an answer from a verified standalone product.
    pub(crate) kind: ResultKind,
    /// manifest is the complete public receipt set for this Work.
    pub(crate) manifest: Vec<DeliveryFile>,
    /// files indexes only receipt-owned frozen paths for bounded download lookup.
    pub(super) files: HashMap<String, FrozenFile>,
    /// freeze_root scopes verification and cleanup to this Work's private tree.
    pub(super) freeze_root: Option<PathBuf>,
}

// This record carries verified native output into durable Work delivery.
impl Harvested {
    /// rehydrate restores delivery only when every durable receipt still matches immutable bytes.
    pub(crate) fn rehydrate(
        answer: String,
        kind: ResultKind,
        manifest: Vec<DeliveryFile>,
        freeze_root: PathBuf,
    ) -> Result<Self, String> {
        if kind != ResultKind::Artifact || manifest.is_empty() {
            return Err("Stored artifact manifest is invalid".to_owned());
        }
        let root_metadata = fs::symlink_metadata(&freeze_root)
            .map_err(|_| "Stored artifact freeze is unavailable".to_owned())?;
        if !root_metadata.is_dir()
            || linked(&root_metadata)
            || !root_metadata.permissions().readonly()
        {
            return Err("Stored artifact freeze is invalid".to_owned());
        }
        let canonical_root = fs::canonicalize(&freeze_root)
            .map_err(|_| "Stored artifact freeze is unavailable".to_owned())?;
        let mut files = HashMap::with_capacity(manifest.len());

        // Every receipt must independently prove the same frozen tree before Work can remain Done.
        for receipt in &manifest {
            let relative = safe_name(&receipt.name)?;
            let path = freeze_root.join(&relative);
            let metadata = path_metadata_without_links(&freeze_root, &relative)?;
            let canonical =
                fs::canonicalize(&path).map_err(|_| "Stored artifact is unavailable".to_owned())?;
            if !metadata.is_file()
                || !metadata.permissions().readonly()
                || metadata.len() != receipt.bytes
                || !canonical.starts_with(&canonical_root)
                || digest_file(&path)? != receipt.sha256
                || artifact_id(receipt) != receipt.artifact_id
            {
                return Err("Stored artifact no longer matches its receipt".to_owned());
            }
            if files
                .insert(
                    receipt.name.clone(),
                    FrozenFile {
                        path,
                        receipt: receipt.clone(),
                    },
                )
                .is_some()
            {
                return Err("Stored artifact manifest lists a file twice".to_owned());
            }
        }
        Ok(Self {
            answer,
            kind,
            manifest,
            files,
            freeze_root: Some(freeze_root),
        })
    }

    /// open streams only an unchanged ordinary file owned by this Work receipt.
    pub(crate) fn open(&self, name: &str) -> Result<(File, DeliveryFile), String> {
        let frozen = self
            .files
            .get(name)
            .ok_or_else(|| "File is not available".to_owned())?;
        let metadata = fs::symlink_metadata(&frozen.path)
            .map_err(|_| "Frozen artifact is unavailable".to_owned())?;
        let canonical = fs::canonicalize(&frozen.path)
            .map_err(|_| "Frozen artifact is unavailable".to_owned())?;
        let canonical_root = self
            .freeze_root
            .as_ref()
            .and_then(|root| fs::canonicalize(root).ok());
        if !metadata.is_file()
            || linked(&metadata)
            || metadata.len() != frozen.receipt.bytes
            || !metadata.permissions().readonly()
            || canonical_root
                .as_ref()
                .is_none_or(|root| !canonical.starts_with(root))
        {
            return Err("Frozen artifact no longer matches its receipt".to_owned());
        }
        File::open(&frozen.path)
            .map(|file| (file, frozen.receipt.clone()))
            .map_err(|_| "Frozen artifact is unavailable".to_owned())
    }
}
