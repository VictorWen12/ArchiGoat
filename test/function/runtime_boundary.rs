//! Two ends of one law: a finished product survives everything that is not its owner, and a runner
//! already proven dead is replaced now. This drives the shipped terminal path over a durable Work
//! whose delivery a server refused, and the shipped drive loop over a runner that dies every time it
//! launches, measuring the exact quiet the person watching the screen would have sat through.

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

    pub(crate) use observe::Observer;
}

// WorkState keeps the real module shape: the drive loop and the terminal path are its own children,
// and their entries are called from inside the module that owns them.
#[path = "."]
mod work_state {
    #[path = "../../daemon/src/work_state/drive.rs"]
    mod drive;
    #[path = "../../daemon/src/work_state/terminal.rs"]
    mod terminal;

    use crate::{state::DaemonState, work::RuntimeWork};

    /// NativeOwnership publishes Running for the lifetime of one owned runner; nothing here reads it.
    pub(crate) struct NativeOwnership;

    /// Completion names the two shapes one delivery commit can fail in.
    pub(crate) mod completion {
        pub(crate) enum CompletionError {
            Retry(String),
            Repair(String),
        }
    }

    /// Model carries the one cause a runner that ended without delivering reports.
    pub(crate) mod model {
        pub(crate) const RUNNER_END: &str = "Your Agent stopped without delivering";
    }

    /// DriveRun runs the shipped driver for one Work until it settles or parks.
    pub(crate) async fn drive_run(state: DaemonState, runtime: RuntimeWork) {
        drive::run(state, runtime).await;
    }
}

// Work supplies the evidence bounds the decoder reads and the durable facts the driver carries
// between runner generations.
mod work {
    use std::path::{Path, PathBuf};

    use crate::{host::AgentRun, provider::Provider, state::DaemonState};

    pub(crate) const MAX_PROTECTED_ITEMS: usize = 64;
    pub(crate) const MAX_PROTECTED_BYTES: usize = 256 * 1024;
    pub(crate) const MAX_PROTECTED_ITEM_BYTES: usize = 64 * 1024;

    pub(crate) mod runtime {
        use std::time::{SystemTime, UNIX_EPOCH};

        pub(crate) const NETWORK: bool = true;

        pub(crate) fn now_ms() -> Result<u64, String> {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| "System time is unavailable".to_owned())?
                .as_millis()
                .try_into()
                .map_err(|_| "System time is unavailable".to_owned())
        }
    }

    pub(crate) mod input_view {
        use std::path::Path;

        pub(crate) struct InputView;

        impl InputView {
            /// DiscardSession removes the Cursor view of one acknowledged session.
            pub(crate) fn discard_session(session: &Path) -> Result<(), String> {
                crate::delivery::discard_private_tree(&session.join(".app"))
            }
        }
    }

    /// RuntimeSteer identifies one queued follow-up, which the driver compares across generations.
    #[derive(Clone)]
    pub(crate) struct RuntimeSteer {
        pub(crate) id: String,
    }

    /// RuntimeWork carries only the durable facts the drive loop reads between runner generations.
    #[derive(Clone)]
    pub(crate) struct RuntimeWork {
        pub(crate) work_id: String,
        pub(crate) session: PathBuf,
        pub(crate) freeze_root: PathBuf,
        pub(crate) native_session: String,
        pub(crate) provider: Provider,
        pub(crate) started_at: u64,
        pub(crate) launched: bool,
        pub(crate) rotating: bool,
        pub(crate) steer: Option<RuntimeSteer>,
    }

    impl RuntimeWork {
        /// Launch starts one runner generation and records the moment the person could see it start.
        pub(crate) async fn launch(&self, state: &DaemonState) -> Result<AgentRun, String> {
            state.record_launch();
            Ok(AgentRun::dead_runner())
        }

        /// Recovered names the next generation of the same Work: same steer, never yet launched.
        pub(crate) fn recovered(work_id: &str, session: &Path, native_session: String) -> Self {
            Self {
                work_id: work_id.to_owned(),
                session: session.to_owned(),
                freeze_root: session.join("freeze"),
                native_session,
                provider: Provider::Codex,
                started_at: crate::work::runtime::now_ms().unwrap_or(0),
                launched: false,
                rotating: false,
                steer: None,
            }
        }

        pub(crate) fn is_brief(&self) -> Result<bool, String> {
            Ok(false)
        }

        pub(crate) fn has_durable_app_bytes(&self) -> Result<bool, String> {
            Ok(false)
        }
    }
}

