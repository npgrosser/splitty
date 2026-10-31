//! terminal-sessions, PTY Size Policy: two real WebSocket connections share a
//! real PTY through the router, without a child server.
use super::*;

/// The next text message of `expected` type, answering pings meanwhile.
async fn message_type(socket: &mut Socket, expected: &str) -> serde_json::Value {
    loop {
        match next(socket).await {
            Message::Text(text) => {
                let value: serde_json::Value = serde_json::from_str(&text).expect("JSON");
                if value["type"] == expected {
                    return value;
                }
            }
            Message::Ping(bytes) => socket.send(Message::Pong(bytes)).await.expect("pong"),
            _ => {}
        }
    }
}

async fn send(socket: &mut Socket, value: serde_json::Value) {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .expect("send");
}

/// Read until the session's output contains `needle`.
async fn output_until(socket: &mut Socket, session: &str, needle: &str) {
    let mut output = String::new();
    while !output.contains(needle) {
        match next(socket).await {
            Message::Binary(frame) => {
                if let Some((id, data)) = crate::ws::codec::decode_binary_frame(&frame) {
                    if id == session {
                        output.push_str(&String::from_utf8_lossy(data));
                    }
                }
            }
            Message::Ping(bytes) => socket.send(Message::Pong(bytes)).await.expect("pong"),
            _ => {}
        }
    }
}

fn size_of(event: &serde_json::Value) -> (u64, u64) {
    (
        event["cols"].as_u64().expect("cols"),
        event["rows"].as_u64().expect("rows"),
    )
}

#[tokio::test]
async fn two_connections_share_the_smaller_active_size() {
    use splitty_core::pty::testing::pty_available;
    if !pty_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let served = Served::start(hub(&host, dir.path()), dir.path()).await;

    let mut a = served.open_local().await;
    let welcome = message_type(&mut a, "mux.welcome").await;
    assert!(welcome["capabilities"]
        .as_array()
        .expect("capabilities")
        .contains(&json!("pty-size-v1")));
    send(&mut a, json!({"type":"activity"})).await;
    send(
        &mut a,
        json!({"type":"create","shell":"/bin/sh","cols":120,"rows":40,"dir":dir.path()}),
    )
    .await;
    let created = message_type(&mut a, "created").await;
    let session = created["sessionId"].as_str().expect("session").to_owned();
    let size = message_type(&mut a, "size").await;
    assert_eq!(size_of(&size), (120, 40));
    assert_eq!(size["policy"], "active");

    let mut b = served.open_local().await;
    message_type(&mut b, "mux.welcome").await;
    send(&mut b, json!({"type":"activity"})).await;
    send(&mut b, json!({"type":"attach","id":session})).await;
    assert_eq!(size_of(&message_type(&mut b, "size").await), (120, 40));
    message_type(&mut b, "attached").await;
    send(
        &mut b,
        json!({"type":"resize","id":session,"cols":100,"rows":30,"redraw":true}),
    )
    .await;
    assert_eq!(size_of(&message_type(&mut b, "size").await), (100, 30));
    assert_eq!(size_of(&message_type(&mut a, "size").await), (100, 30));

    send(
        &mut a,
        json!({"type":"input","id":session,"data":"stty size\n"}),
    )
    .await;
    output_until(&mut a, &session, "30 100").await;

    // B's window is hidden: A gets its size back.
    send(&mut b, json!({"type":"idle"})).await;
    assert_eq!(size_of(&message_type(&mut a, "size").await), (120, 40));

    send(&mut a, json!({"type":"close","id":session})).await;
    message_type(&mut a, "closed").await;
}

/// terminal-sessions, PTY Size Policy: an update of the settings applies to
/// the running server at once.
#[tokio::test]
async fn updated_settings_reconfigure_the_size_policy() {
    use splitty_core::protocol::sizing::{SizeConfig, SizePolicy};
    let dir = tempfile::tempdir().expect("dir");
    let state = state(ADDR, dir.path());
    let mut updated = splitty_core::config::HomeConfig::default();
    updated.terminal.size_policy = "latest".into();
    updated.terminal.size_idle_seconds = 30;
    routes::apply_home_settings(&state, updated);
    assert_eq!(
        state.ws_state.dispatcher.size_config(),
        SizeConfig {
            policy: SizePolicy::Latest,
            idle: Duration::from_secs(30)
        }
    );
    assert_eq!(state.config.read().unwrap().terminal.size_policy, "latest");
}
