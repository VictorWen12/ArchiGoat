//! Windows helper launch keeps native Provider execution headless and owned by one Job.

use base64::{Engine, engine::general_purpose::STANDARD};
use std::process::Stdio;
use tokio::process::Command;

/// Starts the signed helper in the Work's own hidden PowerShell host.
pub(super) fn open(record: &super::super::active::Record) -> Result<tokio::process::Child, String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("Could not locate ArchiGoat: {error}"))?;
    let powershell = super::super::command::system_program("powershell.exe")?;
    let script = format!(
        "$ErrorActionPreference='Stop';& '{}' '--windows-work' '{}';exit $LASTEXITCODE",
        quote(&executable.to_string_lossy()),
        quote(&record.root.join(super::admission::JOB).to_string_lossy()),
    );
    let encoded = STANDARD.encode(
        script
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    let mut command = Command::new(powershell);
    command
        .args(["-NoLogo", "-NoProfile", "-EncodedCommand", &encoded])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    super::super::job::hidden_suspended(&mut command);
    command
        .spawn()
        .map_err(|error| format!("Could not start the Work runner: {error}"))
}

/// Preserves arbitrary paths as one single-quoted PowerShell argument.
fn quote(value: &str) -> String {
    value.replace('\'', "''")
}
