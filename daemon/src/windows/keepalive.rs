//! Keepalive registers a per-user scheduled task that relaunches ArchiGoat every minute after any death.

use std::os::windows::process::CommandExt as _;

// CREATE_NO_WINDOW stops the schtasks helper from flashing a console window on a background ArchiGoat.
const NO_WINDOW: u32 = 0x0800_0000;

/// Ensure idempotently registers the minute scheduled task that revives ArchiGoat after it exits.
pub(crate) fn ensure() {
    // An opted-out ArchiGoat owns no machine liveness, so the owner's task scheduler stays exactly as it was.
    if crate::keepalive::disabled() {
        crate::trace::line("keepalive skipped: ARCHIGOAT_KEEPALIVE=off");
        return;
    }
    // The task must relaunch exactly this installed binary, so its own path anchors the action.
    let exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            crate::trace::line(&format!(
                "keepalive skipped: current exe unavailable: {error}"
            ));
            return;
        }
    };
    // The run value keeps the quoted exe path and its autostart flag as one schtasks argument.
    let mut run = std::ffi::OsString::from("\"");
    run.push(exe.as_os_str());
    run.push("\" --autostart");
    // Registration replaces any prior task so repeated startups converge on one current definition.
    let outcome = std::process::Command::new("schtasks")
        .args([
            "/Create",
            "/F",
            "/TN",
            "ArchiGoat Keepalive",
            "/SC",
            "MINUTE",
            "/MO",
            "1",
            "/TR",
        ])
        .arg(&run)
        .creation_flags(NO_WINDOW)
        .status();
    // Any registration failure is logged for diagnosis but never stops the live ArchiGoat.
    match outcome {
        Ok(status) if status.success() => {}
        Ok(status) => crate::trace::line(&format!("keepalive registration exited: {status}")),
        Err(error) => crate::trace::line(&format!("keepalive registration failed: {error}")),
    }
}