// Delivery removes private trees for real here, so a refusal that destroyed bytes could not hide.
mod delivery {
    use std::{fs, path::Path};

    /// DeliveryFile names one delivered artifact; this check reads none of its fields.
    pub(crate) struct DeliveryFile;

    /// DiscardPrivateTree removes one private tree exactly as the shipped freeze cleanup does.
    pub(crate) fn discard_private_tree(root: &Path) -> Result<(), String> {
        match fs::remove_dir_all(root) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("could not discard {}: {error}", root.display())),
        }
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

// Host replays one already-durable journal, exactly as the runner would have written it.
mod host {
    use std::path::Path;

    use crate::{
        execution::{AgentEvent, AgentFrame},
        state::{DaemonState, OwnerStop, TurnStop},
    };

    /// The first line a Codex runner journals, which binds this turn's native conversation.
    const STARTED: &str = r#"{"type":"thread.started","thread_id":"thread-1","model":"gpt-5"}"#;

    pub(crate) struct AgentRun {
        frames: std::vec::IntoIter<AgentFrame>,
        terminal: Option<u64>,
    }

    impl AgentRun {
        /// DeadRunner is the journal of a runner that opened its turn and then ended itself.
        pub(crate) fn dead_runner() -> Self {
            Self {
                frames: vec![
                    AgentFrame {
                        sequence: 1,
                        event: AgentEvent::Stdout(format!("{STARTED}\n").into_bytes()),
                    },
                    AgentFrame {
                        sequence: 2,
                        event: AgentEvent::Stopped,
                    },
                ]
                .into_iter(),
                terminal: None,
            }
        }

        pub(crate) async fn next(
            &mut self,
            _stop: OwnerStop,
            _turn: TurnStop,
        ) -> Result<AgentFrame, String> {
            let frame = self
                .frames
                .next()
                .ok_or_else(|| "the scripted journal ended without a terminal frame".to_owned())?;
            if matches!(frame.event, AgentEvent::Done | AgentEvent::Stopped) {
                self.terminal = Some(frame.sequence);
            }
            Ok(frame)
        }

        pub(crate) fn proves_liveness(&mut self) -> bool {
            true
        }

        pub(crate) async fn fresh_liveness(&mut self) -> bool {
            true
        }

        pub(crate) fn terminal_sequence(&self) -> Option<u64> {
            self.terminal
        }

        /// Every runner in this script ends for its own reasons, never because the machine stopped it.
        pub(crate) fn machine_stop_cause(&self) -> Option<String> {
            None
        }

        pub(crate) fn complete(&mut self, sequence: u64) -> Result<(), String> {
            (self.terminal == Some(sequence))
                .then_some(())
                .ok_or_else(|| "completion does not match its terminal event".to_owned())
        }
    }

    /// Reattach finds no surviving runner, so every generation here launches its own.
    pub(crate) fn reattach(
        _work_id: &str,
        _session: &Path,
        _state: &DaemonState,
    ) -> Result<Option<AgentRun>, String> {
        Ok(None)
    }
}

