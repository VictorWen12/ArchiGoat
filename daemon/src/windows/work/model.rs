//! Signed Windows Work data preserves the exact Provider command across ArchiGoat lifecycles.

use crate::provider::Provider;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Carries the exact Provider command and artifact boundaries for one Work.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::windows) struct WorkRequest {
    /// Binds execution to the accepted public Work.
    pub(in crate::windows) work_id: String,
    /// Binds proofs and files to one stable runner.
    pub(in crate::windows) nonce: String,
    /// Identifies the owning ArchiGoat state.
    pub(in crate::windows) state: PathBuf,
    /// Selects the installed Provider executable.
    pub(in crate::windows) program: PathBuf,
    /// Preserves executable wrapper arguments.
    pub(in crate::windows) prefix: Vec<String>,
    /// Preserves the Provider's native Work arguments.
    pub(in crate::windows) args: Vec<String>,
    /// Preserves the frozen user request without rewriting.
    pub(in crate::windows) input: String,
    /// Gives the Provider its Work directory.
    pub(in crate::windows) cwd: PathBuf,
    /// Selects the native stdout grammar.
    pub(in crate::windows) provider: Provider,
    /// Carries the user-owned Desktop artifact boundary.
    pub(in crate::windows) desktop_root: PathBuf,
    /// Carries the private runner-owned freeze destination.
    pub(in crate::windows) freeze_root: PathBuf,
}

/// Authenticates every immutable Work field before native execution.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::windows) struct SignedWork {
    /// Carries the verified native command.
    pub(in crate::windows) request: WorkRequest,
    /// Prevents another process from replacing that command.
    pub(in crate::windows) proof: String,
}
