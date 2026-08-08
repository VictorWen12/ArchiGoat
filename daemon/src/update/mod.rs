//! Maintenance keeps the installed ArchiGoat current and removes only expired private Work state.

mod janitor;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod release;

// A quiet cadence catches alpha releases quickly without coupling maintenance to relay traffic.
use std::time::Duration;

// DaemonState carries the one admission gate and private storage owner shared by maintenance.
use crate::state::DaemonState;

/// Fifteen minutes bounds update delay while keeping the release feed cold.
const MAINTENANCE_PERIOD: Duration = Duration::from_secs(15 * 60);

/// Run checks immediately, then maintains the installation independently of registration.
pub(crate) async fn run(state: DaemonState) {
    loop {
        if let Err(error) = janitor::sweep(&state) {
            crate::trace::line(&format!("workspace sweep failed: {error}"));
        }
        #[cfg(target_os = "macos")]
        if let Err(error) = release::check(&state).await {
            crate::trace::line(&format!("update check failed: {error}"));
        }
        tokio::time::sleep(MAINTENANCE_PERIOD).await;
    }
}
