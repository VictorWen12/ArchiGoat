//! One RuntimeWork gives one accepted Work a private workspace, immutable Agent input, and durable artifact identity.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    api::work::StagedInput,
    host,
    provider::Provider,
    state::DaemonState,
    work::{WorkRequest, evidence::valid_work_id},
};

/// Control keeps immutable input and runner journals outside user-deliverable workspace bytes.
const CONTROL: &str = ".app";
/// Workspace is the only provider-writable tree inside one private Work container.
const WORKSPACE: &str = "Work";
/// RuntimeSteer preserves one exact follow-up and its verified private attachments.
#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct RuntimeSteer {
    pub(crate) id: String,
    #[serde(flatten)]
    pub(crate) request: WorkRequest,
    pub(crate) attachments: Vec<StagedInput>,
}

// This follow-up validates both fresh admission and durable restore through one path.
impl RuntimeSteer {
    /// Prepare accepts one protocol steer identity without changing its request or attachments.
    pub(crate) fn prepare(
        id: String,
        request: WorkRequest,
        inputs: Vec<StagedInput>,
    ) -> Result<Self, String> {
        if !crate::proof::valid_nonce(&id) {
            return Err("Steer identity is invalid".to_owned());
        }
        Ok(Self {
            id,
            request,
            attachments: inputs,
        })
    }

    /// Validate proves this follow-up still names exact readonly bytes under its Work's private input root.
    pub(crate) fn validate(&self, work_id: &str, session: &Path) -> Result<(), String> {
        if !crate::proof::valid_nonce(&self.id) {
            return Err("Steer identity is invalid".to_owned());
        }
        validate_steer_inputs(&input_root(work_id, session)?, &self.attachments)
    }
}

/// RuntimeWork binds every machine path and native session hint to exactly one external Work ID.
#[derive(Clone)]
pub struct RuntimeWork {
    pub(crate) work_id: String,
    pub(crate) provider: Provider,
    /// ModelSelection is the exact optional native tier chosen at connection time.
    pub(crate) model_selection: Option<String>,
    /// EffortSelection is the exact optional native reasoning tier chosen at connection time.
    pub(crate) effort_selection: Option<String>,
    pub(crate) session: PathBuf,
    pub(crate) freeze_root: PathBuf,
    pub(crate) native_session: String,
    pub(crate) resume: bool,
    pub(crate) runner_id: String,
    pub(crate) started_at: u64,
    pub(crate) input_path: PathBuf,
    pub(crate) steer: Option<RuntimeSteer>,
    /// SteerDelivered proves a runner already received this follow-up's exact words, so a replacement
    /// runner continues the turn instead of asking the Agent to do the same thing twice.
    pub(crate) steer_delivered: bool,
    /// Rotating proves the active turn is ending before this follow-up resumes its native session.
    pub(crate) rotating: bool,
    /// Repair selects Provider-native continuation without changing immutable Work input.
    pub(crate) repair: bool,
    /// Failure names the exact correction or stalled turn the same native session must recover.
    pub(crate) failure: Option<String>,
    /// Launched makes restart recovery reattach before any host command can reopen.
    pub(crate) launched: bool,
}

/// RuntimeRecovery carries only durable facts required to resume one admitted native Work.
pub(crate) struct RuntimeRecovery {
    pub(crate) work_id: String,
    pub(crate) conversation: String,
    pub(crate) provider: Provider,
    pub(crate) model_selection: Option<String>,
    pub(crate) effort_selection: Option<String>,
    pub(crate) session: PathBuf,
    pub(crate) freeze_root: PathBuf,
    pub(crate) native_session: String,
    pub(crate) resume: bool,
    pub(crate) runner_id: String,
    pub(crate) started_at: u64,
    pub(crate) input_path: PathBuf,
    pub(crate) steer: Option<RuntimeSteer>,
    pub(crate) steer_delivered: bool,
    pub(crate) rotating: bool,
    pub(crate) repair: bool,
    pub(crate) failure: Option<String>,
    pub(crate) launched: bool,
}

