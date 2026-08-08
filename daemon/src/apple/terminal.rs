//! Apple Terminal binds one signed Work to one durable native runner.

mod admission;
mod browser;
mod execute;
mod journal;
mod model;
mod observer;
mod sweep;

pub(crate) use admission::{launch, reattach};
pub(crate) use observer::AgentRun;

use std::path::Path;

// The Terminal helper verifies admission, claims execution once, and records its terminal fact.
pub(super) async fn run_terminal_work(job_path: &Path) -> Result<(), String> {
    admission::run_terminal_work(job_path).await
}
