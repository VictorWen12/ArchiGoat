//! Manifest construction turns verified Work artifact facts into public delivery receipts.

use super::{
    freeze::artifact_id,
    model::{DeliveryFile, FrozenFile, Harvested},
};
use crate::work::{ArtifactFact, DeliveredWork};
use std::collections::HashMap;

/// harvest transfers already-frozen facts directly into one streamable terminal result.
pub(crate) fn harvest(delivered: DeliveredWork) -> Harvested {
    let kind = delivered.kind;
    let (answer, artifacts) = delivered.into_artifacts();
    let freeze_root = artifacts
        .first()
        .map(|artifact| artifact.freeze_root.clone());
    let mut manifest = Vec::with_capacity(artifacts.len());
    let mut files = HashMap::with_capacity(artifacts.len());

    // One pass preserves the verified artifact order while indexing exact downloads by name.
    for artifact in artifacts {
        let receipt = receipt(&artifact);
        files.insert(
            artifact.name.clone(),
            FrozenFile {
                path: artifact.frozen_path,
                receipt: receipt.clone(),
            },
        );
        manifest.push(receipt);
    }
    Harvested {
        answer,
        kind,
        manifest,
        files,
        freeze_root,
    }
}

/// receipt exposes only verified facts and a deterministic Work-scoped artifact identity.
fn receipt(artifact: &ArtifactFact) -> DeliveryFile {
    let mut receipt = DeliveryFile {
        artifact_id: String::new(),
        work_id: artifact.work_id.clone(),
        name: artifact.name.clone(),
        title: artifact.title.clone(),
        tags: artifact.tags.clone(),
        bytes: artifact.bytes,
        sha256: artifact.sha256.clone(),
        format: artifact.format.clone(),
        width: artifact.width,
        height: artifact.height,
    };
    receipt.artifact_id = artifact_id(&receipt);
    receipt
}