/// AgentInput preserves the complete request while adding only verified local attachment locations.
#[derive(Serialize)]
struct AgentInput<'a> {
    #[serde(flatten)]
    request: &'a WorkRequest,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<&'a str>,
    resume: bool,
    attachments: Vec<AgentAttachment<'a>>,
}

/// AgentSteer frames one follow-up exactly like the first turn.
#[derive(Serialize)]
struct AgentSteer<'a> {
    #[serde(flatten)]
    steer: &'a RuntimeSteer,
    resume: bool,
}

/// AgentAttachment lets the native Agent read the exact bytes already proven for this Work.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentAttachment<'a> {
    id: &'a str,
    name: &'a str,
    media: &'a str,
    bytes: u64,
    sha256: &'a str,
    path: &'a Path,
}

/// StoredInput recovers only the immutable attachment facts needed for Provider launch input.
#[derive(Deserialize)]
struct StoredInput {
    #[serde(default)]
    conversation: String,
    #[serde(default)]
    resume: bool,
    #[serde(default)]
    instructions: Option<String>,
    #[serde(default)]
    attachments: Vec<StoredAttachment>,
}

#[derive(Clone, Deserialize)]
struct StoredAttachment {
    bytes: u64,
    sha256: String,
    path: PathBuf,
}

// This runtime binds one frozen Work to its live native session and terminal.
impl RuntimeWork {
    /// Prepare creates one fresh private workspace without classifying or reducing the user's Work.
    pub(crate) fn prepare(
        state: &DaemonState,
        work_id: String,
        provider: Provider,
        model_selection: Option<String>,
        effort_selection: Option<String>,
        mut request: WorkRequest,
        inputs: Vec<StagedInput>,
        bound_session: Option<String>,
    ) -> Result<Self, String> {
        valid_work_id(&work_id)?;
        let conversation = request.conversation_id(&work_id)?;
        let instructions = request.take_designer_guidance()?;
        let parent = state.private_root()?;
        let identity = crate::proof::nonce()?;
        let session = parent.join("Works").join(&identity);
        let freeze_root = parent.join("Deliveries").join(&identity);
        host::create_private_dir(&session)?;
        let workspace = session.join(WORKSPACE);
        host::create_private_dir(&workspace)?;
        for path in
            crate::work::app::seed(&state.conversation_app_root(&conversation)?, &workspace)?
        {
            host::make_writable(&path)?;
        }
        let control = session.join(CONTROL);
        host::create_private_dir(&control)?;
        let attachments = inputs
            .iter()
            .map(|input| AgentAttachment {
                id: &input.id,
                name: &input.name,
                media: &input.media,
                bytes: input.bytes,
                sha256: &input.sha256,
                path: &input.path,
            })
            .collect();
        let input = serde_json::to_string(&AgentInput {
            request: &request,
            instructions: instructions.as_deref(),
            resume: bound_session.is_some(),
            attachments,
        })
        .map_err(|error| format!("Could not preserve Work input: {error}"))?;
        let input_path = control.join("input.json");
        preserve_input(&input_path, input.as_bytes())?;
        Ok(Self {
            work_id,
            provider,
            model_selection,
            effort_selection,
            session,
            freeze_root,
            // Claude accepts an explicit session; Codex and Cursor bind the identity their first event emits.
            native_session: bound_session.clone().unwrap_or_else(|| match provider {
                Provider::Claude => native_session(&identity),
                Provider::Codex | Provider::Cursor => String::new(),
            }),
            resume: bound_session.is_some(),
            runner_id: identity,
            started_at: now_ms()?,
            input_path,
            steer: None,
            steer_delivered: false,
            rotating: false,
            repair: false,
            failure: None,
            launched: false,
        })
    }

