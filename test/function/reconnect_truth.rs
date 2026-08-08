//! A Provider reconnecting to its own service is still working. This drives the shipped observer and
//! the shipped Provider grammar over one scripted native journal: a reconnect notice must not end the
//! turn, a turn that recovered and completed must deliver, and a real cause must still reach the user.

// The production sources under test compile here unchanged; only their platform seams are scripted.
#![allow(dead_code)]

#[path = "../../daemon/src/execution.rs"]
mod execution;
#[path = "../../daemon/src/provider/mod.rs"]
mod provider;
#[path = "../../daemon/src/trace.rs"]
mod trace;

// Process keeps the real module shape, so the observer resolves its own event grammar.
#[path = "."]
mod process {
    #[path = "../../daemon/src/process/observe.rs"]
    pub(crate) mod observe;
    #[path = "../../daemon/src/process/provider_events.rs"]
    pub(crate) mod provider_events;
    #[path = "../../daemon/src/process/turn.rs"]
    pub(crate) mod turn;
}

// Work supplies only the evidence bounds the decoder reads; no bound is under test here.
mod work {
    pub(crate) const MAX_PROTECTED_ITEMS: usize = 64;
    pub(crate) const MAX_PROTECTED_BYTES: usize = 256 * 1024;
    pub(crate) const MAX_PROTECTED_ITEM_BYTES: usize = 64 * 1024;
    pub(crate) mod runtime {
        pub(crate) const NETWORK: bool = true;
    }
}

// Proof and version exist for Provider code this check does not exercise.
mod proof {
    pub(crate) fn valid_nonce(_name: &str) -> bool {
        true
    }
}

fn version() -> &'static str {
    "test"
}

// State scripts owner authority as untouched: every turn here ends on its own native evidence.
mod state {
    #[derive(Clone)]
    pub(crate) struct OwnerStop;

    impl OwnerStop {
        pub(crate) fn requested(&self) -> bool {
            false
        }
    }

    #[derive(Clone)]
    pub(crate) struct TurnStop;

    impl TurnStop {
        pub(crate) fn requested(&self) -> bool {
            false
        }
    }
}

// Host replays one already-durable journal, exactly as the runner would have written it.
mod host {
    use crate::{
        execution::AgentFrame,
        state::{OwnerStop, TurnStop},
    };

    pub(crate) struct AgentRun(std::vec::IntoIter<AgentFrame>);

    impl AgentRun {
        pub(crate) fn scripted(frames: Vec<AgentFrame>) -> Self {
            Self(frames.into_iter())
        }

        /// No machine took these runners down; every turn here ends on its own native evidence.
        pub(crate) fn machine_stop_cause(&self) -> Option<String> {
            None
        }

        pub(crate) async fn next(
            &mut self,
            _stop: OwnerStop,
            _turn: TurnStop,
        ) -> Result<AgentFrame, String> {
            self.0
                .next()
                .ok_or_else(|| "the scripted journal ended without a terminal frame".to_owned())
        }
    }
}

use execution::{AgentEvent, AgentFrame};
use process::observe::{ObservedWork, Observer};
use provider::Provider;

/// The exact notice the Codex CLI prints while it reopens its own model stream.
const RECONNECT: &str = r#"{"type":"error","message":"Reconnecting... 5/5 (request timed out)"}"#;
/// A stream fault Codex reports without a retry in flight.
const STREAM_FAULT: &str = r#"{"type":"error","message":"stream disconnected before completion: Connection reset by peer"}"#;
/// A cause the creator can actually act on.
const USAGE_LIMIT: &str = r#"{"type":"error","message":"You've hit your usage limit."}"#;

const STARTED: &str = r#"{"type":"thread.started","thread_id":"thread-1","model":"gpt-5"}"#;
const ANSWER: &str = r#"{"type":"item.completed","item":{"type":"agent_message","id":"m1","text":"Your runner game is ready."}}"#;
const COMPLETED: &str = r#"{"type":"turn.completed","usage":{"output_tokens":42}}"#;

#[tokio::test]
async fn a_provider_reconnecting_to_its_own_service_still_delivers_its_turn() {
    let delivered = run(&[STARTED, RECONNECT, ANSWER, COMPLETED])
        .await
        .expect("a Provider retry ended a turn that went on to complete");
    assert_eq!(
        delivered.answer.as_deref(),
        Some("Your runner game is ready."),
        "a Provider retry cost the creator the answer of that same turn",
    );
    assert_eq!(
        delivered.native_session, "thread-1",
        "the delivered turn left its own native conversation",
    );
}

