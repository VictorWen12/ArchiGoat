//! Account transport independently receives jobs, publishes status, and delivers results.

mod backoff;
mod delivery;
mod frozen;

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use reqwest::{Client, RequestBuilder, StatusCode, header::HeaderValue};
use serde::Serialize;
use tokio::sync::{Mutex, Notify, Semaphore, watch};
use tokio::task::JoinSet;

use crate::{DaemonState, api, state::RunPhase};
use backoff::Backoff;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
// A beat bounded well under the Account's 45-second presence window leaves room for one slow or
// failed send before a live computer would read as away.
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(10);
const EMPTY_POLL_INTERVAL: Duration = Duration::from_millis(500);
const ACCOUNT_LEASE_DURATION: Duration = Duration::from_secs(60);
const LOCAL_LEASE_DURATION: Duration = Duration::from_secs(ACCOUNT_LEASE_DURATION.as_secs() - 5);
const LEASE_RENEW_INTERVAL: Duration = Duration::from_secs(20);
const LEASE_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) const METADATA_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const STREAM_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_EXECUTORS: usize = 4;
const MAX_PROGRESS_POSTS: usize = 4;

// MachineName supplies the stable human-visible computer fact Account displays.
pub(crate) fn machine_name() -> Option<String> {
    static NAME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        crate::host::machine_name()
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty() && name.len() <= 255)
            .filter(|name| HeaderValue::from_str(name).is_ok())
    })
    .clone()
}

// DaemonRequest proves the exact installation and live runtime on every Account call.
pub(crate) fn daemon_request(request: RequestBuilder, state: &DaemonState) -> RequestBuilder {
    let request = request
        .header("x-app-device", state.device_id())
        .header("x-app-instance", state.runtime_id())
        .header("x-app-version", crate::config::version())
        .header("x-app-protocol", api::PROTOCOL.to_string());
    match machine_name() {
        Some(host) => request.header("x-app-host", host),
        None => request,
    }
}

// AuthorizedRequest adds the durable ArchiGoat credential to daemon identity.
pub(super) fn authorized_request(
    request: RequestBuilder,
    state: &DaemonState,
    credential: &str,
) -> RequestBuilder {
    daemon_request(request.bearer_auth(credential), state)
}

// Job carries one exact leased phone order without generic action bytes.
#[derive(Clone)]
struct Job {
    id: String,
    lease: String,
    kind: String,
    work: String,
    turn: Option<i64>,
    deadline: Arc<StdMutex<tokio::time::Instant>>,
}

// LeaseReceipt renews or settles one exact Account lease.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LeaseReceipt<'a> {
    job_id: &'a str,
    lease_id: &'a str,
}

// Settlement ends one exact lease as accepted or failed.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Settlement<'a> {
    job_id: &'a str,
    lease_id: &'a str,
    terminal: &'a str,
    reason: Option<&'a str>,
}

// ReceiptResult distinguishes durable acceptance from terminal stale reconciliation.
enum ReceiptResult {
    Accepted,
    Stale,
}

// ReceiptDisposition separates terminal reconciliation from genuinely transient retry.
#[derive(Debug, Eq, PartialEq)]
enum ReceiptDisposition {
    Accepted,
    Terminal,
    Retry,
}

// WorkOrders serializes commands per Work while unrelated Work executes concurrently.
type WorkOrders = Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>;

// Progress publishes the latest durable native Work snapshot.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress<'a> {
    work_id: &'a str,
    snapshot: &'a crate::state::RunSnapshot,
}

// ProgressPost returns one Account attempt to its per-Work publication queue.
struct ProgressPost {
    work: String,
    credential: String,
    encoded: String,
    phase: RunPhase,
    status: Result<StatusCode, String>,
}

// ProgressPosts bounds Account traffic while serializing each Work independently.
struct ProgressPosts {
    sent: HashMap<String, (String, tokio::time::Instant)>,
    active: HashSet<String>,
    posts: JoinSet<ProgressPost>,
}

// PublicationDue renews only physically live Running; parked attention publishes once on change.
fn publication_due(
    sent: Option<&(String, tokio::time::Instant)>,
    encoded: &str,
    now: tokio::time::Instant,
    renew: bool,
) -> bool {
    sent.is_none_or(|(body, at)| {
        body != encoded || (renew && now.saturating_duration_since(*at) >= HEARTBEAT_INTERVAL)
    })
}

impl ProgressPosts {
    fn new() -> Self {
        Self {
            sent: HashMap::new(),
            active: HashSet::new(),
            posts: JoinSet::new(),
        }
    }

