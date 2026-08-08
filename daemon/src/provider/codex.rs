//! Codex receives the Work envelope and keeps policy decisions in its native CLI.

use super::{ModelChoice, ModelSource};

// Native status evidence determines whether Codex is connected.
pub(super) fn auth_status_args() -> Vec<String> {
    words(&["login", "status"])
}

// Codex owns its official interactive authorization flow.
pub(super) fn login_args() -> Vec<String> {
    words(&["login"])
}

/// Codex opens every notice about its own backend stream with this exact word, so one grammar
/// separates a retry still in flight from a cause the creator could act on.
pub(super) const RECONNECTING: &str = "Reconnecting...";

// A reconnect notice reports Codex reopening its own model stream; the turn it belongs to is alive.
pub(super) fn retry_notice(event: &serde_json::Value) -> bool {
    event.get("type").and_then(serde_json::Value::as_str) == Some("error")
        && notice(event).is_some_and(|notice| notice.trim_start().starts_with(RECONNECTING))
}

// The notice text sits wherever Codex puts its error prose, read in that same order.
fn notice(event: &serde_json::Value) -> Option<&str> {
    [&["error", "message"][..], &["message"][..], &["error"][..]]
        .into_iter()
        .find_map(|path| {
            path.iter()
                .try_fold(event, |current, key| current.get(*key))?
                .as_str()
        })
}

// The catalog request identity separates the answer line from protocol noise.
const CATALOG_REQUEST: u64 = 2;

/// Codex reserves its built-in provider names, so the HTTP-from-the-start copy carries its own.
const TRANSPORT: &str = "openai-http";

// Codex publishes its live catalog over its own app-server protocol.
pub(super) fn model_source() -> ModelSource {
    let requests = [
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"clientInfo": {"name": "archigoat", "title": "ArchiGoat", "version": crate::version()}}}),
        serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
        serde_json::json!({"jsonrpc": "2.0", "id": CATALOG_REQUEST, "method": "model/list", "params": {}}),
    ];
    ModelSource::Dialogue {
        args: words(&["app-server"]),
        input: requests.map(|request| format!("{request}\n")).concat(),
        finished: catalog_line,
    }
}

// The catalog answer is the one line whose identity matches the model/list request.
fn catalog_line(line: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|message| message.get("id")?.as_u64())
        == Some(CATALOG_REQUEST)
}

// The catalog keeps Codex's own order and visibility rules.
pub(super) fn parse_models(line: &str) -> Vec<ModelChoice> {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|message| message.get("result")?.get("data")?.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|item| {
            if item.get("hidden").and_then(serde_json::Value::as_bool) == Some(true) {
                return None;
            }
            let id = item.get("id")?.as_str()?;
            let label = item
                .get("displayName")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(id);
            Some(ModelChoice {
                id: id.to_owned(),
                label: label.to_owned(),
            })
        })
        .collect()
}

// Codex launches natively with only session, model, effort, and protocol arguments.
pub(super) fn run_args(
    session: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
    instructions: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut args = words(&["exec"]);
    if session.is_some() {
        args.push("resume".to_owned());
    }
    if let Some(session) = session {
        args.push(session.to_owned());
    }
    if let Some(model) = model {
        args.extend(["--model".to_owned(), model.to_owned()]);
    }
    if let Some(effort) = effort {
        config(&mut args, "model_reasoning_effort", json_string(effort));
    }
    if let Some(instructions) = instructions {
        config(
            &mut args,
            "developer_instructions",
            json_string(instructions),
        );
    }
    // Codex reaches its own backend over WebSockets first and only falls back to HTTP after five
    // timed-out sampling retries, so a Work would pay that ladder before its first token. Its
    // built-in provider entry is sealed, and Codex itself names the remedy: a renamed copy carrying
    // the same official identity. This one is HTTP from the start, and only for this launch.
    args.extend([
        "-c".to_owned(),
        format!("model_providers.{TRANSPORT} = {{ name = \"OpenAI\", wire_api = \"responses\", requires_openai_auth = true, supports_standalone_web_search = true, supports_websockets = false }}"),
    ]);
    config(&mut args, "model_provider", json_string(TRANSPORT));
    // The machine's own skill library belongs to the person at this keyboard, not to a Work: its
    // instructions turn one build into scaffolding and verification rounds. Its skills stay
    // installed and stay theirs; this launch simply is not told about them.
    config(&mut args, "skills.include_instructions", "false".to_owned());
    args.extend(words(&[
        "--skip-git-repo-check",
        "--dangerously-bypass-approvals-and-sandbox",
        "--json",
    ]));
    if session.is_some() {
        args.push("-".to_owned());
    }
    Ok(args)
}

fn config(args: &mut Vec<String>, key: &str, value: String) {
    args.extend(["-c".to_owned(), format!("{key}={value}")]);
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("a string always serializes")
}

fn words(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}
