//! Cursor has no privileged instruction channel, so it receives Work plus selected attachment copies.

use super::{ModelChoice, ModelSource};
use std::path::{Path, PathBuf};

// Native status evidence determines whether Cursor is connected.
pub(super) fn auth_status_args() -> Vec<String> {
    words(&["status"])
}

// Cursor owns its official browser authorization flow.
pub(super) fn login_args() -> Vec<String> {
    words(&["login"])
}

// Cursor publishes its live catalog through its own models command.
pub(super) fn model_source() -> ModelSource {
    ModelSource::Command(words(&["models"]))
}

// Each catalog line reads `id - label`; every other line is prose.
pub(super) fn parse_models(output: &str) -> Vec<ModelChoice> {
    output
        .lines()
        .filter_map(|line| {
            let (id, label) = line.trim().split_once(" - ")?;
            (!id.is_empty() && !id.contains(' ')).then(|| ModelChoice {
                id: id.to_owned(),
                label: label.trim().to_owned(),
            })
        })
        .collect()
}

// Cursor receives the Work workspace and selected attachments through official native arguments.
pub(super) fn run_args(
    session: Option<&str>,
    model: Option<&str>,
    workspace: &Path,
    readable: &[PathBuf],
) -> Result<Vec<String>, String> {
    let workspace = workspace
        .to_str()
        .ok_or_else(|| "Cursor workspace path is not valid Unicode".to_owned())?;
    let mut args = words(&[
        "-p",
        "--workspace",
        workspace,
        "--output-format",
        "stream-json",
        "--force",
        "--sandbox",
        "disabled",
        "--approve-mcps",
        "--trust",
    ]);
    let mut roots: Vec<&Path> = Vec::new();
    for input in readable {
        let root = input
            .parent()
            .ok_or_else(|| "Cursor input view path is invalid".to_owned())?;
        if root
            .parent()
            .and_then(|parent| parent.file_name())
            .is_none_or(|name| name != "InputViews")
            || root
                .file_name()
                .and_then(|name| name.to_str())
                .is_none_or(|name| !crate::proof::valid_nonce(name))
        {
            return Err("Cursor input view path is invalid".to_owned());
        }
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    for root in roots {
        args.push("--add-dir".to_owned());
        args.push(
            root.to_str()
                .ok_or_else(|| "Cursor input parent is not valid Unicode".to_owned())?
                .to_owned(),
        );
    }
    if let Some(model) = model {
        args.extend(["--model".to_owned(), model.to_owned()]);
    }
    if let Some(session) = session {
        args.extend(words(&["--resume", session]));
    }
    Ok(args)
}

fn words(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}
