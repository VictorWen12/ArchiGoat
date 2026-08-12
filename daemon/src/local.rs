//! This module exposes the durable ArchiGoat core only to the Account-authorized native shell.

mod handoff;
mod voucher;

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Query, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

use crate::{DaemonState, Provider, api, state::Phase};
use voucher::{Authorization, VoucherAuthorizer};

// Loopback accepts only the native shell; browser-shaped requests carry Origin and are rejected.
const PROTOCOL_HEADER: &str = "x-app-protocol";

// LocalState adds only shell voucher checks and an ephemeral port-owner challenge.
#[derive(Clone)]
struct LocalState {
    daemon: DaemonState,
    vouchers: VoucherAuthorizer,
    challenge: Arc<Mutex<Option<Challenge>>>,
}

// One short-lived nonce pair makes each port-owner proof single-use and replay-resistant.
struct Challenge {
    client_nonce: String,
    server_nonce: String,
    created: Instant,
}

// Health exposes registration and compatibility without requiring shell authority.
#[derive(Serialize)]
struct Health {
    device: String,
    registered: bool,
    version: &'static str,
    protocol: u16,
}

// Observation is the shell's complete connection fact and long-read cursor.
#[derive(Clone, PartialEq, Serialize)]
struct Observation {
    device: String,
    registered: bool,
    state: &'static str,
    provider: Option<Provider>,
    model: Option<String>,
    effort: Option<String>,
    models: Vec<crate::provider::ModelChoice>,
    presets: Option<crate::provider::PresetPair>,
    installed: Vec<Provider>,
    version: &'static str,
    protocol: u16,
}

// ObserveQuery accepts the shell's prior fact; an exact match waits for a real event.
#[derive(Default, Deserialize)]
struct ObserveQuery {
    registered: Option<bool>,
    state: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    version: Option<String>,
    protocol: Option<u16>,
    #[serde(default)]
    installed: Option<Vec<String>>,
}

// WorkQuery binds every durable action to one explicit owner Work identity.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkQuery {
    work_id: String,
}

// DesignMock carries the optional fixed-path image without creating another upload channel.
#[derive(Serialize)]
pub(crate) struct DesignMock {
    pub(crate) media: &'static str,
    pub(crate) bytes: String,
}

// SavedWorkPath reads only the durable path facts needed to find one terminal Work workspace.
#[derive(Deserialize)]
struct SavedWorkPath {
    work_id: String,
    #[serde(default)]
    session: Option<PathBuf>,
}

// DesignMock reads the optional design image from the Work workspace at delivery time.
pub(crate) async fn design_mock(
    state: &DaemonState,
    work_id: &str,
) -> Result<Option<DesignMock>, String> {
    let Some(workspace) = work_workspace(state, work_id)? else {
        return Ok(None);
    };
    let path = workspace.join("design").join("mock.png");
    read_design_mock(&path).await
}

async fn read_design_mock(path: &std::path::Path) -> Result<Option<DesignMock>, String> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(Some(DesignMock {
            media: "image/png",
            bytes: STANDARD.encode(bytes),
        })),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Design mock is unavailable: {error}")),
    }
}

// WorkWorkspace resolves the session saved for this Work and keeps reads inside private Works.
fn work_workspace(state: &DaemonState, work_id: &str) -> Result<Option<PathBuf>, String> {
    let bytes = match fs::read(state.work_state_path()) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Work state is unavailable: {error}")),
    };
    let records: Vec<SavedWorkPath> = serde_json::from_slice(&bytes)
        .map_err(|_| "Work state is invalid".to_owned())?;
    let Some(session) = records
        .into_iter()
        .find(|record| record.work_id == work_id)
        .and_then(|record| record.session)
    else {
        return Ok(None);
    };
    let works_root = state.private_root()?.join("Works");
    if !session.starts_with(&works_root) {
        return Err("Work workspace is outside private storage".to_owned());
    }
    Ok(Some(session.join("Work")))
}

// LocalDeliveryRequest carries only the ephemeral Account session and frozen destination facts.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalDeliveryRequest {
    account_token: String,
    scope_kind: String,
    scope_id: String,
    delivery_id: String,
}

