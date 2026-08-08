//! The ArchiGoat starts one authenticated loopback owner and owns native CLI execution.

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use archigoat::{Config, run_autostart, run_host_helper};
use std::env;

// Process failure remains local and never becomes a false Work terminal state.
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("ArchiGoat: {error}");
        std::process::exit(1);
    }
}

// Startup accepts only release verification, a host helper, or the one supervised daemon mode.
async fn run() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.as_slice() == ["--version"] {
        println!("{}", archigoat::version());
        return Ok(());
    }
    // Release verification binds the packaged binary to its declared version and commit.
    if args
        .first()
        .is_some_and(|value| value == "--verify-release")
    {
        if args.len() != 3
            || args[1] != archigoat::version()
            // A build without an embedded commit is no release, so verification refuses it whole.
            || archigoat::commit().is_empty()
            || args[2] != archigoat::commit()
        {
            return Err(
                "ArchiGoat release identity does not match its version and commit".to_owned(),
            );
        }
        return Ok(());
    }
    // Uninstall releases launchd ownership so a stopped ArchiGoat stays stopped until reinstalled.
    if args.as_slice() == ["--uninstall"] {
        return archigoat::uninstall().await;
    }
    if run_host_helper(&args).await? {
        return Ok(());
    }
    let config = Config::load()?;
    match args.as_slice() {
        [] => run_autostart(config).await,
        [value] if value == "--autostart" => run_autostart(config).await,
        _ => Err("ArchiGoat accepts only --autostart".to_owned()),
    }
}
