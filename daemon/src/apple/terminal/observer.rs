//! Apple observation exposes durable ordered events and owner-only Stop.

use super::{
    journal::{append_event, read_event},
    model::{CLAIM, DONE, LIVENESS, STOP},
};
use crate::{
    apple::write_private,
    execution::{AgentEvent, AgentFrame},
    state::{OwnerStop, TurnStop},
};
use std::{fs, path::PathBuf, time::Duration};

/// Quiet journals back off so an idle Work does not spend a CPU core polling its private file.
const MAX_IDLE_WAIT: Duration = Duration::from_secs(1);
const INITIAL_IDLE_WAIT: Duration = Duration::from_millis(25);

// AgentRun tracks one observer's exact position in one runner journal.
pub(crate) struct AgentRun {
    root: PathBuf,
    offset: usize,
    sequence: u64,
    terminal: Option<u64>,
    stop_proof: String,
    stop_sent: bool,
    supervisor: Option<tokio::process::Child>,
    /// The end recorded once on a dead runner's behalf, so it is written a single time and still
    /// reaches the Work even when this journal's own torn tail can no longer read it back.
    settled: Option<u64>,
}

// This observer turns native Agent output into the latest truthful Work state.
impl AgentRun {
    // New observers replay the durable journal from its first event.
    pub(super) fn new(
        root: PathBuf,
        supervisor: Option<tokio::process::Child>,
        stop_proof: String,
    ) -> Self {
        Self {
            root,
            offset: 0,
            sequence: 0,
            terminal: None,
            stop_proof,
            stop_sent: false,
            supervisor,
            settled: None,
        }
    }

    /// Physical liveness requires the exact runner to hold its kernel-backed claim lock.
    pub(crate) fn proves_liveness(&mut self) -> bool {
        let path = self.root.join(CLAIM);
        if !matches!(fs::read(&path), Ok(content) if content.starts_with(LIVENESS)) {
            return false;
        }
        let Ok(claim) = fs::OpenOptions::new().read(true).write(true).open(path) else {
            return false;
        };
        matches!(claim.try_lock(), Err(fs::TryLockError::WouldBlock))
    }

    /// FreshLiveness waits until the spawned runner either owns its claim lock or physically exits.
    pub(crate) async fn fresh_liveness(&mut self) -> bool {
        let mut idle_wait = INITIAL_IDLE_WAIT;
        loop {
            if self.proves_liveness() {
                if let Some(mut supervisor) = self.supervisor.take() {
                    tokio::spawn(async move { supervisor.wait().await });
                }
                return true;
            }
            // A runner this launch attached to instead of starting has no child of its own to wait
            // on, so its claim marker is the whole proof and an unclaimed one is answered now. The
            // journal is observed either way; this only decides which liveness the screen publishes.
            if self.supervisor.is_none() || self.supervisor_exited().unwrap_or(true) {
                return false;
            }
            tokio::time::sleep(idle_wait).await;
            idle_wait = (idle_wait * 2).min(MAX_IDLE_WAIT);
        }
    }

    // Next returns strict ordered output and converts only owner intent into Stop.
    pub(crate) async fn next(
        &mut self,
        stop: OwnerStop,
        turn: TurnStop,
    ) -> Result<AgentFrame, String> {
        let mut idle_wait = INITIAL_IDLE_WAIT;
        loop {
            // The private Stop file communicates owner intent across the process boundary.
            if (stop.requested() || turn.requested()) && !self.stop_sent {
                if !self.root.join(STOP).exists() {
                    write_private(&self.root.join(STOP), self.stop_proof.as_bytes())?;
                }
                self.stop_sent = true;
            }
            if let Some((frame, offset)) = read_event(&self.root, self.offset)? {
                // A frame that does not follow the last one means frames between them were lost —
                // a torn append, a power cut. What survived is still this turn's own output, so the
                // gap is named and the Work goes on reading.
                if frame.sequence > self.sequence + 1 {
                    crate::trace::line(&format!(
                        "an Apple Work journal lost {} frame(s) before frame {}",
                        frame.sequence - self.sequence - 1,
                        frame.sequence
                    ));
                }
                self.offset = offset;
                self.sequence = frame.sequence;
                if matches!(frame.event, AgentEvent::Done | AgentEvent::Stopped) {
                    self.terminal = Some(frame.sequence);
                }
                return Ok(frame);
            }
            if self.supervisor_exited()? && self.settled.is_none() {
                self.settle_dead_runner(true);
                if self.settled.is_some() {
                    // The end is durable now, so the next read hands it back in its own place.
                    continue;
                }
            }
            // PHYSICS: the runner process is gone and its end is already recorded on its behalf. A
            // journal whose own torn tail can no longer read that fact back hands it over here, so
            // the Work settles on what it froze instead of watching an unreadable file for life.
            if let Some(sequence) = self.settled.filter(|_| self.terminal.is_none()) {
                self.sequence = sequence;
                self.terminal = Some(sequence);
                return Ok(AgentFrame {
                    sequence,
                    event: AgentEvent::Done,
                });
            }
            tokio::time::sleep(idle_wait).await;
            // A journal quiet past the idle cap earns one liveness probe of its runner's claim lock.
            if idle_wait >= MAX_IDLE_WAIT && self.terminal.is_none() {
                self.settle_dead_runner(false);
            }
            idle_wait = (idle_wait * 2).min(MAX_IDLE_WAIT);
        }
    }

