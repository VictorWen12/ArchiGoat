//! Apple journal commits complete ordered Provider events before observation.

use super::model::{DONE, EVENTS, STDERR, STDOUT, STOPPED};
use crate::execution::{AgentEvent, AgentFrame};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

const HEADER: usize = 17;
/// The kind a previous runner used to report its own quiet. No runner writes it any more, and a
/// journal that still holds one stays readable: it reports a command, never an ending.
const LEGACY_QUIET: u8 = 5;

// AppendEvent commits one whole event so observers never consume partial truth.
pub(super) fn append_event(
    root: &Path,
    sequence: u64,
    kind: u8,
    bytes: &[u8],
) -> Result<(), String> {
    let size =
        u64::try_from(bytes.len()).map_err(|_| "Apple Work event is too large".to_owned())?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join(EVENTS))
        .map_err(|error| format!("Could not open Apple Work journal: {error}"))?;
    file.write_all(&sequence.to_le_bytes())
        .and_then(|_| file.write_all(&[kind]))
        .and_then(|_| file.write_all(&size.to_le_bytes()))
        .and_then(|_| file.write_all(bytes))
        // Only a terminal fact must outlive power loss; a torn output tail is already discarded by its length check.
        .and_then(|_| {
            if matches!(kind, DONE | STOPPED) {
                file.sync_all()
            } else {
                Ok(())
            }
        })
        .map_err(|error| format!("Could not persist Apple Work: {error}"))
}

// ReadEvent returns the next complete record this daemon understands, skipping past a kind it does
// not: a frame written by another generation of the runner is one lost frame, never an unreadable
// journal, and a turn is never ended by a byte it could not name.
pub(super) fn read_event(
    root: &Path,
    offset: usize,
) -> Result<Option<(AgentFrame, usize)>, String> {
    let mut file = match fs::File::open(root.join(EVENTS)) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Could not read Apple Work: {error}")),
    };
    let length = file
        .metadata()
        .map_err(|error| format!("Could not inspect Apple Work: {error}"))?
        .len();
    let mut offset = offset;
    loop {
        if length < offset as u64 + HEADER as u64 {
            return Ok(None);
        }
        file.seek(SeekFrom::Start(offset as u64))
            .map_err(|error| error.to_string())?;
        let mut header = [0; HEADER];
        file.read_exact(&mut header)
            .map_err(|error| error.to_string())?;
        let sequence = u64::from_le_bytes(header[..8].try_into().unwrap());
        let size = usize::try_from(u64::from_le_bytes(header[9..].try_into().unwrap()))
            .map_err(|_| "Apple Work journal is too large".to_owned())?;
        let Some(end) = offset
            .checked_add(HEADER)
            .and_then(|value| value.checked_add(size))
        else {
            // A length this machine cannot address ends the readable journal; the tail is released.
            crate::trace::line("released an Apple Work frame this machine cannot address");
            return Ok(None);
        };
        if length < end as u64 {
            return Ok(None);
        }
        let mut bytes = vec![0; size];
        file.read_exact(&mut bytes)
            .map_err(|error| error.to_string())?;
        let event = match header[8] {
            STDOUT => AgentEvent::Stdout(bytes),
            STDERR => AgentEvent::Stderr,
            LEGACY_QUIET if !bytes.is_empty() => AgentEvent::Stalled,
            DONE if bytes.is_empty() => AgentEvent::Done,
            STOPPED if bytes.is_empty() => AgentEvent::Stopped,
            kind => {
                crate::trace::line(&format!(
                    "released an Apple Work frame of kind {kind}; this Work keeps observing"
                ));
                offset = end;
                continue;
            }
        };
        return Ok(Some((AgentFrame { sequence, event }, end)));
    }
}