    // Spawn admits one changed Work only when its prior publication has ended.
    fn spawn(
        &mut self,
        state: &DaemonState,
        client: &Client,
        credential: &str,
        work: String,
        mut snapshot: crate::state::RunSnapshot,
    ) -> bool {
        // A Work whose turn cannot be admitted yet never pays for its conversation to be copied.
        if self.posts.len() >= MAX_PROGRESS_POSTS || self.active.contains(&work) {
            return false;
        }
        // The phone renders the same conversation this desktop shows.
        snapshot.events = state.work_conversation(&work);
        let encoded = match serde_json::to_string(&snapshot) {
            Ok(encoded) => encoded,
            Err(error) => {
                eprintln!("Product could not publish Work {work}: {error}");
                return false;
            }
        };
        let renew = !snapshot
            .progress
            .as_ref()
            .is_some_and(|progress| is_attention(&progress.text));
        if !publication_due(
            self.sent.get(&work),
            &encoded,
            tokio::time::Instant::now(),
            renew,
        ) {
            return false;
        }
        self.active.insert(work.clone());
        let state = state.clone();
        let client = client.clone();
        let credential = credential.to_owned();
        self.posts.spawn(async move {
            let status = authorized_request(
                client.post(endpoint(&state, "/auth/app/work/progress")),
                &state,
                &credential,
            )
            .json(&Progress {
                work_id: &work,
                snapshot: &snapshot,
            })
            .timeout(METADATA_TIMEOUT)
            .send()
            .await
            .map(|response| response.status())
            .map_err(network);
            ProgressPost {
                work,
                credential,
                encoded,
                phase: snapshot.phase,
                status,
            }
        });
        true
    }

    // JoinNext releases one Work lane only after Account ended its prior request.
    async fn join_next(&mut self, state: &DaemonState) -> Option<String> {
        let post = match self.posts.join_next().await? {
            Ok(post) => post,
            Err(error) => {
                eprintln!("Product progress worker failed: {error}");
                return None;
            }
        };
        self.active.remove(&post.work);
        match post.status {
            Ok(status) if status.is_success() => {
                if matches!(post.phase, RunPhase::Stopped | RunPhase::Failed) {
                    if let Err(error) = state.acknowledge_work(&post.work) {
                        eprintln!("Product terminal cleanup retry: {error}");
                    }
                } else {
                    self.sent.insert(
                        post.work.clone(),
                        (post.encoded, tokio::time::Instant::now()),
                    );
                }
            }
            Ok(status) => eprintln!(
                "Product progress retry: {}",
                rejected(state, &post.credential, status, "Work progress").await
            ),
            Err(error) => eprintln!("Product progress retry: {error}"),
        }
        Some(post.work)
    }

    fn is_empty(&self) -> bool {
        self.posts.is_empty()
    }
}

/// Run starts independent outbound workers; slow delivery never deafens new phone orders.
pub(crate) async fn run(state: DaemonState) {
    let client = match client() {
        Ok(client) => client,
        Err(error) => {
            eprintln!("Product Account client unavailable: {error}");
            return;
        }
    };
    tokio::spawn(heartbeat(state.clone(), client.clone()));
    tokio::spawn(progress_worker(state.clone(), client.clone()));
    tokio::spawn(delivery_worker(state.clone(), client.clone()));
    receive_worker(
        state,
        client,
        Arc::new(Semaphore::new(MAX_EXECUTORS)),
        Arc::new(Mutex::new(HashMap::new())),
    )
    .await;
}

/// DeliverLocal streams one completed local Work through the shared Account delivery path.
pub(crate) async fn deliver_local(
    state: &DaemonState,
    account_token: &str,
    work_id: &str,
    delivery_id: &str,
    scope_kind: &str,
    scope_id: &str,
) -> Result<(), String> {
    let client = client()?;
    let snapshot = state
        .run_snapshot(work_id)
        .ok_or_else(|| "Work is unavailable".to_owned())?;
    delivery::send_local(
        &client,
        state,
        account_token,
        work_id,
        &snapshot,
        delivery::LocalDelivery {
            delivery_id,
            scope_kind,
            scope_id,
        },
    )
    .await
}