// State scripts the durable store and the seams the two sources under test reach through. Owner Stop
// is never requested here: every turn in this check ends on its own native evidence.
mod state {
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
    };

    use tokio::{sync::Notify, time::Instant};

    use crate::{
        delivery::DeliveryFile,
        process::observe::ObservedWork,
        work::RuntimeWork,
        work_state::{NativeOwnership, completion::CompletionError},
    };

    #[derive(Clone, Default)]
    pub(crate) struct OwnerStop(Arc<AtomicBool>);

    impl OwnerStop {
        pub(crate) fn requested(&self) -> bool {
            self.0.load(Ordering::Acquire)
        }
    }

    #[derive(Clone, Default)]
    pub(crate) struct TurnStop(Arc<AtomicBool>);

    impl TurnStop {
        pub(crate) fn request(&self) {
            self.0.store(true, Ordering::Release);
        }

        pub(crate) fn requested(&self) -> bool {
            self.0.load(Ordering::Acquire)
        }
    }

    #[derive(Clone, Copy, PartialEq)]
    pub(crate) enum RunPhase {
        Running,
        Done,
        Stopped,
        Failed,
    }

    /// RunSnapshot carries only the delivered files the driver puts in the conversation.
    pub(crate) struct RunSnapshot {
        pub(crate) phase: RunPhase,
        pub(crate) files: Vec<DeliveryFile>,
    }

    /// Entry is the durable shape the terminal path reads: a Work still running, or one terminal
    /// result that still addresses its own session and frozen bytes.
    pub(crate) enum Entry {
        Running,
        Terminal {
            session: Option<PathBuf>,
            freeze_root: Option<PathBuf>,
        },
    }

    /// WorkStore is the durable map, with exactly the transitions the terminal path performs on it.
    pub(crate) struct WorkStore {
        entries: HashMap<String, Entry>,
        /// Saves counts every durable commit, so a path that changed nothing cannot claim one.
        pub(crate) saves: usize,
    }

    impl WorkStore {
        pub(crate) fn contains(&self, work_id: &str) -> bool {
            self.entries.contains_key(work_id)
        }

        pub(crate) fn terminal_paths(
            &self,
            work_id: &str,
        ) -> Option<(Option<PathBuf>, Option<PathBuf>)> {
            match self.entries.get(work_id)? {
                Entry::Terminal {
                    session,
                    freeze_root,
                } => Some((session.clone(), freeze_root.clone())),
                Entry::Running => None,
            }
        }

        pub(crate) fn settle_checkpoint(&mut self, _work_id: &str) -> Option<(PathBuf, bool)> {
            None
        }

        pub(crate) fn rollback_checkpoint(&mut self, _work_id: &str) {}

        pub(crate) fn publish_paths(&self, _work_id: &str) -> Option<(PathBuf, PathBuf)> {
            None
        }

        pub(crate) fn take_published(&mut self, _work_id: &str) -> Option<Entry> {
            None
        }

        pub(crate) fn take_terminal(&mut self, work_id: &str) -> Option<Entry> {
            if !matches!(self.entries.get(work_id), Some(Entry::Terminal { .. })) {
                return None;
            }
            self.entries.remove(work_id)
        }

        pub(crate) fn restore_entry(&mut self, work_id: &str, entry: Entry) {
            self.entries.insert(work_id.to_owned(), entry);
        }

        pub(crate) fn save(&mut self, _path: &Path) -> Result<(), String> {
            self.saves += 1;
            Ok(())
        }
    }

    /// Generations records every runner this Work launched, in the order the person saw them start.
    #[derive(Default)]
    struct Generations {
        launched_at: Vec<Instant>,
        deaths: u32,
        /// Replacements this Work may still admit before its driver parks it.
        limit: u32,
    }

    #[derive(Clone)]
    pub(crate) struct DaemonState {
        pub(crate) works: Arc<Mutex<WorkStore>>,
        pub(crate) work_events: Arc<Notify>,
        state_path: Arc<PathBuf>,
        session: Arc<PathBuf>,
        generations: Arc<Mutex<Generations>>,
    }

    impl DaemonState {
        /// Terminal builds one durable Work whose result Account has not acknowledged yet.
        pub(crate) fn terminal(
            work_id: &str,
            session: PathBuf,
            freeze_root: PathBuf,
        ) -> (Self, PathBuf) {
            let mut entries = HashMap::new();
            entries.insert(
                work_id.to_owned(),
                Entry::Terminal {
                    session: Some(session.clone()),
                    freeze_root: Some(freeze_root.clone()),
                },
            );
            (Self::with(entries, session.clone(), 0), freeze_root)
        }

        /// Running builds one durable Work that is still executing its turn.
        pub(crate) fn running(work_id: &str, session: PathBuf) -> Self {
            let mut entries = HashMap::new();
            entries.insert(work_id.to_owned(), Entry::Running);
            Self::with(entries, session, 0)
        }

        /// Dying builds a Work whose every runner ends itself, admitting this many replacements.
        pub(crate) fn dying(session: PathBuf, limit: u32) -> Self {
            Self::with(HashMap::new(), session, limit)
        }

        fn with(entries: HashMap<String, Entry>, session: PathBuf, limit: u32) -> Self {
            Self {
                works: Arc::new(Mutex::new(WorkStore { entries, saves: 0 })),
                work_events: Arc::new(Notify::new()),
                state_path: Arc::new(session.join("work.json")),
                session: Arc::new(session),
                generations: Arc::new(Mutex::new(Generations {
                    launched_at: Vec::new(),
                    deaths: 0,
                    limit,
                })),
            }
        }

        /// FirstRuntime is the admitted Work its own driver launches first.
        pub(crate) fn first_runtime(&self, work_id: &str) -> RuntimeWork {
            RuntimeWork::recovered(work_id, &self.session, "thread-1".to_owned())
        }

        /// RecordLaunch marks the moment one runner generation started.
        pub(crate) fn record_launch(&self) {
            self.locked().launched_at.push(tokio::time::Instant::now());
        }

        /// Quiet returns the wait the person watching the screen sat through before each replacement.
        pub(crate) fn quiet(&self) -> Vec<std::time::Duration> {
            let generations = self.locked();
            generations
                .launched_at
                .windows(2)
                .map(|pair| pair[1].duration_since(pair[0]))
                .collect()
        }

        /// Saves reports every durable commit this Work's state has taken.
        pub(crate) fn saves(&self) -> usize {
            self.works
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .saves
        }

        /// Addressable reports whether the durable result still names its session and frozen bytes.
        pub(crate) fn addressable(
            &self,
            work_id: &str,
        ) -> Option<(Option<PathBuf>, Option<PathBuf>)> {
            self.works
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .terminal_paths(work_id)
        }

        fn locked(&self) -> std::sync::MutexGuard<'_, Generations> {
            self.generations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        }

        pub(crate) fn work_state_path(&self) -> &Path {
            &self.state_path
        }

        pub(crate) fn discard_work_inputs(&self, _work_id: &str) -> Result<(), String> {
            Ok(())
        }

        pub(crate) fn owner_stop(&self, _work_id: &str) -> Option<OwnerStop> {
            Some(OwnerStop::default())
        }

        pub(crate) fn turn_stop(&self, _work_id: &str) -> Option<TurnStop> {
            Some(TurnStop::default())
        }

        pub(crate) fn request_pending_rotation(&self, _work_id: &str) {}

        pub(crate) fn mark_launched(&self, _work_id: &str) -> Result<(), String> {
            Ok(())
        }

        pub(crate) fn own_native_run(
            &self,
            _work_id: &str,
            _physically_live: bool,
        ) -> Option<NativeOwnership> {
            Some(NativeOwnership)
        }

        pub(crate) fn work_progress_sequence(&self, _work_id: &str) -> usize {
            0
        }

        pub(crate) fn replace_work_progress(
            &self,
            _work_id: &str,
            _index: usize,
            _update: String,
        ) -> Result<bool, String> {
            Ok(true)
        }

        pub(crate) fn record_work_stage(
            &self,
            _work_id: &str,
            _index: usize,
            _label: String,
        ) -> Result<(), String> {
            Ok(())
        }

        pub(crate) fn bind_native_session(
            &self,
            _work_id: &str,
            _session: String,
        ) -> Result<(), String> {
            Ok(())
        }

        pub(crate) fn replace_work_tokens(
            &self,
            _work_id: &str,
            _total: u64,
        ) -> Result<(), String> {
            Ok(())
        }

        pub(crate) fn replace_work_model(
            &self,
            _work_id: &str,
            _model: String,
        ) -> Result<(), String> {
            Ok(())
        }

        pub(crate) fn record_protected_literal(
            &self,
            _work_id: &str,
            _output: &str,
        ) -> Result<(), String> {
            Ok(())
        }

        pub(crate) fn record_protected_outputs(
            &self,
            _work_id: &str,
            _outputs: &[String],
        ) -> Result<(), String> {
            Ok(())
        }

        pub(crate) fn record_agent_message(
            &self,
            _work_id: &str,
            _id: Option<&str>,
            _text: &str,
        ) -> Result<(), String> {
            Ok(())
        }

        pub(crate) fn append_work_answer(
            &self,
            _work_id: &str,
            _candidate: &str,
        ) -> Result<(), String> {
            Ok(())
        }

        pub(crate) fn record_steer_rotation(&self, _work_id: &str) -> Result<bool, String> {
            Ok(false)
        }

        pub(crate) fn push_work_turn_boundary(&self, _work_id: &str, _reason: &str) {}

        pub(crate) fn complete_observed(
            &self,
            _work_id: &str,
            _observed: &ObservedWork,
        ) -> Result<(), CompletionError> {
            Ok(())
        }

        pub(crate) fn run_snapshot(&self, _work_id: &str) -> Option<RunSnapshot> {
            Some(RunSnapshot {
                phase: RunPhase::Running,
                files: Vec::new(),
            })
        }

        pub(crate) fn mark_failed(&self, _work_id: &str, _reason: String) -> bool {
            true
        }

        pub(crate) fn record_artifacts(&self, _work_id: &str, _files: &[DeliveryFile]) {}

        pub(crate) fn prepare_steer(&self, _work_id: &str) -> Result<RuntimeWork, String> {
            Err("this check queues no follow-up".to_owned())
        }

        pub(crate) fn prepare_attention(
            &self,
            _work_id: &str,
            _reason: String,
        ) -> Result<(), String> {
            Ok(())
        }

        /// PrepareRepair admits the next generation of this same Work until its budget is spent.
        pub(crate) fn prepare_repair(
            &self,
            work_id: &str,
            native_session: String,
            _reason: String,
        ) -> Result<Option<RuntimeWork>, String> {
            let mut generations = self.locked();
            generations.deaths += 1;
            if generations.deaths >= generations.limit {
                return Ok(None);
            }
            drop(generations);
            Ok(Some(RuntimeWork::recovered(
                work_id,
                &self.session,
                native_session,
            )))
        }

        pub(crate) fn mark_owner_stopped(&self, _work_id: &str) -> bool {
            true
        }
    }
}

