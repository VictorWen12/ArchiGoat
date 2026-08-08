#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args
        .first()
        .is_some_and(|value| value == "--verify-release")
    {
        let version = option_env!("ARCHIGOAT_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"));
        let commit = option_env!("ARCHIGOAT_COMMIT").unwrap_or_default();
        if args.len() == 3 && args[1] == version && !commit.is_empty() && args[2] == commit {
            return;
        }
        std::process::exit(1);
    }
    archigoat_shell_lib::run();
}
