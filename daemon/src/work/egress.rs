//! One egress gate keeps ArchiGoat's own private facts out of public answers and artifacts.

use super::ArtifactFact;
use std::{fs::File, io::Read, path::Path};

const CHUNK_BYTES: usize = 64 * 1024;
const REDACTED: &str = "[redacted]";
const REPAIR: &str = "Delivery crossed the Work boundary; replace the final answer and every artifact with only the intended user result";

/// A per-Work literal makes disclosure of any privileged build contract mechanically visible.
pub(crate) fn boundary_canary(runner_id: &str) -> String {
    format!("ARCHIGOAT_BOUNDARY_CANARY_{runner_id}")
}

/// ValidateEgress is the single path from native completion to Done, persistence, and upload.
pub(crate) fn validate_egress(
    runner_id: &str,
    session: &Path,
    input_path: &Path,
    freeze_root: &Path,
    answer: &mut Option<String>,
    artifacts: &[ArtifactFact],
) -> Result<(), String> {
    if let Some(answer) = answer {
        redact_answer(runner_id, session, input_path, freeze_root, answer);
    }
    // BOUNDARY: the canary, credentials and this Work's private roots are the only facts a product
    // may not carry. Our own served prose is struck from the answer above and is never a verdict on
    // the creator's files, which legitimately name their own paths and quote what they were built from.
    let needles = secret_needles(runner_id, session, input_path, freeze_root);
    for artifact in artifacts {
        if unsafe_bytes(artifact.name.as_bytes(), &needles)
            || unsafe_file(&artifact.frozen_path, &needles)?
        {
            return Err(REPAIR.to_owned());
        }
    }
    Ok(())
}

/// RedactAnswer removes only proven private spans before any Running snapshot can expose text.
pub(crate) fn redact_answer(
    runner_id: &str,
    session: &Path,
    input_path: &Path,
    freeze_root: &Path,
    answer: &mut String,
) {
    let needles = answer_needles(runner_id, session, input_path, freeze_root);
    redact_machine_paths(answer);
    redact_text(answer, &needles);
}

/// AnswerNeedles joins the exact private facts a Work may never publish, longest first.
fn answer_needles(
    runner_id: &str,
    session: &Path,
    input_path: &Path,
    freeze_root: &Path,
) -> Vec<Vec<u8>> {
    let mut needles = protected_literals(runner_id, session, input_path, freeze_root);
    push_literal(
        &mut needles,
        super::envelope::REPAIR_CONTINUATION.as_bytes(),
    );
    order_needles(&mut needles);
    needles
}

/// SecretNeedles are the facts no product may ever carry: this Work's canary, its private roots and
/// every credential in this environment. Detection of each one is unchanged.
fn secret_needles(
    runner_id: &str,
    session: &Path,
    input_path: &Path,
    freeze_root: &Path,
) -> Vec<Vec<u8>> {
    let mut needles = protected_literals(runner_id, session, input_path, freeze_root);
    order_needles(&mut needles);
    needles
}

fn order_needles(needles: &mut Vec<Vec<u8>>) {
    needles.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    needles.dedup();
}

/// Protected literals are ArchiGoat's own private facts. A product the creator receives names its
/// own files, so what the Agent read back from its workspace is the product, never a private fact.
fn protected_literals(
    runner_id: &str,
    session: &Path,
    input_path: &Path,
    freeze_root: &Path,
) -> Vec<Vec<u8>> {
    let mut values = vec![boundary_canary(runner_id).into_bytes()];
    for path in [session, input_path, freeze_root] {
        push_literal(&mut values, path.to_string_lossy().as_bytes());
    }
    for key in [
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "CODEX_HOME",
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
        "CODEX_ACCESS_TOKEN",
        "ANTHROPIC_API_KEY",
        "CURSOR_API_KEY",
    ] {
        if let Some(value) = std::env::var_os(key) {
            push_literal(&mut values, value.to_string_lossy().as_bytes());
        }
    }
    for (key, value) in std::env::vars_os() {
        let key = key.to_string_lossy().to_ascii_uppercase();
        if ["SECRET", "TOKEN", "PASSWORD", "API_KEY", "CREDENTIAL"]
            .into_iter()
            .any(|marker| key.contains(marker))
        {
            push_literal(&mut values, value.to_string_lossy().as_bytes());
        }
    }
    values.sort();
    values.dedup();
    values
}