// ReceiveWorker keeps outbound polling independent from execution and delivery.
async fn receive_worker(
    state: DaemonState,
    client: Client,
    executors: Arc<Semaphore>,
    orders: WorkOrders,
) {
    let mut backoff = Backoff::new();
    let mut wake_generation = 0;
    let mut wake_pending = false;
    loop {
        if crate::keepalive::wake_changed(&mut wake_generation) {
            wake_pending = true;
        }
        if wake_pending {
            wake_pending = !state.resume_transport_attention_after_wake();
        }
        let Some(credential) = state.credential().await else {
            wait_for_registration(&state).await;
            continue;
        };
        // Polling never waits on an executor: a queued job is leased the moment it lands, and its
        // renewal keeps the lease alive while the job waits for an execution slot.
        match receive(&client, &state, &credential).await {
            Ok(Some(job)) => {
                tokio::spawn(execution_worker(
                    state.clone(),
                    client.clone(),
                    credential,
                    job,
                    executors.clone(),
                    orders.clone(),
                ));
                backoff.reset();
            }
            Ok(None) => {
                backoff.reset();
                tokio::time::sleep(EMPTY_POLL_INTERVAL).await;
            }
            Err(error) => {
                eprintln!("Product job receive retry: {error}");
                backoff.wait(&state).await;
            }
        }
    }
}

// IsAttention keeps renewal paused for both the bare and cause-prefixed public label.
fn is_attention(text: &str) -> bool {
    text == "Needs attention" || text.starts_with("Needs attention — ")
}