impl LocalDeliveryRequest {
    fn validate(&self) -> Result<(), &'static str> {
        if !valid_account_token(&self.account_token)
            || self.scope_kind != "goat"
            || !valid_component(&self.scope_id)
            || !valid_component(&self.delivery_id)
        {
            return Err("Local delivery identity is invalid");
        }
        Ok(())
    }
}

// ConnectRequest admits only a typed native Provider selection.
#[derive(Deserialize)]
struct ConnectRequest {
    provider: Provider,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    effort: Option<String>,
}

// CodeRequest carries the browser's one-time sign-in code.
#[derive(Deserialize)]
struct CodeRequest {
    code: String,
}

// Internal challenge requests never carry shell authority or token bytes.
#[derive(Deserialize)]
struct ChallengeRequest {
    client_nonce: String,
}

// ExitRequest carries a newer instance's protocol claim under the same installation key.
#[derive(Deserialize)]
struct ExitRequest {
    client_nonce: String,
    server_nonce: String,
    protocol: u16,
    proof: String,
}

// HandoffRequest carries only the Studio one-time token over launcher IPC.
#[derive(Deserialize)]
struct HandoffRequest {
    token: String,
}

// ApiError preserves deterministic HTTP rejection without leaking native diagnostics.
struct ApiError(StatusCode, &'static str);

// This conversion preserves each local rejection as its exact public HTTP fact.
impl IntoResponse for ApiError {
    // Plain stable reasons keep local protocol failures actionable and bounded.
    fn into_response(self) -> Response {
        (self.0, self.1).into_response()
    }
}

// App fixes every public action and private handoff route before applying shell security once.
pub(crate) fn app(daemon: DaemonState) -> Router {
    let state = LocalState {
        vouchers: VoucherAuthorizer::new(),
        daemon,
        challenge: Arc::new(Mutex::new(None)),
    };
    // Product routes share protocol and Account voucher enforcement.
    let web = Router::new()
        .route("/v1/status", get(status))
        .route("/v1/connect", post(connect))
        .route("/v1/connect/code", post(connect_code))
        .route("/v1/input", post(stage_input))
        .route(
            "/v1/work",
            get(observe_work).post(start_work).delete(stop_work),
        )
        .route(
            "/v1/work/deliver",
            post(deliver_work).layer(DefaultBodyLimit::max(16 * 1024)),
        )
        .route("/v1/work/steer", post(steer_work))
        .route("/v1/work/publish", post(publish_work))
        .layer(middleware::from_fn_with_state(state.clone(), security))
        .with_state(state.clone());
    // Health carries no native authority and remains readable without a voucher.
    let public = Router::new()
        .route("/v1/health", get(health))
        .route("/v1/session/end", post(end_session))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            health_security,
        ))
        .with_state(state.clone());
    // Port-owner handoff stays unreachable to browser requests because each handler rejects Origin.
    let internal = Router::new()
        .route("/internal/challenge", post(challenge))
        .route("/internal/exit", post(exit_stale))
        .route("/internal/handoff", post(receive_handoff))
        .with_state(state);
    public
        .merge(web)
        .merge(internal)
        .layer(middleware::from_fn(native_only))
}

// NativeOnly rejects browser-shaped traffic even for an unknown local route.
async fn native_only(request: Request, next: Next) -> Response {
    let path = request.uri().path();
    if (path == "/v1"
        || path == "/internal"
        || path.starts_with("/v1/")
        || path.starts_with("/internal/"))
        && request.headers().contains_key(header::ORIGIN)
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(request).await
}

