//! The `splitty-server` crate: an Axum-based HTTP and WebSocket server for the
//! Splitty terminal multiplexer.
//!
//! - [`control`] — local Unix-socket control channel for processes running in a pane.
//! - [`embed`] — embeds the compiled frontend assets via `rust_embed` and injects theme colors.
//! - [`host`] — what the binary does on a remote host for a client over SSH: the handshake and the bridge.
//! - [`http`] — HTTP router, middleware (auth, CORS, localhost guard), reverse proxy, and REST API routes.
//! - [`hub`] — the remote screen hub: `/ws/remote` relays a host daemon's WebSocket over SSH.
//! - [`ws`] — WebSocket connection handling, binary/JSON codec, mux protocol handler, and sender abstraction.

pub mod cli;
#[cfg(unix)]
pub mod control;
pub mod embed;
pub mod host;
pub mod http;
pub mod hub;
pub mod jobs;
pub mod ws;
