//! Account relay backoff limits outage churn while local registration remains immediate.

use std::time::Duration;

use crate::DaemonState;

// Backoff limits outage churn while allowing local registration and Work to wake the relay.
pub(super) struct Backoff(Duration);

// Relay pacing keeps remote retries bounded without delaying newly available Work.
impl Backoff {
    // New relays retry quickly so available Account Work starts promptly.
    pub(super) fn new() -> Self {
        Self(Duration::from_millis(250))
    }

    // A successful exchange restores prompt polling for the next user action.
    pub(super) fn reset(&mut self) {
        self.0 = Duration::from_millis(250);
    }

    // Waiting bounds repeated failures but wakes immediately when relay state changes.
    pub(super) async fn wait(&mut self, state: &DaemonState) {
        tokio::select! {
            _ = state.relay_events.notified() => self.reset(),
            _ = state.registration_events.notified() => self.reset(),
            _ = tokio::time::sleep(self.0) => {
                self.0 = self.0.saturating_mul(2).min(Duration::from_secs(30));
            }
        }
    }
}
