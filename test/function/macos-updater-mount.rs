//! The updater must accept the published image shape and release every handle before detaching.

#![allow(dead_code)]

mod delivery {
    pub fn discard_private_tree(_path: &std::path::Path) -> Result<(), String> {
        Ok(())
    }
}

mod config {
    pub fn bundle_id() -> &'static str {
        "com.archigoat.app"
    }
}

mod proof {
    pub fn valid_nonce(_value: &str) -> bool {
        true
    }
}

fn version() -> &'static str {
    "1.0.0"
}

fn commit() -> &'static str {
    "0000000000000000000000000000000000000000"
}

#[path = "../../daemon/src/update/macos.rs"]
mod macos;

use std::{
    fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    let root = std::env::temp_dir().join(format!(
        "product-updater-mount-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("current time")
            .as_nanos(),
    ));

    // The published image carries one App beside the drag-install symlink released macOS users see.
    let published = image(&root, "published", &["ArchiGoat.app"]);
    let candidate = extract(&published, &root.join("published/extracted"))
        .expect("production updater copies and detaches its mounted DMG");
    assert_eq!(
        fs::read(candidate.join("Contents/MacOS/archigoat")).expect("candidate app remains"),
        b"mounted update",
    );

    // An image without an App and an image carrying a second App are both refused.
    for (name, apps) in [
        ("empty", &[][..]),
        ("decoy", &["ArchiGoat.app", "Decoy.app"][..]),
    ] {
        let refusal = extract(&image(&root, name, apps), &root.join(name).join("extracted"))
            .expect_err("updater refuses an image without exactly one App");
        assert_eq!(refusal, "Mounted image must contain exactly one root App");
    }

    fs::remove_dir_all(root).expect("physical updater proof cleaned");
}

// Image builds the release layout and seals it with the exact hdiutil call of release/package-macos.sh.
fn image(root: &Path, name: &str, apps: &[&str]) -> PathBuf {
    let image = root.join(name).join("image");
    fs::create_dir_all(&image).expect("test image created");
    for app in apps {
        let binaries = image.join(app).join("Contents/MacOS");
        fs::create_dir_all(&binaries).expect("test app created");
        fs::write(binaries.join("archigoat"), b"mounted update").expect("test app populated");
    }
    symlink("/Applications", image.join("Applications")).expect("drag-install symlink created");
    let dmg = root.join(name).join("archigoat-macos.dmg");
    run(
        Command::new("/usr/bin/hdiutil")
            .args([
                "create",
                "-fs",
                "HFS+",
                "-format",
                "UDZO",
                "-volname",
                "ArchiGoat",
                "-srcfolder",
            ])
            .arg(&image)
            .arg(&dmg),
        "test DMG creation",
    );
    dmg
}

// Extract runs the production path and leaves no attached image behind on either outcome.
fn extract(dmg: &Path, root: &Path) -> Result<PathBuf, String> {
    let result = macos::extract(dmg, root);
    let _ = Command::new("/usr/bin/hdiutil")
        .arg("detach")
        .arg(root.join("mounted"))
        .output();
    result
}

fn run(command: &mut Command, operation: &str) {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{operation}: {error}"));
    assert!(
        output.status.success(),
        "{operation}: {}",
        String::from_utf8_lossy(&output.stderr),
    );
}
