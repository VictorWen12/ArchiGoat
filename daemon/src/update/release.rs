//! Release fetches, pins, verifies, and admits one silent signed macOS update.

// Streaming, hashing, and serialization bind downloaded bytes to the release feed.
use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    cmp::Ordering,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};
use tokio::{fs::File, io::AsyncWriteExt};
use tokio_stream::StreamExt;
use url::Url;

// ArchiGoat state supplies the zero-active-Work admission lock for the final exchange.
use crate::DaemonState;

// Connect timeout abandons a network path that cannot reach the release host.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
// Request timeout bounds each manifest or archive transfer without rejecting ordinary slow links.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);
// Manifest limit admits ample release metadata without unbounded memory use.
const MANIFEST_MAX_BYTES: u64 = 64 * 1024;
// Archive limit admits a universal signed App without unbounded disk use.
const ARCHIVE_MAX_BYTES: u64 = 256 * 1024 * 1024;
const RELEASE_FLOOR: Version = Version(1, 0, 0);

// Release is the complete identity that must remain unchanged through installation.
#[derive(Clone, Deserialize, Eq, PartialEq)]
struct Release {
    version: String,
    commit: String,
    protocol: u16,
    // An absent floor is an older feed that predates the field, and it admits every installed daemon.
    #[serde(default, rename = "minProtocol")]
    min_protocol: Option<u16>,
    #[serde(rename = "macosApp")]
    macos_app: Asset,
    // The Windows asset is never installed here, so this build only carries it through the unchanged-feed comparison.
    #[serde(default)]
    windows: Option<Asset>,
}

// Asset pins the one signed macOS App archive; a feed that adds fields must never strand installed daemons.
#[derive(Clone, Deserialize, Eq, PartialEq)]
struct Asset {
    name: String,
    sha256: String,
    signed: bool,
}

// Version stores the stable semver core used by the release pipeline.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
struct Version(u64, u64, u64);

// UpdateFailure preserves a staged old App only when rollback could not restore it.
struct UpdateFailure {
    message: String,
    preserve_staging: bool,
}

// String failures happen before or without an incomplete atomic rollback.
impl From<String> for UpdateFailure {
    // From records the failure with nothing staged worth keeping.
    fn from(message: String) -> Self {
        Self {
            message,
            preserve_staging: false,
        }
    }
}

// Swap failures retain staging only when it is the remaining whole old bundle.
impl From<super::macos::SwapFailure> for UpdateFailure {
    // From carries the swap's own judgement on whether staging still holds the whole old bundle.
    fn from(failure: super::macos::SwapFailure) -> Self {
        Self {
            message: failure.message,
            preserve_staging: failure.preserve_candidate,
        }
    }
}

// Check independently updates a writable Developer-ID installation only between Works.
pub(super) async fn check(state: &DaemonState) -> Result<(), String> {
    let Some(current) = super::macos::current()? else {
        return Ok(());
    };
    let current_version = parse_version(crate::version())?;
    let current_commit = crate::commit();
    let client = release_client(&state.config.release_feed_origin)?;
    let release = fetch(&client, &state.config.release_feed_origin).await?;
    match parse_version(&release.version)?.cmp(&current_version) {
        Ordering::Less => return Ok(()),
        Ordering::Equal if release.commit == current_commit => {
            super::macos::reclaim_staging(&current, crate::version(), current_commit)?;
            return Ok(());
        }
        Ordering::Equal => {
            return Err("Release feed changed commit without increasing version".to_owned());
        }
        Ordering::Greater => {}
    }

    let root = current
        .parent
        .join(format!(".archigoat-update-{}", crate::proof::nonce()?));
    std::fs::create_dir(&root)
        .map_err(|error| format!("Update staging directory could not be created: {error}"))?;
    let result = prepare_and_swap(state, &client, &current, &release, &root).await;
    let clean_staging = matches!(&result, Ok(false))
        || result
            .as_ref()
            .is_err_and(|failure| !failure.preserve_staging);
    if clean_staging {
        if let Err(error) = crate::delivery::discard_private_tree(&root) {
            crate::trace::line(&format!("update staging cleanup failed: {error}"));
        }
    }
    if result.map_err(|failure| failure.message)? {
        crate::trace::line(&format!(
            "updated ArchiGoat to {} {}",
            release.version, release.commit
        ));
        // The detached helper closes the old owning shell, then opens the verified replacement.
        Command::new("/bin/sh")
            .args([
                "-c",
                "kill -TERM \"$1\" 2>/dev/null || true; sleep 1; exec /usr/bin/open -n \"$2\"",
                "archigoat-update",
            ])
            .arg(std::os::unix::process::parent_id().to_string())
            .arg(&current.app)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("Updated ArchiGoat could not reopen: {error}"))?;
        std::process::exit(0);
    }
    Ok(())
}

