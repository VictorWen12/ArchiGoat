//! Provider contracts preserve official authentication and autonomous native execution.

mod claude;
mod codex;
mod cursor;

// JSON names providers in the browser contract without leaking CLI details.
use serde::{Deserialize, Serialize};
// OS strings and paths preserve native executable arguments across platforms.
use std::{
    ffi::OsString,
    fmt,
    path::{Path, PathBuf},
};

/// ModelChoice is one selectable model exactly as the Provider's own CLI reports it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModelChoice {
    pub id: String,
    pub label: String,
}

/// PresetChoice is one published quality tier exactly as the product feed states it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct PresetChoice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// PresetPair carries both published quality tiers for one Agent.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct PresetPair {
    #[serde(default)]
    pub best: PresetChoice,
    #[serde(default)]
    pub fast: PresetChoice,
}

/// PresetFile is the published preset map keyed by Agent.
#[derive(Deserialize, Serialize)]
pub struct PresetFile {
    #[serde(default)]
    pub agents: std::collections::HashMap<String, PresetPair>,
}

/// ModelSource is the native mechanism a Provider publishes its model catalog through.
pub(crate) enum ModelSource {
    /// Dialogue holds one protocol exchange open until the catalog answer arrives.
    Dialogue {
        args: Vec<String>,
        input: String,
        finished: fn(&str) -> bool,
    },
    /// Command reads the catalog from one plain CLI command.
    Command(Vec<String>),
    /// Fixed carries the CLI's own documented aliases when it publishes no catalog command.
    Fixed(Vec<ModelChoice>),
}

// Provider selects only official CLI protocol; it never classifies Work.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    // Codex uses the official ChatGPT local CLI.
    Codex,
    // Claude uses the official Anthropic local CLI.
    Claude,
    // Cursor uses the official cursor-agent local CLI.
    Cursor,
}

// LocalCli keeps user text out of command-interpreter arguments.
#[derive(Clone)]
pub struct LocalCli {
    // Program is the native executable launched by the daemon.
    program: PathBuf,
    // Prefix holds trusted wrapper arguments needed before provider arguments.
    prefix: Vec<OsString>,
}

// This local CLI contract invokes each Provider in its supported native mode.
impl LocalCli {
    // Program is the exact operating-system process entrypoint.
    pub fn program(&self) -> &Path {
        &self.program
    }

    // Prefix contains only trusted wrapper arguments before Provider arguments.
    pub fn prefix(&self) -> &[OsString] {
        &self.prefix
    }
}

