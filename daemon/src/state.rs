//! DaemonState owns one installation identity, native connection, and every durable Work truth.

mod identity;
mod model;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
};
use tokio::sync::{Notify, RwLock};

use crate::{
    Config,
    provider::{ModelChoice, PresetPair, Provider},
    work_state::WorkStore,
};

pub(crate) use crate::work_state::TurnStop;
use model::Conversations;
pub(crate) use model::{
    OwnerStop, Phase, RunPhase, RunProgress, RunSlot, RunSnapshot, Status, WorkEvent, WorkEventKind,
};

/// DaemonState shares one installation and independent Work map across loopback and platform runners.
#[derive(Clone)]
pub struct DaemonState {
    pub(crate) config: Config,
    pub(crate) instance_secret: Arc<String>,
    device_id: Arc<String>,
    runtime_id: Arc<String>,
    credential: Arc<RwLock<Option<String>>>,
    pub(crate) registration_events: Arc<Notify>,
    pub(crate) relay_events: Arc<Notify>,
    pub(crate) connection_events: Arc<Notify>,
    pub(crate) status: Arc<RwLock<Status>>,
    pub(crate) model_catalog: Arc<RwLock<Option<(Provider, Vec<ModelChoice>)>>>,
    pub(crate) preset_map: Arc<RwLock<Option<(Provider, PresetPair)>>>,
    pub(crate) login: Arc<StdMutex<Option<(Provider, crate::cli::Login)>>>,
    pub(crate) run_slot: Arc<StdMutex<RunSlot>>,
    pub(crate) work_notify: Arc<Notify>,
    pub(crate) works: Arc<StdMutex<WorkStore>>,
    pub(crate) work_events: Arc<Notify>,
    /// Conversations keep each Work's events in arrival order for every screen that renders it.
    conversations: Arc<StdMutex<Conversations>>,
    conversation_file: Arc<PathBuf>,
    work_file: Arc<PathBuf>,
    #[cfg(target_os = "windows")]
    pub(crate) host: Arc<crate::host::State>,
}

// This state roots each daemon operation in its durable device identity.
impl DaemonState {
    /// New restores one installation identity and every Work without launching or replaying execution.
    pub fn new(config: Config) -> Result<Self, String> {
        let state_file = config
            .state_file
            .clone()
            .ok_or_else(|| "ArchiGoat state path is unavailable".to_owned())?;
        let parent = state_file
            .parent()
            .ok_or_else(|| "ArchiGoat state path is invalid".to_owned())?;
        crate::host::create_private_dir(parent)?;
        let saved = identity::read(&state_file)?;
        let instance_secret = saved
            .as_ref()
            .map(|value| value.instance_secret.clone())
            .unwrap_or(crate::proof::nonce()?);
        if !crate::proof::valid_nonce(&instance_secret) {
            return Err("ArchiGoat installation identity is invalid".to_owned());
        }
        let device_id = saved
            .as_ref()
            .and_then(|value| value.device_id.clone())
            .filter(|value| crate::proof::valid_nonce(value))
            .map_or_else(crate::proof::nonce, Ok)?;
        let credential = saved
            .as_ref()
            .and_then(|value| value.app_credential.clone());
        let model = saved.as_ref().and_then(|value| value.model.clone());
        let effort = saved.as_ref().and_then(|value| value.effort.clone());
        let provider = saved.as_ref().and_then(|value| value.provider);
        let work_file = parent.join("works.json");
        let works = WorkStore::load(&work_file)?;
        works.save(&work_file)?;
        // A Work that outlives the machine it runs on keeps the conversation it already showed.
        let conversation_file = parent.join("conversations.json");
        let conversations = Conversations::load(&conversation_file);
        let active = works.active_ids();
        let state = Self {
            config: config.clone(),
            instance_secret: Arc::new(instance_secret),
            device_id: Arc::new(device_id),
            runtime_id: Arc::new(crate::proof::nonce()?),
            credential: Arc::new(RwLock::new(credential)),
            registration_events: Arc::new(Notify::new()),
            relay_events: Arc::new(Notify::new()),
            connection_events: Arc::new(Notify::new()),
            status: Arc::new(RwLock::new(Status {
                phase: Phase::Disconnected,
                provider,
                model,
                effort,
                epoch: 0,
            })),
            model_catalog: Arc::new(RwLock::new(None)),
            preset_map: Arc::new(RwLock::new(None)),
            login: Arc::new(StdMutex::new(None)),
            run_slot: Arc::new(StdMutex::new(RunSlot {
                active,
                connecting: None,
                next_connection: 0,
            })),
            work_notify: Arc::new(Notify::new()),
            works: Arc::new(StdMutex::new(works)),
            work_events: Arc::new(Notify::new()),
            conversations: Arc::new(StdMutex::new(conversations)),
            conversation_file: Arc::new(conversation_file),
            work_file: Arc::new(work_file),
            #[cfg(target_os = "windows")]
            host: Arc::new(crate::host::State::new(config.state_file.as_deref())?),
        };
        state.save_identity(provider)?;
        Ok(state)
    }

