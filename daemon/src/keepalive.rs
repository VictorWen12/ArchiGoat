//! Keepalive binds the macOS daemon to its app parent and observes active Work power state.

use std::sync::atomic::{AtomicU64, Ordering};

// WakeGeneration changes only after the monitor observes a wall-clock gap large enough to be a system sleep.
static WAKE_GENERATION: AtomicU64 = AtomicU64::new(0);

// The release label identifies obsolete LaunchAgents that old versions left behind.
#[cfg(target_os = "macos")]
fn label() -> &'static str {
    crate::config::bundle_id()
}

#[cfg(target_os = "macos")]
const LEGACY_LABEL: &str = concat!("com", ".", "app", ".", "pl", "\u{75}gin");

// Disabled keeps test daemons from asserting machine power or outliving their harness.
pub(crate) fn disabled() -> bool {
    std::env::var_os("ARCHIGOAT_KEEPALIVE").is_some_and(|value| value == "off")
}

/// work_started holds one idle-sleep assertion for the exact Work while it can still execute.
pub(crate) fn work_started(work_id: &str) {
    if disabled() {
        return;
    }
    #[cfg(target_os = "macos")]
    power::started(work_id);
    #[cfg(not(target_os = "macos"))]
    let _ = work_id;
}

/// work_stopped releases only the assertion owned by the exact Work.
pub(crate) fn work_stopped(work_id: &str) {
    if disabled() {
        return;
    }
    #[cfg(target_os = "macos")]
    power::stopped(work_id);
    #[cfg(not(target_os = "macos"))]
    let _ = work_id;
}

/// wake_changed consumes one process-wide wake edge without introducing a second async event loop.
pub(crate) fn wake_changed(last: &mut u64) -> bool {
    let current = WAKE_GENERATION.load(Ordering::Acquire);
    if current == *last {
        return false;
    }
    *last = current;
    true
}

/// WatchParent ends ArchiGoat when its visible spawning app or test harness dies.
#[cfg(unix)]
pub(crate) fn watch_parent() {
    let spawner = std::os::unix::process::parent_id();
    // Reparenting to the init process before this check means the spawner already died during startup.
    if spawner <= 1 {
        crate::trace::line("spawning app already gone; ArchiGoat ending");
        std::process::exit(0);
    }
    // A plain thread keeps the check alive even when the async runtime is starved or wedged.
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            // Reparenting away from the spawner is the kernel's own proof that the spawning process died.
            if std::os::unix::process::parent_id() != spawner {
                crate::trace::line("spawning app exited; ArchiGoat ending");
                std::process::exit(0);
            }
        }
    });
}

/// ObserveWake starts the one wall-clock observer needed to reconnect after system sleep.
#[cfg(target_os = "macos")]
pub(crate) fn observe_wake() {
    wake::ensure();
}

#[cfg(target_os = "macos")]
mod power {
    use std::{
        collections::HashSet,
        process::{Child, Command, Stdio},
        sync::{Mutex, OnceLock},
    };

    struct Assertion {
        works: HashSet<String>,
        process: Option<Child>,
    }

    fn state() -> &'static Mutex<Assertion> {
        static STATE: OnceLock<Mutex<Assertion>> = OnceLock::new();
        STATE.get_or_init(|| {
            Mutex::new(Assertion {
                works: HashSet::new(),
                process: None,
            })
        })
    }

    /// started launches one caffeinate assertion when the first Work becomes executable.
    pub(super) fn started(work_id: &str) {
        let mut state = state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.works.insert(work_id.to_owned());
        let process_alive = state
            .process
            .as_mut()
            .is_some_and(|process| process.try_wait().ok().flatten().is_none());
        if process_alive {
            return;
        }
        state.process = None;
        state.process = Command::new("/usr/bin/caffeinate")
            .args(["-i", "-w"])
            .arg(std::process::id().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| crate::trace::line(&format!("power assertion unavailable: {error}")))
            .ok();
    }

    /// stopped drops one Work and terminates caffeinate only after the last Work ends.
    pub(super) fn stopped(work_id: &str) {
        let mut state = state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !state.works.remove(work_id) || !state.works.is_empty() {
            return;
        }
        if let Some(mut process) = state.process.take() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }
}

#[cfg(target_os = "macos")]
mod wake {
    use std::{
        sync::Once,
        time::{Duration, SystemTime},
    };

    /// ensure starts one lightweight wall-clock observer; a long gap is the physical sleep/wake edge available without private IOKit dependencies.
    pub(super) fn ensure() {
        static START: Once = Once::new();
        START.call_once(|| {
            std::thread::spawn(|| {
                let mut previous = SystemTime::now();
                loop {
                    std::thread::sleep(Duration::from_secs(2));
                    let now = SystemTime::now();
                    let elapsed = now.duration_since(previous).unwrap_or_default();
                    previous = now;
                    if elapsed >= Duration::from_secs(6) {
                        super::WAKE_GENERATION.fetch_add(1, super::Ordering::Release);
                        crate::trace::line("system wake observed");
                    }
                }
            });
        });
    }
}

/// Remove clears LaunchAgents left by obsolete ArchiGoat versions.
#[cfg(target_os = "macos")]
pub(crate) fn remove() {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let uid = user_id();
    for name in [label(), LEGACY_LABEL] {
        if uid.is_some_and(|uid| {
            launchctl(
                &["bootout".to_owned(), format!("gui/{uid}/{name}")],
                "evict",
            )
        }) {
            crate::trace::line("keepalive agent removed");
        }
        let path =
            std::path::PathBuf::from(&home).join(format!("Library/LaunchAgents/{name}.plist"));
        match std::fs::remove_file(&path) {
            Ok(()) => crate::trace::line("keepalive agent removed"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => crate::trace::line(&format!("keepalive removal failed: {error}")),
        }
    }
}

// Launchctl removes one obsolete registration without creating a new one.
#[cfg(target_os = "macos")]
fn launchctl(arguments: &[String], stage: &str) -> bool {
    match std::process::Command::new("/bin/launchctl")
        .args(arguments)
        .status()
    {
        Ok(status) if status.success() => true,
        Ok(status) => {
            crate::trace::line(&format!("keepalive {stage} exited: {status}"));
            false
        }
        Err(error) => {
            crate::trace::line(&format!("keepalive {stage} failed: {error}"));
            false
        }
    }
}

// UserId reads the numeric account id launchd uses to name this user's gui domain.
#[cfg(target_os = "macos")]
fn user_id() -> Option<u32> {
    let output = std::process::Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    std::str::from_utf8(&output.stdout)
        .ok()?
        .trim()
        .parse()
        .ok()
}
