//! Voucher authorization binds each local shell request to its Account owner.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Client, StatusCode, header::HeaderValue};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, watch};

use crate::DaemonState;

// Account bounds the complete Authorization value to 4096 bytes.
const MAX_CREDENTIAL_BYTES: usize = 4096 - "Bearer ".len();
// LastGoodGrace spans one Account timeout without turning revocation into cached authorization.
const LAST_GOOD_GRACE: Duration = Duration::from_secs(60);
const MAX_VOUCHER_TTL: Duration = Duration::from_secs(10 * 60);
const MAX_CACHED_VERDICTS: usize = 256;

// Authorization preserves Account's owner verdict instead of collapsing it into availability.
#[derive(Clone, Copy)]
pub(super) enum Authorization {
    Valid,
    Invalid,
    Foreign,
    Unavailable,
}

// VoucherAuthorizer shares only each live exact-voucher check and serializes first registration.
#[derive(Clone)]
pub(super) struct VoucherAuthorizer {
    client: Option<Client>,
    registration: Arc<Mutex<()>>,
    verifications:
        Arc<Mutex<HashMap<(String, String, u64), watch::Receiver<Option<Authorization>>>>>,
    cache: Arc<Mutex<HashMap<String, CachedVerdict>>>,
    last_good: Arc<Mutex<VoucherGrace>>,
    generation: Arc<AtomicU64>,
}

// LastGood binds one recent Account acceptance to the credential that earned it.
struct LastGood {
    credential: String,
    accepted_at: Instant,
}

// CachedVerdict stays bound to the credential that earned it and the voucher's signed TTL.
struct CachedVerdict {
    credential: String,
    verdict: Authorization,
    expires_at: Instant,
}

// VoucherGrace holds only recent exact-voucher acceptances for transient recovery.
#[derive(Default)]
struct VoucherGrace(HashMap<String, LastGood>);

impl VoucherGrace {
    // Remember replaces one voucher and prunes expired or rotated authority.
    fn remember(&mut self, voucher: &str, credential: &str, now: Instant) {
        self.0.retain(|_, accepted| {
            accepted.credential == credential
                && now.duration_since(accepted.accepted_at) <= LAST_GOOD_GRACE
        });
        if self.0.len() >= MAX_CACHED_VERDICTS
            && let Some(oldest) = self
                .0
                .iter()
                .min_by_key(|(_, accepted)| accepted.accepted_at)
                .map(|(voucher, _)| voucher.clone())
        {
            self.0.remove(&oldest);
        }
        self.0.insert(
            voucher.to_owned(),
            LastGood {
                credential: credential.to_owned(),
                accepted_at: now,
            },
        );
    }

    // Accepts admits only one fresh exact voucher under the unchanged Account credential.
    fn accepts(&mut self, voucher: &str, credential: &str, now: Instant) -> bool {
        let accepted = self.0.get(voucher).is_some_and(|accepted| {
            accepted.credential == credential
                && now.duration_since(accepted.accepted_at) <= LAST_GOOD_GRACE
        });
        if !accepted {
            self.0.remove(voucher);
        }
        accepted
    }

    // Forget removes the explicitly rejected voucher only.
    fn forget(&mut self, voucher: &str) {
        self.0.remove(voucher);
    }

    // Clear drops every acceptance when Account rejects the installation identity.
    fn clear(&mut self) {
        self.0.clear();
    }
}

// VoucherBody sends shell authority without mixing daemon identity into JSON.
#[derive(Serialize)]
struct VoucherBody<'a> {
    voucher: &'a str,
}

// Registration returns only the durable Account credential.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Registration {
    app_credential: String,
}

// Verification names invalid vouchers explicitly.
#[derive(Deserialize)]
struct Verification {
    valid: bool,
    #[serde(default)]
    reason: Option<String>,
}

