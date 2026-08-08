//! The preset map is the one authority for quality tiers; code names no model.

use std::time::Duration;

use crate::{
    provider::{PresetChoice, PresetFile, PresetPair, Provider},
    state::DaemonState,
};

/// The map that ships with this build answers before any request is made.
const SHIPPED: &str = include_str!("../../../presets.json");

/// MAX_BYTES bounds the published document.
const MAX_BYTES: usize = 64 * 1024;

/// The map is one small document, so its whole exchange finishes inside these bounds.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

// The map is published beside the release feed, which redirects once to GitHub's object host.
const GITHUB_HOST: &str = "github.com";
const GITHUB_ASSET_HOST: &str = "objects.githubusercontent.com";

/// Fetch returns the Provider's tiers, preferring the published map over the one this build shipped.
pub(crate) async fn fetch(state: &DaemonState, provider: Provider) -> PresetPair {
    match published(&state.config.release_feed_origin).await {
        Ok(file) => pair(&file, provider),
        Err(_) => shipped(provider),
    }
}

/// Shipped reads the map compiled into this build.
pub(crate) fn shipped(provider: Provider) -> PresetPair {
    serde_json::from_str::<PresetFile>(SHIPPED)
        .map(|file| pair(&file, provider))
        .unwrap_or_default()
}

/// Pair selects and sanitizes one Agent's tiers.
fn pair(file: &PresetFile, provider: Provider) -> PresetPair {
    file.agents
        .get(&provider.to_string())
        .map(|pair| PresetPair {
            best: sanitized(&pair.best),
            fast: sanitized(&pair.fast),
        })
        .unwrap_or_default()
}

/// Sanitized admits only plain short tier names.
fn sanitized(choice: &PresetChoice) -> PresetChoice {
    let admit = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| {
                !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
            })
            .map(str::to_owned)
    };
    PresetChoice {
        model: admit(&choice.model),
        effort: admit(&choice.effort),
    }
}

/// Published reads the current map from the release feed without admitting arbitrary hosts.
async fn published(feed_origin: &str) -> Result<PresetFile, String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let target = attempt.url();
            if attempt.previous().len() <= 3
                && target.scheme() == "https"
                && matches!(
                    target.host_str().unwrap_or_default(),
                    GITHUB_HOST | GITHUB_ASSET_HOST
                )
            {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|error| format!("Preset client could not be created: {error}"))?;
    let response = client
        .get(format!(
            "{}/presets.json",
            feed_origin.trim_end_matches('/')
        ))
        .send()
        .await
        .map_err(|error| format!("Preset map request failed: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Preset map request failed: {error}"))?;
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("Preset map could not be read: {error}"))?;
    if bytes.len() > MAX_BYTES {
        return Err("Preset map is oversized".to_owned());
    }
    serde_json::from_slice(&bytes).map_err(|error| format!("Preset map is invalid: {error}"))
}
