//! WebSocket carrier for the common service client loop.
use crate::client_session::{self, ClientState, Message};
use axum::extract::ws::{Message as WsMessage, WebSocket};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;

pub async fn handle_socket(socket: WebSocket, state: Arc<ClientState>) {
    let (mut writer, mut reader) = socket.split();
    let Ok(mut connection) = client_session::connect(state) else {
        return;
    };
    let mut closing = connection.closing();
    loop {
        if *closing.borrow() {
            break;
        }
        tokio::select! {
            _ = closing.changed() => break,
            received = reader.next() => {
                let message = match received {
                    Some(Ok(WsMessage::Text(text))) => {
                        if connection.input.send_text(text.as_str()).is_err() { break; }
                        continue;
                    }
                    // Client session input is JSON; the common loop likewise
                    // ignores binary input. Do not allocate a redundant copy.
                    Some(Ok(WsMessage::Binary(_))) => continue,
                    Some(Ok(WsMessage::Ping(bytes))) => Message::Ping(bytes.to_vec()),
                    Some(Ok(WsMessage::Pong(bytes))) => Message::Pong(bytes.to_vec()),
                    Some(Ok(WsMessage::Close(_))) | Some(Err(_)) | None => break,
                };
                if connection.input.send(message).is_err() { break; }
            }
            outgoing = connection.output.recv() => {
                let Some(outgoing) = outgoing else { break; };
                let (message, _outbound_bytes) = outgoing.into_parts();
                let message = match message {
                    Message::Text(text) => WsMessage::Text(text.into()),
                    Message::Binary(bytes) => WsMessage::Binary(bytes.into()),
                    Message::Ping(bytes) => WsMessage::Ping(bytes.into()),
                    Message::Pong(bytes) => WsMessage::Pong(bytes.into()),
                    Message::Close => break,
                };
                if !matches!(tokio::time::timeout(Duration::from_secs(30), writer.send(message)).await, Ok(Ok(()))) {
                    break;
                }
            }
        }
    }
    connection.shutdown().await;
}