use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};

use state::DaemonState;

/// This Work replaces this many dead runners before its driver parks it, far enough past the
/// doubling to prove where the cooldown stops growing.
const GENERATIONS: u32 = 8;

/// A dead runner is dead: its replacement starts with nothing between them but the launch itself.
#[tokio::test(start_paused = true)]
async fn a_dead_runners_replacement_starts_now() {
    let quiet = replacement_quiet().await;
    assert_eq!(
        quiet.first().copied(),
        Some(Duration::ZERO),
        "the owner watched a dead runner's replacement wait before it started",
    );
}

/// Only a runner that keeps dying at once is paused at all, and never past two seconds: half a
/// minute of a screen saying nothing is not a thing this product does.
#[tokio::test(start_paused = true)]
async fn a_replacement_never_waits_past_the_anti_hot_loop_cap() {
    let quiet = replacement_quiet().await;
    let milliseconds = quiet
        .iter()
        .map(|wait| wait.as_millis())
        .collect::<Vec<_>>();
    assert_eq!(
        milliseconds,
        [0, 250, 500, 1000, 2000, 2000, 2000],
        "the pause between a dead runner and its replacement left the anti-hot-loop schedule",
    );
    assert!(
        quiet.iter().sum::<Duration>() < Duration::from_secs(10),
        "replacing {GENERATIONS} dead runners spent {:?} of the owner's time saying nothing",
        quiet.iter().sum::<Duration>(),
    );
}

