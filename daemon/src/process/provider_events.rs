//! This module projects each Provider's native JSONL events into one minimal process contract.

// Provider identity selects the matching native event grammar.
use crate::{
    provider::Provider,
    work::{MAX_PROTECTED_BYTES, MAX_PROTECTED_ITEM_BYTES, MAX_PROTECTED_ITEMS},
};

// One native event may contribute a session identity, public message, final answer, terminal state, or failure.
#[derive(Default)]
pub(super) struct ProviderEvent {
    /// The provider-native session identity stays bound to this exact Work.
    pub(super) native_session: Option<String>,
    /// A native completion event proves the provider turn ended normally.
    pub(super) completed: bool,
    /// Only Provider-native structured events may expose one public stage.
    pub(super) stage: Option<Stage>,
    /// The provider-native final message is the only answer text eligible for delivery.
    pub(super) answer: Option<String>,
    /// The provider's own message identity, so one message is never rendered twice.
    pub(super) answer_id: Option<String>,
    /// Native command and tool output stays private and becomes exact egress-deny evidence.
    pub(super) private_output: Vec<String>,
    /// Provider-reported output tokens for this event, additive across one Work's turns.
    pub(super) tokens: Option<u64>,
    /// The provider-reported executing model makes answer quality attributable to its real source.
    pub(super) model: Option<String>,
    /// Native failure keeps the Work recoverable without inventing a public terminal path.
    pub(super) failed: bool,
    /// Structured Provider failure text preserves the actionable cause instead of a generic label.
    pub(super) failure: Option<String>,
}

// Project one event with current Provider-declared build activity, never inferring intent from a tool name.
pub(super) fn provider_event_with_activity(
    provider: Provider,
    event: &serde_json::Value,
    building: bool,
) -> ProviderEvent {
    match provider {
        Provider::Codex => codex_event(event, building),
        Provider::Claude => claude_event(event, building),
        Provider::Cursor => cursor_event(event, building),
    }
}

// Project Codex thread, item, and turn events without inventing progress.
fn codex_event(event: &serde_json::Value, building: bool) -> ProviderEvent {
    match event.get("type").and_then(serde_json::Value::as_str) {
        Some("thread.started") => ProviderEvent {
            native_session: text_at(event, &["thread_id"]),
            model: text_at(event, &["model"]),
            ..ProviderEvent::default()
        },
        // Item starts expose only their structured stage, never payloads.
        Some("item.started") => ProviderEvent {
            stage: event
                .get("item")
                .and_then(|item| codex_step(item, building)),
            ..ProviderEvent::default()
        },
        Some("item.completed")
            if event
                .get("item")
                .and_then(|item| item.get("type"))
                .and_then(serde_json::Value::as_str)
                == Some("agent_message") =>
        {
            let text = event.get("item").and_then(|item| text_at(item, &["text"]));
            // A message the Agent writes mid-run is that message, not a stage the run reached.
            ProviderEvent {
                stage: text.as_ref().and_then(|_| waiting_stage(event)),
                answer_id: text.as_ref().and_then(|_| text_at(event, &["item", "id"])),
                answer: text,
                ..ProviderEvent::default()
            }
        }
        Some("item.completed") => private_event(codex_private_output(event)),
        // The turn that settles is the one that delivers, so Delivering is named here and nowhere else.
        Some("turn.completed") => ProviderEvent {
            completed: true,
            stage: Some(answer_stage(event)),
            tokens: usage_output(event),
            ..ProviderEvent::default()
        },
        // Item failure can be recovered inside the same native turn; only turn failure is terminal.
        Some("turn.failed") => ProviderEvent {
            failed: true,
            failure: failure_text(event, &[&["error", "message"], &["message"], &["error"]]),
            ..ProviderEvent::default()
        },
        // Codex reports each reconnect to its own model stream as an error frame while the turn
        // keeps running. Its retry is not this turn's end, and its transport prose is not a cause.
        Some("error") if Provider::Codex.retry_notice(event) => ProviderEvent::default(),
        Some("error") => ProviderEvent {
            failed: true,
            failure: failure_text(event, &[&["error", "message"], &["message"], &["error"]]),
            ..ProviderEvent::default()
        },
        _ => ProviderEvent::default(),
    }
}

// Project Claude initialization, assistant, and result events without inventing progress.
fn claude_event(event: &serde_json::Value, building: bool) -> ProviderEvent {
    match event.get("type").and_then(serde_json::Value::as_str) {
        Some("system")
            if event.get("subtype").and_then(serde_json::Value::as_str) == Some("init") =>
        {
            ProviderEvent {
                native_session: text_at(event, &["session_id"]),
                model: text_at(event, &["model"]),
                ..ProviderEvent::default()
            }
        }
        Some("assistant") => {
            let text = claude_message_text(event);
            // Assistant frames are candidates; Claude's result frame alone settles the turn.
            ProviderEvent {
                stage: claude_tool_stage(event, building)
                    .or_else(|| text.as_ref().and_then(|_| waiting_stage(event))),
                answer_id: text
                    .as_ref()
                    .and_then(|_| text_at(event, &["message", "id"])),
                answer: text,
                tokens: event.get("message").and_then(usage_output),
                ..ProviderEvent::default()
            }
        }
        Some("result")
            if event.get("is_error").and_then(serde_json::Value::as_bool) == Some(true) =>
        {
            ProviderEvent {
                failed: true,
                failure: failure_text(event, &[&["result"], &["error"], &["message"]]),
                ..ProviderEvent::default()
            }
        }
        Some("result") => {
            let text = text_at(event, &["result"]);
            ProviderEvent {
                completed: true,
                stage: text.as_ref().map(|_| answer_stage(event)),
                answer: text,
                ..ProviderEvent::default()
            }
        }
        Some("user") => private_event(claude_private_output(event)),
        _ => ProviderEvent::default(),
    }
}