// This provider identity selects the user's native Agent without semantic routing.
impl Provider {
    // Program names the official Provider executable for local discovery.
    pub fn program(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Cursor => "cursor-agent",
        }
    }

    // Native admission prevents Windows shells from interpreting Work text.
    pub fn accepts_native_program(self, path: &Path) -> bool {
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            return false;
        };
        name.eq_ignore_ascii_case(self.program())
            || name.eq_ignore_ascii_case(&format!("{}.exe", self.program()))
    }

    // Wrapper prefixes are fixed; Work bytes remain on stdin.
    pub fn local_cli(self, path: PathBuf, wrapper: Option<PathBuf>) -> Option<LocalCli> {
        if self.accepts_native_program(&path) {
            return Some(LocalCli {
                program: path,
                prefix: Vec::new(),
            });
        }
        let name = path.file_name()?.to_str()?;
        let fixed = if name.eq_ignore_ascii_case(&format!("{}.ps1", self.program())) {
            vec![
                "-NoLogo",
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ]
        } else if name.eq_ignore_ascii_case(&format!("{}.cmd", self.program())) {
            vec!["/d", "/s", "/c"]
        } else {
            return None;
        };
        Some(LocalCli {
            program: wrapper?,
            prefix: fixed
                .into_iter()
                .map(OsString::from)
                .chain(std::iter::once(path.into_os_string()))
                .collect(),
        })
    }

    // Authentication arguments request official Provider evidence.
    pub(crate) fn auth_status_args(self) -> Vec<String> {
        match self {
            Self::Codex => codex::auth_status_args(),
            Self::Claude => claude::auth_status_args(),
            Self::Cursor => cursor::auth_status_args(),
        }
    }

    // Login arguments preserve the Provider-owned authorization flow.
    pub fn login_args(self) -> Vec<String> {
        match self {
            Self::Codex => codex::login_args(),
            Self::Claude => claude::login_args(),
            Self::Cursor => cursor::login_args(),
        }
    }

    // Work arguments enable native autonomy without overriding tools or resources.
    pub(crate) fn work_args(
        self,
        session: &str,
        resume: bool,
        model: Option<&str>,
        effort: Option<&str>,
        instructions: Option<&str>,
        workspace: &Path,
        readable: &[PathBuf],
    ) -> Result<Vec<String>, String> {
        match self {
            Self::Codex => codex::run_args(resume.then_some(session), model, effort, instructions),
            Self::Claude => claude::run_args(session, resume, model, instructions),
            Self::Cursor => cursor::run_args(resume.then_some(session), model, workspace, readable),
        }
    }

    // Model catalogs come from each Provider's own CLI; ArchiGoat curates no list of its own.
    pub(crate) fn model_source(self) -> ModelSource {
        match self {
            Self::Codex => codex::model_source(),
            Self::Claude => claude::model_source(),
            Self::Cursor => cursor::model_source(),
        }
    }

    // Catalog output parses under the same Provider that produced it.
    pub(crate) fn model_catalog(self, output: &str) -> Vec<ModelChoice> {
        match self {
            Self::Codex => codex::parse_models(output),
            Self::Cursor => cursor::parse_models(output),
            // Claude's catalog is fixed, so there is no output to parse.
            Self::Claude => Vec::new(),
        }
    }

    // Authentication requires explicit Provider evidence; exit success alone is insufficient.
    pub fn authenticated(self, success: bool, output: &str) -> bool {
        if !success {
            return false;
        }
        match self {
            Self::Claude => logged_in(output) == Some(true),
            Self::Codex | Self::Cursor => output.lines().any(affirms),
        }
    }

    // Signed-out state requires explicit Provider evidence.
    pub fn signed_out(self, output: &str) -> bool {
        match self {
            Self::Claude => logged_in(output) == Some(false),
            Self::Codex | Self::Cursor => output.lines().any(denies),
        }
    }

    // Labels expose the Provider identity users selected.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Codex => "ChatGPT",
            Self::Claude => "Claude",
            Self::Cursor => "Cursor",
        }
    }

    // A Provider narrating its own reconnect to its own service is still working on this turn.
    pub(crate) fn retry_notice(self, event: &serde_json::Value) -> bool {
        match self {
            Self::Codex => codex::retry_notice(event),
            // Claude and Cursor publish no retry grammar, so nothing here may guess one for them.
            Self::Claude | Self::Cursor => false,
        }
    }

    // Cursor's native exit codes are unreliable, so only its result event proves completion.
    pub(crate) fn native_completion(self, line: &str) -> bool {
        match self {
            // Codex and Claude exit truthfully, so their status codes stay the completion evidence.
            Self::Codex | Self::Claude => false,
            Self::Cursor => {
                // The cheap contains prefilter keeps per-line cost near zero; the parse is authoritative.
                if !(line.contains("\"type\":\"result\"")
                    && line.contains("\"subtype\":\"success\""))
                {
                    return false;
                }
                serde_json::from_str::<serde_json::Value>(line.trim()).is_ok_and(|event| {
                    event.get("type").and_then(serde_json::Value::as_str) == Some("result")
                        && event.get("subtype").and_then(serde_json::Value::as_str)
                            == Some("success")
                        && event.get("is_error").and_then(serde_json::Value::as_bool) != Some(true)
                })
            }
        }
    }
}

// Denial wording the official CLIs print; a denial outranks any affirmation sharing its line.
const DENIALS: [&str; 5] = [
    "not logged in",
    "not signed in",
    "not authenticated",
    "no account",
    "sign in required",
];

// Affirmation wording is version-loose, so text evidence matches leniently but never over a denial.
const AFFIRMATIONS: [&str; 2] = ["logged in", "authenticated"];

// One text line proves a connected account only when it carries no denial.
fn affirms(line: &str) -> bool {
    let line = line.trim().to_ascii_lowercase();
    AFFIRMATIONS.iter().any(|word| line.contains(word))
        && !DENIALS.iter().any(|word| line.contains(word))
}

// One text line proves the user's account is signed out of that CLI.
fn denies(line: &str) -> bool {
    let line = line.trim().to_ascii_lowercase();
    DENIALS.iter().any(|word| line.contains(word))
}

// Claude publishes an exact JSON login fact, so its evidence never depends on wording.
fn logged_in(output: &str) -> Option<bool> {
    serde_json::from_str::<serde_json::Value>(output)
        .ok()?
        .get("loggedIn")?
        .as_bool()
}

// This display form gives the user a stable, readable native Provider name.
impl fmt::Display for Provider {
    // Display preserves the lowercase Account protocol identity.
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Codex => output.write_str("codex"),
            Self::Claude => output.write_str("claude"),
            Self::Cursor => output.write_str("cursor"),
        }
    }
}