    /// RuntimeId binds Account leases to this exact live process.
    pub(crate) fn runtime_id(&self) -> &str {
        self.runtime_id.as_str()
    }

    /// DeviceId keeps Work on this installation across process restarts.
    pub(crate) fn device_id(&self) -> &str {
        self.device_id.as_str()
    }

    /// Credential returns the current Account proof only to daemon Account requests.
    pub(crate) async fn credential(&self) -> Option<String> {
        self.credential.read().await.clone()
    }

    /// Registered reports whether Account has issued this installation a durable credential.
    pub(crate) async fn registered(&self) -> bool {
        self.credential.read().await.is_some()
    }

    /// AcceptCredential persists an Account-issued device proof before command polling begins.
    pub(crate) async fn accept_credential(&self, credential: String) -> Result<(), String> {
        let status = self.status.read().await;
        let provider = status.provider;
        let model = status.model.clone();
        let effort = status.effort.clone();
        drop(status);
        let mut current = self.credential.write().await;
        let gained = current.is_none();
        self.save_identity_with(
            provider,
            model.as_deref(),
            effort.as_deref(),
            Some(&credential),
        )?;
        *current = Some(credential);
        drop(current);
        if gained {
            self.registration_events.notify_waiters();
        }
        Ok(())
    }

    /// RetireCredential forgets only the exact credential Account retired, never its replacement.
    pub(crate) async fn retire_credential(&self, expected: &str) -> Result<bool, String> {
        let status = self.status.read().await;
        let provider = status.provider;
        let model = status.model.clone();
        let effort = status.effort.clone();
        drop(status);
        let mut current = self.credential.write().await;
        if current.as_deref() != Some(expected) {
            return Ok(false);
        }
        self.save_identity_with(provider, model.as_deref(), effort.as_deref(), None)?;
        current.take();
        drop(current);
        self.registration_events.notify_waiters();
        Ok(true)
    }

    /// ReconnectProvider restores the last authenticated Agent without publishing it as live.
    pub(crate) async fn reconnect_provider(&self) -> Option<Provider> {
        self.status.read().await.provider
    }

    /// Persist retains a Provider only while native authentication proves it connected.
    pub(crate) async fn persist(&self) -> Result<(), String> {
        let (provider, model, effort) = {
            let status = self.status.read().await;
            if status.phase == Phase::Connected {
                (status.provider, status.model.clone(), status.effort.clone())
            } else {
                (None, None, None)
            }
        };
        let credential = self.credential.read().await;
        self.save_identity_with(
            provider,
            model.as_deref(),
            effort.as_deref(),
            credential.as_deref(),
        )
    }

    /// PrivateRoot keeps every staged input, runner, and frozen artifact below one owned directory.
    pub(crate) fn private_root(&self) -> Result<PathBuf, String> {
        self.config
            .state_file
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .ok_or_else(|| "ArchiGoat private storage is unavailable".to_owned())
    }

    /// SaveIdentityFull commits only daemon identity; shell sessions live in one private app file.
    pub(crate) fn save_identity_full(
        &self,
        provider: Option<Provider>,
        model: Option<&str>,
        effort: Option<&str>,
        credential: Option<&str>,
    ) -> Result<(), String> {
        let path = self
            .config
            .state_file
            .as_deref()
            .ok_or_else(|| "ArchiGoat state path is unavailable".to_owned())?;
        let bytes = serde_json::to_vec(&identity::SavedIdentity {
            device_id: Some(self.device_id.as_ref().clone()),
            instance_secret: self.instance_secret.as_ref().clone(),
            app_credential: credential.map(str::to_owned),
            provider,
            model: model.map(str::to_owned),
            effort: effort.map(str::to_owned),
        })
        .map_err(|error| format!("Could not encode ArchiGoat identity: {error}"))?;
        crate::host::replace_private(path, &bytes)
    }

    /// WorkStatePath gives the Work owner one atomic file independent of connection persistence.
    pub(crate) fn work_state_path(&self) -> &Path {
        self.work_file.as_path()
    }

    /// PushWorkEvent appends one conversation fact and wakes every screen reading this Work.
    pub(crate) fn push_work_event(&self, work_id: &str, kind: WorkEventKind) {
        self.keep_conversation(|conversations, at| conversations.push(work_id, at, kind));
    }

    /// PushWorkStage names the Agent's current action once, however many frames report it.
    pub(crate) fn push_work_stage(&self, work_id: &str, label: String) {
        self.keep_conversation(|conversations, at| conversations.push_stage(work_id, at, label));
    }

