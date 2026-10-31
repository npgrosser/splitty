//! Thread-safe WebSocket writer that wraps the write half of a split `WebSocket`.
//!
//! Uses an unbounded `mpsc` channel so that both async callers (`send_text`,
//! `send_response`) and the synchronous `protocol::Sender` trait implementation
//! can enqueue messages without blocking. A background task drains the channel
//! into the actual socket. Terminal output uses binary frames for efficiency.

use axum::extract::ws::{Message, WebSocket};
use futures::stream::SplitSink;
use futures::SinkExt;
use tokio::sync::mpsc;

use splitty_core::protocol::{self, ServerEvent};

/// Thread-safe WebSocket writer. Wraps the write half of a split WebSocket.
///
/// Uses a channel internally so that synchronous `protocol::Sender` calls
/// can enqueue messages without blocking.
pub struct WsSender {
    tx: mpsc::UnboundedSender<Message>,
}

impl WsSender {
    /// Create a new WsSender. Spawns a background task that drains the channel
    /// and writes to the WebSocket.
    pub fn new(write: SplitSink<WebSocket, Message>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(Self::drain_loop(write, rx));
        Self { tx }
    }

    /// A sender that hands its messages to the returned receiver instead of a
    /// socket, so the handlers can be tested without a connection.
    #[cfg(test)]
    pub(crate) fn channel() -> (Self, mpsc::UnboundedReceiver<Message>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    async fn drain_loop(
        mut write: SplitSink<WebSocket, Message>,
        mut rx: mpsc::UnboundedReceiver<Message>,
    ) {
        while let Some(msg) = rx.recv().await {
            if let Err(e) = write.send(msg).await {
                tracing::warn!("ws write failed: {}", e);
                break;
            }
        }
    }

    /// Send a text message.
    pub async fn send_text(&self, text: &str) -> Result<(), axum::Error> {
        self.tx
            .send(Message::Text(text.to_string().into()))
            .map_err(axum::Error::new)
    }

    /// Send a binary message.
    pub async fn send_binary_data(&self, data: Vec<u8>) -> Result<(), axum::Error> {
        self.tx
            .send(Message::Binary(data.into()))
            .map_err(axum::Error::new)
    }

    /// Send a WebSocket ping frame for heartbeat.
    pub fn send_ping(&self) -> Result<(), axum::Error> {
        self.tx
            .send(Message::Ping(Vec::new().into()))
            .map_err(axum::Error::new)
    }

    /// Send a protocol ServerEvent as a JSON text frame.
    pub async fn send_event(&self, event: &ServerEvent) -> Result<(), axum::Error> {
        let text = serde_json::to_string(event).unwrap_or_default();
        self.send_text(&text).await
    }
}

/// Implement the core protocol::Sender trait so that the Dispatcher/Connection
/// can send events back through the WebSocket.
impl protocol::Sender for WsSender {
    fn send(&self, event: &ServerEvent) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let text = serde_json::to_string(event).unwrap_or_default();
        self.tx.send(Message::Text(text.into()))?;
        Ok(())
    }

    fn send_binary(
        &self,
        session_id: &str,
        data: &[u8],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let frame = super::codec::encode_binary_frame(session_id, data);
        self.tx.send(Message::Binary(frame.into()))?;
        Ok(())
    }
}
