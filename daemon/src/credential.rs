//! Shell sessions live in one private app file; the shell writes it, uninstall clears it.

use std::path::PathBuf;

fn session_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    return std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support/ArchiGoat/session"));
    #[cfg(target_os = "windows")]
    return std::env::var_os("LOCALAPPDATA")
        .map(|root| PathBuf::from(root).join("ArchiGoat").join("session"));
}

pub(crate) fn clear_session() -> Result<(), String> {
    let Some(path) = session_path() else {
        return Ok(());
    };
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("ArchiGoat could not clear the session".to_owned()),
    }
}
