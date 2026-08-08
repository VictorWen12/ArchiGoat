//! Only the Provider's own death or the owner's Stop ends a turn. This drives the shipped observer,
//! the shipped decoder, and both shipped journals over the defects that used to throw finished work
//! away: a session header that never arrived, frames lost to a torn append, a line and a record too
//! big to read, every bookkeeping write refusing to land, a frame kind this build never wrote, and a
//! machine that took the runner down. In every one of them the turn keeps running and its product
//! reaches the person who asked.

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

    /// The frame kinds every platform journal numbers its facts with.
    pub(crate) const STDOUT: u8 = 1;
    pub(crate) const STDERR: u8 = 2;
    pub(crate) const DONE: u8 = 3;
    pub(crate) const STOPPED: u8 = 4;
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

// Host replays one already-durable journal, exactly as the runner would have written it, and reports
// a machine-imposed end exactly as the runner's own marker does.
mod host {
    use crate::{
        execution::AgentFrame,
        state::{OwnerStop, TurnStop},
    };

    pub(crate) struct AgentRun {
        frames: std::vec::IntoIter<AgentFrame>,
        machine_stop: Option<String>,
    }

    impl AgentRun {
        pub(crate) fn scripted(frames: Vec<AgentFrame>) -> Self {
            Self {
                frames: frames.into_iter(),
                machine_stop: None,
            }
        }

        pub(crate) fn stopped_by_this_machine(frames: Vec<AgentFrame>, cause: &str) -> Self {
            Self {
                frames: frames.into_iter(),
                machine_stop: Some(cause.to_owned()),
            }
        }

        pub(crate) fn machine_stop_cause(&self) -> Option<String> {
            self.machine_stop.clone()
        }

        pub(crate) async fn next(
            &mut self,
            _stop: OwnerStop,
            _turn: TurnStop,
        ) -> Result<AgentFrame, String> {
            self.frames
                .next()
                .ok_or_else(|| "the scripted journal ended without a terminal frame".to_owned())
        }
    }
}

use execution::{AgentEvent, AgentFrame};
use process::observe::{ObservedWork, Observer};
use provider::Provider;

const STARTED: &str = r#"{"type":"thread.started","thread_id":"thread-1","model":"gpt-5"}"#;
const REBOUND: &str = r#"{"type":"thread.started","thread_id":"thread-2","model":"gpt-5"}"#;
const BUILDING: &str = r#"{"type":"item.started","item":{"id":"i1","type":"file_change"}}"#;
const ANSWER: &str = r#"{"type":"item.completed","item":{"type":"agent_message","id":"m1","text":"Your runner game is ready."}}"#;
const COMPLETED: &str = r#"{"type":"turn.completed","usage":{"output_tokens":42}}"#;
const PRODUCT: &str = "Your runner game is ready.";

/// A Provider that finished its turn and never named its own conversation still delivers: the Work is
/// already bound to the session that produced this, and the product is what the person is waiting for.
#[tokio::test]
async fn a_finished_turn_delivers_without_a_session_header() {
    let delivered = deliver(&[ANSWER, COMPLETED]).await;
    assert_eq!(
        delivered.answer.as_deref(),
        Some(PRODUCT),
        "a finished turn lost its product because its stream never repeated the session header",
    );
}

/// Frames a torn append lost are lost frames, not a dead Work: what survived still delivers.
#[tokio::test]
async fn a_journal_that_lost_frames_keeps_observing() {
    // Frames 3 and 4 never reached the disk; 5 and 6 did.
    let frames = vec![
        stdout(1, STARTED),
        stdout(2, BUILDING),
        stdout(5, ANSWER),
        stdout(6, COMPLETED),
        AgentFrame {
            sequence: 7,
            event: AgentEvent::Done,
        },
    ];
    let (settled, watched) = observe(host::AgentRun::scripted(frames)).await;
    assert_eq!(
        settled.expect("a journal that lost frames threw away the turn that survived them"),
        PRODUCT,
        "a torn journal cost this turn the product the Agent finished",
    );
    assert_eq!(
        watched.sessions,
        ["thread-1"],
        "a turn that survived a torn journal lost its own conversation",
    );
}

/// A Provider that answers with a conversation of its own naming rebinds to it and delivers. Two
/// authorities disagreeing about the session is what used to make a follow-up never arrive at all.
#[tokio::test]
async fn a_provider_that_rebinds_its_own_session_still_delivers() {
    let (settled, watched) = observe(host::AgentRun::scripted(journal(&[
        STARTED, BUILDING, REBOUND, ANSWER, COMPLETED,
    ])))
    .await;
    assert_eq!(
        settled.expect("a Provider that renamed its own conversation lost the turn"),
        PRODUCT,
        "a rebound session cost this turn its finished product",
    );
    assert_eq!(
        watched.sessions,
        ["thread-1", "thread-2"],
        "the Work was never told which conversation its Agent moved to",
    );
}

