//! The `splitty-server` crate: native services and local control for the
//! Splitty terminal multiplexer.
//!
//! - [`control`] — local Unix-socket control channel for processes running in a pane.
//! - [`host`] — what the binary does on a remote host for a client over SSH: the handshake and the bridge.
//! - [`hub`] — the remote screen hub: native host streams to a host's daemon over SSH.
//! - [`service`] — shared operations and native request authority.
//! - [`client_session`] — common session loop, registrations, codec and ordered delivery.

pub mod cli;
pub mod client_session;
#[cfg(unix)]
pub mod control;
pub mod host;
pub mod hub;
pub mod jobs;
pub mod native;
pub mod service;
