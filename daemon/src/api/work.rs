//! This module maps one Account Work identity to connection, execution, observation, and Stop.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    connection,
    provider::Provider,
    state::{DaemonState, RunSnapshot},
    work::WorkRequest,
};

/// StagedInput binds browser-declared attachment facts to one ArchiGoat-private exact-byte file.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StagedInput {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) media: String,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
    pub(crate) path: PathBuf,
}

/// Connect admits one Provider transition and keeps its ownership guard alive for the full check.
pub(crate) async fn connect(
    state: DaemonState,
    provider: Provider,
    model: Option<String>,
    effort: Option<String>,
) -> Result<(), String> {
    let (guard, epoch) = state.begin_connect(provider, model, effort).await?;
    tokio::spawn(async move {
        let _guard = guard;
        connection::run(state, provider, epoch).await;
    });
    Ok(())
}

/// Start hands the untouched request and verified attachment paths to the one Work state owner.
pub(crate) async fn start(
    state: DaemonState,
    work_id: String,
    request: WorkRequest,
    inputs: Vec<StagedInput>,
) -> Result<(), crate::work_state::StartRefusal> {
    state.start_work(work_id, request, inputs).await
}

/// Steer freezes one follow-up on the existing Running Work without minting another Work.
pub(crate) fn steer(
    state: &DaemonState,
    work_id: &str,
    steer_id: String,
    request: WorkRequest,
    inputs: Vec<StagedInput>,
) -> Result<bool, String> {
    state.steer_work(work_id, steer_id, request, inputs)
}

/// Observe returns the next public snapshot bound to the exact Work identity.
pub(crate) async fn observe(state: &DaemonState, work_id: &str) -> Option<RunSnapshot> {
    state.observe_work(work_id).await
}

/// Stop waits for cleanup of only the addressed Work before the API confirms success.
pub(crate) async fn stop(state: &DaemonState, work_id: &str) {
    state.stop_work(work_id).await;
}
