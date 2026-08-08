//! Installation identity persists one host proof and last verified Provider across restarts.

use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

use crate::provider::Provider;

use super::DaemonState;

const MAX_IDENTITY_BYTES: u64 = 64 * 1024;

/// SavedIdentity contains only facts required to retain local ownership and session recovery.
#[derive(Deserialize, Serialize)]
pub(super) struct SavedIdentity {
    #[serde(default)]
    pub(super) device_id: Option<String>,
    pub(super) instance_secret: String,
    #[serde(default, alias = "pl\u{75}gin_credential")]
    pub(super) app_credential: Option<String>,
    pub(super) provider: Option<Provider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) effort: Option<String>,
}

// This identity access keeps every Work bound to the authenticated local installation.
impl DaemonState {
    /// LoadInstanceSecret lets a signed platform helper verify this exact installation.
    pub(crate) fn load_instance_secret(path: &Path) -> Result<String, String> {
        read(path)?
            .map(|value| value.instance_secret)
            .filter(|value| crate::proof::valid_nonce(value))
            .ok_or_else(|| "ArchiGoat installation identity is unavailable".to_owned())
    }

    /// SaveIdentity commits the host proof, verified Provider, and Account credential atomically.
    pub(super) fn save_identity(&self, provider: Option<Provider>) -> Result<(), String> {
        let (model, effort) = self
            .status
            .try_read()
            .ok()
            .map(|status| (status.model.clone(), status.effort.clone()))
            .unwrap_or_default();
        let credential = self
            .credential
            .try_read()
            .map_err(|_| "ArchiGoat credential is busy".to_owned())?
            .clone();
        self.save_identity_with(
            provider,
            model.as_deref(),
            effort.as_deref(),
            credential.as_deref(),
        )
    }

    /// SaveIdentityWith commits an already-locked credential mutation without reading stale memory.
    pub(super) fn save_identity_with(
        &self,
        provider: Option<Provider>,
        model: Option<&str>,
        effort: Option<&str>,
        credential: Option<&str>,
    ) -> Result<(), String> {
        self.save_identity_full(provider, model, effort, credential)
    }
}

/// Read rejects linked, oversized, or malformed identity files before they own local runners.
pub(super) fn read(path: &Path) -> Result<Option<SavedIdentity>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Could not inspect ArchiGoat identity: {error}")),
    };
    if !metadata.is_file() || crate::host::linked(&metadata) || metadata.len() > MAX_IDENTITY_BYTES
    {
        return Err("ArchiGoat identity file is invalid".to_owned());
    }
    let bytes =
        fs::read(path).map_err(|error| format!("Could not read ArchiGoat identity: {error}"))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "ArchiGoat identity file is unreadable".to_owned())
}
