//! This module receives an owned artifact receipt, opens its exact frozen bytes, and hands delivery facts to the root dispatcher.

use std::fs::File;

use crate::{delivery::DeliveryFile, state::DaemonState};

/// A download owns one frozen file and the exact headers proven by its receipt.
pub(crate) struct Download {
    pub(crate) file: File,
    pub(crate) content_type: String,
    pub(crate) content_length: u64,
    pub(crate) work_id: String,
    pub(crate) artifact_id: String,
    pub(crate) sha256: String,
    pub(crate) encoded_name: String,
    pub(crate) encoded_title: String,
    pub(crate) encoded_tags: String,
}

/// An unavailable result reveals no fact about another Work or run.
#[derive(Debug)]
pub(crate) struct Unavailable;

/// Opens one file only when the run, Work, name, and frozen receipt match exactly.
pub(crate) fn download(
    state: &DaemonState,
    work_id: &str,
    run: &str,
    name: &str,
) -> Result<Download, Unavailable> {
    let expected = owned_receipt(state, work_id, run, name).ok_or(Unavailable)?;
    let (file, actual) = state.open_artifact(run, name).map_err(|_| Unavailable)?;
    if actual != expected {
        return Err(Unavailable);
    }
    Ok(Download {
        file,
        content_type: actual.format,
        content_length: actual.bytes,
        work_id: actual.work_id,
        artifact_id: actual.artifact_id,
        sha256: actual.sha256,
        encoded_name: url::form_urlencoded::byte_serialize(actual.name.as_bytes()).collect(),
        encoded_title: url::form_urlencoded::byte_serialize(actual.title.as_bytes()).collect(),
        encoded_tags: url::form_urlencoded::byte_serialize(actual.tags.join(",").as_bytes())
            .collect(),
    })
}

/// Finds the one public receipt bound to the exact owned run and name.
fn owned_receipt(
    state: &DaemonState,
    work_id: &str,
    run: &str,
    name: &str,
) -> Option<DeliveryFile> {
    let snapshot = state.run_snapshot(work_id)?;
    if snapshot.run.as_deref() != Some(run) {
        return None;
    }
    let mut matches = snapshot
        .files
        .into_iter()
        .filter(|file| file.work_id == work_id && file.name == name);
    let receipt = matches.next()?;
    matches.next().is_none().then_some(receipt)
}