/// A short value is not a secret worth matching: "dev" or "true" on a token variable occurs in
/// ordinary product bytes, and holding a correct delivery back over one is a loss with no visible
/// cause, so a literal earns protection only once it is long enough to name something private.
const LITERAL_BYTES: usize = 8;

fn push_literal(values: &mut Vec<Vec<u8>>, value: &[u8]) {
    if value.len() >= LITERAL_BYTES && value.len() <= CHUNK_BYTES {
        values.push(value.to_vec());
    }
}

/// Streaming inspection keeps large user products bounded while applying the same egress policy.
fn unsafe_file(path: &Path, needles: &[Vec<u8>]) -> Result<bool, String> {
    let mut file = File::open(path).map_err(|_| REPAIR.to_owned())?;
    let overlap = needles
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(1)
        .max(256)
        .saturating_sub(1);
    let mut tail = Vec::new();
    let mut chunk = [0_u8; CHUNK_BYTES];
    loop {
        let read = file.read(&mut chunk).map_err(|_| REPAIR.to_owned())?;
        if read == 0 {
            return Ok(false);
        }
        let mut window = Vec::with_capacity(tail.len() + read);
        window.extend_from_slice(&tail);
        window.extend_from_slice(&chunk[..read]);
        if unsafe_bytes(&window, needles) {
            return Ok(true);
        }
        let keep = overlap.min(window.len());
        tail.clear();
        tail.extend_from_slice(&window[window.len() - keep..]);
    }
}

fn unsafe_bytes(bytes: &[u8], needles: &[Vec<u8>]) -> bool {
    needles.iter().any(|needle| contains(bytes, needle))
}

fn redact_text(value: &mut String, needles: &[Vec<u8>]) {
    for needle in needles {
        let Ok(literal) = std::str::from_utf8(needle) else {
            continue;
        };
        if !literal.is_empty() && literal != REDACTED {
            *value = value.replace(literal, REDACTED);
        }
    }
}

fn redact_machine_paths(value: &mut String) {
    let source = std::mem::take(value);
    let mut redacted = String::with_capacity(source.len());
    let mut index = 0;
    while index < source.len() {
        let rest = &source[index..];
        if let Some(prefix) = machine_path_prefix(rest) {
            let end = machine_path_end(&source, index + prefix);
            redacted.push_str(REDACTED);
            index = end;
        } else {
            let character = rest
                .chars()
                .next()
                .expect("path scan index stays on a character");
            redacted.push(character);
            index += character.len_utf8();
        }
    }
    *value = redacted;
}

fn machine_path_prefix(value: &str) -> Option<usize> {
    for prefix in [
        ["/", "Users/"].concat(),
        ["/", "home/"].concat(),
        ["/private", "/var/folders/"].concat(),
    ] {
        if value.starts_with(&prefix) {
            return Some(prefix.len());
        }
    }
    for prefix in [
        ["c:\\", "users\\"].concat().into_bytes(),
        ["c:/", "users/"].concat().into_bytes(),
    ] {
        if value
            .as_bytes()
            .get(..prefix.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(&prefix))
        {
            return Some(prefix.len());
        }
    }
    None
}

fn machine_path_end(value: &str, start: usize) -> usize {
    value[start..]
        .char_indices()
        .find_map(|(offset, character)| {
            (character.is_whitespace()
                || character.is_control()
                || matches!(
                    character,
                    '"' | '\'' | '`' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';'
                ))
            .then_some(start + offset)
        })
        .unwrap_or(value.len())
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && positions(haystack, needle).next().is_some()
}

fn positions<'a>(haystack: &'a [u8], needle: &'a [u8]) -> impl Iterator<Item = usize> + 'a {
    haystack
        .windows(needle.len())
        .enumerate()
        .filter_map(move |(index, window)| (window == needle).then_some(index))
}
