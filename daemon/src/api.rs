//! Loopback sends typed product actions here; this facade preserves durable execution semantics.

pub(crate) mod artifact;
pub(crate) mod input;
pub(crate) mod work;

use axum::{
    Json,
    body::Body,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use http_body_util::BodyExt as _;
use serde::{Deserialize, Serialize};

use crate::{
    Provider,
    delivery::DeliveryFile,
    state::{DaemonState, RunPhase, RunProgress, RunSnapshot, WorkEvent},
    work::{ResultKind, WorkRequest},
};

/// PROTOCOL identifies the one Browser-to-ArchiGoat action contract used by this build.
pub(crate) const PROTOCOL: u16 = 15;

/// MIN_PROTOCOL is the exact server floor; older clients must upgrade.
pub(crate) const MIN_PROTOCOL: u16 = 15;

/// Action is one complete product request already authenticated by the loopback boundary.
pub(crate) enum Action {
    Connect {
        provider: Provider,
        model: Option<String>,
        effort: Option<String>,
    },
    StageInput {
        work_id: String,
        nonce: String,
        input: input::Upload,
        body: Body,
    },
    StartWork {
        work_id: String,
        body: Body,
    },
    SteerWork {
        work_id: String,
        steer_id: String,
        body: Body,
    },
    ObserveWork {
        work_id: String,
    },
    StopWork {
        work_id: String,
    },
}

/// WorkEnvelope preserves the browser WorkRequest and its already-staged attachment receipts.
#[derive(Deserialize)]
struct WorkEnvelope {
    #[serde(flatten)]
    request: WorkRequest,
    #[serde(default)]
    attachments: Vec<input::Receipt>,
}

/// PublicRun is the only Work state returned to the browser.
#[derive(Serialize)]
struct PublicRun {
    phase: RunPhase,
    text: String,
    #[serde(rename = "startedAt")]
    started_at: u64,
    progress: Option<RunProgress>,
    tokens: Option<u64>,
    model: Option<String>,
    kind: Option<ResultKind>,
    run: Option<String>,
    files: Vec<DeliveryFile>,
    /// Events is the conversation in the order it happened; text stays its joined Agent prose.
    events: Vec<WorkEvent>,
    /// Awaiting is true only while the Agent itself says this turn is parked on the creator.
    awaiting: bool,
}

// This conversion exposes only truthful, public Work state to Product clients.
impl From<RunSnapshot> for PublicRun {
    /// from exposes only stable user-visible Work facts.
    fn from(value: RunSnapshot) -> Self {
        Self {
            phase: value.phase,
            text: value.text,
            started_at: value.started_at,
            progress: value.progress,
            tokens: value.tokens,
            model: value.model,
            kind: value.kind,
            run: value.run,
            files: value.files,
            events: value.events,
            awaiting: value.awaiting,
        }
    }
}

/// execute completes one typed action and returns only its product result.
pub(crate) async fn execute(state: DaemonState, action: Action) -> Response {
    match action {
        Action::Connect {
            provider,
            model,
            effort,
        } => match work::connect(state, provider, model, effort).await {
            Ok(()) => StatusCode::ACCEPTED.into_response(),
            // The connect refusal is the one reason that is already user-facing prose, so it travels whole.
            Err(reason) => {
                crate::trace::line(&format!("connect refused: {reason}"));
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({ "message": reason })),
                )
                    .into_response()
            }
        },
        Action::StageInput {
            work_id,
            nonce,
            input: upload,
            body,
        } => match input::stage(&state, work_id, nonce, upload, body).await {
            Ok(receipt) => (StatusCode::CREATED, Json(receipt)).into_response(),
            Err(reason) => unavailable("Attachment bytes could not be preserved", reason),
        },
        Action::StartWork { work_id, body } => start(state, work_id, body).await,
        Action::SteerWork {
            work_id,
            steer_id,
            body,
        } => steer(state, work_id, steer_id, body).await,
        Action::ObserveWork { work_id } => match work::observe(&state, &work_id).await {
            Some(mut snapshot) => {
                snapshot.events = state.work_conversation(&work_id);
                Json(PublicRun::from(snapshot)).into_response()
            }
            None => StatusCode::NO_CONTENT.into_response(),
        },
        Action::StopWork { work_id } => {
            work::stop(&state, &work_id).await;
            StatusCode::NO_CONTENT.into_response()
        }
    }
}

/// Composed keeps one decoded request beside the exact words and the turn identity it carries.
struct Composed {
    request: WorkRequest,
    inputs: Vec<work::StagedInput>,
    text: String,
    turn: Option<String>,
}

