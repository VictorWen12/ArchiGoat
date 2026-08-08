// Windows metadata binds the executable to the release identity users inspect.

use std::{env, error::Error};

// Release metadata lets Windows users verify the ArchiGoat build they install.
fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-env-changed=ARCHIGOAT_VERSION");
    println!("cargo:rerun-if-env-changed=ARCHIGOAT_COMMIT");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return Ok(());
    }

    let version =
        env::var("ARCHIGOAT_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").into());
    let parts = version
        .split('.')
        .map(|part| part.parse::<u16>())
        .collect::<Result<Vec<_>, _>>()?;
    if parts.len() != 3 {
        return Err("ARCHIGOAT_VERSION must be MAJOR.MINOR.PATCH".into());
    }
    let packed = ((parts[0] as u64) << 48) | ((parts[1] as u64) << 32) | ((parts[2] as u64) << 16);
    let file_version = format!("{version}.0");
    let product_title = "ArchiGoat";
    let original_filename = format!("{}.exe", env!("CARGO_PKG_NAME"));

    let mut resource = winresource::WindowsResource::new();
    resource
        .set("FileDescription", &product_title)
        .set("ProductName", &product_title)
        .set("OriginalFilename", &original_filename)
        .set("FileVersion", &file_version)
        .set("ProductVersion", &file_version)
        .set_version_info(winresource::VersionInfo::FILEVERSION, packed)
        .set_version_info(winresource::VersionInfo::PRODUCTVERSION, packed);
    resource.compile()?;
    Ok(())
}
