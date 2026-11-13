//! The `splitty-server` crate: an Axum-based HTTP and WebSocket server for the
//! Splitty terminal multiplexer.
//!
//! - [`control`] — local Unix-socket control channel for processes running in a pane.
//! - [`embed`] — the compiled frontend assets (embedded with the `web-assets` feature) and their themed `index.html`.
//! - [`host`] — what the binary does on a remote host for a client over SSH: the handshake and the bridge.
//! - [`http`] — the browser gateway (sessions, tickets, origin and Host guards, the catalog's `/api` routes, the embedded app) and the control-socket router.
//! - [`hub`] — the remote screen hub: native host streams to a host's daemon over SSH.
//! - [`service`] — shared operations and native request authority.
//! - [`client_session`] — common session loop, registrations, codec and ordered delivery.

pub mod cli;
pub mod client_session;
#[cfg(unix)]
pub mod control;
pub mod embed;
pub mod host;
pub mod http;
pub mod hub;
pub mod jobs;
pub mod native;
pub mod service;
