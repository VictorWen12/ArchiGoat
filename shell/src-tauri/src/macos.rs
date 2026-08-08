use std::{
    env, fs,
    io::Read,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const APP_NAME: &str = "ArchiGoat.app";
const SHELL: &str = "archigoat-shell";
const DAEMON: &str = "archigoat";
const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
const HEALTH_URL: &str = "http://127.0.0.1:17891/v1/health";

#[derive(Clone, Debug, Eq, PartialEq)]
struct Release {
    version: [u64; 3],
    commit: String,
}

// Prepare converges a downloaded App onto one permanent bundle before its real Tauri window starts.
pub(super) fn prepare(label: &str) -> Result<(), String> {
    let executable =
        env::current_exe().map_err(|error| format!("ArchiGoat path is unavailable: {error}"))?;
    let Some(source) = bundle_for_executable(&executable) else {
        return Ok(());
    };
    let home = env::var_os("HOME").ok_or_else(|| "Home folder is unavailable".to_owned())?;
    let installed = install_destination(&source, &PathBuf::from(home));
    if canonical(&source) != canonical(&installed) {
        let candidate =
            read_release(&source).ok_or_else(|| "Downloaded ArchiGoat is invalid".to_owned())?;
        let current = read_release(&installed);
        if should_replace(&candidate, current.as_ref()) || !bundle_ready(&installed) {
            install(&source, &installed)?;
            register_launch_services(&installed)?;
            unregister_launch_services(&source);
            relaunch(&installed)?;
            std::process::exit(0);
        }
        // An already-current install keeps this process alive: exiting here would drop the
        // archigoat:// Apple Event that macOS delivers only after the event loop starts.
        unregister_launch_services(&source);
    }
    register_launch_services(&installed)?;
    retire_agents(label)?;
    start_daemon(&installed)?;
    wait_for_health()?;
    Ok(())
}

fn bundle_for_executable(executable: &Path) -> Option<PathBuf> {
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (executable.file_name()? == SHELL
        && macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app")
        .then(|| bundle.to_owned())
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

fn install_destination(source: &Path, home: &Path) -> PathBuf {
    let system = PathBuf::from("/Applications").join(APP_NAME);
    let user = home.join("Applications").join(APP_NAME);
    let source = canonical(source);
    if system.exists() || source == canonical(&system) {
        system
    } else {
        user
    }
}

fn read_release(bundle: &Path) -> Option<Release> {
    if !bundle_ready(bundle) {
        return None;
    }
    let plist = bundle.join("Contents/Info.plist");
    let version = plist_value(&plist, "CFBundleShortVersionString")?;
    let parts = version
        .split('.')
        .map(str::parse)
        .collect::<Result<Vec<u64>, _>>()
        .ok()?;
    let version: [u64; 3] = parts.try_into().ok()?;
    let commit = plist_value(&plist, "ArchiGoatCommit")?;
    (!commit.is_empty()).then_some(Release { version, commit })
}

fn plist_value(plist: &Path, key: &str) -> Option<String> {
    let output = Command::new("/usr/bin/plutil")
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(plist)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn bundle_ready(bundle: &Path) -> bool {
    [SHELL, DAEMON].iter().all(|name| {
        fs::metadata(bundle.join("Contents/MacOS").join(name))
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    })
}

fn should_replace(candidate: &Release, installed: Option<&Release>) -> bool {
    installed.is_none_or(|current| {
        candidate.version > current.version
            || candidate.version == current.version && candidate.commit != current.commit
    })
}

fn install(source: &Path, destination: &Path) -> Result<(), String> {
    let parent = destination
        .parent()
        .ok_or_else(|| "Install destination is invalid".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create Applications: {error}"))?;
    let suffix = std::process::id();
    let staging = parent.join(format!(".archigoat-install-{suffix}"));
    let backup = parent.join(format!(".archigoat-previous-{suffix}"));
    for path in [&staging, &backup] {
        if path.exists() {
            fs::remove_dir_all(path)
                .map_err(|error| format!("Could not clear install staging: {error}"))?;
        }
    }
    run(
        "/usr/bin/ditto",
        &[source.as_os_str(), staging.as_os_str()],
        false,
    )?;
    if !bundle_ready(&staging) || read_release(&staging).is_none() {
        let _ = fs::remove_dir_all(&staging);
        return Err("Copied ArchiGoat is invalid".to_owned());
    }
    let had_current = destination.exists();
    if had_current {
        fs::rename(destination, &backup)
            .map_err(|error| format!("Could not stage the prior ArchiGoat: {error}"))?;
    }
    if let Err(error) = fs::rename(&staging, destination) {
        if had_current {
            let _ = fs::rename(&backup, destination);
        }
        return Err(format!("Could not install ArchiGoat: {error}"));
    }
    if had_current {
        let _ = fs::remove_dir_all(backup);
    }
    Ok(())
}

fn relaunch(app: &Path) -> Result<(), String> {
    if !bundle_ready(app) {
        return Err("Installed ArchiGoat is invalid".to_owned());
    }
    let mut command = Command::new("/usr/bin/open");
    command
        .arg("-n")
        .arg(app)
        .arg("--args")
        .args(env::args().skip(1));
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not open installed ArchiGoat: {error}"))
}

fn register_launch_services(app: &Path) -> Result<(), String> {
    run(LSREGISTER, &["-f".as_ref(), app.as_os_str()], false).map(|_| ())
}

// Source copies stop claiming archigoat:// so LaunchServices always routes to the install.
fn unregister_launch_services(app: &Path) {
    let _ = run(LSREGISTER, &["-u".as_ref(), app.as_os_str()], true);
}

fn retire_agents(label: &str) -> Result<(), String> {
    let home = env::var_os("HOME").ok_or_else(|| "Home folder is unavailable".to_owned())?;
    let uid = command_text("/usr/bin/id", &["-u"])?;
    let domain = format!("gui/{uid}");
    let directory = PathBuf::from(home).join("Library/LaunchAgents");
    for name in [label, "com.app.plugin"] {
        let target = format!("{domain}/{name}");
        run(
            "/bin/launchctl",
            &["bootout".as_ref(), target.as_ref()],
            true,
        )?;
        let path = directory.join(format!("{name}.plist"));
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Could not remove background service: {error}")),
        }
    }
    Ok(())
}

fn start_daemon(app: &Path) -> Result<(), String> {
    let daemon = app.join("Contents/MacOS").join(DAEMON);
    if !bundle_ready(app) {
        return Err("ArchiGoat background service is missing".to_owned());
    }
    let mut child = Command::new(&daemon)
        .arg("--autostart")
        .spawn()
        .map_err(|error| format!("ArchiGoat daemon could not start: {error}"))?;
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn wait_for_health() -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let detail = match Command::new("/usr/bin/curl")
            .args([
                "--fail",
                "--silent",
                "--show-error",
                "--max-time",
                "1",
                "--output",
                "/dev/null",
                "--write-out",
                "%{http_code}",
                HEALTH_URL,
            ])
            .output()
        {
            Ok(output) if output.status.success() && output.stdout.as_slice() == b"200" => {
                return Ok(());
            }
            Ok(output) => {
                let status = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                if status.is_empty() {
                    if stderr.is_empty() {
                        format!("curl exited with {}", output.status)
                    } else {
                        stderr
                    }
                } else if stderr.is_empty() {
                    format!("HTTP {status}")
                } else {
                    format!("HTTP {status}: {stderr}")
                }
            }
            Err(error) => return Err(format!("ArchiGoat health probe could not start: {error}")),
        };
        if Instant::now() >= deadline {
            return Err(format!(
                "ArchiGoat daemon at {HEALTH_URL} did not become healthy: {detail}"
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn command_text(tool: &str, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new(tool)
        .args(arguments)
        .output()
        .map_err(|error| format!("{tool} could not start: {error}"))?;
    if !output.status.success() {
        return Err(format!("{tool} failed"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn run(tool: &str, arguments: &[&std::ffi::OsStr], allow_failure: bool) -> Result<i32, String> {
    let mut child = Command::new(tool)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("{tool} could not start: {error}"))?;
    let stderr = child.stderr.take().map(|mut stream| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stream.read_to_end(&mut bytes);
            String::from_utf8_lossy(&bytes).trim().to_owned()
        })
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("{tool} status failed: {error}"))?
        {
            let code = status.code().unwrap_or(1);
            let detail = stderr
                .map(|reader| reader.join().unwrap_or_default())
                .unwrap_or_default();
            let detail = if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            };
            if allow_failure || status.success() {
                return Ok(code);
            }
            return Err(format!("{tool} failed with {code}{detail}"));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let detail = stderr
                .map(|reader| reader.join().unwrap_or_default())
                .unwrap_or_default();
            let detail = if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            };
            return Err(format!("{tool} timed out{detail}"));
        }
        thread::sleep(Duration::from_millis(25));
    }
}