impl VoucherAuthorizer {
    // New fixes one config-validated Account client and empty transient authority lanes.
    pub(super) fn new() -> Self {
        Self {
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .build()
                .ok(),
            registration: Arc::new(Mutex::new(())),
            verifications: Arc::new(Mutex::new(HashMap::new())),
            cache: Arc::new(Mutex::new(HashMap::new())),
            last_good: Arc::new(Mutex::new(VoucherGrace::default())),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    // Authorize serializes registration and reuses only an Account verdict inside its signed TTL.
    pub(super) async fn authorize(&self, state: &DaemonState, voucher: &str) -> Authorization {
        let Some(client) = &self.client else {
            return Authorization::Unavailable;
        };
        if state.credential().await.is_none() {
            return self.register_missing(client, state, voucher).await;
        }
        let credential = state.credential().await;
        let verdict = self.verify_shared(state, voucher).await;
        if matches!(verdict, Authorization::Invalid) && state.credential().await != credential {
            return self.register_missing(client, state, voucher).await;
        }
        verdict
    }

    // RegisterMissing repairs only a credential Account has explicitly rejected; concurrent repair verifies its winner.
    async fn register_missing(
        &self,
        client: &Client,
        state: &DaemonState,
        voucher: &str,
    ) -> Authorization {
        let registration = self.registration.lock().await;
        if state.credential().await.is_some() {
            drop(registration);
            return self.verify_shared(state, voucher).await;
        }
        let generation = self.generation.load(Ordering::Acquire);
        let verdict = self.register(client, state, voucher, generation).await;
        if let Some(credential) = state.credential().await {
            self.cache_verdict(voucher, &credential, verdict, generation)
                .await;
        }
        (self.generation.load(Ordering::Acquire) == generation)
            .then_some(verdict)
            .unwrap_or(Authorization::Unavailable)
    }

    // VerifyShared returns a signed-TTL verdict and coalesces any first check for that voucher.
    async fn verify_shared(&self, state: &DaemonState, voucher: &str) -> Authorization {
        let Some(credential) = state.credential().await else {
            return Authorization::Unavailable;
        };
        let cached_generation = self.generation.load(Ordering::Acquire);
        if let Some(verdict) = self.cached(voucher, &credential).await {
            return (self.generation.load(Ordering::Acquire) == cached_generation)
                .then_some(verdict)
                .unwrap_or(Authorization::Unavailable);
        }
        let generation = self.generation.load(Ordering::Acquire);
        let key = (voucher.to_owned(), credential.clone(), generation);
        let mut result = {
            let mut verifications = self.verifications.lock().await;
            if let Some(result) = verifications.get(&key) {
                result.clone()
            } else {
                let (publish, result) = watch::channel(None);
                verifications.insert(key.clone(), result.clone());
                let authorizer = self.clone();
                let state = state.clone();
                let voucher = voucher.to_owned();
                tokio::spawn(async move {
                    let verdict = match &authorizer.client {
                        Some(client) => {
                            authorizer
                                .verify(client, &state, &voucher, &credential, generation)
                                .await
                        }
                        None => Authorization::Unavailable,
                    };
                    if state.credential().await.as_deref() == Some(credential.as_str()) {
                        authorizer
                            .cache_verdict(&voucher, &credential, verdict, generation)
                            .await;
                    }
                    authorizer.verifications.lock().await.remove(&key);
                    let _ = publish.send(Some(verdict));
                });
                result
            }
        };
        match result.wait_for(Option::is_some).await {
            Ok(verdict) if self.generation.load(Ordering::Acquire) == generation => {
                verdict.unwrap_or(Authorization::Unavailable)
            }
            Err(_) => Authorization::Unavailable,
            _ => Authorization::Unavailable,
        }
    }

    // Register lets Account derive ownership from the voucher and daemon facts from headers only.
    async fn register(
        &self,
        client: &Client,
        state: &DaemonState,
        voucher: &str,
        generation: u64,
    ) -> Authorization {
        let request = crate::account_relay::daemon_request(
            client.post(endpoint(state, "/auth/app/register")),
            state,
        )
        .json(&VoucherBody { voucher });
        let response = match request.send().await {
            Ok(response) => response,
            Err(_) => return Authorization::Unavailable,
        };
        match response.status() {
            StatusCode::UNAUTHORIZED => {
                self.clear_grace().await;
                self.clear_cache().await;
                Authorization::Invalid
            }
            StatusCode::CONFLICT => {
                self.forget_grace(voucher).await;
                self.forget_cached(voucher).await;
                Authorization::Foreign
            }
            StatusCode::GONE => {
                self.clear_grace().await;
                self.clear_cache().await;
                Authorization::Unavailable
            }
            status if status.is_success() => {
                let Ok(registration) = response.json::<Registration>().await else {
                    return Authorization::Unavailable;
                };
                let Some(credential) = registration_credential(registration.app_credential) else {
                    return Authorization::Unavailable;
                };
                if state.accept_credential(credential.clone()).await.is_err() {
                    return Authorization::Unavailable;
                }
                if self.remember_grace(voucher, &credential, generation).await {
                    Authorization::Valid
                } else {
                    Authorization::Unavailable
                }
            }
            _ => Authorization::Unavailable,
        }
    }

    // Verify asks Account to compare the page voucher with the registered installation owner.
    async fn verify(
        &self,
        client: &Client,
        state: &DaemonState,
        voucher: &str,
        credential: &str,
        generation: u64,
    ) -> Authorization {
        let request = crate::account_relay::daemon_request(
            client
                .post(endpoint(state, "/auth/app/voucher/verify"))
                .bearer_auth(credential),
            state,
        )
        .json(&VoucherBody { voucher });
        let response = match request.send().await {
            Ok(response) => response,
            Err(_) => {
                return self
                    .recover_transient(state, voucher, credential, generation)
                    .await;
            }
        };
        match response.status() {
            StatusCode::UNAUTHORIZED => {
                self.clear_grace().await;
                self.clear_cache().await;
                self.retire(state, credential).await;
                Authorization::Invalid
            }
            StatusCode::GONE => {
                self.clear_grace().await;
                self.clear_cache().await;
                self.retire(state, credential).await;
                Authorization::Unavailable
            }
            status if status.is_success() => match response.json::<Verification>().await {
                Ok(Verification { valid: true, .. }) => {
                    if state.credential().await.as_deref() == Some(credential) {
                        if self.remember_grace(voucher, credential, generation).await {
                            Authorization::Valid
                        } else {
                            Authorization::Unavailable
                        }
                    } else {
                        self.clear_grace().await;
                        Authorization::Unavailable
                    }
                }
                Ok(Verification {
                    valid: false,
                    reason: Some(reason),
                }) if reason == "foreign" => {
                    self.forget_grace(voucher).await;
                    self.forget_cached(voucher).await;
                    Authorization::Foreign
                }
                Ok(Verification { valid: false, .. }) => {
                    self.forget_grace(voucher).await;
                    self.forget_cached(voucher).await;
                    Authorization::Invalid
                }
                Err(_) => {
                    self.recover_transient(state, voucher, credential, generation)
                        .await
                }
            },
            status
                if status.is_server_error()
                    || matches!(
                        status,
                        StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS
                    ) =>
            {
                self.recover_transient(state, voucher, credential, generation)
                    .await
            }
            _ => {
                self.clear_grace().await;
                self.clear_cache().await;
                Authorization::Unavailable
            }
        }
    }

    // RecoverTransient uses recent authority only while the installation credential is unchanged.
    async fn recover_transient(
        &self,
        state: &DaemonState,
        voucher: &str,
        credential: &str,
        generation: u64,
    ) -> Authorization {
        let Some(current) = state.credential().await else {
            self.clear_grace().await;
            return Authorization::Unavailable;
        };
        if self
            .last_good
            .lock()
            .await
            .accepts(voucher, &current, Instant::now())
            && current == credential
            && self.generation.load(Ordering::Acquire) == generation
            && voucher_deadline(voucher).is_some_and(|deadline| Instant::now() < deadline)
        {
            Authorization::Valid
        } else {
            Authorization::Unavailable
        }
    }

    // RememberGrace records one live Account acceptance.
    async fn remember_grace(&self, voucher: &str, credential: &str, generation: u64) -> bool {
        let mut grace = self.last_good.lock().await;
        if self.generation.load(Ordering::Acquire) != generation {
            return false;
        }
        grace.remember(voucher, credential, Instant::now());
        true
    }

    // ForgetGrace removes one explicitly rejected voucher.
    async fn forget_grace(&self, voucher: &str) {
        self.last_good.lock().await.forget(voucher);
    }

    // ClearGrace invalidates every cached acceptance after identity rejection or loss.
    async fn clear_grace(&self) {
        self.last_good.lock().await.clear();
    }

    // Cached returns a still-live verdict only for the exact credential that earned it.
    async fn cached(&self, voucher: &str, credential: &str) -> Option<Authorization> {
        let now = Instant::now();
        let mut cache = self.cache.lock().await;
        let verdict = cache.get(voucher).and_then(|entry| {
            (entry.credential == credential && now < entry.expires_at).then_some(entry.verdict)
        });
        if verdict.is_none() {
            cache.remove(voucher);
        }
        verdict
    }

    // CacheVerdict binds only Account's durable verdicts to the signed voucher expiration.
    async fn cache_verdict(
        &self,
        voucher: &str,
        credential: &str,
        verdict: Authorization,
        generation: u64,
    ) {
        if matches!(verdict, Authorization::Unavailable) {
            return;
        }
        let Some(expires_at) = voucher_deadline(voucher) else {
            return;
        };
        let mut cache = self.cache.lock().await;
        if self.generation.load(Ordering::Acquire) != generation {
            return;
        }
        let now = Instant::now();
        cache.retain(|_, entry| entry.expires_at > now);
        if cache.len() >= MAX_CACHED_VERDICTS
            && let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, entry)| entry.expires_at)
                .map(|(voucher, _)| voucher.clone())
        {
            cache.remove(&oldest);
        }
        cache.insert(
            voucher.to_owned(),
            CachedVerdict {
                credential: credential.to_owned(),
                verdict,
                expires_at,
            },
        );
    }