    /// Launch hands the untouched Work envelope to the official Provider CLI in its owned workspace.
    pub(crate) async fn launch(&self, state: &DaemonState) -> Result<host::AgentRun, String> {
        let cli = crate::cli::find(self.provider, &state.config.cli_dirs)
            .ok_or_else(|| format!("{} CLI is unavailable", self.provider.label()))?;
        let original = read_input(&self.input_path)?;
        let stored = stored_input(&original)?;
        let instructions = stored.instructions;
        let mut selected = stored.attachments;
        let steer_input = if let Some(steer) = &self.steer {
            if self.native_session.is_empty() {
                return Err("Steering requires a bound native session".to_owned());
            }
            steer.validate(&self.work_id, &self.session)?;
            selected.extend(steer.attachments.iter().map(|input| StoredAttachment {
                bytes: input.bytes,
                sha256: input.sha256.clone(),
                path: input.path.clone(),
            }));
            let framed = serde_json::to_string(&AgentSteer {
                steer,
                resume: true,
            })
            .map_err(|error| format!("Could not preserve steer input: {error}"))?;
            Some(framed)
        } else {
            None
        };
        if self.repair {
            if self.native_session.is_empty() {
                return Err("Repair requires a bound native session".to_owned());
            }
            // The provider's own report stays a local diagnostic; delivery repair is the daemon's work.
            if let Some(failure) = &self.failure {
                crate::trace::line(&format!("repair relaunch after: {failure}"));
            }
        }
        let delivery = super::envelope::select(
            &original,
            steer_input.as_deref(),
            self.steer_delivered,
            self.repair,
            self.resume,
        );
        let mut envelope = delivery.envelope.to_owned();
        let resume = delivery.resume;
        if delivery.request && self.provider != Provider::Cursor {
            envelope = without_native_instructions(&envelope)?;
        }
        let mut paths = HashSet::new();
        selected.retain(|input| paths.insert(input.path.clone()));
        let readable = if self.provider == Provider::Cursor {
            let selected = selected
                .iter()
                .map(|input| super::input_view::SelectedInput {
                    path: &input.path,
                    bytes: input.bytes,
                    sha256: &input.sha256,
                })
                .collect::<Vec<_>>();
            let view =
                super::input_view::InputView::rebuild(&self.session, &self.runner_id, &selected)?;
            // Only a stored request envelope names attachment paths; a plain continuation has none to move.
            if delivery.request {
                envelope = remap_attachment_paths(&envelope, &view)?;
            }
            view.paths()
        } else {
            selected.iter().map(|input| input.path.clone()).collect()
        };
        let workspace = workspace(&self.session);
        let args = self.provider.work_args(
            &self.native_session,
            resume && !self.native_session.is_empty(),
            self.model_selection.as_deref(),
            self.effort_selection.as_deref(),
            instructions.as_deref(),
            &workspace,
            &readable,
        )?;
        host::launch(
            &self.work_id,
            &self.runner_id,
            self.provider,
            &cli,
            args,
            envelope,
            workspace,
            self.session.clone(),
            self.freeze_root.clone(),
            state,
        )
        .await
    }

    /// Recover rebuilds only an admitted launch command; its stable runner ID makes retry execution-safe.
    pub(crate) fn recover(recovery: RuntimeRecovery) -> Self {
        Self {
            work_id: recovery.work_id,
            provider: recovery.provider,
            model_selection: recovery.model_selection,
            effort_selection: recovery.effort_selection,
            session: recovery.session,
            freeze_root: recovery.freeze_root,
            native_session: recovery.native_session,
            resume: recovery.resume,
            runner_id: recovery.runner_id,
            started_at: recovery.started_at,
            input_path: recovery.input_path,
            steer: recovery.steer,
            steer_delivered: recovery.steer_delivered,
            rotating: recovery.rotating,
            repair: recovery.repair,
            failure: recovery.failure,
            launched: recovery.launched,
        }
    }

    /// Discard removes only an unlaunched workspace and reports any private-data cleanup failure.
    pub(crate) fn discard(&self) -> Result<(), String> {
        super::input_view::InputView::discard_session(&self.session)?;
        crate::delivery::discard_private_tree(&self.session)
    }
}

