//! The lifecycle log records ArchiGoat births, deaths, and keepalive failures so a lost connection stays diagnosable.

use std::{io::Write as _, path::PathBuf};

/// Line records one timestamped lifecycle event and never fails the ArchiGoat that emitted it.
pub(crate) fn line(event: &str) {
    // A log write that fails must stay invisible to the running ArchiGoat.
    let _ = append(event);
}

// Append performs the real, fallible write so the caller can honestly discard its outcome.
fn append(event: &str) -> std::io::Result<()> {
    // The lifecycle log shares the ArchiGoat's private directory.
    let directory = log_dir();
    std::fs::create_dir_all(&directory)?;
    // One fixed file name accumulates the whole lifecycle history.
    let path = directory.join("archigoat.log");
    // Milliseconds since the epoch keep events ordered across process restarts.
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    // Process identity names the exact ArchiGoat instance behind each event.
    let pid = std::process::id();
    // Build version ties each event to one released ArchiGoat identity.
    let version = crate::version();
    // One formatted record carries time, instance, build, and event together.
    let entry = format!("{millis} pid={pid} v={version} {event}\n");
    // An oversized log restarts from this single line instead of growing without bound.
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > 1_000_000) {
        return std::fs::write(&path, entry.as_bytes());
    }
    // The common path appends one line to preserve recent lifecycle history.
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    file.write_all(entry.as_bytes())
}

// LogDir resolves the ArchiGoat directory that holds the lifecycle log on every platform.
fn log_dir() -> PathBuf {
    #[cfg(windows)]
    {
        // Windows mirrors the daemon state root under the account's application data.
        if let Some(root) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(root).join("ArchiGoat");
        }
    }
    // Other platforms keep a compiling fallback beneath the system temporary directory.
    std::env::temp_dir().join("ArchiGoat")
}