    // ForgetCached removes only one voucher after an explicit Account rejection.
    async fn forget_cached(&self, voucher: &str) {
        self.cache.lock().await.remove(voucher);
    }

    // ClearCache drops all verdicts after installation identity rejection or rotation.
    async fn clear_cache(&self) {
        self.cache.lock().await.clear();
    }

    // EndSession invalidates cached shell authority and prevents older checks from restoring it.
    pub(super) async fn end_session(&self) {
        let mut cache = self.cache.lock().await;
        let mut grace = self.last_good.lock().await;
        self.generation.fetch_add(1, Ordering::AcqRel);
        cache.clear();
        grace.clear();
    }

    // Retire clears only the exact Account credential proven gone.
    async fn retire(&self, state: &DaemonState, credential: &str) {
        let _ = state.retire_credential(credential).await;
    }
}

// RegistrationCredential admits only an exact value usable by every Account request.
fn registration_credential(credential: String) -> Option<String> {
    (!credential.is_empty()
        && credential.len() <= MAX_CREDENTIAL_BYTES
        && HeaderValue::from_str(&format!("Bearer {credential}")).is_ok())
    .then_some(credential)
}

// VoucherDeadline reads only the signed Account expiration before granting a cache lifetime.
fn voucher_deadline(voucher: &str) -> Option<Instant> {
    #[derive(Deserialize)]
    struct Payload {
        #[serde(rename = "expiresAt")]
        expires_at: u64,
    }
    let payload = voucher.split_once('.')?.0;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let payload = serde_json::from_slice::<Payload>(&bytes).ok()?;
    let expires = std::time::UNIX_EPOCH.checked_add(Duration::from_secs(payload.expires_at))?;
    let ttl = expires
        .duration_since(std::time::SystemTime::now())
        .ok()?
        .min(MAX_VOUCHER_TTL);
    (ttl > Duration::ZERO).then(|| Instant::now() + ttl)
}

// Endpoint joins only fixed Account paths to the validated Account origin.
fn endpoint(state: &DaemonState, path: &str) -> String {
    format!("{}{}", state.config.account_url, path)
}
