use reqwest::{redirect::Policy, Client, Method};
use serde::{Deserialize, Serialize};
use std::env;
#[cfg(target_os = "macos")]
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tauri::{Manager, Runtime};
use url::Url;

#[cfg(target_os = "macos")]
mod macos;
mod preview;

const SERVICE: &str = "com.archigoat.app";
const ACCOUNT_URL: &str = match option_env!("ACCOUNT_URL") {
    Some(value) => value,
    None => "https://triangoat.com",
};
const LOOPBACK_URL: &str = "http://127.0.0.1:17891";
const PROTOCOL: u16 = 15;
const HANDOFF_URL: &str = "http://127.0.0.1:17891/internal/handoff";
const HANDOFF_ATTEMPTS: usize = 20;
const HANDOFF_RETRY_DELAY: Duration = Duration::from_millis(250);
pub(crate) const MAX_ACCOUNT_BODY: usize = 64 * 1024 * 1024;
static HANDOFF_SLOT: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static START_ERROR_SLOT: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn valid_token(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_voucher(value: &str) -> bool {
    value.len() <= 1024
        && value.split_once('.').is_some_and(|(payload, signature)| {
            !payload.is_empty()
                && !signature.is_empty()
                && !signature.contains('.')
                && payload
                    .bytes()
                    .chain(signature.bytes())
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
}

fn account_authorization_url(origin: &str) -> Result<String, String> {
    let mut url = Url::parse(origin).map_err(|_| "TrianGoat URL is invalid".to_owned())?;
    url.set_path("/");
    url.set_query(Some("authorize=archigoat"));
    url.set_fragment(None);
    Ok(url.into())
}

fn open_browser(url: &str) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        Command::new("/usr/bin/open")
            .arg(url)
            .spawn()
            .map(|_| true)
            .map_err(|error| error.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("Open TrianGoat in your browser".to_owned())
    }
}

#[tauri::command]
fn open_account() -> Result<bool, String> {
    open_browser(ACCOUNT_URL)
}

#[tauri::command]
fn authorize_account() -> Result<bool, String> {
    open_browser(&account_authorization_url(ACCOUNT_URL)?)
}

// The session lives in one private app file inside the user's own application data directory.
fn session_file() -> Result<std::path::PathBuf, String> {
    #[cfg(target_os = "macos")]
    let root = env::var_os("HOME")
        .map(|home| std::path::PathBuf::from(home).join("Library/Application Support/ArchiGoat"));
    #[cfg(target_os = "windows")]
    let root =
        env::var_os("LOCALAPPDATA").map(|base| std::path::PathBuf::from(base).join("ArchiGoat"));
    root.map(|directory| directory.join("session"))
        .ok_or_else(|| "ArchiGoat session location is unavailable".to_owned())
}

fn credential_get_native() -> Result<Option<String>, String> {
    let path = session_file()?;
    match std::fs::read_to_string(&path) {
        Ok(value) => Ok(Some(value.trim().to_owned()).filter(|value| valid_token(value))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("ArchiGoat could not read its session: {error}")),
    }
}

// The staged write keeps the saved session complete or absent, never torn.
fn credential_set_native(token: &str) -> Result<(), String> {
    if !valid_token(token) {
        return Err("ArchiGoat session is invalid".to_owned());
    }
    let path = session_file()?;
    let parent = path
        .parent()
        .ok_or_else(|| "ArchiGoat session location is unavailable".to_owned())?;
    let fault = |error: std::io::Error| format!("ArchiGoat could not save its session: {error}");
    std::fs::create_dir_all(parent).map_err(fault)?;
    let staged = parent.join("session.next");
    {
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        use std::io::Write;
        let mut file = options.open(&staged).map_err(fault)?;
        file.write_all(token.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(fault)?;
    }
    std::fs::rename(&staged, &path).map_err(fault)
}

fn credential_clear_native() -> Result<(), String> {
    let path = session_file()?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("ArchiGoat could not clear its session: {error}")),
    }
}

#[tauri::command]
fn credential_get() -> Result<Option<String>, String> {
    credential_get_native()
}

#[tauri::command]
fn credential_clear() -> Result<(), String> {
    credential_clear_native()
}

fn handoff_token(value: &str) -> Option<String> {
    let parsed = Url::parse(value).ok()?;
    let trusted = matches!(
        (parsed.scheme(), parsed.host_str(), parsed.path()),
        ("archigoat", Some("open"), "" | "/")
    );
    if !trusted
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
    {
        return None;
    }
    let mut query = parsed.query_pairs();
    let (name, token) = query.next()?;
    if query.next().is_some() || name != "token" || !valid_token(&token) {
        return None;
    }
    Some(token.into_owned())
}

fn trusted_handoff(value: &str) -> Option<String> {
    handoff_token(value).map(|_| value.to_owned())
}

#[tauri::command]
fn handoff_argument() -> Option<String> {
    peek_handoff()
}

fn handoff_slot() -> &'static Mutex<Option<String>> {
    HANDOFF_SLOT.get_or_init(|| Mutex::new(None))
}

