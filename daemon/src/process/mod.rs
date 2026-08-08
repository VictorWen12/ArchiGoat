//! Provider events prove outcomes while recovery restores each Work's single lifecycle owner.

mod observe;
mod provider_events;
mod recover;
mod turn;

// Work ownership consumes the same observer contract for fresh and recovered runners.
pub(crate) use observe::{ObservedWork, Observer};
// One Provider answer folds into durable text by exactly one rule, wherever it is folded.
pub(crate) use observe::append_answer;
// Every surface reads the same stage word for a turn parked on its creator.
pub(crate) use provider_events::WAITING;

use crate::state::DaemonState;

/// One journal frame kind, numbered once for every platform, so a runner and the observer reading it
/// back can never disagree about which fact a frame carries.
pub(crate) const STDOUT: u8 = 1;
pub(crate) const STDERR: u8 = 2;
pub(crate) const DONE: u8 = 3;
pub(crate) const STOPPED: u8 = 4;

/// Resume asks durable state to reattach every unfinished Work in the background.
pub(crate) fn resume(state: DaemonState) {
    // Recovery must not delay ArchiGoat availability or create another foreground execution path.
    tokio::spawn(async move {
        recover::resume(state).await;
    });
}
