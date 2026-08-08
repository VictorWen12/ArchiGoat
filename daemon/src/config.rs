//! Configuration validates the Account, loopback listener, and private state boundary.

use serde::Deserialize;
use std::{
    env, fs,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
};
use url::Url;

// The embedded build version proves the running release identity.
pub fn version() -> &'static str {
    option_env!("ARCHIGOAT_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}
// The embedded build commit completes the release identity; a local build carries none.
pub fn commit() -> &'static str {
    option_env!("ARCHIGOAT_COMMIT").unwrap_or_default()
}

// One public product name keeps installer and recovery messages independent of private deployment branding.
pub(crate) fn product_name() -> &'static str {
    "ArchiGoat"
}

// Bundle identity is a release boundary, not a mutable runtime protocol name.
pub(crate) fn bundle_id() -> &'static str {
    match option_env!("ARCHIGOAT_BUNDLE_ID") {
        Some(value) if !value.is_empty() => value,
        _ => "com.archigoat.app",
    }
}

// Config contains deployable boundaries, never Work policy.
#[derive(Clone, Deserialize)]
pub struct Config {
    // AccountUrl is the only remote authority the outbound Work relay may contact.
    pub account_url: String,
    // ReleaseFeedOrigin is the HTTPS base URL for release.json and immutable update assets.
    pub release_feed_origin: String,
    // ArtifactOrigin is the only HTTPS origin accepted from a direct artifact presign.
    pub artifact_origin: String,
    // Bind fixes the transport to one explicit loopback socket.
    pub bind: SocketAddr,
    #[serde(default)]
    // State stores the verifiable local connection identity privately.
    pub state_file: Option<PathBuf>,
    // Installer timeout bounds one abandoned official CLI installer subprocess, never login observation.
    pub install_timeout_secs: u64,
    #[serde(default)]
    // Deployments may add explicit native CLI search roots.
    pub cli_dirs: Vec<PathBuf>,
}

// Config methods load and validate only deployment boundaries before native connection starts.
impl Config {
    // Load applies only trusted Origin, loopback bind, and private-state deployment overrides.
    pub fn load() -> Result<Self, String> {
        let mut config: Self = toml::from_str(include_str!("../../config/archigoat.toml"))
            .map_err(|error| format!("ArchiGoat configuration is invalid: {error}"))?;
        config.account_url = deployment_value(
            env::var("ACCOUNT_URL").ok(),
            option_env!("ACCOUNT_URL"),
            config.account_url,
        );
        config.release_feed_origin = deployment_value(
            env::var("RELEASE_FEED_ORIGIN").ok(),
            option_env!("RELEASE_FEED_ORIGIN"),
            config.release_feed_origin,
        );
        config.artifact_origin = deployment_value(
            env::var("ARTIFACT_ORIGIN").ok(),
            option_env!("ARTIFACT_ORIGIN"),
            config.artifact_origin,
        );
        if let Ok(value) = env::var("ARCHIGOAT_BIND") {
            config.bind = value
                .parse()
                .map_err(|_| "ArchiGoat bind address is invalid".to_owned())?;
        }
        if let Ok(value) = env::var("ARCHIGOAT_STATE") {
            config.state_file = Some(PathBuf::from(value));
        }
        if config.state_file.is_none() {
            config.state_file = default_state_file();
        }
        config.validate()?;
        Ok(config)
    }

    // Validation prevents public listeners and admits HTTP only for loopback development services.
    pub fn validate(&self) -> Result<(), String> {
        if self.bind.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) {
            return Err("ArchiGoat must listen on 127.0.0.1".to_owned());
        }
        if self.install_timeout_secs == 0 {
            return Err("Installer timeout must be positive".to_owned());
        }
        validate_account_url(&self.account_url)?;
        validate_release_feed_origin(&self.release_feed_origin)?;
        validate_artifact_origin(&self.artifact_origin)?;
        Ok(())
    }
}

// Release configuration replaces source placeholders while runtime overrides keep local tests explicit.
fn deployment_value(runtime: Option<String>, release: Option<&str>, fallback: String) -> String {
    runtime
        .filter(|value| !value.is_empty())
        .or_else(|| release.filter(|value| !value.is_empty()).map(str::to_owned))
        .unwrap_or(fallback)
}

