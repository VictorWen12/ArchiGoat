//! Claude receives the Work envelope through its native print protocol.

use super::{ModelChoice, ModelSource};

// Native status evidence determines whether Claude is connected.
pub(super) fn auth_status_args() -> Vec<String> {
    words(&["auth", "status"])
}

// Claude owns its official interactive authorization flow.
pub(super) fn login_args() -> Vec<String> {
    words(&["auth", "login", "--claudeai"])
}

// Claude publishes no catalog command; these are its CLI's own documented model aliases.
pub(super) fn model_source() -> ModelSource {
    ModelSource::Fixed(
        [("fable", "Fable"), ("opus", "Opus"), ("sonnet", "Sonnet")]
            .into_iter()
            .map(|(id, label)| ModelChoice {
                id: id.to_owned(),
                label: label.to_owned(),
            })
            .collect(),
    )
}

// Claude launches with only the native protocol needed by Work.
pub(super) fn run_args(
    session: &str,
    resume: bool,
    model: Option<&str>,
    instructions: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut args = words(&["-p"]);
    args.extend(words(&["--output-format", "stream-json", "--verbose"]));
    args.push("--dangerously-skip-permissions".to_owned());
    // The machine's own skill library belongs to the person at this keyboard, not to a Work: its
    // instructions turn one build into scaffolding and verification rounds. Its skills stay
    // installed and stay theirs; this launch simply is not told about them.
    args.push("--disable-slash-commands".to_owned());
    if let Some(instructions) = instructions {
        args.extend(["--append-system-prompt".to_owned(), instructions.to_owned()]);
    }
    if let Some(model) = model {
        args.extend(["--model".to_owned(), model.to_owned()]);
    }
    args.push(if resume { "--resume" } else { "--session-id" }.to_owned());
    args.push(session.to_owned());
    Ok(args)
}

fn words(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}
