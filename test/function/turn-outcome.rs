//! A turn the Agent finished is finished. A stream that dropped and reconnected inside that turn is
//! progress the Agent recovered from, so it can never turn a delivered answer into a dead turn the
//! product keeps re-running behind the creator's back.

#[path = "../../daemon/src/process/turn.rs"]
mod turn;

// The exact reconnect notice a live Codex turn emitted while it went on to finish its product.
const RECONNECT: &str = "Reconnecting... 5/5 (request timed out)";

fn main() {
    // A stream failure followed by the turn's own completion: the turn delivered.
    let mut outcome = turn::TurnOutcome::new();
    outcome.observe(false, true, Some(RECONNECT.to_owned()));
    assert!(
        outcome.failed(),
        "a turn that only reported a stream failure claimed to be finished",
    );
    outcome.observe(true, false, None);
    assert!(
        outcome.completed(),
        "a completed turn was not recorded as completed",
    );
    assert!(
        !outcome.failed(),
        "a turn that finished after reconnecting was still treated as failed",
    );
    assert_eq!(
        outcome.failure(),
        None,
        "a finished turn still carried a reconnect notice as its cause",
    );

    // The same notice arriving after completion cannot reopen a settled turn either.
    outcome.observe(false, true, Some(RECONNECT.to_owned()));
    assert!(
        outcome.completed() && !outcome.failed(),
        "a late reconnect notice replaced a finished turn's outcome",
    );

    // A turn that died without completing is still repairable, and keeps the Agent's own cause.
    let mut dead = turn::TurnOutcome::new();
    dead.observe(false, true, Some("stream disconnected".to_owned()));
    assert!(dead.failed(), "a dead turn was reported as healthy");
    assert_eq!(
        dead.failure(),
        Some("stream disconnected"),
        "a dead turn lost the cause the Agent reported",
    );
    assert!(
        !dead.completed(),
        "a dead turn claimed the Agent's completion",
    );

    // A turn that ends with neither proof settles as neither: it never claims a silent success.
    let quiet = turn::TurnOutcome::new();
    assert!(
        !quiet.completed() && !quiet.failed() && quiet.failure().is_none(),
        "a turn with no outcome invented one",
    );

    println!("turn outcome proven");
}
