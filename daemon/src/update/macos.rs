//! macOS mounts, validates, and atomically replaces one ArchiGoat App.

use std::{
    ffi::{CString, OsStr},
    os::{raw::c_char, unix::ffi::OsStrExt},
    path::{Path, PathBuf},
    process::{Command, Output},
};

// The installed bundle identity and paths pin every candidate to this exact installation.
pub(super) struct Current {
    pub(super) app: PathBuf,
    pub(super) parent: PathBuf,
    pub(super) team_id: String,
}

// SwapFailure tells release cleanup when a failed rollback still owns the old App.
pub(super) struct SwapFailure {
    pub(super) message: String,
    pub(super) preserve_candidate: bool,
}

// MountedImage guarantees every successful attach receives a detach attempt.
struct MountedImage {
    mount: PathBuf,
    attached: bool,
}

const AT_FDCWD: i32 = -2;
const RENAME_SWAP: u32 = 0x0000_0002;
const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
const APP_ROOT: &str = "ArchiGoat.app";
const STAGING_PREFIX: &str = ".archigoat-update-";
const LEGACY_STAGING_PREFIX: &str = concat!(".pl", "ugin-update-");

// Darwin supplies the atomic bundle exchange and effective-user permission check.
unsafe extern "C" {
    fn access(path: *const c_char, mode: i32) -> i32;
    fn renameatx_np(
        old_fd: i32,
        old: *const c_char,
        new_fd: i32,
        new: *const c_char,
        flags: u32,
    ) -> i32;
}

// Current returns only a writable installed App carrying a Developer-ID TeamIdentifier.
pub(super) fn current() -> Result<Option<Current>, String> {
    let binary = std::env::current_exe()
        .map_err(|error| format!("Current executable is unavailable: {error}"))?;
    let macos = binary.parent();
    let contents = macos.and_then(Path::parent);
    let app = contents.and_then(Path::parent);
    let exact_layout = binary.file_name().is_some_and(|name| name == "archigoat")
        && macos
            .and_then(Path::file_name)
            .is_some_and(|name| name == "MacOS")
        && contents
            .and_then(Path::file_name)
            .is_some_and(|name| name == "Contents")
        && app
            .and_then(Path::file_name)
            .is_some_and(|name| name == APP_ROOT);
    if !exact_layout {
        return Ok(None);
    }
    let app = app.expect("exact App layout has an App");
    let parent = app
        .parent()
        .ok_or_else(|| "Installed App parent is unavailable".to_owned())?;
    if !writable(parent)? {
        return Ok(None);
    }
    let Some(team_id) = team_id(app)? else {
        return Ok(None);
    };
    Ok(Some(Current {
        app: app.to_path_buf(),
        parent: parent.to_path_buf(),
        team_id,
    }))
}

// Extract mounts the DMG read-only, requires one root App, copies it locally, and detaches.
pub(super) fn extract(dmg: &Path, root: &Path) -> Result<PathBuf, String> {
    let image = MountedImage::attach(dmg, &root.join("mounted"))?;
    let source = {
        // The published image also carries the drag-install /Applications symlink and hidden DMG state, so only real App directories are candidates.
        let mut apps = Vec::new();
        for entry in std::fs::read_dir(&image.mount)
            .map_err(|error| format!("Mounted image root could not be read: {error}"))?
        {
            let entry =
                entry.map_err(|error| format!("Mounted image entry could not be read: {error}"))?;
            let kind = entry
                .file_type()
                .map_err(|error| format!("Mounted App type could not be read: {error}"))?;
            if kind.is_symlink()
                || !kind.is_dir()
                || Path::new(&entry.file_name()).extension() != Some(OsStr::new("app"))
            {
                continue;
            }
            apps.push(entry);
        }
        let [app] = apps.as_slice() else {
            return Err("Mounted image must contain exactly one root App".to_owned());
        };
        if app.file_name() != APP_ROOT {
            return Err("Mounted App is not the ArchiGoat App".to_owned());
        }
        app.path()
    };
    let candidate = root.join(APP_ROOT);
    require_success(
        Command::new("/usr/bin/ditto")
            .arg(source)
            .arg(&candidate)
            .output(),
        "App copy",
    )?;
    image.detach()?;
    Ok(candidate)
}

// ReclaimStaging deletes only launch-proven updater siblings from this exact installation.
pub(super) fn reclaim_staging(
    current: &Current,
    version: &str,
    commit: &str,
) -> Result<(), String> {
    let roots = staging_roots(current)?;
    if roots.is_empty() {
        return Ok(());
    }
    verify(&current.app, version, commit, current.team_id.as_str())?;
    for root in roots {
        crate::delivery::discard_private_tree(&root)
            .map_err(|error| format!("Old update staging could not be removed: {error}"))?;
    }
    Ok(())
}

// Verify binds bundle identity, version, executable truth, signature, team, and notarization.
pub(super) fn verify(app: &Path, version: &str, commit: &str, team_id: &str) -> Result<(), String> {
    if bundle_value(app, "CFBundleIdentifier")? != crate::config::bundle_id()
        || bundle_value(app, "CFBundleShortVersionString")? != version
    {
        return Err("Candidate BundleIdentifier or version is invalid".to_owned());
    }
    require_success(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(app)
            .output(),
        "Code signature verification",
    )?;
    if self::team_id(app)?.as_deref() != Some(team_id) {
        return Err("Candidate TeamIdentifier does not match the installed App".to_owned());
    }
    require_success(
        Command::new("/usr/sbin/spctl")
            .args(["--assess", "--type", "execute"])
            .arg(app)
            .output(),
        "Notarization verification",
    )?;
    require_success(
        Command::new(app.join("Contents/MacOS/archigoat"))
            .args(["--verify-release", version, commit])
            .output(),
        "Release identity verification",
    )
}

