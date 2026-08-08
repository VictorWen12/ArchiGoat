//! Recovery returns every durable Running Work to its single lifecycle owner.

use crate::{state::DaemonState, work_state};

/// Spaced resumes keep a restart from relaunching every recovered Work's runner at once.
const RESUME_STAGGER: std::time::Duration = std::time::Duration::from_secs(2);

/// Resume reattaches launched runners and consumes only unclaimed signed admissions.
pub(super) async fn resume(state: DaemonState) {
    // Independent owners preserve parallel Work while each Work repairs serially.
    for (index, work) in state.recovery_candidates().into_iter().enumerate() {
        if index > 0 {
            tokio::time::sleep(RESUME_STAGGER).await;
        }
        let state = state.clone();
        tokio::spawn(async move { work_state::resume_admitted(state, work).await });
    }
}
