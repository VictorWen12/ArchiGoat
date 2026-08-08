//! Serves a delivered product to its own frame. The bytes arrive on a scheme of their own, so the
//! card runs under the boundary its own page declares instead of the app's, and it plays in
//! Projects exactly as it plays in the feed.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

use reqwest::{redirect::Policy, Client};
use serde::Serialize;
use tauri::http::{header, Request, Response, StatusCode};

/// The scheme a product document loads on; a document here carries its own policy, never the app's.
pub(crate) const SCHEME: &str = "preview";
/// One page of products stays ready to open; older bytes leave once either bound is passed.
const MAX_ITEMS: usize = 24;
const MAX_BYTES: usize = 96 * 1024 * 1024;
const MAX_MEDIA: usize = 128;
const MAX_POLICY: usize = 2048;
const MAX_ID: usize = 80;
const DEFAULT_MEDIA: &str = "application/octet-stream";

/// PreviewHandle names the frame URL for ready bytes, or the exact status TrianGoat answered.
#[derive(Debug, Serialize)]
pub(crate) struct PreviewHandle {
    pub status: u16,
    pub url: String,
}

struct Staged {
    sha256: String,
    media: String,
    policy: Option<String>,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct Store {
    order: VecDeque<String>,
    items: HashMap<String, Staged>,
    held: usize,
}

fn store() -> &'static Mutex<Store> {
    static SLOT: OnceLock<Mutex<Store>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(Store::default()))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// The frame URL for one product, in the exact form this platform's webview serves the scheme.
fn frame_url(id: &str) -> String {
    #[cfg(any(target_os = "windows", target_os = "android"))]
    {
        format!("http://{SCHEME}.localhost/{id}")
    }
    #[cfg(not(any(target_os = "windows", target_os = "android")))]
    {
        format!("{SCHEME}://localhost/{id}")
    }
}

fn ready(id: &str, sha256: &str) -> bool {
    store().lock().is_ok_and(|store| {
        store
            .items
            .get(id)
            .is_some_and(|item| item.sha256 == sha256)
    })
}

fn keep(id: &str, staged: Staged) {
    let Ok(mut store) = store().lock() else {
        return;
    };
    if let Some(previous) = store.items.remove(id) {
        store.held = store.held.saturating_sub(previous.bytes.len());
        store.order.retain(|held| held != id);
    }
    store.held = store.held.saturating_add(staged.bytes.len());
    store.order.push_back(id.to_owned());
    store.items.insert(id.to_owned(), staged);
    // The product just opened always stays; only older bytes are given up.
    while store.order.len() > 1 && (store.order.len() > MAX_ITEMS || store.held > MAX_BYTES) {
        let Some(oldest) = store.order.pop_front() else {
            break;
        };
        if let Some(item) = store.items.remove(&oldest) {
            store.held = store.held.saturating_sub(item.bytes.len());
        }
    }
}

fn copy(id: &str) -> Option<(String, Option<String>, Vec<u8>)> {
    let store = store().lock().ok()?;
    let item = store.items.get(id)?;
    Some((item.media.clone(), item.policy.clone(), item.bytes.clone()))
}

fn header_text(headers: &reqwest::header::HeaderMap, name: &str, maximum: usize) -> Option<String> {
    let value = headers.get(name)?.to_str().ok()?.trim();
    (!value.is_empty() && value.len() <= maximum).then(|| value.to_owned())
}

// The product's own boundary travels with it; only the web page's framing rule is dropped, because
// the frame that runs the card here is this app.
fn framed_policy(value: &str) -> Option<String> {
    let kept = value
        .split(';')
        .map(str::trim)
        .filter(|directive| {
            !directive.is_empty()
                && !directive
                    .split_whitespace()
                    .next()
                    .is_some_and(|name| name.eq_ignore_ascii_case("frame-ancestors"))
        })
        .collect::<Vec<_>>();
    (!kept.is_empty()).then(|| kept.join("; "))
}

// One byte range keeps long video seekable inside the frame.
fn span(value: &str, total: usize) -> Option<(usize, usize)> {
    if total == 0 {
        return None;
    }
    let spec = value.trim().strip_prefix("bytes=")?.trim();
    if spec.contains(',') {
        return None;
    }
    let (from, to) = spec.split_once('-')?;
    let (from, to) = (from.trim(), to.trim());
    let (start, end) = if from.is_empty() {
        let last: usize = to.parse().ok()?;
        if last == 0 {
            return None;
        }
        (total.saturating_sub(last.min(total)), total - 1)
    } else {
        let start: usize = from.parse().ok()?;
        let end = if to.is_empty() {
            total - 1
        } else {
            to.parse::<usize>().ok()?.min(total - 1)
        };
        (start, end)
    };
    (start <= end && start < total).then_some((start, end))
}

fn refuse(status: StatusCode) -> Response<Vec<u8>> {
    let mut response = Response::new(Vec::new());
    *response.status_mut() = status;
    response
}

/// Answers one frame request with the product bytes already held for it.
pub(crate) fn respond(request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let id = request.uri().path().trim_start_matches('/');
    let Some((media, policy, bytes)) = copy(id) else {
        return refuse(StatusCode::NOT_FOUND);
    };
    let total = bytes.len();
    let wanted = request
        .headers()
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| span(value, total));
    let mut builder = Response::builder()
        .header(header::CONTENT_TYPE, media)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-content-type-options", "nosniff");
    if let Some(policy) = policy {
        builder = builder.header("content-security-policy", policy);
    }
    let body = match wanted {
        Some((start, end)) => {
            builder = builder.status(StatusCode::PARTIAL_CONTENT).header(
                header::CONTENT_RANGE,
                format!("bytes {start}-{end}/{total}"),
            );
            bytes[start..=end].to_vec()
        }
        None => {
            builder = builder.status(StatusCode::OK);
            bytes
        }
    };
    builder
        .body(body)
        .unwrap_or_else(|_| refuse(StatusCode::INTERNAL_SERVER_ERROR))
}

/// Holds one product's verified bytes for its frame and names the URL that plays them.
pub(crate) async fn stage(
    account_url: &str,
    token: Option<String>,
    id: String,
    sha256: String,
) -> Result<PreviewHandle, String> {
    if !valid_id(&id) {
        return Err("Product is not available".to_owned());
    }
    if ready(&id, &sha256) {
        return Ok(PreviewHandle {
            status: StatusCode::OK.as_u16(),
            url: frame_url(&id),
        });
    }
    let Some(token) = token else {
        return Ok(PreviewHandle {
            status: StatusCode::UNAUTHORIZED.as_u16(),
            url: String::new(),
        });
    };
    let client = Client::builder()
        .redirect(Policy::none())
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get(format!("{account_url}/auth/mine/preview?id={id}"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    if !response.status().is_success() {
        return Ok(PreviewHandle {
            status,
            url: String::new(),
        });
    }
    let media = header_text(response.headers(), "content-type", MAX_MEDIA)
        .unwrap_or_else(|| DEFAULT_MEDIA.to_owned());
    let policy = header_text(response.headers(), "content-security-policy", MAX_POLICY)
        .and_then(|value| framed_policy(&value));
    let bytes = crate::read_response(response).await?;
    keep(
        &id,
        Staged {
            sha256,
            media,
            policy,
            bytes,
        },
    );
    Ok(PreviewHandle {
        status,
        url: frame_url(&id),
    })
}
