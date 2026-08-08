//! This interface sends CLI discovery and process ownership to the current operating-system Host.

use crate::{
    host,
    provider::{LocalCli, Provider},
};
use std::path::PathBuf;

pub(crate) use host::{Login, Output};

// Provider discovery follows the installed Host's executable and wrapper rules.
pub(crate) fn find(provider: Provider, extra: &[PathBuf]) -> Option<LocalCli> {
    host::find_cli(provider, extra)
}

// Health checks use the Host-owned process tree and return native output.
pub(crate) async fn capture(
    program: &LocalCli,
    args: &[String],
    seconds: u64,
) -> Result<Output, String> {
    host::capture_cli(program, args, None, seconds).await
}

// Catalog dialogues hold one protocol exchange open until the Provider answers.
pub(crate) async fn dialogue(
    program: &LocalCli,
    args: &[String],
    input: &str,
    finished: fn(&str) -> bool,
    seconds: u64,
) -> Result<String, String> {
    host::capture_dialogue(program, args, input, finished, seconds).await
}

// Provider login runs silently in the background; the Provider opens its own browser window.
pub(crate) fn spawn_login(program: &LocalCli, args: &[String]) -> Result<Login, String> {
    host::spawn_login(program, args)
}
