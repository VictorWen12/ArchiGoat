//! The ArchiGoat turns authorized local and phone briefs into native Provider execution.

mod account_relay;
mod api;
mod cli;
mod config;
mod connection;
mod delivery;
mod execution;
// Keepalive binds macOS lifetime to the app and observes power state.
mod credential;
mod keepalive;
mod local;
mod process;
mod proof;
mod provider;
mod runtime;
mod state;
pub(crate) mod trace;
mod update;
pub mod work;
// Apple Work uses one headless background runner per Work.
#[cfg(target_os = "macos")]
pub(crate) mod apple;
#[cfg(target_os = "macos")]
pub(crate) use apple as host;
// Windows Work owns one isolated native process tree without Apple Terminal code.
#[cfg(target_os = "windows")]
pub(crate) mod windows;
#[cfg(target_os = "windows")]
pub(crate) use windows as host;
mod work_state;

pub use config::{Config, commit, version};
pub use provider::{LocalCli, Provider};
pub use runtime::run_autostart;
pub use state::DaemonState;

/// Platform helpers stay behind the selected host; the binary has no OS implementation branches.
pub async fn run_host_helper(args: &[String]) -> Result<bool, String> {
    host::run_helper(args).await
}

/// Uninstall stops this installation and removes everything it created, keeping only the owner's delivered artifacts.
#[cfg(target_os = "macos")]
pub async fn uninstall() -> Result<(), String> {
    // Releasing launchd first stops it reviving the ArchiGoat that the next step ends.
    keepalive::remove();
    // Account retirement happens before local identity deletion; an outage never strands local removal.
    apple::retire_remote().await;
    let _ = credential::clear_session();
    host::remove_installation()?;
    println!("ArchiGoat is removed; your delivered files are kept.");
    Ok(())
}

/// Windows installer removal clears only the shell bearer; MSI owns files and daemon state.
#[cfg(not(target_os = "macos"))]
pub async fn uninstall() -> Result<(), String> {
    let _ = credential::clear_session();
    Ok(())
}