// Project Cursor initialization, assistant, tool, and result events without inventing progress.
fn cursor_event(event: &serde_json::Value, building: bool) -> ProviderEvent {
    match event.get("type").and_then(serde_json::Value::as_str) {
        Some("system")
            if event.get("subtype").and_then(serde_json::Value::as_str) == Some("init") =>
        {
            ProviderEvent {
                native_session: text_at(event, &["session_id"]),
                model: text_at(event, &["model"]),
                ..ProviderEvent::default()
            }
        }
        // Cursor assistant prose remains an answer candidate, never public progress.
        Some("assistant") => {
            let text = claude_message_text(event);
            ProviderEvent {
                stage: text.as_ref().and_then(|_| waiting_stage(event)),
                answer_id: text
                    .as_ref()
                    .and_then(|_| text_at(event, &["message", "id"])),
                answer: text,
                ..ProviderEvent::default()
            }
        }
        Some("tool_call")
            if event.get("subtype").and_then(serde_json::Value::as_str) == Some("started") =>
        {
            ProviderEvent {
                stage: Some(tool_stage(
                    event.get("name").and_then(serde_json::Value::as_str),
                    building,
                )),
                ..ProviderEvent::default()
            }
        }
        // Only the successful result event settles a Cursor turn; native exit codes stay untrusted.
        Some("result")
            if event.get("subtype").and_then(serde_json::Value::as_str) == Some("success")
                && event.get("is_error").and_then(serde_json::Value::as_bool) != Some(true) =>
        {
            ProviderEvent {
                completed: true,
                stage: text_at(event, &["result"])
                    .as_ref()
                    .map(|_| answer_stage(event)),
                answer: text_at(event, &["result"]),
                ..ProviderEvent::default()
            }
        }
        Some("tool_call")
            if event.get("subtype").and_then(serde_json::Value::as_str) == Some("completed") =>
        {
            private_event(values_at(event, &["result", "output"]))
        }
        Some("result") => ProviderEvent {
            failed: true,
            failure: failure_text(event, &[&["result"], &["error"], &["message"]]),
            ..ProviderEvent::default()
        },
        _ => ProviderEvent::default(),
    }
}

fn private_event(output: PrivateOutput) -> ProviderEvent {
    ProviderEvent {
        private_output: output.values,
        ..ProviderEvent::default()
    }
}

fn codex_private_output(event: &serde_json::Value) -> PrivateOutput {
    let Some(item) = event.get("item") else {
        return PrivateOutput::default();
    };
    if item.get("type").and_then(serde_json::Value::as_str) != Some("command_execution") {
        return PrivateOutput::default();
    }
    values_at(item, &["aggregated_output", "output"])
}

fn claude_private_output(event: &serde_json::Value) -> PrivateOutput {
    let mut output = PrivateOutput::default();
    for part in event
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|part| part.get("type").and_then(serde_json::Value::as_str) == Some("tool_result"))
    {
        if let Some(value) = part.get("content") {
            collect_strings(value, &mut output);
        }
    }
    output
}

#[derive(Default)]
struct PrivateOutput {
    values: Vec<String>,
    bytes: usize,
}

fn values_at(value: &serde_json::Value, keys: &[&str]) -> PrivateOutput {
    let mut output = PrivateOutput::default();
    for key in keys {
        if let Some(value) = value.get(*key) {
            collect_strings(value, &mut output);
        }
    }
    output
}

fn collect_strings(value: &serde_json::Value, output: &mut PrivateOutput) {
    match value {
        serde_json::Value::String(value) if !value.is_empty() => {
            if output.values.contains(value) {
                return;
            }
            let fits = value.len() <= MAX_PROTECTED_ITEM_BYTES
                && output.values.len() < MAX_PROTECTED_ITEMS
                && output.bytes.saturating_add(value.len()) <= MAX_PROTECTED_BYTES;
            if fits {
                output.bytes += value.len();
                output.values.push(value.clone());
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_strings(value, output);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values() {
                collect_strings(value, output);
            }
        }
        _ => {}
    }
}

/// WAITING is the one stage that means the Provider parked this turn on the creator.
pub(crate) const WAITING: &str = "Waiting";

// Stage is the complete public vocabulary in increasing precedence order.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Stage {
    Designing,
    Building,
    Verifying,
    Waiting,
    Delivering,
}

