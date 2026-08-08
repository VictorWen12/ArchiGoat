//! Platform-neutral events from one already-owned local Agent execution.

/// One ordered frame from the exact Work-owned runner.
pub(crate) struct AgentFrame {
    // Sequence preserves the Provider's event order for one Work.
    pub(crate) sequence: u64,
    // Event carries the runner output or terminal state to the bridge.
    pub(crate) event: AgentEvent,
}

/// Raw local Agent output and its truthful process terminal.
pub(crate) enum AgentEvent {
    Stdout(Vec<u8>),
    Stderr,
    Stalled,
    Done,
    Stopped,
}
