//! Runner liveness binds durable identity to one process-held claim-file lock.

use std::{
    fs::{self, File, OpenOptions},
    path::Path,
};

/// Holds or observes the exact durable claim locked by one runner process.
pub(crate) struct RunnerLiveness {
    claim: File,
}

impl RunnerLiveness {
    /// Creates and locks the only claim allowed to execute this runner identity.
    pub(crate) fn claim(path: &Path) -> Result<Option<Self>, String> {
        let claim = match OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(None),
            Err(error) => return Err(format!("Could not claim Windows Work: {error}")),
        };
        claim
            .try_lock()
            .map_err(|error| format!("Could not hold Windows runner liveness: {error}"))?;
        claim
            .sync_all()
            .map_err(|error| format!("Could not commit Windows Work claim: {error}"))?;
        Ok(Some(Self { claim }))
    }

    /// Opens only a claim currently locked by the surviving runner.
    pub(crate) fn reattach(path: &Path) -> Result<Option<Self>, String> {
        let claim = match OpenOptions::new().read(true).write(true).open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("Could not inspect Windows runner claim: {error}")),
        };
        if held(&claim)? {
            Ok(Some(Self { claim }))
        } else {
            Ok(None)
        }
    }

    /// Proves another process still owns this exact claim-file lock.
    pub(crate) fn is_live(&self) -> bool {
        held(&self.claim).unwrap_or(false)
    }
}

/// Tests ownership without leaving a dead claim locked by its observer.
fn held(claim: &File) -> Result<bool, String> {
    match claim.try_lock() {
        Err(fs::TryLockError::WouldBlock) => Ok(true),
        Ok(()) => {
            claim
                .unlock()
                .map_err(|error| format!("Could not release Windows runner claim: {error}"))?;
            Ok(false)
        }
        Err(error) => Err(format!(
            "Could not inspect Windows runner liveness: {error}"
        )),
    }
}