    // PHYSICS: a claim lock no process holds, or a supervisor that exited, is a runner that is gone.
    // The physical turn then ends on its behalf, once. A record that will not save is traced and handed
    // over from memory instead: a runner's end is a fact about the runner, never a write to wait on.
    fn settle_dead_runner(&mut self, supervisor_dead: bool) {
        if self.settled.is_some() {
            return;
        }
        let path = self.root.join(CLAIM);
        // A claim is created once and outlives its runner, so an absent one means no runner ever took
        // this journal — proof of death whenever there is no child of ours still starting up behind it.
        let unclaimable = supervisor_dead || self.supervisor.is_none();
        let content = match fs::read(&path) {
            Ok(content) => content,
            Err(error) if unclaimable && error.kind() == std::io::ErrorKind::NotFound => {
                self.record_end();
                return;
            }
            Err(_) => return,
        };
        // An unstamped claim proves a runner that died mid-admission; only claims written before lock discipline prove nothing.
        if !content.is_empty() && !content.starts_with(LIVENESS) {
            return;
        }
        let Ok(claim) = fs::File::open(&path) else {
            return;
        };
        if claim.try_lock().is_err() {
            return;
        }
        // The held lock excludes every writer while the tail is re-checked and terminated exactly once.
        if !matches!(read_event(&self.root, self.offset), Ok(None)) {
            return;
        }
        self.record_end();
    }

    // RecordEnd journals this dead runner's end once so a successor reads the same terminal fact.
    fn record_end(&mut self) {
        let sequence = self.sequence + 1;
        self.settled = Some(sequence);
        if let Err(error) = append_event(&self.root, sequence, DONE, &[]) {
            crate::trace::line(&format!("this runner's end went unjournaled: {error}"));
        }
    }

    // SupervisorExited is physical proof only for the child this launch retained.
    fn supervisor_exited(&mut self) -> Result<bool, String> {
        match self
            .supervisor
            .as_mut()
            .map(tokio::process::Child::try_wait)
        {
            None | Some(Ok(None)) => Ok(false),
            Some(Ok(Some(_))) => Ok(true),
            Some(Err(error)) => Err(format!("Could not inspect the Apple Work runner: {error}")),
        }
    }

    // Complete deletes private state only after its exact terminal event was observed.
    // BOUNDARY: this is the one path that removes a runner's tree, so it refuses on anything but the
    // terminal fact this observer itself read. Refusing keeps the tree; it ends no turn.
    pub(crate) fn complete(&mut self, sequence: u64) -> Result<(), String> {
        if self.terminal != Some(sequence) {
            return Err("Apple Work completion does not match its terminal event".to_owned());
        }
        fs::remove_dir_all(&self.root)
            .or_else(|error| {
                (error.kind() == std::io::ErrorKind::NotFound)
                    .then_some(())
                    .ok_or(error)
            })
            .map_err(|error| format!("Could not remove Apple Work: {error}"))
    }

    // TerminalSequence gives shared state the durable owner-stop completion identity.
    pub(crate) fn terminal_sequence(&self) -> Option<u64> {
        self.terminal
    }
}
