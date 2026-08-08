//! Runtime models separate Provider connection facts from the three public Work states.

use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Notify;

use crate::{delivery::DeliveryFile, provider::Provider, work::ResultKind};

/// Phase reports only the current Provider connection lifecycle, never a Work result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Phase {
    Disconnected,
    Authorizing,
    Connected,
}

/// Status binds a connection epoch to the Provider selected for new Works.
pub(crate) struct Status {
    pub(crate) phase: Phase,
    pub(crate) provider: Option<Provider>,
    /// Model preserves the exact optional native tier selected for new Work.
    pub(crate) model: Option<String>,
    /// Effort preserves the exact optional native reasoning tier selected for new Work.
    pub(crate) effort: Option<String>,
    pub(crate) epoch: u64,
}

/// RunSlot makes Provider changes and Work admission atomic without limiting parallel Works.
pub(crate) struct RunSlot {
    pub(crate) active: HashSet<String>,
    pub(crate) connecting: Option<u64>,
    pub(crate) next_connection: u64,
}

/// OwnerStop is the only authority that may turn a public Running Work into Stopped.
#[derive(Clone)]
pub(crate) struct OwnerStop(Arc<StopSignal>);

/// StopSignal wakes the exact Running owner without periodic inference or global cancellation.
struct StopSignal {
    requested: AtomicBool,
    changed: Notify,
}

// This owner request carries the only user-authorized public Work stop.
impl OwnerStop {
    /// A new Work begins without Stop authority exercised.
    pub(crate) fn new() -> Self {
        Self(Arc::new(StopSignal {
            requested: AtomicBool::new(false),
            changed: Notify::new(),
        }))
    }

    /// Request records the owner's explicit action for every live or recovered runner.
    pub(crate) fn request(&self) {
        self.0.requested.store(true, Ordering::Release);
        self.0.changed.notify_one();
    }

    /// Requested lets platform runners reject internal interruption as false owner Stop.
    pub(crate) fn requested(&self) -> bool {
        self.0.requested.load(Ordering::Acquire)
    }
}

/// RunPhase is the complete public Work state space: Running, Done, owner Stopped, or Failed.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RunPhase {
    Running,
    Done,
    Stopped,
    /// Failed remains wire-compatible with terminal failures written by older ArchiGoat releases.
    Failed,
}

/// RunSnapshot exposes only user-visible progress and verified terminal delivery facts.
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunSnapshot {
    pub(crate) phase: RunPhase,
    pub(crate) text: String,
    pub(crate) started_at: u64,
    pub(crate) progress: Option<RunProgress>,
    /// Cumulative Provider-reported output tokens while Running; absent once terminal.
    pub(crate) tokens: Option<u64>,
    /// The Provider-reported executing model while Running; absent once terminal.
    pub(crate) model: Option<String>,
    pub(crate) kind: Option<ResultKind>,
    pub(crate) run: Option<String>,
    pub(crate) files: Vec<DeliveryFile>,
    /// Events is the conversation in the order it happened; text stays its joined Agent prose.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) events: Vec<WorkEvent>,
    /// Awaiting is true only while the Provider itself says this turn is parked on the creator.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) awaiting: bool,
}

/// WorkEvent is one conversation fact. `seq` is the append-only order; `at` is RFC 3339 UTC.
#[derive(Clone, Deserialize, PartialEq, Serialize)]
pub(crate) struct WorkEvent {
    pub(crate) seq: u64,
    pub(crate) at: String,
    #[serde(flatten)]
    pub(crate) kind: WorkEventKind,
}

/// The wire shape is internally tagged and flat:
/// `{"seq":3,"at":"…","kind":"agent_message","id":"…","text":"…"}`.
#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum WorkEventKind {
    /// One complete Agent message, deduped by the Provider's message identity.
    AgentMessage { id: String, text: String },
    /// One user turn. `steer_id` names the same turn on every screen, so one bubble renders once.
    UserMessage {
        steer_id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<String>,
    },
    /// A Provider stage label, passed through untranslated.
    Stage { label: String },
    /// A produced artifact, by name.
    Artifact { name: String },
    /// A turn closed: `reason` is delivered, steered, stopped, or failed.
    TurnBoundary {
        reason: String,
        elapsed_seconds: u64,
    },
}

/// A machine keeps the conversations of this many Works; older Works keep only their result.
const MAX_CONVERSATIONS: usize = 64;
/// One Work keeps this many conversation events; the oldest leaves once the transcript is longer.
const MAX_CONVERSATION_EVENTS: usize = 512;

/// Conversation keeps one Work's events and the clock its next turn boundary reports.
#[derive(Deserialize, Serialize)]
struct Conversation {
    events: Vec<WorkEvent>,
    next_seq: u64,
    turn_started_at: u64,
    /// The action this turn is already showing, so one long stage is named once, not per frame.
    #[serde(default)]
    stage: Option<String>,
}

/// Conversations keep every live Work's events in arrival order for the screens that render them.
pub(crate) struct Conversations {
    works: Vec<(String, Conversation)>,
}