impl RuntimeRecovery {
    /// ValidateInputs re-proves every original and follow-up attachment before Done.
    pub(crate) fn validate_inputs(&self) -> Result<(), String> {
        let mut selected = stored_input(&read_input(&self.input_path)?)?.attachments;
        if let Some(steer) = &self.steer {
            steer.validate(&self.work_id, &self.session)?;
            selected.extend(steer.attachments.iter().map(|input| StoredAttachment {
                bytes: input.bytes,
                sha256: input.sha256.clone(),
                path: input.path.clone(),
            }));
        }
        if self.provider == Provider::Cursor {
            let mut paths = HashSet::new();
            selected.retain(|input| paths.insert(input.path.clone()));
            let selected = selected
                .iter()
                .map(|input| super::input_view::SelectedInput {
                    path: &input.path,
                    bytes: input.bytes,
                    sha256: &input.sha256,
                })
                .collect::<Vec<_>>();
            super::input_view::InputView::open(&self.session, &self.runner_id, &selected)?
                .validate()?;
        }
        Ok(())
    }
}

pub(crate) fn stored_runtime(input_path: &Path, work_id: &str) -> Result<(String, bool), String> {
    let stored: StoredInput = serde_json::from_str(&read_input(input_path)?)
        .map_err(|_| "Stored Work input is invalid".to_owned())?;
    let request = WorkRequest {
        conversation: stored.conversation,
        goal: String::new(),
        context: Vec::new(),
    };
    Ok((request.conversation_id(work_id)?, stored.resume))
}

/// StoredInput re-proves every immutable attachment before including it in the Provider launch.
fn stored_input(envelope: &str) -> Result<StoredInput, String> {
    let stored: StoredInput =
        serde_json::from_str(envelope).map_err(|_| "Stored Work input is invalid".to_owned())?;
    for input in &stored.attachments {
        let metadata = fs::symlink_metadata(&input.path)
            .map_err(|_| "Stored Work attachment is unavailable".to_owned())?;
        if !metadata.is_file()
            || host::linked(&metadata)
            || !metadata.permissions().readonly()
            || metadata.len() != input.bytes
            || digest_file(&input.path)? != input.sha256
        {
            return Err("Stored Work attachment changed after staging".to_owned());
        }
    }
    Ok(stored)
}

/// Native provider instructions leave the user envelope after moving to their privileged channel.
fn without_native_instructions(envelope: &str) -> Result<String, String> {
    let mut value: serde_json::Value =
        serde_json::from_str(envelope).map_err(|_| "Stored Work input is invalid".to_owned())?;
    let input = value
        .as_object_mut()
        .ok_or_else(|| "Stored Work input is invalid".to_owned())?;
    input.remove("instructions");
    if let Some(authority) = input
        .get_mut("authority")
        .and_then(serde_json::Value::as_array_mut)
    {
        authority.retain(|field| field.as_str() != Some("instructions"));
    }
    serde_json::to_string(&value).map_err(|_| "Stored Work input is invalid".to_owned())
}

/// RemapAttachmentPaths changes only Cursor's disposable attachment locations, never request text.
fn remap_attachment_paths(
    envelope: &str,
    view: &super::input_view::InputView,
) -> Result<String, String> {
    let value: serde_json::Value =
        serde_json::from_str(envelope).map_err(|_| "Stored Work input is invalid".to_owned())?;
    let Some(attachments) = value
        .get("attachments")
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(envelope.to_owned());
    };
    let mut remapped = envelope.to_owned();
    for attachment in attachments {
        let original = attachment
            .get("path")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "Stored Work attachment path is invalid".to_owned())?;
        let mapped = view
            .path_for(Path::new(original))
            .and_then(Path::to_str)
            .ok_or_else(|| "Cursor input view path is invalid".to_owned())?;
        let from = format!(
            "\"path\":{}",
            serde_json::to_string(original)
                .map_err(|_| "Stored Work input is invalid".to_owned())?
        );
        let to = format!(
            "\"path\":{}",
            serde_json::to_string(mapped)
                .map_err(|_| "Cursor input view path is invalid".to_owned())?
        );
        if !remapped.contains(&from) {
            return Err("Stored Work attachment path changed".to_owned());
        }
        remapped = remapped.replacen(&from, &to, 1);
    }
    Ok(remapped)
}