const GITHUB_HOST: &str = "github.com";
const GITHUB_ASSET_HOST: &str = "release-assets.githubusercontent.com";

// ReleaseClient follows GitHub's short HTTPS redirect chain without admitting arbitrary hosts.
fn release_client(feed_origin: &str) -> Result<Client, String> {
    let feed_origin = feed_origin.to_owned();
    Client::builder()
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            let source_host = attempt
                .previous()
                .last()
                .and_then(|url| url.host_str().map(str::to_owned))
                .or_else(|| {
                    Url::parse(&feed_origin)
                        .ok()
                        .and_then(|url| url.host_str().map(str::to_owned))
                })
                .unwrap_or_default();
            let target = attempt.url();
            let target_host = target.host_str().unwrap_or_default();
            let same_host = target_host == source_host;
            let github_asset_redirect =
                source_host == GITHUB_HOST && target_host == GITHUB_ASSET_HOST;
            if attempt.previous().len() <= 3
                && target.scheme() == "https"
                && matches!(target_host, GITHUB_HOST | GITHUB_ASSET_HOST)
                && (same_host || github_asset_redirect)
            {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|error| format!("Release client could not be created: {error}"))
}

// PrepareAndSwap completes all slow checks before holding Work admission for the exchange.
async fn prepare_and_swap(
    state: &DaemonState,
    client: &Client,
    current: &super::macos::Current,
    release: &Release,
    root: &Path,
) -> Result<bool, UpdateFailure> {
    let archive = root.join(configured_archive());
    download(
        client,
        &state.config.release_feed_origin,
        &release.macos_app,
        &archive,
    )
    .await?;
    let extracted = root.join("extracted");
    let candidate = super::macos::extract(&archive, &extracted)?;
    super::macos::verify(
        &candidate,
        &release.version,
        &release.commit,
        &current.team_id,
    )?;
    if fetch(client, &state.config.release_feed_origin).await? != *release {
        return Err("Release feed moved during update".to_owned().into());
    }

    // One panic elsewhere must not disable self-update forever, so a poisoned lock still reads its truth.
    let works = state
        .works
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !works.can_swap_release() {
        return Ok(false);
    }
    super::macos::swap(current, &candidate)?;
    Ok(true)
}

// Fetch reads and validates one exact GitHub latest-release feed identity.
async fn fetch(client: &Client, feed_origin: &str) -> Result<Release, String> {
    let response = client
        .get(format!(
            "{}/release.json",
            feed_origin.trim_end_matches('/')
        ))
        .send()
        .await
        .map_err(|error| format!("Release manifest request failed: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Release manifest request failed: {error}"))?;
    let bytes = read_bounded(response, MANIFEST_MAX_BYTES, "Release manifest").await?;
    let release = serde_json::from_slice::<Release>(&bytes)
        .map_err(|error| format!("Release manifest is invalid: {error}"))?;
    if parse_version(&release.version)? < RELEASE_FLOOR {
        return Err("Release manifest is below the supported 1.0.0 floor".to_owned());
    }
    // A newer release protocol alone must never strand an installed daemon; only the feed's own floor may refuse this build.
    if release
        .min_protocol
        .is_some_and(|floor| floor > crate::api::PROTOCOL)
    {
        return Err("Release manifest requires a newer protocol than this installation".to_owned());
    }
    if !valid_commit(&release.commit)
        || release.macos_app.name != configured_archive()
        || !release.macos_app.signed
        || !valid_digest(&release.macos_app.sha256)
    {
        return Err("Release manifest identity is invalid".to_owned());
    }
    Ok(release)
}