/// ReplacementQuiet drives the shipped loop over a Work whose every runner ends itself, and returns
/// the wait before each replacement in the order the owner would have sat through them.
async fn replacement_quiet() -> Vec<Duration> {
    let session = scratch("dying");
    let state = DaemonState::dying(session.clone(), GENERATIONS);
    let runtime = state.first_runtime("work-dying");
    work_state::drive_run(state.clone(), runtime).await;
    let quiet = state.quiet();
    assert_eq!(
        quiet.len() + 1,
        GENERATIONS as usize,
        "the driver did not replace every dead runner it was given",
    );
    quiet
}

/// A server refusing one delivery says nothing about the product that delivery carried.
#[test]
fn a_refused_delivery_keeps_the_product_it_finished() {
    let (state, freeze_root) = refused_work("work-refused");
    state
        .settle_refused_work("work-refused")
        .expect("a refusal reported an ended Work as still Running");
    let addressable = state
        .addressable("work-refused")
        .expect("a refused delivery erased the durable entry that addresses its own product");
    assert_eq!(
        addressable.1.as_ref(),
        Some(&freeze_root),
        "a refused delivery left its frozen bytes unaddressable",
    );
    assert!(
        freeze_root.join("index.html").is_file(),
        "a refused delivery destroyed the bytes the Agent had already finished",
    );
    assert_eq!(
        state.saves(),
        0,
        "a refusal committed a durable change to a Work nothing had changed about",
    );
    fs::remove_dir_all(freeze_root.parent().unwrap()).ok();
}

