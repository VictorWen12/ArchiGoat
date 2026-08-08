//! Windows private persistence receives completed bytes and atomically hands them to the live ArchiGoat.

use std::{io::Write, os::windows::ffi::OsStrExt, path::Path};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::{
        Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1},
        DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        SetFileSecurityW,
    },
    Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW},
};

/// Creates a directory readable only by the owner and Windows system.
pub(crate) fn create_private_dir(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path)
        .map_err(|error| format!("Could not create private directory: {error}"))?;
    protect(path)
}

// A protected DACL grants full access only to SYSTEM and the directory owner, independent of parent inheritance.
fn protect(path: &Path) -> Result<(), String> {
    let sddl = "D:P(A;;FA;;;SY)(A;;FA;;;OW)\0"
        .encode_utf16()
        .collect::<Vec<_>>();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    };
    if converted == 0 {
        return Err(format!(
            "Could not create private directory security: {}",
            std::io::Error::last_os_error()
        ));
    }
    let secured = unsafe {
        SetFileSecurityW(
            wide(path).as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        )
    };
    unsafe {
        LocalFree(descriptor.cast());
    }
    (secured != 0).then_some(()).ok_or_else(|| {
        format!(
            "Could not protect private directory: {}",
            std::io::Error::last_os_error()
        )
    })
}

/// Restores owner write access before replacing private state.
pub(crate) fn make_writable(path: &Path) -> Result<(), String> {
    let mut permissions = match std::fs::metadata(path) {
        Ok(metadata) => metadata.permissions(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("Could not inspect Work file permissions: {error}")),
    };
    if permissions.readonly() {
        permissions.set_readonly(false);
        std::fs::set_permissions(path, permissions)
            .map_err(|error| format!("Could not restore Work file permissions: {error}"))?;
    }
    Ok(())
}

/// Rejects files replaced through another filesystem link.
pub(crate) fn linked(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    metadata.file_type().is_symlink() || metadata.file_attributes() & 0x0400 != 0
}

/// Creates one private file without overwriting existing state.
pub(super) fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("Could not create private state: {error}"))?;
    file.write_all(bytes)
        .map_err(|error| format!("Could not write private state: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("Could not persist private state: {error}"))
}

/// Replaces private state without exposing partial bytes.
fn atomic_replace(next: &Path, path: &Path) -> Result<(), String> {
    let next = extended_wide(next);
    let path = extended_wide(path);
    let moved = unsafe {
        MoveFileExW(
            next.as_ptr(),
            path.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    (moved != 0).then_some(()).ok_or_else(|| {
        format!(
            "Could not commit private state: {}",
            std::io::Error::last_os_error()
        )
    })
}

// MoveFileEx needs the extended prefix once a private staging path exceeds MAX_PATH.
// The prefix also suppresses Win32 path normalization, so the separators Rust accepts
// interchangeably must become backslashes here or a `/` reads as a filename character.
fn extended_wide(path: &Path) -> Vec<u16> {
    let raw = path.as_os_str().encode_wide().collect::<Vec<_>>();
    let mut value = if !path.is_absolute()
        || raw.starts_with(&[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16])
    {
        raw
    } else {
        let raw = raw
            .into_iter()
            .map(|unit| {
                if unit == b'/' as u16 {
                    b'\\' as u16
                } else {
                    unit
                }
            })
            .collect::<Vec<_>>();
        if raw.starts_with(&[b'\\' as u16, b'\\' as u16]) {
            "\\\\?\\UNC\\"
                .encode_utf16()
                .chain(raw[2..].iter().copied())
                .collect()
        } else {
            "\\\\?\\".encode_utf16().chain(raw).collect()
        }
    };
    value.push(0);
    value
}

/// Writes complete bytes through the shared atomic state path.
pub(crate) fn replace_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Private state path is invalid".to_owned())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create private state directory: {error}"))?;
    let next = path.with_extension(format!("next-{}", crate::proof::nonce()?));
    write_private(&next, bytes)?;
    if let Err(primary) = atomic_replace(&next, path) {
        return match std::fs::remove_file(&next) {
            Ok(()) => Err(primary),
            Err(cleanup) if cleanup.kind() == std::io::ErrorKind::NotFound => Err(primary),
            Err(cleanup) => Err(format!(
                "{primary}; could not remove staged state: {cleanup}"
            )),
        };
    }
    Ok(())
}

/// Converts an operating-system string for Windows security calls.
fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
