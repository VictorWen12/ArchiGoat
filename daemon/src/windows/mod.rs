//! Windows receives one Work command, runs it in one owned Console, and returns ordered Agent bytes.

mod active;
// Provider command support stays separate from Work execution.
mod command;
// Private file writes share one Windows security boundary.
mod file;
// Job objects own each Work process tree.
mod job;
// The journal preserves ordered Work output across ArchiGoat lifecycles.
mod journal;
// Keepalive re-registers the per-user task that revives a dead ArchiGoat.
pub(crate) mod keepalive;
// Liveness binds durable runner identity to a process-held kernel object.
mod liveness;
// The runner executes one signed Work inside its Job.
mod runner;
// Work bridges the shared lifecycle to the Windows runner.
mod work;

pub(crate) use active::State;
pub(crate) use command::{
    Login, Output, capture_cli, capture_dialogue, find_cli, install_cli, spawn_login,
};
pub(crate) use file::{create_private_dir, linked, make_writable, replace_private};
pub(crate) use work::{AgentRun, launch, reattach};

use std::path::{Path, PathBuf};

/// Keeps ArchiGoat state in the current Windows account's application data.
pub(crate) fn default_state_file() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .and_then(|root| {
            let current = root.join("ArchiGoat");
            let legacy = root.join(concat!("Pl", "ugin"));
            let adopted = crate::config::adopt_legacy_state(
                &legacy,
                &current,
                concat!("pl", "ugin.json"),
                "archigoat.json",
            );
            if !adopted {
                return crate::config::legacy_state_file(&legacy, concat!("pl", "ugin.json"));
            }
            crate::config::state_file(&current, "archigoat.json")
        })
}

/// The PC reports the computer name Windows already shows its owner.
pub(crate) fn machine_name() -> Option<String> {
    std::env::var("COMPUTERNAME").ok()
}

/// Runs only a valid Windows Work helper invocation.
pub(crate) async fn run_helper(args: &[String]) -> Result<bool, String> {
    if args.first().is_some_and(|value| value == "--windows-work") {
        let path = args
            .get(1)
            .filter(|_| args.len() == 2)
            .ok_or_else(|| "Windows Work helper arguments are invalid".to_owned())?;
        runner::run(Path::new(path)).await?;
        return Ok(true);
    }
    Ok(false)
}
