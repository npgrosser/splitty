//! In-process router and real WebSocket protocol coverage, without a child server.
use super::*;

async fn message_type(socket: &mut Socket, expected: &str) -> serde_json::Value {
    loop {
        match next(socket).await {
            Message::Text(text) => {
                let value: serde_json::Value = serde_json::from_str(&text).expect("JSON");
                if value["type"] == expected {
                    return value;
                }
            }
            Message::Ping(bytes) => {
                socket.send(Message::Pong(bytes)).await.expect("pong");
            }
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

#[tokio::test]
async fn editor_request_uses_registered_attached_websocket_and_owner_route() {
    use splitty_core::pty::testing::pty_available;
    if !pty_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let served = Served::start(hub(&host, dir.path()), dir.path()).await;
    let mut socket = served.open_local().await;
    let welcome = message_type(&mut socket, "mux.welcome").await;
    assert!(welcome["capabilities"]
        .as_array()
        .expect("capabilities")
        .contains(&json!("editor-open-v1")));
    send(&mut socket, json!({"type":"editor.register"})).await;
    send(
        &mut socket,
        json!({"type":"create","shell":"/bin/sh","cols":80,"rows":24,"dir":dir.path()}),
    )
    .await;
    let created = message_type(&mut socket, "created").await;
    let session = created["sessionId"].as_str().expect("session");
    // A subsequent request/response is a fence after subscription routing updated.
    send(
        &mut socket,
        json!({"type":"attach","id":session,"cols":80,"rows":24}),
    )
    .await;
    message_type(&mut socket, "attached").await;
    let file = dir.path().join("file #? 界.rs");
    std::fs::write(&file, "text").expect("file");
    let control = crate::http::build_control_router(served.state.clone());
    let input = json!({"sessionId":session,"path":file,"editor":"zed"});
    let worker = {
        let app = control.clone();
        let input = input.clone();
        tokio::spawn(async move {
            app.oneshot(
                Request::post("/api/editor/open")
                    .header("content-type", "application/json")
                    .body(Body::from(input.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response")
        })
    };
    let request = message_type(&mut socket, "editor.request").await;
    assert_eq!(request["kind"], "file");
    assert_eq!(request["path"], json!(file));
    assert!(request.get("target").is_none());
    send(
        &mut socket,
        json!({"type":"editor.result","requestId":request["requestId"],"outcome":"dispatched"}),
    )
    .await;
    let response = tokio::time::timeout(WAIT, worker)
        .await
        .expect("reply")
        .expect("worker");
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .expect("body");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).expect("JSON"),
        json!({"outcome":"dispatched"})
    );

    for extra in ["target", "uri", "command", "kind"] {
        let mut bad = input.clone();
        bad[extra] = json!("evil");
        let response = control
            .clone()
            .oneshot(
                Request::post("/api/editor/open")
                    .header("content-type", "application/json")
                    .body(Body::from(bad.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert!(response.status().is_client_error());
    }
    let url = format!("http://127.0.0.1:{}/api/editor/open", served.port);
    let response = reqwest::Client::new()
        .post(url)
        .json(&input)
        .send()
        .await
        .expect("HTTP");
    assert_eq!(response.status().as_u16(), 401);
    served
        .state
        .ws_state
        .dispatcher
        .backend()
        .close(session)
        .expect("close");
}