fn store_handoff_in(slot: &Mutex<Option<String>>, url: String) {
    if let Ok(mut slot) = slot.lock() {
        *slot = Some(url);
    }
}

// The slot is peeked, not taken: the deep link survives a failed forward for the next attempt.
fn peek_handoff_from(slot: &Mutex<Option<String>>) -> Option<String> {
    slot.lock().ok()?.clone()
}

fn clear_handoff_from(slot: &Mutex<Option<String>>, url: &str) {
    if let Ok(mut slot) = slot.lock() {
        if slot.as_deref() == Some(url) {
            *slot = None;
        }
    }
}

fn store_handoff(url: String) {
    store_handoff_in(handoff_slot(), url);
}

fn peek_handoff() -> Option<String> {
    peek_handoff_from(handoff_slot())
}

fn clear_handoff(url: &str) {
    clear_handoff_from(handoff_slot(), url);
}

fn initialize_handoff() {
    if let Some(url) = env::args()
        .skip(1)
        .find(|argument| handoff_token(argument).is_some())
    {
        store_handoff(url);
    }
}

#[tauri::command]
fn start_error() -> Option<String> {
    START_ERROR_SLOT
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|value| value.clone())
        .or_else(|| {
            env::args()
                .skip(1)
                .find_map(|argument| Some(argument.strip_prefix("--start-error=")?.to_owned()))
                .filter(|message| !message.is_empty())
        })
}

#[tauri::command]
async fn forward_handoff(url: String) -> Result<(), String> {
    let token = forward_handoff_to(&url, HANDOFF_URL).await?;
    // The claim consumed the one-time token; parking it would turn every retry into a dead 410.
    clear_handoff(&url);
    credential_set_native(&token)?;
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HandoffSession {
    access_token: String,
    #[allow(dead_code)]
    expires_at: u64,
}

// Only transport failures retry: any HTTP answer is final, because the one-time token is
// consumed by the first claim and a re-POST can only ever come back 410.
async fn forward_handoff_to(url: &str, endpoint: &str) -> Result<String, String> {
    let token = handoff_token(url).ok_or_else(|| "Handoff URL is not trusted".to_owned())?;
    let client = Client::builder()
        .redirect(Policy::none())
        .build()
        .map_err(|error| error.to_string())?;
    let body = format!("{{\"token\":\"{token}\"}}");
    let mut last_error = "Daemon handoff did not respond".to_owned();
    for attempt in 0..HANDOFF_ATTEMPTS {
        match client
            .post(endpoint)
            .header("Content-Type", "application/json")
            .body(body.clone())
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                let bytes = response
                    .bytes()
                    .await
                    .map_err(|_| "Daemon handoff response is invalid".to_owned())?;
                let session = serde_json::from_slice::<HandoffSession>(&bytes)
                    .map_err(|_| "Daemon handoff response is invalid".to_owned())?;
                if !valid_token(&session.access_token) {
                    return Err("Daemon handoff response is invalid".to_owned());
                }
                return Ok(session.access_token);
            }
            Ok(response) => {
                let status = response.status();
                let reason = response.text().await.unwrap_or_default();
                return Err(if reason.trim().is_empty() {
                    format!("Daemon handoff failed ({status})")
                } else {
                    format!("Daemon handoff failed ({status}): {}", reason.trim())
                });
            }
            Err(error) if error.is_connect() || error.is_timeout() => {
                last_error = error.to_string();
            }
            Err(error) => return Err(error.to_string()),
        }
        if attempt + 1 < HANDOFF_ATTEMPTS {
            tokio::time::sleep(HANDOFF_RETRY_DELAY).await;
        }
    }
    Err(last_error)
}