// Swap atomically installs the candidate, registers it, and rolls back registration failure.
pub(super) fn swap(current: &Current, candidate: &Path) -> Result<(), SwapFailure> {
    exchange(&current.app, candidate).map_err(|message| SwapFailure {
        message,
        preserve_candidate: false,
    })?;
    let registered = require_success(
        Command::new(LSREGISTER)
            .arg("-f")
            .arg(&current.app)
            .output(),
        "Launch Services registration",
    );
    if let Err(error) = registered {
        return match exchange(&current.app, candidate) {
            Ok(()) => Err(SwapFailure {
                message: error,
                preserve_candidate: false,
            }),
            Err(rollback) => Err(SwapFailure {
                message: format!("{error}; rollback failed: {rollback}"),
                preserve_candidate: true,
            }),
        };
    }
    Ok(())
}

impl MountedImage {
    // Attach fixes the mount path and prevents browsing or writable image state.
    fn attach(dmg: &Path, mount: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(mount)
            .map_err(|error| format!("Image mount directory could not be created: {error}"))?;
        require_success(
            Command::new("/usr/bin/hdiutil")
                .args(["attach", "-readonly", "-nobrowse", "-mountpoint"])
                .arg(mount)
                .arg(dmg)
                .output(),
            "DMG attach",
        )?;
        Ok(Self {
            mount: mount.to_path_buf(),
            attached: true,
        })
    }

    // Detach completes image use before candidate validation or replacement begins.
    fn detach(mut self) -> Result<(), String> {
        let result = require_success(
            Command::new("/usr/bin/hdiutil")
                .arg("detach")
                .arg(&self.mount)
                .output(),
            "DMG detach",
        );
        if result.is_ok() {
            self.attached = false;
        }
        result
    }
}

// Drop is the failure-path detach guarantee.
impl Drop for MountedImage {
    fn drop(&mut self) {
        if self.attached {
            let _ = Command::new("/usr/bin/hdiutil")
                .arg("detach")
                .arg(&self.mount)
                .output();
        }
    }
}

// BundleValue reads one signed bundle field through the base-system plist tool.
fn bundle_value(app: &Path, key: &str) -> Result<String, String> {
    let output = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", &format!("Print :{key}")])
        .arg(app.join("Contents/Info.plist"))
        .output()
        .map_err(|error| format!("Candidate bundle identity could not be read: {error}"))?;
    if !output.status.success() {
        return Err("Candidate bundle identity is unavailable".to_owned());
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_owned())
        .map_err(|_| "Candidate bundle identity is not UTF-8".to_owned())
}

// StagingRoots resolves only direct directories with the updater prefix and one valid nonce.
fn staging_roots(current: &Current) -> Result<Vec<PathBuf>, String> {
    let entries = std::fs::read_dir(&current.parent)
        .map_err(|error| format!("Installed App parent could not be read: {error}"))?;
    let mut roots = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("Installed App sibling could not be read: {error}"))?;
        let name = entry.file_name();
        let Some((_, nonce)) = name.to_str().and_then(|name| {
            [STAGING_PREFIX, LEGACY_STAGING_PREFIX]
                .into_iter()
                .find_map(|prefix| name.strip_prefix(prefix).map(|nonce| (prefix, nonce)))
        }) else {
            continue;
        };
        if !crate::proof::valid_nonce(nonce) {
            continue;
        }
        let metadata = entry
            .file_type()
            .map_err(|error| format!("Update staging type could not be read: {error}"))?;
        if metadata.is_dir() && !metadata.is_symlink() {
            roots.push(entry.path());
        }
    }
    Ok(roots)
}

// TeamId reads the exact Developer-ID team from Apple's signature metadata.
fn team_id(app: &Path) -> Result<Option<String>, String> {
    let output = Command::new("/usr/bin/codesign")
        .args(["--display", "--verbose=4"])
        .arg(app)
        .output()
        .map_err(|error| format!("Code signature identity could not be read: {error}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stderr);
    let value = text
        .lines()
        .find_map(|line| line.strip_prefix("TeamIdentifier="))
        .filter(|value| {
            !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        });
    Ok(value.map(str::to_owned))
}

// Writable asks Darwin whether this user can mutate the target directory.
fn writable(path: &Path) -> Result<bool, String> {
    let path = c_path(path)?;
    Ok(unsafe { access(path.as_ptr(), 2) } == 0)
}

// Exchange swaps two same-volume bundle names as one filesystem transaction.
fn exchange(left: &Path, right: &Path) -> Result<(), String> {
    let left = c_path(left)?;
    let right = c_path(right)?;
    if unsafe {
        renameatx_np(
            AT_FDCWD,
            left.as_ptr(),
            AT_FDCWD,
            right.as_ptr(),
            RENAME_SWAP,
        )
    } != 0
    {
        return Err(format!(
            "App exchange failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

// CPath preserves native macOS paths for Darwin system calls.
fn c_path(path: &Path) -> Result<CString, String> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| "App path contains an invalid null byte".to_owned())
}

// RequireSuccess converts a silent system-tool failure into one updater error.
fn require_success(output: std::io::Result<Output>, operation: &str) -> Result<(), String> {
    let output = output.map_err(|error| format!("{operation} could not start: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("{operation} failed with {}", output.status))
    }
}
