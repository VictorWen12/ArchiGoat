//! One native turn has one outcome, and only the Provider's own completion or death settles it.

/// TurnOutcome folds one turn's decoded frames into the single fact delivery and repair both read.
pub(super) struct TurnOutcome {
    completed: bool,
    failed: bool,
    failure: Option<String>,
}

// This outcome keeps a recovered stream from ending a turn the Provider went on to finish.
impl TurnOutcome {
    /// New starts a turn that has neither completed nor died.
    pub(super) fn new() -> Self {
        Self {
            completed: false,
            failed: false,
            failure: None,
        }
    }

    /// Observe folds in one decoded frame. A Provider completion is final: a stream failure the
    /// same turn recovered from — a reconnect and retry notice — is progress, never an outcome.
    pub(super) fn observe(&mut self, completed: bool, failed: bool, failure: Option<String>) {
        if completed {
            self.completed = true;
            self.failed = false;
            self.failure = None;
            return;
        }
        if self.completed {
            return;
        }
        self.failed |= failed;
        if failure.is_some() {
            self.failure = failure;
        }
    }

    /// Completed reports the Provider's own proof that this turn ended normally.
    pub(super) fn completed(&self) -> bool {
        self.completed
    }

    /// Failed reports a turn that died without completing, the one outcome repair may replace.
    pub(super) fn failed(&self) -> bool {
        self.failed
    }

    /// Failure names the Provider's own cause for a turn that never completed.
    pub(super) fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }
}