/// A line that is not a record costs that line. Everything the Agent said after it, inside the very
/// same frame, still reaches the person who asked.
#[tokio::test]
async fn an_unreadable_line_costs_only_that_line() {
    // One operating-system frame carrying a broken line and then the whole rest of the turn.
    let frame = format!("{STARTED}\n{{\"type\":\"item.st\n{ANSWER}\n{COMPLETED}\n");
    let frames = vec![
        AgentFrame {
            sequence: 1,
            event: AgentEvent::Stdout(frame.into_bytes()),
        },
        AgentFrame {
            sequence: 2,
            event: AgentEvent::Done,
        },
    ];
    let (settled, watched) = observe(host::AgentRun::scripted(frames)).await;
    assert_eq!(
        settled.expect("one unreadable line consumed the frame that held the finished turn"),
        PRODUCT,
        "a broken line took the rest of its own frame with it",
    );
    assert_eq!(
        watched.answers.len(),
        1,
        "the Agent's message reached the creator more than once",
    );
}

/// A record too large to hold is released at its own newline; the turn keeps decoding and delivers.
#[tokio::test]
async fn a_record_too_large_to_hold_costs_only_that_record() {
    let oversized = format!(
        "{{\"type\":\"item.completed\",\"pad\":\"{}",
        "x".repeat(1_100_000)
    );
    let frames = vec![
        stdout(1, STARTED),
        AgentFrame {
            sequence: 2,
            event: AgentEvent::Stdout(oversized.into_bytes()),
        },
        // The rest of that oversized record arrives, ends at its own newline, and the turn goes on.
        AgentFrame {
            sequence: 3,
            event: AgentEvent::Stdout(format!("\"}}\n{ANSWER}\n{COMPLETED}\n").into_bytes()),
        },
        AgentFrame {
            sequence: 4,
            event: AgentEvent::Done,
        },
    ];
    let (settled, _) = observe(host::AgentRun::scripted(frames)).await;
    assert_eq!(
        settled.expect("one record too large to hold poisoned every frame behind it"),
        PRODUCT,
        "a record this machine could not hold cost the creator the finished product",
    );
}

/// Bookkeeping never outranks the Work. Every display and evidence write refusing to land does not
/// end a live turn, and the product still delivers.
#[tokio::test]
async fn bookkeeping_that_will_not_commit_never_ends_the_turn() {
    let mut run = host::AgentRun::scripted(journal(&[STARTED, BUILDING, ANSWER, COMPLETED]));
    let mut observer = Observer::new(Provider::Codex, 0);
    let refuse = || Err("the disk is busy".to_owned());
    let settled = observer
        .observe(
            &mut run,
            state::OwnerStop,
            state::TurnStop,
            |_, _| refuse(),
            |_| refuse(),
            |_| refuse(),
            |_| refuse(),
            |_| refuse(),
            |_, _| refuse(),
        )
        .await
        .expect("a bookkeeping write that needed another attempt ended a live turn")
        .expect("a finished turn never settled");
    assert_eq!(
        settled.answer.as_deref(),
        Some(PRODUCT),
        "a Work whose bookkeeping stalled lost the product the Agent finished",
    );
}

/// A stop this machine imposed — a restart, a logout, a reload — says so in its own words, so it is
/// never accounted as a Provider that died on the user's request.
#[tokio::test]
async fn a_machine_stop_is_not_a_provider_that_died() {
    let ended = vec![
        stdout(1, STARTED),
        stdout(2, BUILDING),
        AgentFrame {
            sequence: 3,
            event: AgentEvent::Stopped,
        },
    ];
    let mut observer = Observer::new(Provider::Codex, 0);
    let mut run = host::AgentRun::stopped_by_this_machine(ended, "terminate");
    let settled = run_observer(&mut observer, &mut run).await;
    assert!(
        settled.is_ok() && observer.stopped(),
        "a runner this machine ended was not reported as a stop at all",
    );
    assert_eq!(
        run.machine_stop_cause(),
        Some("terminate".to_owned()),
        "a stop this machine imposed reached the Work with no cause of its own",
    );

    let mut observer = Observer::new(Provider::Codex, 0);
    let mut run = host::AgentRun::scripted(vec![
        stdout(1, STARTED),
        AgentFrame {
            sequence: 2,
            event: AgentEvent::Stopped,
        },
    ]);
    let _ = run_observer(&mut observer, &mut run).await;
    assert_eq!(
        run.machine_stop_cause(),
        None,
        "a runner that ended for its own reasons was blamed on this machine",
    );
}