// HealthSecurity admits only native shell traffic; any browser Origin is forbidden.
async fn health_security(
    State(_state): State<LocalState>,
    request: Request,
    next: Next,
) -> Response {
    if request.headers().contains_key(header::ORIGIN) {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(request).await
}

// Health reveals only registration and compatibility; private routes require an Account voucher.
async fn health(State(state): State<LocalState>) -> Json<Health> {
    Json(Health {
        device: state.daemon.device_id().to_owned(),
        registered: state.daemon.registered().await,
        version: crate::version(),
        protocol: api::PROTOCOL,
    })
}

// Status holds an unchanged cursor until Provider or registration truth changes.
async fn status(
    State(state): State<LocalState>,
    Query(known): Query<ObserveQuery>,
) -> Result<Json<Observation>, ApiError> {
    loop {
        // Register both listeners before reads so no connection or registration event is lost.
        let connection = state.daemon.connection_events.notified();
        let registration = state.daemon.registration_events.notified();
        tokio::pin!(connection, registration);
        connection.as_mut().enable();
        registration.as_mut().enable();
        let current = observation(&state.daemon).await;
        if !known.matches(&current) {
            return Ok(Json(current));
        }
        tokio::select! {
            _ = &mut connection => {},
            _ = &mut registration => {},
            // A bounded wait answers unchanged truth so no browser poll can hang forever.
            _ = tokio::time::sleep(std::time::Duration::from_secs(25)) => return Ok(Json(current)),
        }
    }
}

// Connect delegates typed Provider selection to the current native authentication flow.
async fn connect(State(state): State<LocalState>, Json(request): Json<ConnectRequest>) -> Response {
    let Ok(model) = selection(request.model) else {
        return (StatusCode::BAD_REQUEST, "Model is invalid").into_response();
    };
    let Ok(effort) = selection(request.effort) else {
        return (StatusCode::BAD_REQUEST, "Effort is invalid").into_response();
    };
    api::execute(
        state.daemon,
        api::Action::Connect {
            provider: request.provider,
            model,
            effort,
        },
    )
    .await
}

// ConnectCode hands the browser's one-time code to the live sign-in flow.
async fn connect_code(
    State(state): State<LocalState>,
    Json(request): Json<CodeRequest>,
) -> Response {
    let code = request.code.trim();
    if code.is_empty() || code.len() > 4096 || code.chars().any(char::is_control) {
        return (StatusCode::BAD_REQUEST, "Code is invalid").into_response();
    }
    match state.daemon.submit_code(code).await {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(reason) => (StatusCode::CONFLICT, reason).into_response(),
    }
}

// Selection admits only plain short tier names.
fn selection(value: Option<String>) -> Result<Option<String>, ()> {
    match value {
        Some(value)
            if value.trim().is_empty()
                || value.len() > 256
                || value.chars().any(char::is_control) =>
        {
            Err(())
        }
        value => Ok(value),
    }
}

// EndSession revokes local shell authority even when its Account session is already gone.
async fn end_session(State(state): State<LocalState>) -> StatusCode {
    state.vouchers.end_session().await;
    StatusCode::NO_CONTENT
}

// StageInput generates a private nonce locally while preserving declared metadata and exact body bytes.
async fn stage_input(
    State(state): State<LocalState>,
    Query(query): Query<WorkQuery>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    let work_id = valid_work(query.work_id)?;
    let encoded_name = required(&headers, "x-file-name", 6_144)?;
    let input = api::input::Upload {
        id: required(&headers, "x-work-input-id", 256)?,
        name: decoded(encoded_name, 2_048)?,
        media: required(&headers, header::CONTENT_TYPE.as_str(), 256)?,
        bytes: required(&headers, "x-work-input-bytes", 32)?
            .parse()
            .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "Input byte count is invalid"))?,
    };
    let nonce = crate::proof::nonce().map_err(|_| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Input identity is unavailable",
        )
    })?;
    Ok(api::execute(
        state.daemon,
        api::Action::StageInput {
            work_id,
            nonce,
            input,
            body,
        },
    )
    .await)
}

// StartWork hands the untouched JSON body to the current durable Work admission path.
async fn start_work(
    State(state): State<LocalState>,
    Query(query): Query<WorkQuery>,
    body: Body,
) -> Result<Response, ApiError> {
    Ok(api::execute(
        state.daemon,
        api::Action::StartWork {
            work_id: valid_work(query.work_id)?,
            body,
        },
    )
    .await)
}