impl Stage {
    pub(super) fn label(self) -> String {
        match self {
            Self::Designing => "Designing",
            Self::Building => "Building",
            Self::Verifying => "Verifying",
            Self::Waiting => WAITING,
            Self::Delivering => "Delivering",
        }
        .to_owned()
    }

    /// Building reports that a run already reached its build step, so shell work is no longer design.
    pub(super) fn building(self) -> bool {
        matches!(self, Self::Building | Self::Verifying)
    }

    // Rung is the run's own ladder; Waiting and Delivering settle a turn instead of climbing it.
    fn rung(self) -> Option<u8> {
        match self {
            Self::Designing => Some(0),
            Self::Building => Some(1),
            Self::Verifying => Some(2),
            Self::Waiting | Self::Delivering => None,
        }
    }
}

/// Climb keeps a run's public ladder honest: the step it reached rises and never falls back.
pub(super) fn climb(stage: Stage, reached: &mut Stage) -> Stage {
    match (stage.rung(), reached.rung()) {
        (Some(rung), Some(highest)) => {
            if rung > highest {
                *reached = stage;
            }
            *reached
        }
        _ => stage,
    }
}

// Project one Codex item start into its payload-free structured stage.
fn codex_step(item: &serde_json::Value, building: bool) -> Option<Stage> {
    let stage = match item.get("type").and_then(serde_json::Value::as_str)? {
        // A shell command is work activity, not proof. Only a Provider-declared verification
        // item earns Verifying; ordinary command execution follows the Work's goal.
        "command_execution" => tool_stage(Some("command_execution"), building),
        "file_change" => Stage::Building,
        "web_search" => Stage::Designing,
        "test" | "test_execution" | "verification" | "verify" => Stage::Verifying,
        "mcp_tool_call" => tool_stage(
            item.get("name").and_then(serde_json::Value::as_str),
            building,
        ),
        _ => return None,
    };
    Some(stage)
}

// Select the last Provider tool stage in source order; no artificial precedence dominates a mixed message.
fn claude_tool_stage(event: &serde_json::Value, building: bool) -> Option<Stage> {
    event
        .get("message")?
        .get("content")?
        .as_array()?
        .iter()
        .filter(|part| part.get("type").and_then(serde_json::Value::as_str) == Some("tool_use"))
        .map(|part| {
            tool_stage(
                part.get("name").and_then(serde_json::Value::as_str),
                building,
            )
        })
        .last()
}

// ToolStage maps only structured tool identity; unknown tools remain Designing.
fn tool_stage(name: Option<&str>, building: bool) -> Stage {
    match name.unwrap_or_default().to_ascii_lowercase().as_str() {
        "test" | "tests" | "verify" | "verification" | "run_tests" | "test_runner" => {
            Stage::Verifying
        }
        "bash" | "shell" | "terminal" | "command" | "command_execution" | "exec_command" => {
            if building {
                Stage::Building
            } else {
                Stage::Designing
            }
        }
        "edit" | "write" | "notebookedit" | "file_change" | "apply_patch" => Stage::Building,
        _ => Stage::Designing,
    }
}

// AnswerStage marks a Provider message as Waiting only when the Provider's own turn metadata says so.
fn answer_stage(event: &serde_json::Value) -> Stage {
    if waiting_stage(event).is_some() {
        Stage::Waiting
    } else {
        Stage::Delivering
    }
}

// WaitingStage accepts only explicit Provider waiting markers, never private narration.
fn waiting_stage(event: &serde_json::Value) -> Option<Stage> {
    let waiting = [
        "waiting",
        "awaiting_input",
        "awaiting_user",
        "needs_input",
        "input_required",
        "waiting_for_input",
        "waiting_for_user",
    ];
    for key in waiting {
        if event.get(key).and_then(serde_json::Value::as_bool) == Some(true) {
            return Some(Stage::Waiting);
        }
    }
    for key in ["status", "stop_reason", "stopReason", "subtype"] {
        let value = event
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        if waiting.iter().any(|marker| value == *marker) {
            return Some(Stage::Waiting);
        }
    }
    None
}

// UsageOutput reads the provider-reported output token count carried by one event.
fn usage_output(value: &serde_json::Value) -> Option<u64> {
    value.get("usage")?.get("output_tokens")?.as_u64()
}

// Read every Claude assistant text block in provider order.
fn claude_message_text(event: &serde_json::Value) -> Option<String> {
    let parts = event
        .get("message")?
        .get("content")?
        .as_array()?
        .iter()
        .filter(|part| part.get("type").and_then(serde_json::Value::as_str) == Some("text"))
        .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join(""))
}

// Read a JSON string at a short path without guessing alternate fields.
fn text_at(value: &serde_json::Value, path: &[&str]) -> Option<String> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))?
        .as_str()
        .map(str::to_owned)
}

// FailureText keeps one short, single-line Provider-native cause for repair and user attention.
fn failure_text(value: &serde_json::Value, paths: &[&[&str]]) -> Option<String> {
    paths
        .iter()
        .find_map(|path| text_at(value, path))
        .and_then(|value| {
            let text = value
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(512)
                .collect::<String>();
            (!text.is_empty()).then_some(text)
        })
}