// Every composed turn reaches the conversation under the identity both screens already share.
impl Composed {
    /// Attachments names the staged files this turn carries, in the order the creator attached them.
    fn attachments(&self) -> Vec<String> {
        self.inputs
            .iter()
            .map(|input| input.name.clone())
            .collect::<Vec<_>>()
    }
}

/// envelope binds exact staged bytes to one decoded user request and keeps its exact words.
async fn envelope(state: &DaemonState, work_id: &str, body: Body) -> Result<Composed, Response> {
    let value = json::<serde_json::Value>(body)
        .await
        .map_err(|reason| bad_request(&reason))?;
    // The composer's own words travel to the conversation exactly as the creator typed them.
    let text = value
        .get("goal")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    // The turn identity the creator's own screen recorded keeps one message one bubble everywhere.
    let turn = value.get("steerId").and_then(|value| {
        value
            .as_str()
            .map(str::to_owned)
            .or_else(|| value.as_i64().map(|turn| turn.to_string()))
            .filter(|turn| !turn.is_empty())
    });
    let envelope = serde_json::from_value::<WorkEnvelope>(value)
        .map_err(|error| bad_request(&format!("Could not decode Work JSON: {error}")))?;
    let inputs = input::bind(state, work_id, envelope.attachments)
        .await
        .map_err(|reason| unavailable("Attachment receipt does not belong to this Work", reason))?;
    Ok(Composed {
        request: envelope.request,
        inputs,
        text,
        turn,
    })
}

/// start decodes the untouched Work and starts its durable native execution.
async fn start(state: DaemonState, work_id: String, body: Body) -> Response {
    let composed = match envelope(&state, &work_id, body).await {
        Ok(composed) => composed,
        Err(response) => return response,
    };
    let agent = state
        .connected_provider()
        .await
        .map(|(provider, _)| provider.label())
        .unwrap_or("Agent");
    let attachments = composed.attachments();
    let turn = match composed.turn {
        Some(turn) => turn,
        None => match crate::proof::nonce() {
            Ok(nonce) => nonce,
            Err(reason) => return unavailable("This Work could not be started", reason),
        },
    };
    let text = composed.text;
    match work::start(
        state.clone(),
        work_id.clone(),
        composed.request,
        composed.inputs,
    )
    .await
    {
        // The conversation opens with the brief the creator wrote, before the Agent answers it.
        Ok(()) => {
            state.record_user_message(&work_id, &turn, &text, attachments);
            StatusCode::ACCEPTED.into_response()
        }
        // These words belong to the Work already running this conversation, and its identity travels
        // with the refusal so they are sent to that Work as its own turn.
        Err(crate::work_state::StartRefusal::Busy(running)) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "message": "This conversation is already running.",
                "workId": running,
            })),
        )
            .into_response(),
        Err(crate::work_state::StartRefusal::Unavailable(reason)) => {
            unavailable(&format!("{agent} could not start this Work"), reason)
        }
    }
}

/// steer durably appends one exact user follow-up to the Running Work.
async fn steer(state: DaemonState, work_id: String, steer_id: String, body: Body) -> Response {
    let composed = match envelope(&state, &work_id, body).await {
        Ok(composed) => composed,
        Err(response) => return response,
    };
    let attachments = composed.attachments();
    let turn = composed.turn.unwrap_or(steer_id.clone());
    match work::steer(
        &state,
        &work_id,
        steer_id,
        composed.request,
        composed.inputs,
    ) {
        Ok(true) => {
            state.record_user_message(&work_id, &turn, &composed.text, attachments);
            StatusCode::ACCEPTED.into_response()
        }
        Ok(false) => (StatusCode::CONFLICT, "Work is no longer running").into_response(),
        Err(reason) => unavailable("Follow-up could not be added", reason),
    }
}

/// json reads the browser's complete Work object without semantic filtering.
async fn json<T: for<'de> Deserialize<'de>>(body: Body) -> Result<T, String> {
    let bytes = body
        .collect()
        .await
        .map_err(|error| format!("Could not receive Work bytes: {error}"))?
        .to_bytes();
    serde_json::from_slice(&bytes).map_err(|error| format!("Could not decode Work JSON: {error}"))
}

/// bad_request reports malformed Browser-to-ArchiGoat JSON without exposing native state.
fn bad_request(message: &str) -> Response {
    (StatusCode::BAD_REQUEST, message.to_owned()).into_response()
}

/// unavailable keeps internal diagnostics local and gives the browser one stable product fact.
fn unavailable(message: &str, reason: impl std::fmt::Display) -> Response {
    eprintln!("Product: {reason}");
    (StatusCode::SERVICE_UNAVAILABLE, message.to_owned()).into_response()
}
