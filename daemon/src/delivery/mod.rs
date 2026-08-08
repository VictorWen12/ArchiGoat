//! Delivery exposes verified Work results without leaking machine paths or mutable bytes.

mod freeze;
mod manifest;
mod model;

pub(crate) use freeze::discard_private_tree;
pub(crate) use manifest::harvest;
pub(crate) use model::{DeliveryFile, Harvested};
