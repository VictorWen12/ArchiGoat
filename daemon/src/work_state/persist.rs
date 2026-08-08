//! Work persistence makes every restart recover the same owner, terminal truth, and artifact receipts.

use serde::{
    Deserialize, Deserializer, Serialize,
    de::{SeqAccess, Visitor},
};
use std::{
    collections::{HashMap, VecDeque},
    fmt, fs,
    io::BufReader,
    path::{Path, PathBuf},
};

use crate::{
    delivery::DeliveryFile,
    provider::Provider,
    state::RunProgress,
    work::{ResultKind, RuntimeSteer},
};

use super::model::Entry;

/// SavedWork omits live channels and atomics while preserving every restart-relevant fact.
#[derive(Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(super) enum SavedWork {
    Running {
        #[serde(default)]
        remote: bool,
        work_id: String,
        provider: Provider,
        #[serde(default)]
        model_selection: Option<String>,
        #[serde(default)]
        effort_selection: Option<String>,
        session: PathBuf,
        freeze_root: PathBuf,
        native_session: String,
        #[serde(default)]
        runner_id: String,
        #[serde(default)]
        input_path: PathBuf,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        input: String,
        #[serde(default = "legacy_launched")]
        launched: bool,
        #[serde(default)]
        repair: bool,
        #[serde(default)]
        steer: Option<RuntimeSteer>,
        #[serde(default)]
        steers: VecDeque<RuntimeSteer>,
        #[serde(default)]
        steering: bool,
        // A record written before this fact existed says nothing about delivery, so nothing is claimed.
        #[serde(default)]
        steer_delivered: bool,
        #[serde(default)]
        rotating: bool,
        #[serde(default)]
        stopping: bool,
        #[serde(default)]
        repairs: u32,
        #[serde(default)]
        attention: bool,
        #[serde(default)]
        failure: Option<String>,
        started_at: u64,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        answer: String,
        #[serde(default)]
        progress: Option<RunProgress>,
        #[serde(default)]
        tokens: Option<u64>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        protected_outputs: Vec<String>,
    },
    Checkpoint {
        #[serde(default)]
        remote: bool,
        work_id: String,
        provider: Provider,
        #[serde(default)]
        model_selection: Option<String>,
        #[serde(default)]
        effort_selection: Option<String>,
        session: PathBuf,
        freeze_root: PathBuf,
        native_session: String,
        #[serde(default)]
        runner_id: String,
        #[serde(default)]
        input_path: PathBuf,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        input: String,
        #[serde(default = "legacy_launched")]
        launched: bool,
        #[serde(default)]
        repair: bool,
        #[serde(default)]
        steer: Option<RuntimeSteer>,
        #[serde(default)]
        steers: VecDeque<RuntimeSteer>,
        #[serde(default)]
        steering: bool,
        #[serde(default)]
        steer_delivered: bool,
        #[serde(default)]
        rotating: bool,
        #[serde(default)]
        stopping: bool,
        #[serde(default)]
        repairs: u32,
        #[serde(default)]
        attention: bool,
        #[serde(default)]
        failure: Option<String>,
        started_at: u64,
        answer: String,
        #[serde(default)]
        progress: Option<RunProgress>,
        #[serde(default)]
        tokens: Option<u64>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        protected_outputs: Vec<String>,
        kind: ResultKind,
        run: Option<String>,
        manifest: Vec<DeliveryFile>,
        #[serde(default)]
        settled: bool,
        #[serde(default)]
        ended_at: Option<u64>,
    },
    ArtifactPending {
        #[serde(default)]
        remote: bool,
        work_id: String,
        #[serde(default)]
        session: Option<PathBuf>,
        answer: String,
        kind: ResultKind,
        run: String,
        native_session: String,
        manifest: Vec<DeliveryFile>,
        freeze_root: PathBuf,
        started_at: u64,
    },
    Done {
        #[serde(default)]
        remote: bool,
        work_id: String,
        #[serde(default)]
        session: Option<PathBuf>,
        answer: String,
        kind: ResultKind,
        run: Option<String>,
        native_session: String,
        manifest: Vec<DeliveryFile>,
        freeze_root: Option<PathBuf>,
        started_at: u64,
        // A record written before terminal times existed carries no end, so retention never guesses one.
        #[serde(default)]
        ended_at: Option<u64>,
    },
    Stopped {
        #[serde(default)]
        remote: bool,
        work_id: String,
        #[serde(default)]
        session: Option<PathBuf>,
        #[serde(default)]
        freeze_root: Option<PathBuf>,
        started_at: u64,
        // A record written before terminal times existed carries no end, so retention never guesses one.
        #[serde(default)]
        ended_at: Option<u64>,
        #[serde(default = "legacy_owner")]
        owner: bool,
        #[serde(default)]
        reason: String,
    },
}

