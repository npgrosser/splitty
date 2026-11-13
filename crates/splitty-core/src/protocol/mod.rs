//! WebSocket protocol layer: message types, request dispatching, and notifications.
//!
//! The capabilities a server announces in `mux.welcome` are named in
//! `capability`; the messages for one connection that are not events
//! (answers, editor requests, file offers) are `session::SessionMessage`;
//! clients tell every message to a window apart by its
//! `message_type::MessageType`.
//!
//! Defines the transport-agnostic `Request`/`Response` types, message type constants
//! for both plain terminal and mux protocols, the `Dispatcher` that routes requests
//! to the PTY backend, the `NotificationEngine` for tracking session lifecycle events and the
//! `Notice` that classifies each notification event for the windows,
//! the store of images pasted into a pane (`paste`) and of files dropped into
//! one (`upload`), and the arbiter of the PTY size of sessions that several
//! connections show (`sizing`).

pub mod capability;
pub mod dispatcher;
pub mod message_type;
mod messages;
pub mod notice;
pub mod notify;
pub mod paste;
pub mod session;
pub mod sizing;
pub mod upload;

pub use messages::*;

/// The version of the WebSocket protocols (session and mux). It changes only
/// when a client of the previous version could no longer talk to this server;
/// a client that relays the protocol supports its own version and the one
/// before (see the `remote-hosts` spec). Recorded in `daemon.json`.
pub const PROTOCOL_VERSION: u32 = splitty_transport::wire::MUX_PROTOCOL_VERSION;

/// Trait for sending protocol events and binary data to connected clients.
pub trait Sender: Send + Sync {
    fn delivery_context(&self) -> splitty_transport::queue::Context {
        splitty_transport::queue::Context::default()
    }
    fn close(&self) {}
    fn send(&self, event: &ServerEvent) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    fn send_binary(
        &self,
        session_id: &str,
        data: &[u8],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}