// ExecutionWorker owns one bounded lease through renewal, native execution, and settlement.
async fn execution_worker(
    state: DaemonState,
    client: Client,
    credential: String,
    job: Job,
    executors: Arc<Semaphore>,
    orders: WorkOrders,
) {
    let order = {
        let mut active = orders.lock().await;
        active
            .entry(job.work.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    let ordered = order.lock().await;
    let stale = Arc::new(AtomicBool::new(false));
    let stale_event = Arc::new(Notify::new());
    let (cancel, cancelled) = watch::channel(false);
    let renewing = tokio::spawn(renew_worker(
        state.clone(),
        client.clone(),
        credential.clone(),
        job.clone(),
        cancelled,
        stale.clone(),
        stale_event.clone(),
    ));
    // Renewal already owns the lease, so waiting for an execution slot loses nothing; a lease
    // that goes stale during the wait releases the job without ever starting it.
    let admitted = tokio::select! {
        permit = executors.clone().acquire_owned() => permit.ok(),
        _ = stale_event.notified() => None,
    };
    let outcome = match admitted {
        Some(_permit) if !stale.load(Ordering::Acquire) => {
            let execution = execute(&client, &state, &credential, &job);
            tokio::pin!(execution);
            tokio::select! {
                outcome = &mut execution => Some(outcome),
                _ = stale_event.notified() => None,
            }
        }
        _ => None,
    };
    if let Some(outcome) = outcome {
        let (terminal, reason) = match outcome {
            Ok(()) => ("accepted", None),
            Err(error) => (terminal_for(&error), Some(error)),
        };
        settle_until_terminal(
            &client,
            &state,
            &credential,
            &job,
            terminal,
            reason.as_deref(),
            &stale,
        )
        .await;
    }
    let _ = cancel.send(true);
    let _ = renewing.await;
    drop(ordered);
    let mut active = orders.lock().await;
    if active
        .get(&job.work)
        .is_some_and(|current| Arc::ptr_eq(current, &order) && Arc::strong_count(current) == 2)
    {
        active.remove(&job.work);
    }
}

// RenewWorker keeps a live lease valid during frozen transfer and native execution.
async fn renew_worker(
    state: DaemonState,
    client: Client,
    credential: String,
    job: Job,
    mut cancelled: watch::Receiver<bool>,
    stale: Arc<AtomicBool>,
    stale_event: Arc<Notify>,
) {
    let Some(mut delay) = renewal_initial_delay(lease_remaining(&job)) else {
        stale.store(true, Ordering::Release);
        stale_event.notify_waiters();
        return;
    };
    loop {
        tokio::select! {
            changed = cancelled.changed() => {
                if changed.is_err() || *cancelled.borrow() { return; }
            }
            _ = tokio::time::sleep(delay) => {}
        }
        match renew(&client, &state, &credential, &job).await {
            Ok(ReceiptResult::Accepted) => {
                delay = LEASE_RENEW_INTERVAL;
            }
            Ok(ReceiptResult::Stale) => {
                stale.store(true, Ordering::Release);
                stale_event.notify_waiters();
                return;
            }
            Err(error) => {
                eprintln!("Product lease renewal retry: {error}");
                let Some(retry) = renewal_retry_delay(lease_remaining(&job)) else {
                    stale.store(true, Ordering::Release);
                    stale_event.notify_waiters();
                    return;
                };
                delay = retry;
            }
        }
    }
}

// Receive leases the next exact job for this authenticated installation.
async fn receive(
    client: &Client,
    state: &DaemonState,
    credential: &str,
) -> Result<Option<Job>, String> {
    let requested_at = tokio::time::Instant::now();
    let response = authorized_request(
        client.get(endpoint(state, "/auth/app/jobs/next")),
        state,
        credential,
    )
    .timeout(LEASE_REQUEST_TIMEOUT)
    .send()
    .await
    .map_err(network)?;
    if response.status() == StatusCode::NO_CONTENT {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(rejected(state, credential, response.status(), "job poll").await);
    }
    let headers = response.headers();
    Ok(Some(Job {
        id: required(headers, "x-work-job-id")?,
        lease: required(headers, "x-work-lease-id")?,
        kind: required(headers, "x-work-kind")?,
        work: required(headers, "x-work-id")?,
        turn: optional(headers, "x-work-turn-id")?
            .map(|value| value.parse())
            .transpose()
            .map_err(|_| "Account sent invalid turn identity".to_owned())?,
        deadline: Arc::new(StdMutex::new(local_lease_deadline(requested_at))),
    }))
}

// Renew extends one live lease; conflict means another runtime owns reconciliation.
async fn renew(
    client: &Client,
    state: &DaemonState,
    credential: &str,
    job: &Job,
) -> Result<ReceiptResult, String> {
    let requested_at = tokio::time::Instant::now();
    if !lease_request_fits(lease_deadline(job), requested_at) {
        return Ok(ReceiptResult::Stale);
    }
    let response = authorized_request(
        client.post(endpoint(state, "/auth/app/jobs/renew")),
        state,
        credential,
    )
    .json(&LeaseReceipt {
        job_id: &job.id,
        lease_id: &job.lease,
    })
    .timeout(LEASE_REQUEST_TIMEOUT)
    .send()
    .await
    .map_err(network)?;
    if tokio::time::Instant::now() >= lease_deadline(job) {
        return Ok(ReceiptResult::Stale);
    }
    let result = receipt(state, credential, response.status(), "job renewal").await?;
    if matches!(result, ReceiptResult::Accepted) {
        confirm_lease(job, requested_at);
    }
    Ok(result)
}

// Execute maps the three mailbox job kinds to native Work authority.
async fn execute(
    client: &Client,
    state: &DaemonState,
    credential: &str,
    job: &Job,
) -> Result<(), String> {
    match job.kind.as_str() {
        "start" => frozen::start(client, state, credential, &job.id, &job.work).await,
        "steer" => {
            let turn = job
                .turn
                .ok_or_else(|| "Account omitted steer turn".to_owned())?;
            frozen::steer(client, state, credential, &job.id, &job.work, turn)
                .await
                .and_then(|accepted| {
                    accepted
                        .then_some(())
                        .ok_or_else(|| "Work is no longer running".to_owned())
                })
        }
        "stop" => {
            api::work::stop(state, &job.work).await;
            Ok(())
        }
        _ => Err("Account sent an invalid job kind".to_owned()),
    }
}

// SettleUntilTerminal retries transient delivery but releases stale receipts immediately.
async fn settle_until_terminal(
    client: &Client,
    state: &DaemonState,
    credential: &str,
    job: &Job,
    terminal: &str,
    reason: Option<&str>,
    stale: &AtomicBool,
) {
    let mut backoff = Backoff::new();
    while !stale.load(Ordering::Acquire) {
        match settle(client, state, credential, job, terminal, reason).await {
            Ok(ReceiptResult::Accepted | ReceiptResult::Stale) => return,
            Err(error) => {
                eprintln!("Product job settlement retry: {error}");
                backoff.wait(state).await;
            }
        }
    }
}

// Settle persists one terminal lease result in Account.
async fn settle(
    client: &Client,
    state: &DaemonState,
    credential: &str,
    job: &Job,
    terminal: &str,
    reason: Option<&str>,
) -> Result<ReceiptResult, String> {
    let requested_at = tokio::time::Instant::now();
    if !lease_request_fits(lease_deadline(job), requested_at) {
        return Ok(ReceiptResult::Stale);
    }
    let response = authorized_request(
        client.post(endpoint(state, "/auth/app/jobs/settle")),
        state,
        credential,
    )
    .json(&Settlement {
        job_id: &job.id,
        lease_id: &job.lease,
        terminal,
        reason,
    })
    .timeout(LEASE_REQUEST_TIMEOUT)
    .send()
    .await
    .map_err(network)?;
    if tokio::time::Instant::now() >= lease_deadline(job) {
        return Ok(ReceiptResult::Stale);
    }
    receipt(state, credential, response.status(), "job settlement").await
}

// Receipt treats a stale lease as terminal local reconciliation, never a retry loop.
async fn receipt(
    state: &DaemonState,
    credential: &str,
    status: StatusCode,
    context: &str,
) -> Result<ReceiptResult, String> {
    match receipt_disposition(status) {
        ReceiptDisposition::Accepted => Ok(ReceiptResult::Accepted),
        ReceiptDisposition::Terminal => {
            let reason = rejected(state, credential, status, context).await;
            eprintln!("Product terminal lease reconciled: {reason}");
            Ok(ReceiptResult::Stale)
        }
        ReceiptDisposition::Retry => Err(rejected(state, credential, status, context).await),
    }
}

// ReceiptDisposition retries only server pressure and server failure.
fn receipt_disposition(status: StatusCode) -> ReceiptDisposition {
    if status.is_success() {
        ReceiptDisposition::Accepted
    } else if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        ReceiptDisposition::Retry
    } else {
        ReceiptDisposition::Terminal
    }
}

// RenewalRetryDelay refuses any attempt whose full timeout could cross lease expiry.
fn renewal_retry_delay(remaining: Duration) -> Option<Duration> {
    renewal_safe_window(remaining).map(|safe| safe.min(Duration::from_secs(5)))
}

// RenewalInitialDelay accounts for time spent waiting on the per-Work execution lane.
fn renewal_initial_delay(remaining: Duration) -> Option<Duration> {
    renewal_safe_window(remaining).map(|safe| safe.min(LEASE_RENEW_INTERVAL))
}

// RenewalSafeWindow reserves enough lease lifetime for one full metadata request.
fn renewal_safe_window(remaining: Duration) -> Option<Duration> {
    remaining
        .checked_sub(LEASE_REQUEST_TIMEOUT)
        .filter(|safe| !safe.is_zero())
}

// LocalLeaseDeadline anchors confirmed ownership before network round-trip time begins.
fn local_lease_deadline(requested_at: tokio::time::Instant) -> tokio::time::Instant {
    requested_at + LOCAL_LEASE_DURATION
}

// LeaseDeadline reads the latest confirmed server round trip without mutating it.
fn lease_deadline(job: &Job) -> tokio::time::Instant {
    *job.deadline
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// LeaseRemaining never represents expired ownership as live time.
fn lease_remaining(job: &Job) -> Duration {
    lease_deadline(job).saturating_duration_since(tokio::time::Instant::now())
}

// ConfirmLease advances ownership only from the exact renewal request start.
fn confirm_lease(job: &Job, requested_at: tokio::time::Instant) {
    *job.deadline
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = local_lease_deadline(requested_at);
}

// LeaseRequestFits requires one full request timeout before local ownership ends.
fn lease_request_fits(deadline: tokio::time::Instant, requested_at: tokio::time::Instant) -> bool {
    deadline.saturating_duration_since(requested_at) > LEASE_REQUEST_TIMEOUT
}

// TerminalFor retries operational admission failures but permanently rejects invalid frozen truth.
fn terminal_for(error: &str) -> &'static str {
    let error = error.to_ascii_lowercase();
    if ["invalid", "changed", "omitted", "unknown"]
        .iter()
        .any(|marker| error.contains(marker))
    {
        "failed"
    } else {
        "retryable"
    }
}

// ProgressWorker pushes only changed durable snapshots.
async fn progress_worker(state: DaemonState, client: Client) {
    let mut posts = ProgressPosts::new();
    loop {
        // Register before scanning so admission cannot disappear into the scan-to-sleep window.
        let changed = state.work_events.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        let Some(credential) = state.credential().await else {
            wait_for_registration(&state).await;
            continue;
        };
        for (work, snapshot) in state.remote_snapshots() {
            posts.spawn(&state, &client, &credential, work, snapshot);
        }
        if posts.is_empty() {
            let _ = tokio::time::timeout(HEARTBEAT_INTERVAL, changed).await;
            continue;
        }
        tokio::select! {
            _ = posts.join_next(&state) => {}
            _ = changed => {}
        }
    }
}

// DeliveryWorker transfers verified terminal products independently from command receipt.
async fn delivery_worker(state: DaemonState, client: Client) {
    let mut active = HashSet::new();
    let mut deliveries = JoinSet::new();
    loop {
        // The listener is enabled before the scan reads the candidates, so a Work that finishes
        // between the two is announced to a listener that already exists. Registering afterwards
        // drops that announcement and leaves the finished product sitting out the whole timeout.
        let changed = state.relay_events.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        for (work, _) in state.remote_delivery_candidates() {
            if !active.insert(work.clone()) {
                continue;
            }
            deliveries.spawn(deliver_work(state.clone(), client.clone(), work));
        }
        if deliveries.is_empty() {
            let _ = tokio::time::timeout(Duration::from_secs(10), changed).await;
            continue;
        }
        tokio::select! {
            joined = deliveries.join_next() => {
                if let Some(Ok(work)) = joined { active.remove(&work); }
            }
            _ = changed => {}
        }
    }
}

// DeliverWork retries one exact remote result without delaying another Work.
async fn deliver_work(state: DaemonState, client: Client, work: String) -> String {
    let mut backoff = Backoff::new();
    loop {
        let Some(credential) = state.credential().await else {
            wait_for_registration(&state).await;
            continue;
        };
        let snapshot = state
            .remote_delivery_candidates()
            .into_iter()
            .find_map(|(candidate, snapshot)| (candidate == work).then_some(snapshot));
        let Some(snapshot) = snapshot else {
            return work;
        };
        match delivery::send(&client, &state, &credential, &work, &snapshot).await {
            Ok(()) => {
                let _ = state.acknowledge_work(&work);
                return work;
            }
            Err(error) => {
                eprintln!("Product Account delivery retry: {error}");
                backoff.wait(&state).await;
            }
        }
    }
}

// Heartbeat publishes presence without participating in idle mailbox reads.
async fn heartbeat(state: DaemonState, client: Client) {
    loop {
        let Some(credential) = state.credential().await else {
            wait_for_registration(&state).await;
            continue;
        };
        let mut request = authorized_request(
            client.post(endpoint(&state, "/auth/app/heartbeat")),
            &state,
            &credential,
        );
        if let Some((provider, _)) = state.connected_provider().await {
            request = request.header("x-agent-provider", provider.to_string());
        }
        request = request.timeout(HEARTBEAT_TIMEOUT);
        match request.send().await {
            Ok(response) if response.status().is_success() => {}
            Ok(response) => eprintln!(
                "Product heartbeat retry: {}",
                rejected(&state, &credential, response.status(), "heartbeat").await
            ),
            Err(error) => eprintln!("Product heartbeat retry: {}", network(error)),
        }
        tokio::time::sleep(HEARTBEAT_INTERVAL).await;
    }
}

// WaitForRegistration sleeps without polling until a credential becomes available.
async fn wait_for_registration(state: &DaemonState) {
    let event = state.registration_events.notified();
    tokio::pin!(event);
    event.as_mut().enable();
    if state.credential().await.is_none() {
        event.await;
    }
}

// Client uses the Account endpoint already validated once at configuration load.
fn client() -> Result<Client, String> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(network)
}