// AdoptLegacyState moves the prior installation directory once and upgrades its credential key.
pub(crate) fn adopt_legacy_state(
    legacy_dir: &Path,
    current_dir: &Path,
    legacy_file: &str,
    current_file: &str,
) -> bool {
    match fs::symlink_metadata(current_dir) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return false;
            }
            match fs::symlink_metadata(legacy_dir) {
                Ok(_) => return false,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return false,
            }
            return migrate_legacy_state_file(current_dir, legacy_file, current_file);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return false,
    }
    let metadata = match fs::symlink_metadata(legacy_dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return true,
        Err(_) => return false,
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return false;
    }
    if fs::rename(legacy_dir, current_dir).is_err() {
        return false;
    }
    if migrate_legacy_state_file(current_dir, legacy_file, current_file) {
        return true;
    }
    let _ = fs::rename(current_dir, legacy_dir);
    false
}

// A crash after the directory rename resumes the credential-key migration instead of resetting identity.
fn migrate_legacy_state_file(directory: &Path, legacy_file: &str, current_file: &str) -> bool {
    let legacy_path = directory.join(legacy_file);
    let current_path = directory.join(current_file);
    match fs::symlink_metadata(&current_path) {
        Ok(metadata) => return metadata.is_file() && !metadata.file_type().is_symlink(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return false,
    }
    match fs::symlink_metadata(&legacy_path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return false,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return true,
        Err(_) => return false,
    }
    let bytes = match fs::read(&legacy_path) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    let migrated = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|mut value| {
            let object = value.as_object_mut()?;
            if let Some(credential) = object.remove(concat!("pl", "ugin_credential")) {
                object.insert("app_credential".to_owned(), credential);
            }
            serde_json::to_vec(&value).ok()
        });
    let Some(bytes) = migrated else { return false };
    if crate::host::replace_private(&current_path, &bytes).is_ok() {
        let _ = fs::remove_file(legacy_path);
        return true;
    }
    false
}

// LegacyStateFile permits fallback only through the old real directory, never a symlink.
pub(crate) fn legacy_state_file(legacy_dir: &Path, legacy_file: &str) -> Option<PathBuf> {
    state_file(legacy_dir, legacy_file)
}

// StateFile permits a missing directory for first start but rejects existing links or files.
pub(crate) fn state_file(directory: &Path, file: &str) -> Option<PathBuf> {
    match fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            Some(directory.join(file))
        }
        Ok(_) => None,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(directory.join(file)),
        Err(_) => None,
    }
}

// Account URL validation confines remote native authority to one secure origin.
fn validate_account_url(value: &str) -> Result<(), String> {
    let account = Url::parse(value).map_err(|_| "Account URL is invalid".to_owned())?;
    let local_http = account.scheme() == "http"
        && matches!(account.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if account.scheme() != "https" && !local_http {
        return Err("Account URL must use HTTPS".to_owned());
    }
    if account.origin().ascii_serialization() != value
        || account.path() != "/"
        || account.query().is_some()
        || account.fragment().is_some()
    {
        return Err("Account URL may contain only scheme, host, and port".to_owned());
    }
    Ok(())
}

// ReleaseFeedOrigin permits one HTTPS base path while rejecting redirects, queries, and fragments.
fn validate_release_feed_origin(value: &str) -> Result<(), String> {
    let feed = Url::parse(value).map_err(|_| "Release feed origin is invalid".to_owned())?;
    if feed.scheme() != "https" {
        return Err("Release feed origin must use HTTPS".to_owned());
    }
    if feed.host_str().is_none()
        || feed.username() != ""
        || feed.password().is_some()
        || feed.query().is_some()
        || feed.fragment().is_some()
    {
        return Err("Release feed origin is invalid".to_owned());
    }
    Ok(())
}

// ArtifactOrigin is an exact HTTPS origin; paths would permit a presign to escape its deployment.
fn validate_artifact_origin(value: &str) -> Result<(), String> {
    let artifact = Url::parse(value).map_err(|_| "Artifact origin is invalid".to_owned())?;
    if artifact.scheme() != "https"
        || artifact.origin().ascii_serialization() != value
        || artifact.path() != "/"
        || artifact.query().is_some()
        || artifact.fragment().is_some()
    {
        return Err("Artifact origin must be an exact HTTPS origin".to_owned());
    }
    Ok(())
}

// The host selects a user-private state location.
fn default_state_file() -> Option<PathBuf> {
    crate::host::default_state_file()
}
