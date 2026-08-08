//! A Cursor input view exposes disposable verified copies while authoritative staged bytes stay private.

use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::Read as _,
    path::{Path, PathBuf},
};

/// SelectedInput binds one authoritative staged file to the facts a disposable copy must preserve.
pub(crate) struct SelectedInput<'a> {
    pub(crate) path: &'a Path,
    pub(crate) bytes: u64,
    pub(crate) sha256: &'a str,
}

/// InputView maps selected authoritative paths to one disposable Cursor attachment view.
pub(crate) struct InputView {
    root: PathBuf,
    files: Vec<ViewFile>,
}

struct ViewFile {
    original: PathBuf,
    path: PathBuf,
    bytes: u64,
    sha256: String,
}

impl InputView {
    /// Rebuild replaces one Work's prior view with exact selected copies before every Cursor turn.
    pub(crate) fn rebuild(
        session: &Path,
        runner_id: &str,
        selected: &[SelectedInput<'_>],
    ) -> Result<Self, String> {
        let root = view_root(session, runner_id)?;
        crate::delivery::discard_private_tree(&root)?;
        let files = selected
            .iter()
            .enumerate()
            .map(|(index, input)| ViewFile {
                original: input.path.to_path_buf(),
                path: root.join(format!("{index:08x}")),
                bytes: input.bytes,
                sha256: input.sha256.to_owned(),
            })
            .collect::<Vec<_>>();
        if files.is_empty() {
            return Ok(Self { root, files });
        }
        crate::host::create_private_dir(
            root.parent()
                .ok_or_else(|| "Cursor input view root is invalid".to_owned())?,
        )?;
        crate::host::create_private_dir(&root)?;
        for file in &files {
            verify_file(&file.original, file.bytes, &file.sha256, true)?;
            copy_file(&file.original, &file.path)?;
        }
        let view = Self { root, files };
        view.validate()?;
        Ok(view)
    }

    /// Open reconstructs a durable view identity so Done can revalidate both originals and copies.
    pub(crate) fn open(
        session: &Path,
        runner_id: &str,
        selected: &[SelectedInput<'_>],
    ) -> Result<Self, String> {
        let root = view_root(session, runner_id)?;
        let files = selected
            .iter()
            .enumerate()
            .map(|(index, input)| ViewFile {
                original: input.path.to_path_buf(),
                path: root.join(format!("{index:08x}")),
                bytes: input.bytes,
                sha256: input.sha256.to_owned(),
            })
            .collect();
        Ok(Self { root, files })
    }

    /// Validate rejects missing, linked, changed, or extra view bytes and re-proves every original.
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.files.is_empty() {
            return match fs::symlink_metadata(&self.root) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                _ => Err("Empty Cursor input view is invalid".to_owned()),
            };
        }
        let metadata = fs::symlink_metadata(&self.root)
            .map_err(|_| "Cursor input view is unavailable".to_owned())?;
        if !metadata.is_dir() || crate::host::linked(&metadata) {
            return Err("Cursor input view is invalid".to_owned());
        }
        let expected = self
            .files
            .iter()
            .filter_map(|file| file.path.file_name().map(|name| name.to_owned()))
            .collect::<HashSet<_>>();
        let actual = fs::read_dir(&self.root)
            .map_err(|_| "Cursor input view is unavailable".to_owned())?
            .map(|entry| {
                entry
                    .map(|entry| entry.file_name())
                    .map_err(|_| "Cursor input view is unavailable".to_owned())
            })
            .collect::<Result<HashSet<_>, _>>()?;
        if actual != expected {
            return Err("Cursor input view contains unselected bytes".to_owned());
        }
        for file in &self.files {
            verify_file(&file.original, file.bytes, &file.sha256, true)?;
            verify_file(&file.path, file.bytes, &file.sha256, false)?;
        }
        Ok(())
    }

    /// Paths supplies disposable selected copies to Cursor's official attachment-directory argument.
    pub(crate) fn paths(&self) -> Vec<PathBuf> {
        self.files.iter().map(|file| file.path.clone()).collect()
    }

    /// PathFor lets the provider envelope name its disposable copy without changing attachment facts.
    pub(crate) fn path_for(&self, original: &Path) -> Option<&Path> {
        self.files
            .iter()
            .find(|file| file.original == original)
            .map(|file| file.path.as_path())
    }

    /// DiscardSession removes the one disposable view derived from this Work container.
    pub(crate) fn discard_session(session: &Path) -> Result<(), String> {
        let runner_id = session
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "Cursor input view identity is invalid".to_owned())?;
        crate::delivery::discard_private_tree(&view_root(session, runner_id)?)
    }
}

/// ViewRoot permits only one nonce-owned sibling of authoritative ArchiGoat input storage.
fn view_root(session: &Path, runner_id: &str) -> Result<PathBuf, String> {
    if !crate::proof::valid_nonce(runner_id) {
        return Err("Cursor input view identity is invalid".to_owned());
    }
    let works = session
        .parent()
        .filter(|path| path.file_name().is_some_and(|name| name == "Works"))
        .ok_or_else(|| "Cursor Work storage is invalid".to_owned())?;
    let private = works
        .parent()
        .ok_or_else(|| "ArchiGoat private storage is unavailable".to_owned())?;
    Ok(private.join("InputViews").join(runner_id))
}

/// CopyFile creates one fresh private copy without following any destination link.
fn copy_file(source: &Path, destination: &Path) -> Result<(), String> {
    let mut source = fs::File::open(source)
        .map_err(|error| format!("Could not read staged attachment: {error}"))?;
    let mut destination_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .map_err(|error| format!("Could not create Cursor input view: {error}"))?;
    std::io::copy(&mut source, &mut destination_file)
        .and_then(|_| destination_file.sync_all())
        .map_err(|error| format!("Could not preserve Cursor input view: {error}"))?;
    let mut permissions = destination_file
        .metadata()
        .map_err(|error| format!("Could not inspect Cursor input view: {error}"))?
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(destination, permissions)
        .map_err(|error| format!("Could not protect Cursor input view: {error}"))
}

/// VerifyFile proves one regular unlinked file still matches its admitted bytes and digest.
fn verify_file(
    path: &Path,
    bytes: u64,
    sha256: &str,
    require_readonly: bool,
) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| "Selected attachment is unavailable".to_owned())?;
    if !metadata.is_file()
        || crate::host::linked(&metadata)
        || (require_readonly && !metadata.permissions().readonly())
        || metadata.len() != bytes
        || digest_file(path)? != sha256
    {
        return Err("Selected attachment bytes changed".to_owned());
    }
    Ok(())
}

/// DigestFile hashes exact bytes without interpreting untrusted attachment content.
fn digest_file(path: &Path) -> Result<String, String> {
    let mut file =
        fs::File::open(path).map_err(|_| "Selected attachment is unavailable".to_owned())?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| "Selected attachment is unavailable".to_owned())?;
        if read == 0 {
            return Ok(format!("{:x}", digest.finalize()));
        }
        digest.update(&buffer[..read]);
    }
}
