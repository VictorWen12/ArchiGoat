//! Work binds lossless requests, private artifacts, verified evidence, and one terminal transition.

pub(crate) mod app;
mod artifact;
mod egress;
pub(crate) mod envelope;
mod evidence;
pub(crate) mod input_view;
mod request;
pub(crate) mod runtime;

pub(crate) use egress::{redact_answer, validate_egress};
pub(crate) use evidence::{
    ArtifactFact, MAX_PROTECTED_BYTES, MAX_PROTECTED_ITEM_BYTES, MAX_PROTECTED_ITEMS, valid_work_id,
};
pub use evidence::{DeliveredWork, ResultKind};
pub use request::WorkRequest;
pub use runtime::RuntimeWork;
pub(crate) use runtime::{RuntimeRecovery, RuntimeSteer};
// Terminal ownership freezes only files proven inside this Work's private workspace.
pub(crate) use artifact::{freeze_delivery_receipt, load_delivery_receipt};
