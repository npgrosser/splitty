//! terminal-sessions, PTY Size Policy: two native session connections share a
//! real PTY through the router, without a child server.
use super::*;

/// The next text message of `expected` type.
async fn message_type(socket: &mut Socket, expected: &str) -> serde_json::Value {
    loop {
        if let Message::Text(text) = next(socket).await {
            let value: serde_json::Value = serde_json::from_str(&text).expect("JSON");
            if value["type"] == expected {
                return value;
            }
        }
    }
}

async fn send(socket: &mut Socket, value: serde_json::Value) {
    socket
        .send(Message::Text(value.to_string()))
        .await
        .expect("send");
}

/// Read until the session's output contains `needle`.
async fn output_until(socket: &mut Socket, session: &str, needle: &str) {
    let mut output = String::new();
    while !output.contains(needle) {
        if let Message::Binary(frame) = next(socket).await {
            if let Some((id, data)) = crate::client_session::codec::decode_binary_frame(&frame) {
                if id == session {
                    output.push_str(&String::from_utf8_lossy(data));
                }
            }
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
    run_two_connections_share_the_smaller_active_size().await;
}

async fn run_two_connections_share_the_smaller_active_size() {
    use splitty_core::pty::testing::pty_available;
    if !pty_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let served = Served::start(hub(&host, dir.path()), dir.path()).await;

    use_test_shell(&served.state);
    let mut a = served.open_local().await;
    let welcome = message_type(&mut a, "mux.welcome").await;
    assert!(welcome["capabilities"]
        .as_array()
        .expect("capabilities")
        .contains(&json!("pty-size-v1")));
    send(&mut a, json!({"type":"activity"})).await;
    let session = spawn_test_pane(&mut a, "test-screen", "test-pane", dir.path(), "").await;
    send(&mut a, json!({"type":"attach","id":session})).await;
    message_type(&mut a, "attached").await;
    send(
        &mut a,
        json!({"type":"resize","id":session,"cols":120,"rows":40}),
    )
    .await;
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

    // Closing B removes only B's contribution and leaves A's session usable.
    send(&mut b, json!({"type":"activity"})).await;
    assert_eq!(size_of(&message_type(&mut a, "size").await), (100, 30));
    b.close().await.expect("close second client");
    assert_eq!(size_of(&message_type(&mut a, "size").await), (120, 40));
    send(
        &mut a,
        json!({"type":"input","id":session,"data":"stty size\n"}),
    )
    .await;
    output_until(&mut a, &session, "40 120").await;

    send(
        &mut a,
        json!({"type":"mux.op","screenId":"test-screen",
        "op":{"type":"close_pane","paneId":"test-pane"}}),
    )
    .await;
    message_type(&mut a, "closed").await;
}

#[tokio::test]
async fn a_failed_process_reports_its_code_and_its_pane_can_still_be_closed() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let host = ScriptedHost::new(dir.path());
    let served = Served::start(hub(&host, dir.path()), dir.path()).await;
    use_test_shell(&served.state);
    served.state.config.write().unwrap().terminal.close_on_exit = "never".into();
    let mut socket = served.open_local().await;
    message_type(&mut socket, "mux.welcome").await;
    let session = spawn_test_pane(
        &mut socket,
        "test-screen",
        "test-pane",
        dir.path(),
        "read line; exit 23",
    )
    .await;
    send(&mut socket, json!({"type":"attach","id":session})).await;
    let attached = message_type(&mut socket, "attached").await;
    assert!(attached.get("homeDir").is_none());
    send(
        &mut socket,
        json!({"type":"input","id":session,"data":"go\n"}),
    )
    .await;
    assert_eq!(message_type(&mut socket, "closed").await["exitCode"], 23);
    send(
        &mut socket,
        json!({"type":"mux.op","screenId":"test-screen",
        "op":{"type":"close_pane","paneId":"test-pane"}}),
    )
    .await;
    send(&mut socket, json!({"type":"attach","id":session})).await;
    let error = message_type(&mut socket, "error").await;
    assert_eq!(error["sessionId"], session);
    assert_eq!(error["error"], "session not found");
}

#[tokio::test]
async fn pane_spawn_resolves_its_directory_and_reports_profile_fallback() {
    use splitty_core::pty::testing::{pty_available, wait_until};
    use splitty_transport::wire::Kind;

    if !pty_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let child = root.join("child");
    std::fs::create_dir(&child).unwrap();
    let home = dirs::home_dir().unwrap().canonicalize().unwrap();
    let state = state(ADDR, dir.path());
    use_test_shell(&state);
    {
        let mut profiles = state.shell_profiles.write().unwrap();
        let mut unavailable = profiles.default_profile().unwrap().clone();
        unavailable.id = "unavailable".into();
        unavailable.available = false;
        profiles.profiles.push(unavailable);
    }
    let (mut peer, task) = conformance::framed(&state, splitty_core::native_api::Role::Main).await;
    conformance::next_text(&mut peer, "mux.welcome").await;
    for (screen, pane, profile, reason, cwd, base, expected_dir) in [
        (
            "root",
            "p1",
            "unknown",
            "missing",
            "child",
            root.as_path(),
            &child,
        ),
        (
            "tilde",
            "p2",
            "unavailable",
            "unavailable",
            "~",
            Path::new(""),
            &home,
        ),
        (
            "missing",
            "p3",
            "unknown",
            "missing",
            "absent",
            root.as_path(),
            &home,
        ),
    ] {
        let command = r#"printf 'cwd=%s pane=%s screen=%s\n' "$(pwd -P)" "$SPLITTY_PANE_ID" "$SPLITTY_SCREEN_ID""#;
        peer.sender
            .json(
                Kind::Text,
                &json!({
                    "type":"mux.op","screenId":screen,"op":{
                        "type":"apply_template","createScreen":true,"paneIds":{"shell":pane},
                        "root":base,"template":{"name":"Test","layout":{"slot":"shell"},
                            "panes":{"shell":{"dir":cwd,"command":command,"restore_mode":"close",
                                              "shell_profile":profile}}}
                    }
                }),
            )
            .unwrap();
        let spawned = conformance::next_text(&mut peer, "mux.pane_spawned").await;
        assert_eq!(spawned["paneId"], pane);
        assert_eq!(spawned["shellProfileId"], "test-sh");
        assert_eq!(spawned["shellProfileFallbackReason"], reason);
        let backend = state.client_state.dispatcher.backend();
        let session = backend.get(spawned["sessionId"].as_str().unwrap()).unwrap();
        let line = format!("cwd={} pane={pane} screen={screen}", expected_dir.display());
        assert!(
            wait_until(WAIT, || String::from_utf8_lossy(&session.scrollback())
                .contains(&line))
            .await,
            "{line}"
        );
        backend.close(session.session_id()).unwrap();
    }
    peer.shutdown().await;
    task.await.unwrap().unwrap();
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
    let detection = splitty_core::shell_profiles::detect_shell_profiles(
        &splitty_core::shell_profiles::DetectionEnvironment::current(),
    );
    crate::service::operations::apply_home_settings(&state.service, updated, &detection);
    assert_eq!(
        state.client_state.dispatcher.size_config(),
        SizeConfig {
            policy: SizePolicy::Latest,
            idle: Duration::from_secs(30)
        }
    );
    assert_eq!(state.config.read().unwrap().terminal.size_policy, "latest");
}