#[derive(Debug, Deserialize)]
struct AccountRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
}

#[derive(Debug, Serialize)]
struct AccountResponse {
    status: u16,
    body: Vec<u8>,
}

fn allowed_path(method: &str, path: &str) -> bool {
    if !path.starts_with("/auth/") || path.contains("..") || path.contains('#') {
        return false;
    }
    match (method, path.split('?').next().unwrap_or(path)) {
        ("GET", "/auth/app/voucher")
        | ("GET", "/auth/me")
        | ("GET", "/auth/goat/sessions")
        | ("GET", "/auth/goat/turns")
        | ("GET", "/auth/mine")
        | ("GET", "/auth/mine/preview")
        | ("GET", "/auth/pair/roster")
        | ("GET", "/auth/work/pending")
        | ("GET", "/auth/work/pending/input")
        | ("GET", "/auth/work/status")
        | ("GET", "/auth/remote/work") => true,
        ("GET", route) if route == "/auth/work/file" => {
            path.contains("id=") && path.contains("file=")
        }
        ("GET", route) if route.starts_with("/auth/attachments/") => true,
        ("POST", "/auth/goat/sessions")
        | ("POST", "/auth/goat/append")
        | ("POST", "/auth/goat/steer")
        | ("POST", "/auth/goat/remove")
        | ("POST", "/auth/goat/rename")
        | ("POST", "/auth/remote/stop")
        | ("POST", "/auth/pair/offer")
        | ("POST", "/auth/mine/public")
        | ("POST", "/auth/mine/private")
        | ("POST", "/auth/mine/delete")
        | ("POST", "/auth/mine/rename")
        | ("POST", "/auth/logout") => true,
        ("POST", "/auth/attachments") => true,
        ("DELETE", route)
            if route.strip_prefix("/auth/pair/").is_some_and(|id| {
                id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
            }) =>
        {
            true
        }
        ("DELETE", route) if route.starts_with("/auth/attachments/") => true,
        _ => false,
    }
}

fn allowed_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization" | "content-type" | "x-app-device" | "x-file-name"
    )
}

fn allowed_loopback_path(method: &str, path: &str) -> bool {
    if !path.starts_with("/v1/") || path.contains("..") || path.contains('#') {
        return false;
    }
    let route = path.split('?').next().unwrap_or(path);
    matches!(
        (method, route),
        ("GET", "/v1/health")
            | ("GET", "/v1/status")
            | ("GET", "/v1/work")
            | ("POST", "/v1/connect")
            | ("POST", "/v1/connect/code")
            | ("POST", "/v1/session/end")
            | ("POST", "/v1/input")
            | ("POST", "/v1/work")
            | ("POST", "/v1/work/deliver")
            | ("POST", "/v1/work/steer")
            | ("DELETE", "/v1/work")
    )
}

fn allowed_loopback_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "content-type"
            | "x-work-id"
            | "x-work-kind"
            | "x-work-input-id"
            | "x-work-input-bytes"
            | "x-work-input-sha256"
            | "x-file-name"
    )
}

#[tauri::command]
async fn account_request(request: AccountRequest) -> Result<AccountResponse, String> {
    let method = request.method.to_ascii_uppercase();
    if !allowed_path(&method, &request.path) {
        return Err("Account route is not allowed".to_owned());
    }
    if request.body.as_ref().map_or(0, Vec::len) > MAX_ACCOUNT_BODY {
        return Err("Account request is too large".to_owned());
    }
    let client = Client::builder()
        .redirect(Policy::none())
        .build()
        .map_err(|error| error.to_string())?;
    let method = Method::from_bytes(method.as_bytes()).map_err(|error| error.to_string())?;
    let mut call = client.request(method, format!("{ACCOUNT_URL}{}", request.path));
    for (name, value) in request.headers {
        if !allowed_header(&name) || name.eq_ignore_ascii_case("origin") {
            return Err("Account header is not allowed".to_owned());
        }
        call = call.header(name, value);
    }
    if let Some(body) = request.body {
        call = call.body(body);
    }
    let response = call.send().await.map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let body = read_response(response).await?;
    Ok(AccountResponse { status, body })
}