/// Quiet is not death. A quiet report left in the journal by a previous generation of the runner
/// names a command, never an ending, and the turn it belongs to still delivers.
#[tokio::test]
async fn a_quiet_report_never_ends_a_turn() {
    let frames = vec![
        stdout(1, STARTED),
        AgentFrame {
            sequence: 2,
            event: AgentEvent::Stalled,
        },
        stdout(3, ANSWER),
        stdout(4, COMPLETED),
        AgentFrame {
            sequence: 5,
            event: AgentEvent::Done,
        },
    ];
    let (settled, _) = observe(host::AgentRun::scripted(frames)).await;
    assert_eq!(
        settled.expect("a quiet command ended a turn the Agent went on to finish"),
        PRODUCT,
        "a report that a command was quiet cost the creator the finished product",
    );

    // And a turn that really did end reports how it ended, never a verdict about its own quiet.
    let ended = vec![
        stdout(1, STARTED),
        AgentFrame {
            sequence: 2,
            event: AgentEvent::Stalled,
        },
        AgentFrame {
            sequence: 3,
            event: AgentEvent::Done,
        },
    ];
    let (settled, _) = observe(host::AgentRun::scripted(ended)).await;
    assert_eq!(
        settled.expect_err("a turn with no completion was reported as delivered"),
        "Local Agent ended without native completion",
        "a quiet command became this Work's reported cause",
    );
}

/// Watched records every public change one observation published.
#[derive(Default)]
struct Watched {
    answers: Vec<String>,
    sessions: Vec<String>,
}

/// Deliver observes one Codex turn built from these native lines and requires its product.
async fn deliver(lines: &[&str]) -> ObservedWork {
    let mut run = host::AgentRun::scripted(journal(lines));
    let mut observer = Observer::new(Provider::Codex, 0);
    run_observer(&mut observer, &mut run)
        .await
        .expect("a finished turn was thrown away")
        .expect("a finished turn never settled")
}

/// Observe runs one epoch and keeps both the delivered answer and everything it published.
async fn observe(mut run: host::AgentRun) -> (Result<String, String>, Watched) {
    let mut observer = Observer::new(Provider::Codex, 0);
    let answers = std::cell::RefCell::new(Vec::new());
    let sessions = std::cell::RefCell::new(Vec::new());
    let settled = observer
        .observe(
            &mut run,
            state::OwnerStop,
            state::TurnStop,
            |_, _| Ok(()),
            |session| {
                sessions.borrow_mut().push(session);
                Ok(())
            },
            |_| Ok(()),
            |_| Ok(()),
            |_| Ok(()),
            |_, answer| {
                answers.borrow_mut().push(answer.to_owned());
                Ok(())
            },
        )
        .await;
    let watched = Watched {
        answers: answers.into_inner(),
        sessions: sessions.into_inner(),
    };
    let settled = match settled {
        Ok(Some(delivered)) => Ok(delivered.answer.unwrap_or_default()),
        Ok(None) => Err("the turn ended without settling".to_owned()),
        Err(cause) => Err(cause),
    };
    (settled, watched)
}

/// RunObserver drives one epoch with every published fact discarded.
async fn run_observer(
    observer: &mut Observer,
    run: &mut host::AgentRun,
) -> Result<Option<ObservedWork>, String> {
    observer
        .observe(
            run,
            state::OwnerStop,
            state::TurnStop,
            |_, _| Ok(()),
            |_| Ok(()),
            |_| Ok(()),
            |_| Ok(()),
            |_| Ok(()),
            |_, _| Ok(()),
        )
        .await
}

/// Journal turns native lines into the ordered frames a runner would have written, then its own end.
fn journal(lines: &[&str]) -> Vec<AgentFrame> {
    let mut frames = lines
        .iter()
        .enumerate()
        .map(|(index, line)| stdout(index as u64 + 1, line))
        .collect::<Vec<_>>();
    frames.push(AgentFrame {
        sequence: lines.len() as u64 + 1,
        event: AgentEvent::Done,
    });
    frames
}

/// Stdout is one journaled Provider line at its exact durable sequence.
fn stdout(sequence: u64, line: &str) -> AgentFrame {
    AgentFrame {
        sequence,
        event: AgentEvent::Stdout(format!("{line}\n").into_bytes()),
    }
}

