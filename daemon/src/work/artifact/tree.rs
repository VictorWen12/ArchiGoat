//! Tree protection and file identity checks stop links or replacement from changing delivery truth.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// Every source component must remain a real workspace directory or file, never a followed link.
pub(super) fn verify_source_route(workspace: &Path, relative: &Path) -> Result<(), String> {
    let mut current = workspace.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        if fs::symlink_metadata(&current)
            .map_err(|_| "Work output path changed".to_owned())?
            .file_type()
            .is_symlink()
        {
            return Err("Work output path contains a link".to_owned());
        }
    }
    Ok(())
}

/// Protect complete staged contents while its root remains writable for the final Apple rename.
pub(super) fn make_contents_readonly(root: &Path) -> Result<(), String> {
    for entry in walk(root)? {
        if entry != root {
            set_readonly(&entry, true)?;
        }
    }
    Ok(())
}

/// Protect the published root after its already-protected contents become visible.
pub(super) fn make_root_readonly(root: &Path) -> Result<(), String> {
    set_readonly(root, true)
}

/// Failed construction restores write access only inside its uncommitted private tree.
pub(super) fn make_writable(root: &Path) -> Result<(), String> {
    for entry in walk(root)? {
        set_readonly(&entry, false)?;
    }
    Ok(())
}

/// Change only filesystem permissions so delivery identity and type stay unchanged.
fn set_readonly(path: &Path, readonly: bool) -> Result<(), String> {
    let mut permissions = fs::metadata(path)
        .map_err(|_| "Could not protect delivery".to_owned())?
        .permissions();
    permissions.set_readonly(readonly);
    fs::set_permissions(path, permissions).map_err(|_| "Could not protect delivery".to_owned())
}

/// Post-order traversal rejects every link before permissions establish immutable ownership.
fn walk(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found = vec![root.to_path_buf()];
    let mut index = 0;
    while index < found.len() {
        let path = found[index].clone();
        index += 1;
        if fs::symlink_metadata(&path)
            .map_err(|_| "Delivery tree changed".to_owned())?
            .is_dir()
        {
            for entry in fs::read_dir(path).map_err(|_| "Delivery tree changed".to_owned())? {
                let path = entry
                    .map_err(|_| "Delivery tree changed".to_owned())?
                    .path();
                if fs::symlink_metadata(&path)
                    .map_err(|_| "Delivery tree changed".to_owned())?
                    .file_type()
                    .is_symlink()
                {
                    return Err("Delivery tree contains a link".to_owned());
                }
                found.push(path);
            }
        }
    }
    found.reverse();
    Ok(found)
}

/// File identity proves the checked workspace path still names the already-opened source handle.
#[cfg(unix)]
pub(super) fn same_file(opened: &fs::File, path: &fs::File) -> bool {
    use std::os::unix::fs::MetadataExt;
    let (Ok(opened), Ok(path)) = (opened.metadata(), path.metadata()) else {
        return false;
    };
    opened.dev() == path.dev() && opened.ino() == path.ino()
}

/// Windows file index and volume bind the opened source to the checked workspace path.
#[cfg(windows)]
pub(super) fn same_file(opened: &fs::File, path: &fs::File) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    // Stable handle facts bind one open Windows file without unstable metadata APIs.
    fn identity(file: &fs::File) -> Option<(u32, u32, u32)> {
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        let read = unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), &mut info) };
        (read != 0).then_some((
            info.dwVolumeSerialNumber,
            info.nFileIndexHigh,
            info.nFileIndexLow,
        ))
    }

    identity(opened).is_some_and(|opened_identity| Some(opened_identity) == identity(path))
}
