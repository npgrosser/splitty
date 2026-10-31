//! Per-connection WebSocket message loop.
//!
//! After upgrade, generates a client ID, sends a `mux.welcome` message with the
//! screen list and the listening ports (`ports.changed`), then enters a
//! `tokio::select!` loop reading both WebSocket frames and the notification
//! broadcast channel. Text messages are dispatched first to the `MuxHandler`,
//! falling back to the core protocol dispatcher. Cleans up on disconnect.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use futures::StreamExt;
use rand::Rng;
use tokio::time::{self, Instant};

use super::codec;
use super::mux_handler::{self, MuxHandler};
use super::sender::WsSender;
use super::{ClientEntry, WsState};

/// Handle an upgraded WebSocket connection.
///
/// This is the main message loop for a client:
/// 1. Generate client ID
/// 2. Send welcome message (with screen list), then the listening ports
/// 3. Loop: read messages, dispatch to mux handler or protocol handler
/// 4. On disconnect: cleanup
pub async fn handle_socket(socket: WebSocket, ws_state: Arc<WsState>) {
    let (write, mut read) = socket.split();

    let sender = Arc::new(WsSender::new(write));
    let client_id = new_client_id();

    // Create protocol connection for this client
    let mut connection = ws_state
        .dispatcher
        .new_connection(sender.clone() as Arc<dyn splitty_core::protocol::Sender>);
    connection.mux_managed = true;

    // Create mux handler with effect execution and broadcast capability.
    // The broadcast spawns its own task so the mux handler never awaits the
    // client map while holding up the connection's message loop.
    let broadcast_ws_state = ws_state.clone();
    let broadcast_fn: mux_handler::BroadcastFn = Arc::new(move |texts, origin| {
        let ws_state = broadcast_ws_state.clone();
        tokio::spawn(async move {
            ws_state.broadcast_events(texts, Some(&origin)).await;
        });
    });
    let mux_handler = Arc::new(MuxHandler::new(
        ws_state.mux_store.clone(),
        sender.clone(),
        client_id.clone(),
        ws_state.server_mode.clone(),
        ws_state.effect_runner.clone(),
        ws_state.shell_profiles.clone(),
        ws_state.env.clone(),
        broadcast_fn,
        ws_state.jobs.clone(),
    ));

    // Register client
    {
        let mut clients = ws_state.clients.lock().await;
        clients.insert(
            client_id.clone(),
            ClientEntry {
                sender: sender.clone(),
                mux_handler: Some(mux_handler.clone()),
            },
        );
    }

    // Send welcome (replaces hello + mux.connect), then the listening ports
    // Subscribe before the first snapshot of the catalog: a concurrent launch
    // must not fall into a gap between welcome and event subscription.
    let mut notify_rx = ws_state.notify_tx.subscribe();
    mux_handler.send_welcome().await;
    super::ports::greet(&ws_state, &sender).await;

    // Subscribe to notification broadcast channel
    let notify_sender = sender.clone();

    // Heartbeat: ping every 15s, consider dead after 30s of silence
    let mut ping_interval = time::interval(Duration::from_secs(15));
    ping_interval.tick().await; // consume the immediate first tick
    let mut last_activity = Instant::now();

    // Message loop: read from WS, notification broadcast, and heartbeat concurrently
    loop {
        tokio::select! {
            msg = read.next() => {
                let msg = match msg {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => {
                        tracing::debug!("ws read error: clientId={}, err={}", client_id, e);
                        break;
                    }
                    None => break,
                };

                last_activity = Instant::now();

                match msg {
                    Message::Text(text) => {
                        if let Ok(value)=serde_json::from_str::<serde_json::Value>(&text) {
                            if value["type"] == "editor.register" {
                                ws_state.editor_requests.register(&client_id, sender.clone(), connection.attached_sessions()).await;
                                continue;
                            }
                            if value["type"] == "editor.result" {
                                ws_state.editor_requests.reply(&client_id, &value).await;
                                continue;
                            }
                            let offers = &ws_state.file_offers;
                            match value["type"].as_str() {
                                Some("file.register") => {
                                    offers.register(&client_id, sender.clone(), connection.attached_sessions()).await;
                                    continue;
                                }
                                Some("file.accept") => { offers.accept(&client_id, &value).await; continue; }
                                Some("file.decline") => { offers.decline(&client_id, &value).await; continue; }
                                Some("file.read") => { offers.read(&client_id, &value).await; continue; }
                                _ => {}
                            }
                            if value["type"]=="mux.op" {
                                let Some(screen)=value["screenId"].as_str() else {continue;};
                                if let Some(result)=ws_state.jobs.mux(screen,value["op"].clone()).await {
                                    // Spawn delivery for this accepted intent precedes this
                                    // connection's next message, including an immediate resync.
                                    while let Ok(event)=notify_rx.try_recv(){let _=sender.send_text(&event).await;}
                                    if let Err(error)=result {
                                        let event=splitty_core::protocol::ServerEvent::mux_op_failed(
                                            value["op"]["type"].as_str().unwrap_or(""),Some(error.details.get("screenId").and_then(serde_json::Value::as_str).unwrap_or(screen).into()),
                                            error.details.get("paneId").and_then(serde_json::Value::as_str).or_else(||value["op"]["newPaneId"].as_str()).map(str::to_owned),error.error);
                                        if let Ok(text)=serde_json::to_string(&event){let _=sender.send_text(&text).await;}
                                    }
                                    continue;
                                }
                            }else if value["type"]=="close"{ws_state.jobs.close_jobs(None,None,value["id"].as_str()).await;}
                        }
                        // Try mux handler first
                        if mux_handler.try_handle(&text).await {
                            continue;
                        }

                        // Fall through to protocol handler
                        match serde_json::from_str::<codec::JsonRequest>(&text) {
                            Ok(jr) => {
                                let req = codec::decode_request(jr);
                                let changes_streams = matches!(req.msg_type.as_str(), "create" | "attach" | "detach" | "close");
                                connection.handle(&req).await;
                                if changes_streams {
                                    ws_state.editor_requests.sessions(&client_id, connection.attached_sessions()).await;
                                    ws_state.file_offers.sessions(&client_id, connection.attached_sessions()).await;
                                }
                            }
                            Err(e) => {
                                tracing::warn!("invalid json message: {}", e);
                            }
                        }
                    }
                    Message::Binary(_) => {
                        // Binary messages from client are unexpected, ignore
                    }
                    Message::Ping(_) | Message::Pong(_) => {
                        // Activity already recorded above
                    }
                    Message::Close(_) => {
                        break;
                    }
                }
            }
            result = notify_rx.recv() => {
                match result {
                    Ok(json) => {
                        let _ = notify_sender.send_text(&json).await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("notification receiver lagged by {} messages", n);
                        for screen in ws_state.mux_store.list_screen_infos() {
                            let event=splitty_core::protocol::ServerEvent::mux_nudge("cli_op".into(),Some(screen.id.to_string()));
                            if let Ok(text)=serde_json::to_string(&event){let _=notify_sender.send_text(&text).await;}
                        }
                    }
                    Err(_) => break,
                }
            }
            _ = ping_interval.tick() => {
                ws_state.editor_requests.expire().await;
                ws_state.file_offers.expire().await;
                if last_activity.elapsed() > Duration::from_secs(30) {
                    tracing::info!("ws heartbeat timeout: clientId={}", client_id);
                    break;
                }
                if sender.send_ping().is_err() {
                    tracing::debug!("ws ping send failed: clientId={}", client_id);
                    break;
                }
            }
        }
    }

    // Disconnect cleanup
    ws_state.editor_requests.disconnect(&client_id).await;
    ws_state.file_offers.disconnect(&client_id).await;
    connection.close();

    {
        let mut clients = ws_state.clients.lock().await;
        clients.remove(&client_id);
    }

    tracing::info!("client disconnected: {}", client_id);
}

/// Generate a 32-character hex client ID (16 random bytes).
fn new_client_id() -> String {
    let mut buf = [0u8; 16];
    rand::thread_rng().fill(&mut buf);
    hex::encode(buf)
}