/// LegacyLaunched preserves existing Running records as reattach-only because their launch was already attempted.
fn legacy_launched() -> bool {
    true
}

/// LegacyOwner keeps every already-written Stop as the owner's, because no earlier record can prove another cause.
fn legacy_owner() -> bool {
    true
}

/// Load rejects malformed ownership and downgrades missing artifact bytes from Done to Running.
pub(super) fn load(path: &Path) -> Result<HashMap<String, Entry>, String> {
    let file = open(path)?;
    let Some(file) = file else {
        return Ok(HashMap::new());
    };
    let mut deserializer = serde_json::Deserializer::from_reader(BufReader::new(file));
    match deserializer.deserialize_seq(WorkVisitor) {
        Ok(entries) => Ok(entries),
        // Bytes this build cannot read stay unreadable, so failing here would fail every later start the same way.
        Err(_) => set_aside(path).map(|()| HashMap::new()),
    }
}

/// SetAside keeps unreadable bytes under one fixed name so the ArchiGoat always starts and the owner can still recover them.
fn set_aside(path: &Path) -> Result<(), String> {
    let kept = path.with_extension("unreadable");
    fs::rename(path, &kept)
        .map_err(|error| format!("Could not set aside unreadable Work state: {error}"))?;
    eprintln!(
        "Product could not read its Work state and kept it at {}",
        kept.display()
    );
    Ok(())
}

/// Save atomically commits the full Work map before runner cleanup or public Done.
pub(super) fn save(entries: &HashMap<String, Entry>, path: &Path) -> Result<(), String> {
    let saved = entries
        .values()
        .map(super::saved::save_entry)
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&saved)
        .map_err(|error| format!("Could not encode Work state: {error}"))?;
    crate::host::replace_private(path, &bytes)
}

/// Open validates the private state before streaming one Work at a time.
fn open(path: &Path) -> Result<Option<fs::File>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Could not inspect Work state: {error}")),
    };
    // A path that is not this ArchiGoat's own regular file can never become one, so it moves aside instead of blocking every start.
    if !metadata.is_file() || crate::host::linked(&metadata) {
        set_aside(path)?;
        return Ok(None);
    }
    fs::File::open(path)
        .map(Some)
        .map_err(|error| format!("Could not read Work state: {error}"))
}

/// WorkVisitor restores each Work before reading the next, bounding recovery memory per Work.
struct WorkVisitor;

// This visitor restores only valid durable Work records after a device restart.
impl<'de> Visitor<'de> for WorkVisitor {
    // This restored map preserves each Work's latest durable truth across restarts.
    type Value = HashMap<String, Entry>;

    // This parser diagnosis lets recovery reject malformed durable Work state truthfully.
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a Work state array")
    }

    // Streaming each saved Work bounds restart memory while restoring every durable Work.
    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut entries = HashMap::new();
        while let Some(item) = sequence.next_element::<SavedWork>()? {
            // One unrestorable record loses only itself; every other Work still recovers its owner.
            let (id, entry) = match super::restore::restore(item) {
                Ok(restored) => restored,
                Err(reason) => {
                    eprintln!("Product skipped an unrestorable Work: {reason}");
                    continue;
                }
            };
            if entries.insert(id, entry).is_some() {
                return Err(serde::de::Error::custom(
                    "Work state contains a duplicate identity",
                ));
            }
        }
        Ok(entries)
    }
}