// The Apple journal keeps reading past a frame this build never wrote, and the Apple observer settles
// a turn whose runner is gone instead of watching its file for the rest of the Work's life.
#[path = "."]
mod apple {
    /// The one private write the observer under test performs; the owner-Stop file is not exercised.
    pub(crate) fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
        std::fs::write(path, bytes)
            .map_err(|error| format!("Could not create private state: {error}"))
    }

    #[path = "."]
    pub(crate) mod terminal {
        #[path = "../../daemon/src/apple/terminal/journal.rs"]
        pub(crate) mod journal;
        #[path = "../../daemon/src/apple/terminal/model.rs"]
        pub(crate) mod model;
        #[path = "../../daemon/src/apple/terminal/observer.rs"]
        pub(crate) mod observer;

        use crate::execution::AgentEvent;
        use journal::{append_event, read_event};
        use model::{DONE, STDOUT};

        /// A runner that is gone leaves a journal with no end in it. The Work settles on what that
        /// journal holds — it never reports Running for the rest of its life waiting for a process
        /// that cannot write again — and the end is durable for whoever reads the journal next.
        #[tokio::test]
        async fn a_runner_that_is_gone_settles_the_turn_it_left() {
            let root = scratch("apple-gone-runner");
            append_event(&root, 1, STDOUT, b"{\"type\":\"thread.started\"}\n")
                .expect("the journal refused a Provider line");
            append_event(&root, 2, STDOUT, b"{\"type\":\"turn.completed\"}\n")
                .expect("the journal refused a Provider line");

            let mut run =
                observer::AgentRun::new(root.clone(), Some(ended().await), "s".to_owned());
            assert_eq!(
                read_until_end(&mut run).await,
                2,
                "a runner that is gone lost the output it had already journaled",
            );
            assert_eq!(
                run.terminal_sequence(),
                Some(3),
                "a turn whose runner is gone never reached its own end",
            );
            let mut offset = 0;
            let mut ends = 0;
            while let Some((frame, next)) =
                read_event(&root, offset).expect("the journal is unreadable")
            {
                if matches!(frame.event, AgentEvent::Done) {
                    ends += 1;
                }
                offset = next;
            }
            assert_eq!(
                ends, 1,
                "the end written on a dead runner's behalf was written the wrong number of times",
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        /// A journal that will not take that end no longer costs the Work its turn: a runner's end is a
        /// fact about the runner, never a write the person waiting has to see land.
        #[tokio::test]
        async fn an_end_that_cannot_be_journaled_still_reaches_the_work() {
            let root = scratch("apple-unwritable-end");
            append_event(&root, 1, STDOUT, b"{\"type\":\"turn.completed\"}\n")
                .expect("the journal refused a Provider line");
            seal(&root.join(model::EVENTS));

            let mut run =
                observer::AgentRun::new(root.clone(), Some(ended().await), "s".to_owned());
            assert_eq!(
                read_until_end(&mut run).await,
                1,
                "a journal that could not take one more record threw away the turn it already held",
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        /// A Work that attached to a runner instead of starting one has no child to wait on. Nothing
        /// ever claimed that journal, so it is answered as gone rather than published as live and
        /// watched forever.
        #[tokio::test]
        async fn an_attached_runner_nothing_ever_claimed_is_answered_as_gone() {
            let root = scratch("apple-attached-runner");
            append_event(&root, 1, STDOUT, b"{\"type\":\"turn.completed\"}\n")
                .expect("the journal refused a Provider line");

            let mut run = observer::AgentRun::new(root.clone(), None, "s".to_owned());
            assert!(
                !run.fresh_liveness().await,
                "a runner nothing ever claimed left its Work waiting on a liveness that cannot come",
            );
            assert_eq!(
                read_until_end(&mut run).await,
                1,
                "an attached runner that is gone left its Work reporting Running with no end at all",
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        /// ReadUntilEnd drives one observation to its terminal fact and counts the output before it.
        async fn read_until_end(run: &mut observer::AgentRun) -> usize {
            let mut lines = 0;
            for _ in 0..8 {
                let frame = run
                    .next(crate::state::OwnerStop, crate::state::TurnStop)
                    .await
                    .expect("this journal could not be read at all");
                match frame.event {
                    AgentEvent::Stdout(_) => lines += 1,
                    AgentEvent::Done | AgentEvent::Stopped => return lines,
                    _ => {}
                }
            }
            panic!("a Work whose runner is gone never settled");
        }

        /// Ended is one real process that has already exited: the physical death of a runner.
        async fn ended() -> tokio::process::Child {
            let mut child = tokio::process::Command::new("/usr/bin/true")
                .spawn()
                .expect("this machine would not start a process that exits");
            child.wait().await.expect("that process never ended");
            child
        }

        /// Seal makes one journal file refuse every further record.
        fn seal(path: &std::path::Path) {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o400))
                .expect("this machine would not seal the journal");
        }

        /// A frame of a kind this build never wrote is one released frame. The Work reads past it to
        /// the frames that follow, because a byte it could not name proves nothing about the turn.
        #[test]
        fn a_frame_this_build_cannot_name_is_read_past() {
            let root = scratch("apple-unknown-kind");
            append_event(&root, 1, STDOUT, b"{\"type\":\"thread.started\"}\n")
                .expect("the journal refused a Provider line");
            // A kind from another generation of this runner, and its own quiet report beside it.
            append_event(&root, 2, 9, b"something this build never wrote")
                .expect("the journal refused a frame of an unknown kind");
            append_event(&root, 3, 5, b"codex").expect("the journal refused a quiet report");
            append_event(&root, 4, STDOUT, b"{\"type\":\"turn.completed\"}\n")
                .expect("the journal refused a Provider line");
            append_event(&root, 5, DONE, &[]).expect("the journal refused its terminal fact");

            let mut kinds = Vec::new();
            let mut offset = 0;
            while let Some((frame, next)) = read_event(&root, offset)
                .expect("a frame this build could not name made the whole journal unreadable")
            {
                kinds.push(match frame.event {
                    AgentEvent::Stdout(_) => "output",
                    AgentEvent::Stderr => "diagnostic",
                    AgentEvent::Stalled => "quiet",
                    AgentEvent::Done => "done",
                    AgentEvent::Stopped => "stopped",
                });
                offset = next;
            }
            assert_eq!(
                kinds,
                ["output", "quiet", "output", "done"],
                "an unreadable frame took the frames behind it with it",
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        /// Scratch is one private directory for one journal under test.
        fn scratch(name: &str) -> std::path::PathBuf {
            let root = std::env::temp_dir().join(format!("archigoat-{name}"));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root)
                .expect("this machine gave the test no scratch directory");
            root
        }
    }
}

// The Windows journal keeps reading past a frame this build never wrote, including the terminal fact
// a previous generation of that runner numbered differently.
#[path = "."]
mod windows {
    #[path = "../../daemon/src/windows/journal.rs"]
    pub(crate) mod journal;

    use crate::execution::AgentEvent;
    use journal::{DONE, STDOUT, append, read};

    /// A frame of a kind this build never wrote is released, and a Work that a previous generation of
    /// the runner stopped still reads as stopped.
    #[test]
    fn a_frame_this_build_cannot_name_is_read_past() {
        let root = scratch("windows-unknown-kind");
        append(&root, 1, STDOUT, b"{\"type\":\"thread.started\"}\n")
            .expect("the journal refused a Provider line");
        append(&root, 2, 9, b"something this build never wrote")
            .expect("the journal refused a frame of an unknown kind");
        append(&root, 3, DONE, &[]).expect("the journal refused its terminal fact");

        let mut kinds = Vec::new();
        let mut offset = 0;
        while let Some((frame, next)) = read(&root, offset)
            .expect("a frame this build could not name made the whole journal unreadable")
        {
            kinds.push(kind(&frame.event));
            offset = next;
        }
        assert_eq!(
            kinds,
            ["output", "done"],
            "an unreadable frame took the frames behind it with it",
        );

        // The previous generation of this runner numbered a Work that did not complete as five.
        let stopped = scratch("windows-legacy-stopped");
        append(&stopped, 1, 5, &[]).expect("the journal refused a legacy terminal fact");
        let (frame, _) = read(&stopped, 0)
            .expect("a legacy terminal fact made the journal unreadable")
            .expect("a legacy terminal fact was released");
        assert_eq!(
            kind(&frame.event),
            "stopped",
            "a Work an earlier runner stopped can no longer be settled",
        );
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&stopped);
    }

    /// Kind names one durable frame in the words the observer reads it by.
    fn kind(event: &AgentEvent) -> &'static str {
        match event {
            AgentEvent::Stdout(_) => "output",
            AgentEvent::Stderr => "diagnostic",
            AgentEvent::Stalled => "quiet",
            AgentEvent::Done => "done",
            AgentEvent::Stopped => "stopped",
        }
    }

    /// Scratch is one private directory for one journal under test.
    fn scratch(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("archigoat-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("this machine gave the test no scratch directory");
        root
    }
}