// SteerWork appends one frozen follow-up to the existing Running Work.
async fn steer_work(
    State(state): State<LocalState>,
    Query(query): Query<WorkQuery>,
    body: Body,
) -> Result<Response, ApiError> {
    let steer_id = crate::proof::nonce().map_err(|_| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Follow-up identity is unavailable",
        )
    })?;
    Ok(api::execute(
        state.daemon,
        api::Action::SteerWork {
            work_id: valid_work(query.work_id)?,
            steer_id,
            body,
        },
    )
    .await)
}

// PublishWork applies Account's successful Publish to this computer's private lifecycle state.
async fn publish_work(
    State(state): State<LocalState>,
    Query(query): Query<WorkQuery>,
) -> Result<Response, ApiError> {
    state
        .daemon
        .publish_work(&valid_work(query.work_id)?)
        .map(|()| StatusCode::NO_CONTENT.into_response())
        .map_err(|_| ApiError(StatusCode::CONFLICT, "Publish requires a delivered Work"))
}

// ObserveWork returns the next durable public snapshot or 204 only when this Work is absent.
async fn observe_work(
    State(state): State<LocalState>,
    Query(query): Query<WorkQuery>,
) -> Result<Response, ApiError> {
    Ok(api::execute(
        state.daemon,
        api::Action::ObserveWork {
            work_id: valid_work(query.work_id)?,
        },
    )
    .await)
}

// StopWork exercises owner authority for only the addressed durable Work.
async fn stop_work(
    State(state): State<LocalState>,
    Query(query): Query<WorkQuery>,
) -> Result<Response, ApiError> {
    Ok(api::execute(
        state.daemon,
        api::Action::StopWork {
            work_id: valid_work(query.work_id)?,
        },
    )
    .await)
}

// DeliverWork moves frozen Done bytes directly from the daemon to Account, then acknowledges locally.
async fn deliver_work(
    State(state): State<LocalState>,
    Query(query): Query<WorkQuery>,
    Json(request): Json<LocalDeliveryRequest>,
) -> Response {
    if let Err(reason) = request.validate() {
        return ApiError(StatusCode::BAD_REQUEST, reason).into_response();
    }
    let work_id = match valid_work(query.work_id) {
        Ok(work_id) => work_id,
        Err(error) => return error.into_response(),
    };
    if let Err(error) = crate::account_relay::deliver_local(
        &state.daemon,
        &request.account_token,
        &work_id,
        &request.delivery_id,
        &request.scope_kind,
        &request.scope_id,
    )
    .await
    {
        // A refused delivery settles here and keeps its bytes: the owner is never asked to finish the
        // same Work again, and only Account ownership may delete a finished product.
        if rejected(&error) {
            eprintln!("Product local Work delivery was rejected: {error}");
            if let Err(reason) = state.daemon.settle_refused_work(&work_id) {
                eprintln!("Product could not settle the refused Work delivery: {reason}");
            }
            return (StatusCode::BAD_GATEWAY, error).into_response();
        }
        eprintln!("Product local Work delivery retry: {error}");
        return ApiError(StatusCode::SERVICE_UNAVAILABLE, "Work delivery unavailable")
            .into_response();
    }
    match state.daemon.acknowledge_work(&work_id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Work acknowledgement unavailable",
        )
        .into_response(),
    }
}

// Account refuses a delivery permanently with any 4xx except its two transient retry codes;
// every other cause, including transport failure, stays retryable. The relay reports one
// rejection as "Account rejected <context> with <status>", which carries that exact answer.
fn rejected(error: &str) -> bool {
    error
        .rsplit_once(" with ")
        .and_then(|(_, status)| status.split_whitespace().next())
        .and_then(|code| code.parse::<u16>().ok())
        .is_some_and(|code| (400..500).contains(&code) && code != 408 && code != 429)
}

// Challenge returns a fresh owner proof while rejecting every browser-shaped request.
async fn challenge(
    State(state): State<LocalState>,
    headers: HeaderMap,
    Json(request): Json<ChallengeRequest>,
) -> Result<Json<crate::proof::ChallengeResponse>, ApiError> {
    reject_browser(&headers)?;
    if !crate::proof::valid_nonce(&request.client_nonce) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Client challenge is invalid",
        ));
    }
    let server_nonce = crate::proof::nonce().map_err(|_| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Challenge is unavailable",
        )
    })?;
    let response = crate::proof::ChallengeResponse {
        proof: crate::proof::server_proof(
            &state.daemon.instance_secret,
            &request.client_nonce,
            &server_nonce,
        )
        .map_err(|_| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Challenge is unavailable",
            )
        })?,
        server_nonce: server_nonce.clone(),
    };
    *state.challenge.lock().await = Some(Challenge {
        client_nonce: request.client_nonce,
        server_nonce,
        created: Instant::now(),
    });
    Ok(Json(response))
}