// Endpoint resolves one fixed Account route against configured origin.
pub(super) fn endpoint(state: &DaemonState, path: &str) -> String {
    format!("{}{}", state.config.account_url, path)
}

// Rejected retires credentials only when Account declares them gone.
pub(super) async fn rejected(
    state: &DaemonState,
    credential: &str,
    status: StatusCode,
    context: &str,
) -> String {
    if status == StatusCode::GONE {
        let _ = state.retire_credential(credential).await;
    }
    format!("Account rejected {context} with {status}")
}

// Required reads one mandatory authenticated response header.
fn required(headers: &reqwest::header::HeaderMap, name: &str) -> Result<String, String> {
    optional(headers, name)?.ok_or_else(|| format!("Account omitted {name}"))
}

// Optional reads one valid response header without inventing a value.
fn optional(headers: &reqwest::header::HeaderMap, name: &str) -> Result<Option<String>, String> {
    headers
        .get(name)
        .map(|value| {
            value
                .to_str()
                .map(str::to_owned)
                .map_err(|_| format!("Account sent invalid {name}"))
        })
        .transpose()
}

// Network keeps transport failures distinct from Account rejection.
pub(super) fn network(error: reqwest::Error) -> String {
    // The detail stays in the local log; the served sentence is one a person can act on.
    eprintln!("Product could not reach Account: {error}");
    "TrianGoat is unreachable from this computer. Check its connection and try again.".to_owned()
}
