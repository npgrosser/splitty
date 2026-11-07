//! Platform-neutral native framing and resource accounting. This crate knows
//! neither PTYs nor application operations, filesystem endpoints or UI shells.
pub mod admission;
pub mod budget;
pub mod codec;
pub mod flow;
pub mod negotiation;
pub mod peer;
pub mod queue;
pub mod requests;
pub mod wire;
pub mod worker;

pub mod api_request;
