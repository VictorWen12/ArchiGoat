//! Windows Work records give each accepted Work a private durable runner identity.

use crate::proof;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone)]
/// Locates one Work's signed runner files.
pub(super) struct Record {
    /// Binds the record to the public Work.
    pub(super) work_id: String,
    /// Names the private runner instance.
    pub(super) identity: String,
    /// Contains this runner's signed files and journal.
    pub(super) root: PathBuf,
}

#[derive(Clone)]
/// Owns the private directory containing Windows Work records.
pub(crate) struct State {
    root: Arc<PathBuf>,
}

// This state tracks each Windows process so only its own Work can clean it up.
impl State {
    /// Opens the account-local root used by durable Windows Works.
    pub(crate) fn new(state_file: Option<&Path>) -> Result<Self, String> {
        let parent = state_file
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| std::env::temp_dir().join("ArchiGoat"));
        let root = parent.join("WindowsWork");
        std::fs::create_dir_all(&root)
            .map_err(|error| format!("Could not create Windows Work state: {error}"))?;
        Ok(Self {
            root: Arc::new(root),
        })
    }

    /// Opens the stable private record that makes launch retries address one runner.
    pub(super) fn begin(&self, work_id: &str, identity: &str) -> Result<Record, String> {
        if !proof::valid_nonce(identity) {
            return Err("Windows Work record identity is invalid".to_owned());
        }
        let root = self.root.join(&identity);
        std::fs::create_dir_all(&root)
            .map_err(|error| format!("Could not create Windows Work state: {error}"))?;
        Ok(Record {
            work_id: work_id.to_owned(),
            identity: identity.to_owned(),
            root,
        })
    }

    /// Resolves only the exact durable record selected by shared Work state.
    pub(super) fn record(&self, work_id: &str, identity: &str) -> Result<Record, String> {
        if !crate::proof::valid_nonce(identity) {
            return Err("Windows Work record identity is invalid".to_owned());
        }
        let root = self.root.join(identity);
        if !root.is_dir() {
            return Err("Windows Work record is unavailable".to_owned());
        }
        Ok(Record {
            work_id: work_id.to_owned(),
            identity: identity.to_owned(),
            root,
        })
    }

    /// Lists private runner identities so shared Work state can find its signed record.
    pub(super) fn record_identities(&self) -> Result<Vec<String>, String> {
        std::fs::read_dir(self.root.as_ref())
            .map_err(|error| format!("Could not inspect Windows Work records: {error}"))?
            .map(|entry| {
                entry
                    .map_err(|error| format!("Could not inspect Windows Work record: {error}"))?
                    .file_name()
                    .into_string()
                    .map_err(|_| "Windows Work record identity is invalid".to_owned())
            })
            .collect()
    }
}
