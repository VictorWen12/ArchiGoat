//! Shared Apple Terminal records preserve exact Work identity across processes.

use crate::provider::Provider;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub(super) const JOB: &str = "job.json";
pub(super) const CLAIM: &str = "claimed";
pub(super) const EVENTS: &str = "events.bin";
pub(super) const STOP: &str = "stop";
/// The file naming the machine event that ended this runner, so a logout is never read as a Provider
/// that died on its own work.
/// The claim marker declaring its runner holds a kernel liveness lock for its whole execution.
pub(super) const LIVENESS: &[u8] = b"lock\n";
/// Frame kinds are the shared process-layer numbering; every platform journals the same fact as the
/// same byte.
pub(super) use crate::process::{DONE, STDERR, STDOUT, STOPPED};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
// Request preserves the exact signed Work needed by the native Provider.
pub(super) struct Request {
    pub(super) work_id: String,
    pub(super) nonce: String,
    pub(super) state: PathBuf,
    pub(super) program: PathBuf,
    pub(super) prefix: Vec<String>,
    pub(super) args: Vec<String>,
    pub(super) input: String,
    pub(super) cwd: PathBuf,
    /// Selects the native stdout grammar; absent on Work admitted before providers carried it.
    /// Absence must survive the verify round-trip, so None is never written back as null.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) provider: Option<Provider>,
    pub(super) desktop_root: PathBuf,
    pub(super) freeze_root: PathBuf,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
// Job couples the exact request to this ArchiGoat installation's proof.
pub(super) struct Job {
    pub(super) request: Request,
    pub(super) proof: String,
}

// Output keeps public and diagnostic bytes distinct until journaled.
pub(super) enum Output {
    Bytes(u8, Vec<u8>),
    Closed(Result<(), String>),
}
