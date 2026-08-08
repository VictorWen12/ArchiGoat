//! Provider connection composes admission, native authentication, and truthful status observation.

mod authentication;
mod flow;
mod models;
pub(crate) mod presets;
mod status;

pub(crate) use authentication::{Authentication, authentication};
pub(crate) use flow::{observe, run};