    /// ExtendAgentMessage keeps one Provider message one bubble as its later frames arrive.
    pub(crate) fn extend_agent_message(&self, work_id: &str, id: &str, text: &str) {
        self.keep_conversation(|conversations, at| {
            conversations.extend_agent_message(work_id, at, id, text)
        });
    }

    /// PushWorkTurnBoundary closes one turn with its own cause and the time that turn took.
    pub(crate) fn push_work_turn_boundary(&self, work_id: &str, reason: &str) {
        self.keep_conversation(|conversations, at| {
            let elapsed_seconds = conversations.turn_elapsed(work_id, at);
            conversations.push(
                work_id,
                at,
                WorkEventKind::TurnBoundary {
                    reason: reason.to_owned(),
                    elapsed_seconds,
                },
            )
        });
    }

    /// WorkConversation returns the events one screen renders for this Work, oldest first.
    pub(crate) fn work_conversation(&self, work_id: &str) -> Vec<WorkEvent> {
        self.conversations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .events(work_id)
    }

    /// KeepConversation commits one change to durable bytes before any screen is woken by it.
    fn keep_conversation(&self, change: impl FnOnce(&mut Conversations, u64) -> bool) {
        let at = crate::work::runtime::now_ms().unwrap_or_default();
        let mut conversations = self
            .conversations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !change(&mut conversations, at) {
            return;
        }
        if let Err(error) = conversations.save(&self.conversation_file) {
            eprintln!("Product could not keep this conversation: {error}");
        }
        drop(conversations);
        self.work_events.notify_waiters();
        self.relay_events.notify_one();
    }

    /// StagedInputPath maps stable Work and command identities to one private replay-safe file.
    pub(crate) fn staged_input_path(&self, work_id: &str, nonce: &str) -> Result<PathBuf, String> {
        if !crate::proof::valid_nonce(nonce) {
            return Err("Attachment identity is invalid".to_owned());
        }
        let root = self.work_input_root(work_id)?;
        crate::host::create_private_dir(&root)?;
        Ok(root.join(nonce))
    }

    /// StagedReceiptPath isolates signed replay truth from browser-visible attachment bytes.
    pub(crate) fn staged_receipt_path(
        &self,
        work_id: &str,
        nonce: &str,
    ) -> Result<PathBuf, String> {
        if !crate::proof::valid_nonce(nonce) {
            return Err("Attachment identity is invalid".to_owned());
        }
        let root = self.work_receipt_root(work_id)?;
        crate::host::create_private_dir(&root)?;
        Ok(root.join(nonce))
    }

    /// DiscardWorkInputs removes only staged bytes whose browser-owned Work reached a durable terminal.
    pub(crate) fn discard_work_inputs(&self, work_id: &str) -> Result<(), String> {
        let root = self.work_input_root(work_id)?;
        crate::delivery::discard_private_tree(&root)?;
        crate::delivery::discard_private_tree(&self.work_receipt_root(work_id)?)
    }

    /// WorkInputRoot hashes external Work identity so caller text can never become a machine path.
    fn work_input_root(&self, work_id: &str) -> Result<PathBuf, String> {
        crate::work::valid_work_id(work_id)?;
        use sha2::{Digest, Sha256};
        Ok(self
            .private_root()?
            .join("Inputs")
            .join(format!("{:x}", Sha256::digest(work_id.as_bytes()))))
    }

    /// ConversationAppRoot keeps one chat's latest delivered app outside every disposable Work tree.
    pub(crate) fn conversation_app_root(&self, conversation: &str) -> Result<PathBuf, String> {
        if conversation.trim().is_empty() || conversation.chars().any(char::is_control) {
            return Err("Conversation identity is invalid".to_owned());
        }
        use sha2::{Digest, Sha256};
        Ok(self
            .private_root()?
            .join("Apps")
            .join(format!("{:x}", Sha256::digest(conversation.as_bytes()))))
    }

    /// WorkReceiptRoot gives every Work one cleanup-owned receipt sidecar directory.
    fn work_receipt_root(&self, work_id: &str) -> Result<PathBuf, String> {
        crate::work::valid_work_id(work_id)?;
        use sha2::{Digest, Sha256};
        Ok(self
            .private_root()?
            .join("InputReceipts")
            .join(format!("{:x}", Sha256::digest(work_id.as_bytes()))))
    }

    /// SignHostWork authenticates the exact request accepted by this ArchiGoat installation.
    pub(crate) fn sign_host_work(&self, payload: &[u8]) -> Result<String, String> {
        crate::proof::host_proof(&self.instance_secret, payload)
    }

    /// VerifyHostWork rejects local request or attachment substitution across process boundaries.
    pub(crate) fn verify_host_work(&self, payload: &[u8], proof: &str) -> bool {
        crate::proof::verify_host(&self.instance_secret, payload, proof)
    }
}
