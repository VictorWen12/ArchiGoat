//! Windows observation returns ordered durable Provider events and forwards only owner Stop.

use crate::{
    execution::{AgentEvent, AgentFrame},
    state::{OwnerStop, TurnStop},
};
use std::time::Duration;

/// Names the signed owner-only Stop signal consumed by the runner.
const STOP: &str = "stop";
/// Quiet journals back off so an idle Work does not spend a CPU core polling its private file.
const MAX_IDLE_WAIT: Duration = Duration::from_secs(1);
const INITIAL_IDLE_WAIT: Duration = Duration::from_millis(25);

/// Observes and controls one durable Windows runner.
pub(crate) struct AgentRun {
    record: super::super::active::Record,
    offset: usize,
    cursor: u64,
    terminal: Option<u64>,
    stop_proof: String,
    stop_sent: bool,
    supervisor: Option<tokio::process::Child>,
    liveness: Option<super::super::liveness::RunnerLiveness>,
    /// The end recorded once on a dead runner's behalf, so it is written a single time and still
    /// reaches the Work even when this journal's own torn tail can no longer read it back.
    settled: Option<u64>,
}

// This observer turns Windows native Agent output into current Work facts.
impl AgentRun {
    /// Starts observation at the first durable event for the exact runner record.
    pub(super) fn new(
        record: super::super::active::Record,
        supervisor: Option<tokio::process::Child>,
        liveness: Option<super::super::liveness::RunnerLiveness>,
        stop_proof: String,
    ) -> Self {
        Self {
            record,
            offset: 0,
            cursor: 0,
            terminal: None,
            stop_proof,
            stop_sent: false,
            supervisor,
            liveness,
            settled: None,
        }
    }

    /// Physical liveness requires either the spawned child or the reattached runner's kernel lock.
    pub(crate) fn proves_liveness(&mut self) -> bool {
        match self.supervisor.as_mut() {
            Some(supervisor) => matches!(supervisor.try_wait(), Ok(None)),
            None => self.liveness.as_mut().is_some_and(|owner| owner.is_live()),
        }
    }

    /// Returns the stable identity required for exact reattachment.
    pub(crate) fn record_identity(&self) -> &str {
        &self.record.identity
    }

    /// Returns the next ordered event while forwarding an exact owner Stop once.
    pub(crate) async fn next(
        &mut self,
        stop: OwnerStop,
        turn: TurnStop,
    ) -> Result<AgentFrame, String> {
        let mut idle_wait = INITIAL_IDLE_WAIT;
        loop {
            // Owner Stop is a signed runner fact, never an inferred failure path.
            if (stop.requested() || turn.requested()) && !self.stop_sent {
                let path = self.record.root.join(STOP);
                if !path.exists()
                    && let Err(error) =
                        super::super::file::write_private(&path, self.stop_proof.as_bytes())
                {
                    return Err(error);
                }
                self.stop_sent = true;
            }
            let frame = super::super::journal::read(&self.record.root, self.offset);
            let frame = match frame {
                Ok(value) => value,
                Err(error) => return Err(error),
            };
            if let Some((next, offset)) = frame {
                // A frame that does not follow the last one means frames between them were lost — a
                // torn append, a power cut. What survived is still this turn's own output, so the
                // gap is named and the Work goes on reading.
                if next.sequence > self.cursor + 1 {
                    crate::trace::line(&format!(
                        "a Windows Work journal lost {} frame(s) before frame {}",
                        next.sequence - self.cursor - 1,
                        next.sequence
                    ));
                }
                self.offset = offset;
                self.cursor = next.sequence;
                if matches!(next.event, AgentEvent::Done | AgentEvent::Stopped) {
                    self.terminal = Some(next.sequence);
                }
                return Ok(next);
            }
            // A runner with no durable terminal fact is only ever settled on its own proven death.
            let exited = match self.supervisor.as_mut().map(|child| child.try_wait()) {
                None => !self.liveness.as_mut().is_some_and(|owner| owner.is_live()),
                Some(Ok(status)) => status.is_some(),
                Some(Err(error)) => {
                    return Err(format!(
                        "Could not inspect the Windows Work runner: {error}"
                    ));
                }
            };
            // PHYSICS: the runner process is gone. A journal with no terminal fact gets one written
            // on the dead runner's behalf, once, so the Work settles on what it froze instead of
            // watching an empty file for the rest of its life. A record that will not save, or a
            // torn tail that cannot read it back, is traced and the fact is handed over from memory:
            // a runner's end is a fact about the runner, never a write the Work has to wait on.
            if exited && self.settled.is_none() {
                let sequence = self.cursor + 1;
                self.settled = Some(sequence);
                if let Err(error) = super::super::journal::append(
                    &self.record.root,
                    sequence,
                    super::super::journal::DONE,
                    &[],
                ) {
                    crate::trace::line(&format!("this runner's end went unjournaled: {error}"));
                }
                continue;
            }
            if let Some(sequence) = self.settled.filter(|_| self.terminal.is_none()) {
                self.cursor = sequence;
                self.terminal = Some(sequence);
                return Ok(AgentFrame {
                    sequence,
                    event: AgentEvent::Done,
                });
            }
            tokio::time::sleep(idle_wait).await;
            idle_wait = (idle_wait * 2).min(MAX_IDLE_WAIT);
        }
    }

    /// Acknowledges one terminal fact before removing only this runner's private files.
    /// BOUNDARY: this is the one path that removes a runner's tree, so it refuses on anything but the
    /// terminal fact this observer itself read. Refusing keeps the tree; it ends no turn.
    pub(crate) fn complete(&mut self, sequence: u64) -> Result<(), String> {
        if sequence != self.cursor || self.terminal != Some(sequence) {
            return Err("Windows Work acknowledgement is invalid".to_owned());
        }
        remove_private_root(&self.record.root)
    }

    /// Exposes the exact journal terminal fact required for owner-Stop cleanup.
    pub(crate) fn terminal_sequence(&self) -> Option<u64> {
        self.terminal
    }
}

/// Removes only the acknowledged Work's private runner directory.
fn remove_private_root(root: &std::path::Path) -> Result<(), String> {
    match std::fs::remove_dir_all(root) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Could not remove Windows Work state: {error}")),
    }
}
