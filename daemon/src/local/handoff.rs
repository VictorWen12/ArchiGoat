//! Studio handoff claims one short-lived deep-link token and saves its Account session.

use axum::http::StatusCode;
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

use crate::{DaemonState, api};

const HANDOFF_PATH: &str = "/auth/app/handoff/claim";

// Cold-start clients use these Account routes when no Studio handoff exists.
#[allow(dead_code)]
pub(crate) const COLD_START_ROUTES: &[&str] = &[
    "/auth/register/start",
    "/auth/register/verify",
    "/auth/pin",
    "/auth/login",
    "/auth/me",
    "/auth/logout",
];

/// HandoffError preserves Account's one-time-token status for the launcher IPC caller.
pub(crate) struct HandoffError {
    pub(crate) status: StatusCode,
    pub(crate) reason: &'static str,
}

/// Claim validates and consumes one Studio token, returning the session for the shell to store.
pub(crate) async fn claim(state: &DaemonState, token: &str) -> Result<Session, HandoffError> {
    if !lower_hex(token) {
        return Err(HandoffError {
            status: StatusCode::BAD_REQUEST,
            reason: "Handoff token is invalid",
        });
    }
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| HandoffError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            reason: "Account handoff is unavailable",
        })?;
    let response = client
        .post(format!("{}{}", state.config.account_url, HANDOFF_PATH))
        .bearer_auth(token)
        .header("x-app-device", state.device_id())
        .header("x-app-version", crate::version())
        .header("x-app-protocol", api::PROTOCOL.to_string())
        .send()
        .await
        .map_err(|_| HandoffError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            reason: "Account handoff is unavailable",
        })?;
    let status = response.status();
    if status == reqwest::StatusCode::GONE {
        return Err(HandoffError {
            status: StatusCode::GONE,
            reason: "Handoff token expired or was already used",
        });
    }
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(HandoffError {
            status: StatusCode::UNAUTHORIZED,
            reason: "Handoff token is not authorized",
        });
    }
    if !status.is_success() {
        return Err(HandoffError {
            status: if status == reqwest::StatusCode::UPGRADE_REQUIRED {
                StatusCode::UPGRADE_REQUIRED
            } else {
                StatusCode::BAD_GATEWAY
            },
            reason: "Account handoff was rejected",
        });
    }
    let session = response.json::<Session>().await.map_err(|_| HandoffError {
        status: StatusCode::BAD_GATEWAY,
        reason: "Account handoff response is invalid",
    })?;
    if !lower_hex(&session.access_token) || session.expires_at <= unix_seconds() {
        return Err(HandoffError {
            status: StatusCode::BAD_GATEWAY,
            reason: "Account handoff response is invalid",
        });
    }
    Ok(session)
}

/// Session carries the claimed Account bearer to the shell, which stores it privately.
#[derive(Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Session {
    pub(crate) access_token: String,
    pub(crate) expires_at: u64,
}

fn lower_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |value| value.as_secs())
}