// ExitStale yields the port to a proven newer installation; durable Work resumes under the new owner.
async fn exit_stale(
    State(state): State<LocalState>,
    headers: HeaderMap,
    Json(request): Json<ExitRequest>,
) -> Result<StatusCode, ApiError> {
    reject_browser(&headers)?;
    let challenge = state.challenge.lock().await.take().ok_or(ApiError(
        StatusCode::UNAUTHORIZED,
        "Exit challenge is missing",
    ))?;
    if challenge.created.elapsed() > Duration::from_secs(10)
        || challenge.client_nonce != request.client_nonce
        || challenge.server_nonce != request.server_nonce
        || !crate::proof::verify_exit(
            &state.daemon.instance_secret,
            &request.client_nonce,
            &request.server_nonce,
            request.protocol,
            &request.proof,
        )
    {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "Exit proof is invalid"));
    }
    if request.protocol <= api::PROTOCOL {
        return Err(ApiError(StatusCode::CONFLICT, "Live ArchiGoat is current"));
    }
    // The acknowledgement returns first so the newer instance can bind and resume durable Work.
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(150)).await;
        std::process::exit(0);
    });
    Ok(StatusCode::NO_CONTENT)
}

// ReceiveHandoff claims Studio's deep-link token only from the local shell, which stores the session.
async fn receive_handoff(
    State(state): State<LocalState>,
    headers: HeaderMap,
    Json(request): Json<HandoffRequest>,
) -> Result<Json<handoff::Session>, ApiError> {
    reject_browser(&headers)?;
    handoff::claim(&state.daemon, &request.token)
        .await
        .map(Json)
        .map_err(|error| ApiError(error.status, error.reason))
}

// Security admits only native shell traffic and an Account-valid voucher.
async fn security(State(state): State<LocalState>, request: Request, next: Next) -> Response {
    if request.headers().contains_key(header::ORIGIN) {
        return StatusCode::FORBIDDEN.into_response();
    }
    // The split is a protocol break: every browser request must name this exact contract.
    let compatible = request
        .headers()
        .get(PROTOCOL_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u16>().ok())
        .is_some_and(|value| value == api::PROTOCOL && value >= api::MIN_PROTOCOL);
    if !compatible {
        return (StatusCode::UPGRADE_REQUIRED, "ArchiGoat update is required").into_response();
    }
    let response = match authorize(&state, request.headers()).await {
        Authorization::Valid => next.run(request).await,
        Authorization::Invalid => StatusCode::UNAUTHORIZED.into_response(),
        Authorization::Foreign => StatusCode::CONFLICT.into_response(),
        Authorization::Unavailable => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    response
}

// Observation preserves every native authentication phase as exact browser truth.
async fn observation(state: &DaemonState) -> Observation {
    let registered = state.registered().await;
    let status = state.status.read().await;
    let installed = [Provider::Codex, Provider::Claude, Provider::Cursor]
        .into_iter()
        .filter(|provider| crate::cli::find(*provider, &state.config.cli_dirs).is_some())
        .collect();
    let models = state.models(status.provider).await;
    let presets = state.presets(status.provider).await;
    Observation {
        device: state.device_id().to_owned(),
        registered,
        state: match status.phase {
            Phase::Disconnected => "offline",
            Phase::Authorizing => "authorizing",
            Phase::Connected => "online",
        },
        provider: status.provider,
        model: status.model.clone(),
        effort: status.effort.clone(),
        models,
        presets,
        installed,
        version: crate::version(),
        protocol: api::PROTOCOL,
    }
}

// Cursor comparison makes changed truth immediate while unchanged truth remains event-backed.
impl ObserveQuery {
    // A long read waits only when every prior public fact exactly matches the current one.
    fn matches(&self, current: &Observation) -> bool {
        self.registered == Some(current.registered)
            && self.state.as_deref() == Some(current.state)
            && self.provider.as_deref().unwrap_or_default()
                == current
                    .provider
                    .map(|provider| provider.to_string())
                    .as_deref()
                    .unwrap_or_default()
            && self.model.as_deref().unwrap_or_default()
                == current.model.as_deref().unwrap_or_default()
            && self.version.as_deref() == Some(current.version)
            && self.protocol == Some(current.protocol)
            && self.installed.as_ref().is_some_and(|known| {
                known
                    == &current
                        .installed
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
            })
    }
}

// Authorization sends only one exact bearer voucher to Account.
async fn authorize(state: &LocalState, headers: &HeaderMap) -> Authorization {
    let Some(voucher) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty() && value.len() <= 4096 - "Bearer ".len())
    else {
        return Authorization::Invalid;
    };
    state.vouchers.authorize(&state.daemon, voucher).await
}

