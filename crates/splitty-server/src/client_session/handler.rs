//! Per-connection service message loop, shared by every carrier.
//!
//! On admission, generates a client ID, sends a `mux.welcome` message with the
//! screen list and the listening ports (`ports.changed`), then enters a
//! `tokio::select!` loop reading both client frames and the notification
//! broadcast channel. Text messages are dispatched first to the `MuxHandler`,
//! falling back to the core protocol dispatcher. Cleans up on disconnect.

use std::sync::Arc;
use std::time::Duration;

use super::Message;
use rand::Rng;
use splitty_transport::queue;
use tokio::time::{self, Instant};

use super::codec;
use super::mux_handler::{self, MuxHandler};
use super::sender::ClientSender;
use super::{ClientEntry, ClientState};

/// Handle an admitted service connection.
///
/// This is the main message loop for a client:
/// 1. Generate client ID
/// 2. Send welcome message (with screen list), then the listening ports
/// 3. Loop: read messages, dispatch to mux handler or protocol handler
/// 4. On disconnect: cleanup
pub(super) async fn run(
    mut read: queue::Receiver<Message>,
    sender: Arc<ClientSender>,
    client_state: Arc<ClientState>,
) {
    let mut closing = sender.closing();
    if *closing.borrow() {
        return;
    }
    let client_id = new_client_id();

    // Create protocol connection for this client
    let mut connection = client_state
        .dispatcher
        .new_connection(sender.clone() as Arc<dyn splitty_core::protocol::Sender>);
    connection.mux_managed = true;

    // Create mux handler with effect execution and broadcast capability.
    // Broadcasting only enqueues an ordered batch; it never spawns per-event
    // tasks or awaits another connection while holding the registry lock.
    let broadcast_client_state = client_state.clone();
    let broadcast_fn: mux_handler::BroadcastFn = Arc::new(move |events, origin| {
        broadcast_client_state.broadcast_events(events, Some(&origin));
    });
    let mux_handler = Arc::new(MuxHandler::new(
        client_state.mux_store.clone(),
        sender.clone(),
        client_id.clone(),
        client_state.server_mode.clone(),
        client_state.effect_runner.clone(),
        client_state.shell_profiles.clone(),
        client_state.env.clone(),
        broadcast_fn,
        client_state.jobs.clone(),
    ));

    // `viewer.read`, answered off this loop (`viewer-panes`).
    let viewer_reads = super::viewer::Reads::new(client_state.mux_store.clone(), sender.clone());

    // Every foreign event uses the pending notification queue, including
    // broadcasts racing registration. Welcome reaches the main output first.
    let (notification_sender, mut notify_rx) = ClientSender::with_context(sender.context());
    let notification_sender = Arc::new(notification_sender);
    let _notifications = client_state
        .notify_tx
        .subscribe(notification_sender.clone());

    // Register client
    {
        let mut clients = client_state.clients.lock().expect("client registry");
        clients.insert(
            client_id.clone(),
            ClientEntry {
                sender: sender.clone(),
                notifications: notification_sender.clone(),
                mux_handler: Some(mux_handler.clone()),
            },
        );
    }

    // Send welcome (replaces hello + mux.connect), then the listening ports
    // Subscribe before the first snapshot of the catalog: a concurrent launch
    // must not fall into a gap between welcome and event subscription.
    mux_handler.send_welcome().await;
    super::ports::greet(&client_state, &sender).await;

    // Heartbeat: ping every 15s, consider dead after 30s of silence
    let mut ping_interval = time::interval(Duration::from_secs(15));
    ping_interval.tick().await; // consume the immediate first tick
    let mut last_activity = Instant::now();

    // Read session input, its bounded notification queue, and heartbeat concurrently
    loop {
        tokio::select! {
            _ = closing.changed() => break,
            msg = read.recv() => {
                let (msg, _incoming_bytes) = match msg {
                    Some(m) => m.into_parts(),
                    None => break,
                };

                last_activity = Instant::now();

                match msg {
                    Message::Text(text) => {
                        let _decoded = match splitty_transport::codec::reserve_json(text.as_bytes(), &sender.context().budget, false) {
                            Ok(lease) => lease,
                            Err(_) => break,
                        };
                        let mut value = match serde_json::from_str::<serde_json::Value>(&text) {
                            Ok(value) => value,
                            Err(error) => { tracing::warn!(%error, "invalid json message"); continue; }
                        };
                        // Mux/job conversions have enums and typed fields whose
                        // error formatting can quote arbitrary input strings.
                        let _typed = if value["type"].as_str().is_some_and(|kind| kind.starts_with("mux.")) {
                            match splitty_transport::codec::reserve_json_typed(text.as_bytes(), &sender.context().budget, false) {
                                Ok(lease) => Some(lease),
                                Err(_) => break,
                            }
                        } else { None };
                        {
                            if value["type"] == "editor.register" {
                                client_state.editor_requests.register(&client_id, sender.clone(), connection.attached_sessions()).await;
                                continue;
                            }
                            if value["type"] == "editor.result" {
                                client_state.editor_requests.reply(&client_id, &value).await;
                                continue;
                            }
                            let offers = &client_state.file_offers;
                            match value["type"].as_str() {
                                Some("file.register") => {
                                    offers.register(&client_id, sender.clone(), connection.attached_sessions()).await;
                                    continue;
                                }
                                Some("file.accept") => { offers.accept(&client_id, &value).await; continue; }
                                Some("file.decline") => { offers.decline(&client_id, &value).await; continue; }
                                Some("file.read") => { offers.read(&client_id, &value).await; continue; }
                                Some(codec::MSG_VIEWER_READ) => { viewer_reads.read(&value); continue; }
                                _ => {}
                            }
                            if value["type"]=="mux.op" {
                                let Some(screen)=value["screenId"].as_str() else {continue;};
                                // Job dispatch consumes its operation. Retain only the small
                                // error-routing fields rather than cloning an arbitrary DOM.
                                if matches!(value["op"]["type"].as_str(), Some("create_job" | "stop_job" | "forget_job")) {
                                    let screen = screen.to_owned();
                                    let op_type = value["op"]["type"].as_str().unwrap_or("").to_owned();
                                    let new_pane = value["op"]["newPaneId"].as_str().map(str::to_owned);
                                    let op = value["op"].take();
                                    let Some(result) = client_state.jobs.mux(&screen, op).await else { continue; };
                                    // Spawn delivery for this accepted intent precedes this
                                    // connection's next message, including an immediate resync.
                                    while let Ok(event)=notify_rx.try_recv(){let _=sender.forward(event);}
                                    if let Err(error)=result {
                                        let event=splitty_core::protocol::ServerEvent::mux_op_failed(
                                            op_type,Some(error.details.get("screenId").and_then(serde_json::Value::as_str).unwrap_or(&screen).into()),
                                            error.details.get("paneId").and_then(serde_json::Value::as_str).or(new_pane.as_deref()).map(str::to_owned),error.error);
                                        let _=sender.send_event(&event).await;
                                    }
                                    continue;
                                }
                            }else if value["type"]=="close"{client_state.jobs.close_jobs(None,None,value["id"].as_str()).await;}
                        }
                        // Try mux handler first
                        if value["type"].as_str().is_some_and(|kind| kind.starts_with("mux.")) {
                            mux_handler.handle_value(value).await;
                            continue;
                        }

                        // Move strings from the admitted DOM into the typed request.
                        // The parser lease follows it through the async handler.
                        match codec::JsonRequest::from_value(value) {
                            Ok(jr) => {
                                let req = codec::decode_request(jr);
                                let changes_streams = matches!(req.msg_type.as_str(), "create" | "attach" | "detach" | "close");
                                drop(text);
                                drop(_incoming_bytes);
                                connection.handle_charged(_decoded.charge(req)).await;
                                if changes_streams {
                                    client_state.editor_requests.sessions(&client_id, connection.attached_sessions()).await;
                                    client_state.file_offers.sessions(&client_id, connection.attached_sessions()).await;
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
                    Message::Close => {
                        break;
                    }
                }
            }
            result = notify_rx.recv() => {
                match result {
                    Some(message) => { if sender.forward(message).is_err() { break; } }
                    None => break,
                }
            }
            _ = ping_interval.tick() => {
                client_state.editor_requests.expire().await;
                client_state.file_offers.expire().await;
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

    // Notify the carrier and release only this client's registrations.
    sender.close();
    client_state.editor_requests.disconnect(&client_id).await;
    client_state.file_offers.disconnect(&client_id).await;
    connection.close();

    {
        let mut clients = client_state.clients.lock().expect("client registry");
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
