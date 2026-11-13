//! The `splitty-server` crate: an Axum-based HTTP and WebSocket server for the
//! Splitty terminal multiplexer.
//!
//! - [`control`] — local Unix-socket control channel for processes running in a pane.
//! - [`embed`] — the compiled frontend assets (embedded with the `web-assets` feature) and their themed `index.html`.
//! - [`host`] — what the binary does on a remote host for a client over SSH: the handshake and the bridge.
//! - [`http`] — HTTP router, middleware (auth, CORS, localhost guard), reverse proxy, and REST API routes.
//! - [`hub`] — the remote screen hub: `/ws/remote` relays a host daemon's WebSocket over SSH.
//! - [`service`] — shared operations and native request authority.
//! - [`client_session`] — common session loop, registrations, codec and ordered delivery.
//! - [`ws`] — WebSocket carrier for the common session loop.

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
pub mod ws;