// ArchiveName binds the manifest to the release-configured asset instead of accepting a redirect.
fn configured_archive() -> String {
    archive_name(option_env!("ARCHIGOAT_ASSET_STEM"))
}

fn archive_name(stem: Option<&str>) -> String {
    format!(
        "{}-macos.dmg",
        stem.filter(|value| !value.is_empty())
            .unwrap_or("archigoat")
    )
}

// Download streams the archive to disk while calculating its pinned SHA-256.
async fn download(
    client: &Client,
    feed_origin: &str,
    asset: &Asset,
    path: &Path,
) -> Result<(), String> {
    // The origin serves these names uncached, so the plain URL is the one the release pipeline proves byte for byte.
    let url = format!("{}/{}", feed_origin.trim_end_matches('/'), asset.name);
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("App download failed: {error}"))?
        .error_for_status()
        .map_err(|error| format!("App download failed: {error}"))?;
    if response
        .content_length()
        .is_some_and(|bytes| bytes > ARCHIVE_MAX_BYTES)
    {
        return Err("App archive exceeds the 256 MiB limit".to_owned());
    }
    let mut stream = response.bytes_stream();
    let mut file = File::create(path)
        .await
        .map_err(|error| format!("App archive could not be created: {error}"))?;
    let mut digest = Sha256::new();
    let mut bytes = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("App download failed: {error}"))?;
        bytes = bytes
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| "App archive size overflowed".to_owned())?;
        if bytes > ARCHIVE_MAX_BYTES {
            return Err("App archive exceeds the 256 MiB limit".to_owned());
        }
        digest.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|error| format!("App archive could not be written: {error}"))?;
    }
    file.sync_all()
        .await
        .map_err(|error| format!("App archive could not be committed: {error}"))?;
    if format!("{:x}", digest.finalize()) != asset.sha256 {
        return Err("App archive SHA-256 does not match the release manifest".to_owned());
    }
    Ok(())
}

// ReadBounded retains one small HTTP body while enforcing its memory ceiling.
async fn read_bounded(
    response: reqwest::Response,
    limit: u64,
    name: &str,
) -> Result<Vec<u8>, String> {
    if response.content_length().is_some_and(|bytes| bytes > limit) {
        return Err(format!("{name} exceeds its size limit"));
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("{name} request failed: {error}"))?;
        if bytes.len() as u64 + chunk.len() as u64 > limit {
            return Err(format!("{name} exceeds its size limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

// ParseVersion accepts only canonical stable semantic versions.
fn parse_version(value: &str) -> Result<Version, String> {
    let mut parts = value.split('.');
    let major = parse_number(parts.next())?;
    let minor = parse_number(parts.next())?;
    let patch = parse_number(parts.next())?;
    if parts.next().is_some() {
        return Err("Release version is not stable semantic version".to_owned());
    }
    Ok(Version(major, minor, patch))
}

// ParseNumber rejects missing, signed, empty, overflowing, or zero-padded semver numbers.
fn parse_number(value: Option<&str>) -> Result<u64, String> {
    let value = value.ok_or_else(|| "Release version is incomplete".to_owned())?;
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("Release version is not canonical semantic version".to_owned());
    }
    value
        .parse()
        .map_err(|_| "Release version number is too large".to_owned())
}

// ValidCommit accepts the immutable lowercase Git identities emitted by release builds.
fn valid_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

// ValidDigest accepts one canonical lowercase SHA-256.
fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}