// The card plays from its own scheme, so its bytes never ride the app bridge twice.
#[tauri::command]
async fn stage_preview(id: String, sha256: String) -> Result<preview::PreviewHandle, String> {
    preview::stage(ACCOUNT_URL, credential_get_native()?, id, sha256).await
}

#[derive(Debug, Deserialize)]
struct LoopbackRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    authenticated: bool,
    voucher: Option<String>,
}

#[tauri::command]
async fn loopback_request(request: LoopbackRequest) -> Result<AccountResponse, String> {
    let method = request.method.to_ascii_uppercase();
    if !allowed_loopback_path(&method, &request.path) {
        return Err("Loopback route is not allowed".to_owned());
    }
    if request.body.as_ref().map_or(0, Vec::len) > MAX_ACCOUNT_BODY {
        return Err("Loopback request is too large".to_owned());
    }
    if request.authenticated && !request.voucher.as_deref().is_some_and(valid_voucher) {
        return Err("Loopback voucher is invalid".to_owned());
    }
    let client = Client::builder()
        .redirect(Policy::none())
        .build()
        .map_err(|error| error.to_string())?;
    let method = Method::from_bytes(method.as_bytes()).map_err(|error| error.to_string())?;
    let mut call = client.request(method, format!("{LOOPBACK_URL}{}", request.path));
    if request.authenticated {
        call = call
            .header(
                "Authorization",
                format!("Bearer {}", request.voucher.unwrap_or_default()),
            )
            .header("x-app-protocol", PROTOCOL);
    }
    for (name, value) in request.headers {
        if !allowed_loopback_header(&name) {
            return Err("Loopback header is not allowed".to_owned());
        }
        call = call.header(name, value);
    }
    if let Some(body) = request.body {
        call = call.body(body);
    }
    let response = call.send().await.map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let body = read_response(response).await?;
    Ok(AccountResponse { status, body })
}

pub(crate) async fn read_response(mut response: reqwest::Response) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_ACCOUNT_BODY as u64)
    {
        return Err("Native response is too large".to_owned());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        if body.len().saturating_add(chunk.len()) > MAX_ACCOUNT_BODY {
            return Err("Native response is too large".to_owned());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn focus_main<R: Runtime>(app: &tauri::AppHandle<R>, reload: bool) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        if reload {
            let _ = window.eval("window.location.reload()");
        }
    }
}

#[cfg(target_os = "macos")]
fn handle_run_event<R: Runtime>(app: &tauri::AppHandle<R>, event: tauri::RunEvent) {
    match event {
        tauri::RunEvent::Opened { urls } => {
            let url = urls
                .into_iter()
                .find_map(|url| trusted_handoff(url.as_str()));
            let reload = url.is_some();
            if let Some(url) = url {
                store_handoff(url);
            }
            focus_main(app, reload);
        }
        tauri::RunEvent::Reopen { .. } => focus_main(app, false),
        _ => {}
    }
}

#[cfg(not(target_os = "macos"))]
fn handle_run_event<R: Runtime>(_app: &tauri::AppHandle<R>, _event: tauri::RunEvent) {}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "macos")]
    if let Err(error) = macos::prepare(SERVICE) {
        if let Ok(mut slot) = START_ERROR_SLOT.get_or_init(|| Mutex::new(None)).lock() {
            *slot = Some(error);
        }
    }
    initialize_handoff();
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            let url = argv
                .into_iter()
                .find_map(|argument| trusted_handoff(&argument));
            let has_handoff = url.is_some();
            if let Some(url) = url {
                store_handoff(url);
            }
            focus_main(app, has_handoff);
        }))
        .invoke_handler(tauri::generate_handler![
            credential_get,
            credential_clear,
            open_account,
            authorize_account,
            handoff_argument,
            start_error,
            forward_handoff,
            account_request,
            loopback_request,
            stage_preview
        ])
        .register_uri_scheme_protocol(preview::SCHEME, |_context, request| {
            preview::respond(&request)
        })
        .build(tauri::generate_context!())
        .expect("ArchiGoat failed to build")
        .run(handle_run_event);
}