// These conversations grow only by appending the facts each Work actually produced.
impl Conversations {
    /// Load restores every conversation this machine kept, so a restart still renders its turns.
    pub(crate) fn load(path: &Path) -> Self {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Self { works: Vec::new() };
            }
            Err(error) => {
                eprintln!("Product could not read the conversations it kept: {error}");
                return Self { works: Vec::new() };
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(works) => Self { works },
            Err(error) => {
                eprintln!("Product could not read the conversations it kept: {error}");
                Self { works: Vec::new() }
            }
        }
    }

    /// Save commits every kept conversation before the screens reading it are woken.
    pub(crate) fn save(&self, path: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec(&self.works)
            .map_err(|error| format!("Could not encode the conversation: {error}"))?;
        crate::host::replace_private(path, &bytes)
    }

    /// Push appends one fact, releasing the oldest Work or stage once either limit is passed.
    pub(crate) fn push(&mut self, work_id: &str, at: u64, kind: WorkEventKind) -> bool {
        // A turn that ends stops showing its last action, so the next turn names its own.
        let closes_turn = matches!(kind, WorkEventKind::TurnBoundary { .. });
        let conversation = self.conversation(work_id, at);
        if closes_turn {
            conversation.stage = None;
        }
        let seq = conversation.next_seq;
        conversation.next_seq = seq.saturating_add(1);
        conversation.events.push(WorkEvent {
            seq,
            at: rfc3339(at),
            kind,
        });
        if conversation.events.len() > MAX_CONVERSATION_EVENTS {
            // Stage labels leave before any message the creator or the Agent wrote.
            let oldest = conversation
                .events
                .iter()
                .position(|event| matches!(event.kind, WorkEventKind::Stage { .. }))
                .unwrap_or_default();
            let released = conversation.events.remove(oldest);
            eprintln!(
                "Product released conversation event {} of Work {work_id}",
                released.seq
            );
        }
        true
    }

    /// PushStage names one action once; the same action repeated across frames stays one entry.
    pub(crate) fn push_stage(&mut self, work_id: &str, at: u64, label: String) -> bool {
        if label.is_empty() {
            return false;
        }
        let conversation = self.conversation(work_id, at);
        if conversation.stage.as_deref() == Some(label.as_str()) {
            return false;
        }
        conversation.stage = Some(label.clone());
        self.push(work_id, at, WorkEventKind::Stage { label })
    }

    /// ExtendAgentMessage keeps one Provider message one bubble as its later frames arrive.
    pub(crate) fn extend_agent_message(
        &mut self,
        work_id: &str,
        at: u64,
        id: &str,
        text: &str,
    ) -> bool {
        let extended = self
            .conversation(work_id, at)
            .events
            .iter_mut()
            .rev()
            .find_map(|event| match &mut event.kind {
                WorkEventKind::AgentMessage {
                    id: published,
                    text: message,
                } if published == id => {
                    message.push_str(text);
                    Some(())
                }
                _ => None,
            });
        if extended.is_some() {
            return true;
        }
        self.push(
            work_id,
            at,
            WorkEventKind::AgentMessage {
                id: id.to_owned(),
                text: text.to_owned(),
            },
        )
    }

    /// TurnElapsed reports the seconds the turn ending now took and starts the next turn's clock.
    pub(crate) fn turn_elapsed(&mut self, work_id: &str, at: u64) -> u64 {
        match self.works.iter_mut().find(|(id, _)| id == work_id) {
            Some((_, conversation)) => {
                let elapsed = at.saturating_sub(conversation.turn_started_at) / 1000;
                conversation.turn_started_at = at;
                elapsed
            }
            None => 0,
        }
    }

    /// Events returns what one screen renders for this Work, oldest first.
    pub(crate) fn events(&self, work_id: &str) -> Vec<WorkEvent> {
        self.works
            .iter()
            .find(|(id, _)| id == work_id)
            .map(|(_, conversation)| conversation.events.clone())
            .unwrap_or_default()
    }

    /// Conversation opens this Work's transcript, releasing the oldest Work once the machine is full.
    fn conversation(&mut self, work_id: &str, at: u64) -> &mut Conversation {
        let index = match self.works.iter().position(|(id, _)| id == work_id) {
            Some(index) => index,
            None => {
                if self.works.len() >= MAX_CONVERSATIONS {
                    let (released, _) = self.works.remove(0);
                    eprintln!("Product released the conversation of Work {released}");
                }
                self.works.push((
                    work_id.to_owned(),
                    Conversation {
                        events: Vec::new(),
                        next_seq: 1,
                        turn_started_at: at,
                        stage: None,
                    },
                ));
                self.works.len() - 1
            }
        };
        &mut self.works[index].1
    }
}

/// Rfc3339 renders one UTC instant exactly as the event wire defines it.
fn rfc3339(epoch_ms: u64) -> String {
    let seconds = epoch_ms / 1000;
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    let time = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3600,
        (time % 3600) / 60,
        time % 60
    )
}

/// CivilFromDays converts days since 1970-01-01 into its proleptic Gregorian calendar date.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// RunProgress keeps the one current Provider action and its replay cursor.
#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunProgress {
    pub(crate) sequence: u64,
    pub(crate) text: String,
}