// Internal routes reject any request carrying the browser's mandatory Origin header.
fn reject_browser(headers: &HeaderMap) -> Result<(), ApiError> {
    (!headers.contains_key(header::ORIGIN))
        .then_some(())
        .ok_or(ApiError(StatusCode::FORBIDDEN, "Browser access denied"))
}

// Required headers stay bounded and control-free before entering durable state.
fn required(headers: &HeaderMap, name: &str, max: usize) -> Result<String, ApiError> {
    let value = headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .ok_or(ApiError(
            StatusCode::BAD_REQUEST,
            "Required input metadata is missing",
        ))?;
    bounded(value.to_owned(), max, "Input metadata is invalid")
}

// External protocol text is non-empty, bounded, and free of control characters.
fn bounded(value: String, max: usize, reason: &'static str) -> Result<String, ApiError> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        Err(ApiError(StatusCode::BAD_REQUEST, reason))
    } else {
        Ok(value)
    }
}

fn valid_component(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn valid_account_token(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

// Percent decoding restores a Unicode filename while rejecting malformed or non-UTF-8 escapes.
fn decoded(value: String, max: usize) -> Result<String, ApiError> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            output.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "Input filename is invalid",
            ));
        }
        let pair = std::str::from_utf8(&bytes[index + 1..index + 3])
            .ok()
            .and_then(|value| u8::from_str_radix(value, 16).ok())
            .ok_or(ApiError(
                StatusCode::BAD_REQUEST,
                "Input filename is invalid",
            ))?;
        output.push(pair);
        index += 3;
    }
    let value = String::from_utf8(output)
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "Input filename is invalid"))?;
    bounded(value, max, "Input filename is invalid")
}

// Work identity receives the browser contract's byte bound before touching local paths.
fn valid_work(value: String) -> Result<String, ApiError> {
    let value = bounded(value, 256, "Work identity is invalid")?;
    crate::work::valid_work_id(&value)
        .map(|()| value)
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "Work identity is invalid"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[tokio::test]
    async fn design_mock_reads_present_bytes_and_returns_none_when_absent() {
        let root = std::env::temp_dir().join(format!(
            "archigoat-design-mock-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("test clock should be after epoch")
                .as_nanos()
        ));
        let path = root.join("design").join("mock.png");
        tokio::fs::create_dir_all(path.parent().expect("mock parent should exist"))
            .await
            .expect("mock directory should be created");
        tokio::fs::write(&path, [0_u8, 1, 2, 255])
            .await
            .expect("mock bytes should be written");

        let mock = read_design_mock(&path)
            .await
            .expect("present mock should be readable")
            .expect("present mock should be attached");
        assert_eq!(
            serde_json::to_value(&mock).expect("mock should serialize"),
            serde_json::json!({
                "media": "image/png",
                "bytes": "AAEC/w=="
            })
        );

        tokio::fs::remove_dir_all(&root)
            .await
            .expect("test mock directory should be removed");
        assert!(read_design_mock(&path)
            .await
            .expect("absent mock should not error")
            .is_none());
    }
}