/// Workspace keeps legacy in-flight Works recoverable while every new Work uses a separate tree.
fn workspace(session: &Path) -> PathBuf {
    let current = session.join(WORKSPACE);
    current
        .is_dir()
        .then_some(current)
        .unwrap_or_else(|| session.to_path_buf())
}

/// ValidateSteerInputs accepts exact readonly staged bytes owned by this Work once each.
fn validate_steer_inputs(root: &Path, inputs: &[StagedInput]) -> Result<(), String> {
    if inputs.is_empty() {
        return Ok(());
    }
    let root_metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("Could not inspect steer attachment root: {error}"))?;
    if !root_metadata.is_dir() || host::linked(&root_metadata) {
        return Err("Steer attachment root is invalid".to_owned());
    }
    let mut ids = HashSet::new();
    let mut paths = HashSet::new();
    for input in inputs {
        if input.id.trim().is_empty()
            || input.id.chars().any(char::is_control)
            || !ids.insert(input.id.clone())
            || !paths.insert(input.path.clone())
            || input.path.parent() != Some(root)
            || !input
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(crate::proof::valid_nonce)
            || !crate::proof::valid_nonce(&input.sha256)
        {
            return Err("Steer attachment identity is invalid".to_owned());
        }
        let metadata = fs::symlink_metadata(&input.path)
            .map_err(|error| format!("Could not inspect steer attachment: {error}"))?;
        if !metadata.is_file()
            || host::linked(&metadata)
            || !metadata.permissions().readonly()
            || metadata.len() != input.bytes
            || digest_file(&input.path)? != input.sha256
        {
            return Err("Steer attachment bytes changed after staging".to_owned());
        }
    }
    Ok(())
}

/// InputRoot derives one Work's staged attachment directory from its private runtime identity.
fn input_root(work_id: &str, session: &Path) -> Result<PathBuf, String> {
    valid_work_id(work_id)?;
    let private = session
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "ArchiGoat private storage is unavailable".to_owned())?;
    Ok(private
        .join("Inputs")
        .join(format!("{:x}", Sha256::digest(work_id.as_bytes()))))
}

/// DigestFile proves the staged bytes still match the signed attachment fact.
fn digest_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path)
        .map_err(|error| format!("Could not read steer attachment: {error}"))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("Could not read steer attachment: {error}"))?;
        if read == 0 {
            return Ok(format!("{:x}", digest.finalize()));
        }
        digest.update(&buffer[..read]);
    }
}

/// PreserveInput commits the complete Agent envelope once inside its private session.
pub(crate) fn preserve_input(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("Could not create immutable Work input: {error}"))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("Could not persist immutable Work input: {error}"))?;
    let mut permissions = file
        .metadata()
        .map_err(|error| format!("Could not inspect immutable Work input: {error}"))?
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions)
        .map_err(|error| format!("Could not protect immutable Work input: {error}"))
}

/// ReadInput rejects missing, linked, empty, or non-text launch input on every recovery path.
pub(crate) fn read_input(path: &Path) -> Result<String, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect immutable Work input: {error}"))?;
    if !metadata.is_file()
        || host::linked(&metadata)
        || metadata.len() == 0
        || !metadata.permissions().readonly()
    {
        return Err("Stored Work input is invalid".to_owned());
    }
    fs::read_to_string(path)
        .map_err(|error| format!("Could not read immutable Work input: {error}"))
}

/// Native session hints use the UUID shape required by Provider CLIs without creating another identity.
fn native_session(identity: &str) -> String {
    format!(
        "{}-{}-{}-{}-{}",
        &identity[0..8],
        &identity[8..12],
        &identity[12..16],
        &identity[16..20],
        &identity[20..32]
    )
}

/// Millisecond wall time lets every UI order, recover, and expire the same accepted Work.
pub(crate) fn now_ms() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "System time is unavailable".to_owned())?
        .as_millis()
        .try_into()
        .map_err(|_| "System time is unavailable".to_owned())
}
