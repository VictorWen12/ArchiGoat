//! Frozen summon transfer pulls Account-owned meaning and bytes directly into native Work.

use axum::body::Body;
use reqwest::{Client, RequestBuilder};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{endpoint, network};
use crate::{
    DaemonState,
    api::{self, input},
    work::WorkRequest,
};

// The manifest preserves Account-owned Work meaning and the attachments it references.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    work_id: String,
    #[serde(flatten)]
    request: WorkRequest,
    attachments: Vec<FrozenInput>,
}

// A follow-up has its own immutable turn identity and text, never a second Start envelope.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SteerManifest {
    work_id: String,
    turn_id: i64,
    text: String,
    attachments: Vec<FrozenInput>,
}

// Frozen input facts let ArchiGoat verify every downloaded attachment before native use.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FrozenInput {
    position: i64,
    id: String,
    name: String,
    media: String,
    bytes: u64,
    sha256: String,
}

// Starting frozen Work verifies its identity and bytes before native execution begins.
pub(super) async fn start(
    client: &Client,
    state: &DaemonState,
    credential: &str,
    command: &str,
    work: &str,
) -> Result<(), String> {
    let (request, inputs, text) = load(client, state, credential, command, work, None).await?;
    let attachments = inputs
        .iter()
        .map(|input| input.name.clone())
        .collect::<Vec<_>>();
    state
        .start_remote_work(work.to_owned(), request, inputs)
        .await?;
    // The conversation opens with the brief the creator wrote, before the Agent answers it.
    state.record_user_message(work, command, &text, attachments);
    Ok(())
}

/// Steering frozen Work verifies one Account-owned follow-up before queueing it on the same Work.
pub(super) async fn steer(
    client: &Client,
    state: &DaemonState,
    credential: &str,
    command: &str,
    work: &str,
    turn: i64,
) -> Result<bool, String> {
    let (request, inputs, text) =
        load(client, state, credential, command, work, Some(turn)).await?;
    let attachments = inputs
        .iter()
        .map(|input| input.name.clone())
        .collect::<Vec<_>>();
    let queued = api::work::steer(state, work, command.to_owned(), request, inputs)?;
    if queued {
        // The turn identity is the creator's own, so the phone and this desktop render one bubble.
        state.record_user_message(work, &turn.to_string(), &text, attachments);
    }
    Ok(queued)
}

/// Load downloads one immutable initial or follow-up manifest, its exact text, and every attachment.
async fn load(
    client: &Client,
    state: &DaemonState,
    credential: &str,
    command: &str,
    work: &str,
    turn: Option<i64>,
) -> Result<(WorkRequest, Vec<api::work::StagedInput>, String), String> {
    let path = match turn {
        Some(turn) => format!(
            "/auth/app/work/steer?work={}&turn={turn}",
            encode(work.as_bytes())
        ),
        None => format!("/auth/app/work/manifest?work={}", encode(work.as_bytes())),
    };
    let response = authenticated(client.get(endpoint(state, &path)), state, credential)
        .timeout(super::METADATA_TIMEOUT)
        .send()
        .await
        .map_err(network)?;
    if !response.status().is_success() {
        return Err(super::rejected(state, credential, response.status(), "Work manifest").await);
    }
    let (request, attachments, text) = match turn {
        Some(turn) => {
            let manifest = response.json::<SteerManifest>().await.map_err(|error| {
                eprintln!("Product could not read the follow-up manifest: {error}");
                "TrianGoat sent this Work a follow-up this computer could not read. Send it again."
                    .to_owned()
            })?;
            if manifest.work_id != work || manifest.turn_id != turn {
                return Err("Account steer manifest changed identity".to_owned());
            }
            (
                WorkRequest::follow_up(manifest.text.clone()),
                manifest.attachments,
                manifest.text,
            )
        }
        None => {
            let value = response
                .json::<serde_json::Value>()
                .await
                .map_err(|error| {
                    eprintln!("Product could not read the Work manifest: {error}");
                    "TrianGoat sent a Work this computer could not read. Send it again.".to_owned()
                })?;
            // The brief's own words travel to the conversation exactly as the creator wrote them.
            let text = value
                .get("goal")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let manifest = serde_json::from_value::<Manifest>(value).map_err(|error| {
                eprintln!("Product could not read the Work manifest: {error}");
                "TrianGoat sent a Work this computer could not read. Send it again.".to_owned()
            })?;
            if manifest.work_id != work {
                return Err("Account Work manifest changed identity".to_owned());
            }
            (manifest.request, manifest.attachments, text)
        }
    };
    let mut receipts = Vec::with_capacity(attachments.len());
    for attachment in attachments {
        if attachment.position < 0 || !crate::proof::valid_nonce(&attachment.sha256) {
            return Err("Account Work input facts are invalid".to_owned());
        }
        let path = match turn {
            Some(turn) => format!(
                "/auth/app/work/steer/input?work={}&turn={turn}&position={}",
                encode(work.as_bytes()),
                attachment.position
            ),
            None => format!(
                "/auth/app/work/input?work={}&position={}",
                encode(work.as_bytes()),
                attachment.position
            ),
        };
        let response = authenticated(client.get(endpoint(state, &path)), state, credential)
            .timeout(super::STREAM_TIMEOUT)
            .send()
            .await
            .map_err(network)?;
        if !response.status().is_success() {
            return Err(super::rejected(state, credential, response.status(), "Work input").await);
        }
        let nonce = input_nonce(command, attachment.position);
        let expected = attachment.sha256.clone();
        let receipt = input::stage(
            state,
            work.to_owned(),
            nonce,
            input::Upload {
                id: attachment.id,
                name: attachment.name,
                media: attachment.media,
                bytes: attachment.bytes,
            },
            Body::from_stream(response.bytes_stream()),
        )
        .await?;
        if receipt.digest() != expected {
            return Err("Account Work input digest changed".to_owned());
        }
        receipts.push(receipt);
    }
    let inputs = input::bind(state, work, receipts).await?;
    Ok((request, inputs, text))
}

// Authenticated downloads bind frozen Work to this exact ArchiGoat installation.
fn authenticated(request: RequestBuilder, state: &DaemonState, credential: &str) -> RequestBuilder {
    super::authorized_request(request, state, credential)
}

// A deterministic input identity makes Account attachment retries replay-safe.
fn input_nonce(command: &str, position: i64) -> String {
    let mut digest = Sha256::new();
    digest.update(command.as_bytes());
    digest.update(position.to_be_bytes());
    format!("{:x}", digest.finalize())
}

// Query encoding preserves external Work identity without changing its meaning.
fn encode(value: &[u8]) -> String {
    url::form_urlencoded::byte_serialize(value).collect()
}