#[tokio::test]
async fn a_turn_that_completed_after_a_stream_fault_delivers() {
    let delivered = run(&[STARTED, STREAM_FAULT, ANSWER, COMPLETED])
        .await
        .expect("a recovered stream fault outranked the Provider's own completion");
    assert_eq!(
        delivered.answer.as_deref(),
        Some("Your runner game is ready."),
        "a turn that recovered and completed delivered nothing",
    );
}

#[tokio::test]
async fn a_reconnecting_turn_that_never_completes_never_reports_transport_prose() {
    let failure = cause(
        run(&[STARTED, RECONNECT]).await,
        "a turn that never completed reported success",
    );
    assert_eq!(
        failure, "Local Agent ended without native completion",
        "the Provider's own transport prose became this Work's reported cause",
    );
}

#[tokio::test]
async fn a_real_provider_cause_still_reaches_the_creator() {
    let failure = cause(
        run(&[STARTED, USAGE_LIMIT]).await,
        "a usage limit was treated as a recoverable retry",
    );
    assert_eq!(
        failure, "You've hit your usage limit.",
        "a cause the creator can act on lost its own words",
    );
    let failed_turn = cause(
        run(&[
            STARTED,
            r#"{"type":"turn.failed","error":{"message":"The model refused this turn."}}"#,
        ])
        .await,
        "a failed turn reported success",
    );
    assert_eq!(
        failed_turn, "The model refused this turn.",
        "a natively failed turn lost its own cause",
    );
}

/// A reconnect is read from the Provider's own structured grammar and from nothing else. No count of
/// them ends a turn, so the only thing this grammar decides is what the machine's log says while the
/// Agent keeps working — and a real cause must never be mistaken for one.
#[tokio::test]
async fn one_reconnect_grammar_is_read_from_the_provider_itself() {
    assert!(
        notice(Provider::Codex, RECONNECT),
        "a Codex turn inside its own reconnect window went unreported",
    );
    assert!(
        !notice(Provider::Codex, USAGE_LIMIT),
        "a real cause was read as a retry in flight",
    );
    assert!(
        !notice(Provider::Codex, STREAM_FAULT),
        "a stream fault with no retry in flight was read as one",
    );
    assert!(
        !notice(Provider::Claude, RECONNECT),
        "a retry grammar was invented for a Provider that publishes none",
    );
    assert!(
        serde_json::from_str::<serde_json::Value>("Reconnecting... 1/5 (request timed out)")
            .is_err(),
        "unstructured prose parsed as Provider grammar",
    );
}

/// Notice reads one journaled Provider line exactly as the decoder does.
fn notice(provider: Provider, line: &str) -> bool {
    let value = serde_json::from_str::<serde_json::Value>(line.trim())
        .expect("this Provider line was not a record at all");
    provider.retry_notice(&value)
}

// Cause keeps the exact words one ended turn reported, and never lets a delivered turn pass as one.
fn cause(settled: Result<ObservedWork, String>, complaint: &str) -> String {
    match settled {
        Ok(_) => panic!("{complaint}"),
        Err(cause) => cause,
    }
}

// Run observes one Codex journal built from these native lines and its own terminal frame.
async fn run(lines: &[&str]) -> Result<ObservedWork, String> {
    let mut frames = lines
        .iter()
        .enumerate()
        .map(|(index, line)| AgentFrame {
            sequence: index as u64 + 1,
            event: AgentEvent::Stdout(format!("{line}\n").into_bytes()),
        })
        .collect::<Vec<_>>();
    frames.push(AgentFrame {
        sequence: lines.len() as u64 + 1,
        event: AgentEvent::Done,
    });
    let mut run = host::AgentRun::scripted(frames);
    let mut observer = Observer::new(Provider::Codex, 0);
    let settled = observer
        .observe(
            &mut run,
            state::OwnerStop,
            state::TurnStop,
            |_, _| Ok(()),
            |_| Ok(()),
            |_| Ok(()),
            |_| Ok(()),
            |_| Ok(()),
            |_, _| Ok(()),
        )
        .await?;
    settled.ok_or_else(|| "the turn ended without settling".to_owned())
}
