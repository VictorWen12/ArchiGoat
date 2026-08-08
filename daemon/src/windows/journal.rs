//! Windows journal receives raw Provider lines and returns durable ordered frames without parsing content.

use crate::execution::{AgentEvent, AgentFrame};
use std::{
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

/// Names the durable event log.
pub(super) const FILE: &str = "events.bin";
/// Names the machine event that ended a runner, so a logout is never read as a Provider that died.
/// Frame kinds are the shared process-layer numbering; every platform journals the same fact as the
/// same byte.
pub(super) use crate::process::{DONE, STDERR, STDOUT, STOPPED};
/// The kind a previous generation of this runner used for a Work that did not complete. A journal
/// that still holds one stays readable.
const LEGACY_STOPPED: u8 = 5;
const HEADER: usize = 17;

/// Persists one ordered event before observers can receive it.
pub(super) fn append(root: &Path, sequence: u64, kind: u8, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join(FILE))
        .map_err(|error| format!("Could not open Windows Work journal: {error}"))?;
    file.write_all(&sequence.to_le_bytes())
        .and_then(|_| file.write_all(&[kind]))
        .and_then(|_| file.write_all(&(bytes.len() as u64).to_le_bytes()))
        .and_then(|_| file.write_all(bytes))
        .map_err(|error| format!("Could not write Windows Work journal: {error}"))?;
    if matches!(kind, DONE | STOPPED) {
        file.sync_all()
            .map_err(|error| format!("Could not persist Windows Work terminal result: {error}"))?;
    }
    Ok(())
}

/// Reads the next complete event this daemon understands without exposing partial writes. A kind it
/// cannot name is skipped past: a frame from another generation of the runner is one lost frame,
/// never an unreadable journal, and no byte it could not name ends a turn.
pub(super) fn read(root: &Path, offset: usize) -> Result<Option<(AgentFrame, usize)>, String> {
    let mut file = match std::fs::File::open(root.join(FILE)) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Could not read Windows Work journal: {error}")),
    };
    let length = file
        .metadata()
        .map_err(|error| format!("Could not inspect Windows Work journal: {error}"))?
        .len();
    let mut offset = offset;
    loop {
        if length < offset as u64 + HEADER as u64 {
            return Ok(None);
        }
        file.seek(SeekFrom::Start(offset as u64))
            .map_err(|error| format!("Could not seek Windows Work journal: {error}"))?;
        let mut header = [0_u8; HEADER];
        file.read_exact(&mut header)
            .map_err(|error| format!("Could not read Windows Work journal: {error}"))?;
        let sequence = u64::from_le_bytes(header[..8].try_into().unwrap());
        let kind = header[8];
        let payload_len = u64::from_le_bytes(header[9..].try_into().unwrap());
        let Some(end) = (offset as u64)
            .checked_add(HEADER as u64)
            .and_then(|value| value.checked_add(payload_len))
        else {
            crate::trace::line("released a Windows Work frame this machine cannot address");
            return Ok(None);
        };
        if length < end {
            return Ok(None);
        }
        let Ok(payload_len) = usize::try_from(payload_len) else {
            crate::trace::line("released a Windows Work frame too large for this machine");
            return Ok(None);
        };
        let mut payload = vec![0_u8; payload_len];
        file.read_exact(&mut payload)
            .map_err(|error| format!("Could not read Windows Work journal: {error}"))?;
        let event = match kind {
            STDOUT => AgentEvent::Stdout(payload),
            STDERR => AgentEvent::Stderr,
            DONE if payload.is_empty() => AgentEvent::Done,
            STOPPED if payload.is_empty() => AgentEvent::Stopped,
            // A previous generation of this runner numbered its own quiet report here; the frame
            // still reads, and it reports a command, never an ending.
            STOPPED => AgentEvent::Stalled,
            LEGACY_STOPPED if payload.is_empty() => AgentEvent::Stopped,
            kind => {
                crate::trace::line(&format!(
                    "released a Windows Work frame of kind {kind}; this Work keeps observing"
                ));
                offset = usize::try_from(end)
                    .map_err(|_| "Windows Work journal is too large for this machine".to_owned())?;
                continue;
            }
        };
        return Ok(Some((AgentFrame { sequence, event }, end as usize)));
    }
}