/// The negative control: acknowledgement is the one authority that removes a finished Work, so the
/// durable entry and every private byte do go when Account owns the result.
#[test]
fn only_acknowledgement_removes_a_finished_product() {
    let (state, freeze_root) = refused_work("work-acknowledged");
    state
        .settle_refused_work("work-acknowledged")
        .expect("a refusal reported an ended Work as still Running");
    state
        .acknowledge_work("work-acknowledged")
        .expect("Account could not acknowledge the Work it owns");
    assert!(
        state.addressable("work-acknowledged").is_none(),
        "an acknowledged Work kept its durable entry",
    );
    assert!(
        !freeze_root.exists(),
        "an acknowledged Work kept its private bytes on this machine",
    );
    fs::remove_dir_all(freeze_root.parent().unwrap()).ok();
}

/// The boundary the refusal path still holds: a Running Work has no terminal truth to settle.
#[test]
fn a_refusal_never_settles_a_running_work() {
    let session = scratch("running");
    let state = DaemonState::running("work-running", session.clone());
    assert!(
        state.settle_refused_work("work-running").is_err(),
        "a refusal settled a Work that was still executing its turn",
    );
    fs::remove_dir_all(&session).ok();
}

/// RefusedWork builds one Done Work with real frozen bytes on this machine.
fn refused_work(work_id: &str) -> (DaemonState, PathBuf) {
    let session = scratch(work_id);
    let freeze_root = session.join("freeze");
    fs::create_dir_all(&freeze_root).expect("scratch freeze tree");
    fs::write(freeze_root.join("index.html"), b"<html>runner game</html>")
        .expect("scratch frozen product");
    let (state, freeze_root) = DaemonState::terminal(work_id, session, freeze_root);
    (state, freeze_root)
}

/// Scratch gives each check its own private tree, so one check never reads another's bytes.
fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let path = std::env::temp_dir().join(format!(
        "archigoat-runtime-boundary-{}-{name}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).expect("scratch session tree");
    path
}
