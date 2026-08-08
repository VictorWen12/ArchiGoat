//! Verified native answers and artifact facts are the only source of Done.

use std::{collections::HashSet, path::PathBuf};

pub(crate) const MAX_PROTECTED_ITEMS: usize = 64;
pub(crate) const MAX_PROTECTED_BYTES: usize = 256 * 1024;
pub(crate) const MAX_PROTECTED_ITEM_BYTES: usize = 64 * 1024;

/// ResultKind reflects only verified delivery facts.
#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ResultKind {
    Answer,
    Artifact,
}

/// ArtifactFact binds Provider output to one Work and immutable verified bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ArtifactFact {
    /// Stores work id.
    pub(crate) work_id: String,
    /// Stores name.
    pub(crate) name: String,
    /// Stores the optional human title carried with this artifact.
    pub(crate) title: String,
    /// Stores optional human labels carried with this artifact.
    pub(crate) tags: Vec<String>,
    /// Stores sha256.
    pub(crate) sha256: String,
    /// Stores bytes.
    pub(crate) bytes: u64,
    /// Stores format.
    pub(crate) format: String,
    /// Stores width.
    pub(crate) width: Option<u32>,
    /// Stores height.
    pub(crate) height: Option<u32>,
    /// Stores freeze root.
    pub(crate) freeze_root: PathBuf,
    /// Stores frozen path.
    pub(crate) frozen_path: PathBuf,
}

/// DeliveredWork requires a native terminal fact plus verified user value.
#[derive(Clone, Debug)]
pub struct DeliveredWork {
    /// Stores kind.
    pub(crate) kind: ResultKind,
    /// Stores answer.
    pub(crate) answer: String,
    artifacts: Vec<ArtifactFact>,
}

// This evidence proves exactly which frozen Work received a native delivery.
impl DeliveredWork {
    pub(crate) fn verified(
        work_id: String,
        answer: Option<String>,
        artifacts: Vec<ArtifactFact>,
    ) -> Result<Self, String> {
        valid_work_id(&work_id)?;
        if artifacts.iter().any(|artifact| artifact.work_id != work_id) {
            return Err("Provider artifact ownership is invalid".to_owned());
        }
        let freeze_root = artifacts.first().map(|artifact| &artifact.freeze_root);
        let mut names = HashSet::with_capacity(artifacts.len());
        if artifacts.iter().any(|artifact| {
            freeze_root.is_none_or(|root| {
                artifact.freeze_root != *root || !artifact.frozen_path.starts_with(root)
            }) || !names.insert(artifact.name.clone())
        }) {
            return Err("Provider artifact delivery is inconsistent".to_owned());
        }
        let answer = answer
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_default();
        if answer.is_empty() && artifacts.is_empty() {
            return Err("Provider completed without a reply or artifact".to_owned());
        }
        Ok(Self {
            kind: if artifacts.is_empty() {
                ResultKind::Answer
            } else {
                ResultKind::Artifact
            },
            answer,
            artifacts,
        })
    }

    /// Delivery receives frozen facts without exposing local paths.
    pub(crate) fn into_artifacts(self) -> (String, Vec<ArtifactFact>) {
        (self.answer, self.artifacts)
    }
}

/// Work identities are bounded protocol text, never paths or control bytes.
pub(crate) fn valid_work_id(work_id: &str) -> Result<(), String> {
    if work_id.trim().is_empty() || work_id.chars().any(char::is_control) {
        Err("Work identity is invalid".to_owned())
    } else {
        Ok(())
    }
}
