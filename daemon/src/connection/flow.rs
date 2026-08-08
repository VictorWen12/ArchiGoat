//! Connection flow keeps native Provider truth live without exposing internal observation failures.

use std::time::Duration;

use tokio::time::{sleep, timeout};

use super::{Authentication, authentication};
use crate::{
    host,
    provider::{LocalCli, Provider},
    state::{DaemonState, Phase},
};

/// A silent restore waits on no human, so it reaches a conclusion inside this bound.
const OBSERVE_BOUND: Duration = Duration::from_secs(30);

/// A finished browser sign-in reaches the app within this pause, so the wait stays short.
const SIGN_IN_PROBE: Duration = Duration::from_secs(2);

/// Run completes one owner-requested install or login flow using official Provider tools.
pub(crate) async fn run(state: DaemonState, provider: Provider, epoch: u64) {
    let Some(program) = ready_cli(&state, provider, epoch).await else {
        return;
    };
    let Some(observed) = observe_authentication(&state, provider, &program, epoch).await else {
        return;
    };
    match observed {
        Authentication::Authenticated => {
            connect(&state, provider, &program, epoch).await;
        }
        Authentication::SignedOut => authorize(&state, provider, &program, epoch).await,
        Authentication::Unavailable | Authentication::CannotStart => (),
    }
}

/// Observe restores a saved Provider only from explicit native CLI evidence, and never asks the owner for anything.
pub(crate) async fn observe(state: DaemonState, provider: Provider, epoch: u64) {
    match timeout(OBSERVE_BOUND, probe(&state, provider, epoch)).await {
        Ok(Some((program, Authentication::Authenticated))) => {
            connect(&state, provider, &program, epoch).await;
        }
        // A newer owner attempt already owns public truth, so this probe stays silent.
        Ok(None) => (),
        // Signed-out, unusable, and inconclusive native evidence all leave the saved Provider unproven.
        Ok(Some(_)) | Err(_) => {
            state.disconnect(epoch, provider).await;
        }
    }
}

/// Probe reads one native authentication fact for a saved Provider without opening a login terminal.
async fn probe(
    state: &DaemonState,
    provider: Provider,
    epoch: u64,
) -> Option<(LocalCli, Authentication)> {
    let program = ready_cli(state, provider, epoch).await?;
    let observed = observe_authentication(state, provider, &program, epoch).await?;
    Some((program, observed))
}

/// ReadyCli tries the official installation once, then keeps discovering while the owner still wants this Provider.
async fn ready_cli(state: &DaemonState, provider: Provider, epoch: u64) -> Option<LocalCli> {
    let mut pause = Duration::from_millis(250);
    // One attempt per request stops an unreachable installer from downloading forever.
    let mut attempted = false;
    loop {
        if let Some(program) = crate::cli::find(provider, &state.config.cli_dirs) {
            return Some(program);
        }
        if !attempted {
            attempted = true;
            tokio::select! {
                _ = host::install_cli(provider, state.config.install_timeout_secs) => {},
                _ = superseded(state, provider, epoch) => return None,
            }
            if let Some(program) = crate::cli::find(provider, &state.config.cli_dirs) {
                return Some(program);
            }
        }
        if !wait_or_superseded(state, provider, epoch, pause).await {
            return None;
        }
        pause = (pause * 2).min(Duration::from_secs(2));
    }
}

/// Superseded waits for a connection fact after arming its listener to avoid a lost wakeup.
async fn superseded(state: &DaemonState, provider: Provider, epoch: u64) {
    loop {
        let changed = state.connection_events.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        if !state.connection_current(epoch, provider).await {
            return;
        }
        changed.await;
    }
}

/// Authorize runs one official login silently and waits for explicit authentication.
async fn authorize(state: &DaemonState, provider: Provider, program: &LocalCli, epoch: u64) {
    // One Agent gets one sign-in window: an open one is kept, never replaced by a second.
    if !state.adopt_login(provider) {
        // The live flow parks on the daemon so a browser-issued code can reach its input.
        if let Ok(login) = crate::cli::spawn_login(program, &provider.login_args()) {
            state.park_login(provider, login);
        }
    }
    authorize_wait(state, provider, program, epoch).await;
}

/// AuthorizeWait probes for explicit authentication while the parked flow completes.
async fn authorize_wait(state: &DaemonState, provider: Provider, program: &LocalCli, epoch: u64) {
    // The person is waiting in the browser, so probes stay close together the whole time.
    let mut pause = Duration::from_millis(250);
    loop {
        if !wait_or_superseded(state, provider, epoch, pause).await {
            return;
        }
        let Some(observed) = observe_authentication(state, provider, program, epoch).await else {
            return;
        };
        if matches!(observed, Authentication::Authenticated) {
            connect(state, provider, program, epoch).await;
            return;
        }
        if matches!(observed, Authentication::SignedOut)
            && state.connection_current(epoch, provider).await
        {
            pause = (pause * 2).min(SIGN_IN_PROBE);
            continue;
        }
        if !state.connection_current(epoch, provider).await {
            return;
        }
        pause = (pause * 2).min(SIGN_IN_PROBE);
    }
}

/// ObserveAuthentication retries inconclusive native checks without converting them into public failure.
async fn observe_authentication(
    state: &DaemonState,
    provider: Provider,
    program: &LocalCli,
    epoch: u64,
) -> Option<Authentication> {
    let mut pause = Duration::from_millis(250);
    loop {
        let observed = tokio::select! {
            result = authentication(provider, program) => result,
            _ = superseded(state, provider, epoch) => return None,
        };
        if !matches!(
            observed,
            Authentication::Unavailable | Authentication::CannotStart
        ) {
            return Some(observed);
        }
        if !wait_or_superseded(state, provider, epoch, pause).await {
            return None;
        }
        pause = (pause * 2).min(Duration::from_secs(2));
    }
}

/// WaitOrSuperseded bounds retry churn while waking immediately when newer ownership arrives.
async fn wait_or_superseded(
    state: &DaemonState,
    provider: Provider,
    epoch: u64,
    pause: Duration,
) -> bool {
    tokio::select! {
        _ = sleep(pause) => state.connection_current(epoch, provider).await,
        _ = superseded(state, provider, epoch) => false,
    }
}

/// Connect publishes native authenticated evidence, then the Provider's catalog and published tiers.
async fn connect(state: &DaemonState, provider: Provider, program: &LocalCli, epoch: u64) {
    if !state
        .set_connection(epoch, Phase::Connected, provider)
        .await
    {
        return;
    }
    // A connected Agent has nothing left to sign in, so its login flow ends here.
    state.end_login(provider);
    // A connected Agent takes Work immediately, so reading its choices happens off the connection.
    let owner = state.clone();
    let program = program.clone();
    tokio::spawn(async move {
        let (models, presets) = tokio::join!(
            super::models::discover(provider, &program),
            super::presets::fetch(&owner, provider),
        );
        owner.publish_models(epoch, provider, models).await;
        owner.publish_presets(epoch, provider, presets).await;
    });
}
