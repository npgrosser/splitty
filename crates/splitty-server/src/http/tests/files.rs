//! `POST /api/file/offer` through the router, answered by a real WebSocket
//! client that registered for offers, without a child server.
use super::*;
use base64::Engine as _;

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

/// Accept the offer `id` and read it to its end.
async fn take_offer(socket: &mut Socket, id: &serde_json::Value) -> Vec<u8> {
    send(socket, json!({"type":"file.accept","offerId":id})).await;
    let mut received = Vec::new();
    loop {
        send(
            socket,
            json!({"type":"file.read","offerId":id,"offset":received.len()}),
        )
        .await;
        let chunk = message_type(socket, "file.chunk").await;
        assert_eq!(chunk["offset"], received.len());
        received.extend(
            base64::engine::general_purpose::STANDARD
                .decode(chunk["data"].as_str().expect("data"))
                .expect("base64"),
        );
        if chunk["eof"] == true {
            return received;
        }
    }
}

async fn offer(app: axum::Router, input: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let response = app
        .oneshot(
            Request::post("/api/file/offer")
                .header("content-type", "application/json")
                .body(Body::from(input.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .expect("body");
    (status, serde_json::from_slice(&body).expect("JSON"))
}

/// control-api, File Offer Requests (Saved by the window, No window, A
/// folder), access-control, File offer on the owner socket, and cli,
/// Download Folders As Zip (A folder saved as a zip, Interrupted while
/// waiting): the owner control socket offers a file to the registered window
/// attached to the session, which reads it in chunks; `splitty download`
/// offers a folder as a zip it removes afterwards; the HTTP listener needs a
/// credential.
#[tokio::test]
async fn a_file_offer_is_saved_by_the_registered_attached_window() {
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
        .contains(&json!("file-offer-v1")));
    send(&mut socket, json!({"type":"file.register"})).await;
    send(
        &mut socket,
        json!({"type":"create","shell":"/bin/sh","cols":80,"rows":24,"dir":dir.path()}),
    )
    .await;
    let created = message_type(&mut socket, "created").await;
    let session = created["sessionId"].as_str().expect("session").to_owned();
    // A subsequent request/response is a fence after subscription routing updated.
    send(
        &mut socket,
        json!({"type":"attach","id":session,"cols":80,"rows":24}),
    )
    .await;
    message_type(&mut socket, "attached").await;
    let file = dir.path().join("report 界.pdf");
    let bytes: Vec<u8> = (0..1_500_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&file, &bytes).expect("file");
    let control = crate::http::build_control_router(served.state.clone());

    let (status, body) = offer(
        control.clone(),
        json!({"sessionId":session,"path":dir.path()}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_path");

    let input = json!({"sessionId":session,"path":file});
    let worker = tokio::spawn(offer(control.clone(), input.clone()));
    let offered = message_type(&mut socket, "file.offer").await;
    assert_eq!(offered["name"], "report 界.pdf");
    assert_eq!(offered["size"], bytes.len());
    assert_eq!(offered["sessionId"], json!(session));
    let received = take_offer(&mut socket, &offered["offerId"]).await;
    assert_eq!(received, bytes);
    let (status, body) = tokio::time::timeout(WAIT, worker)
        .await
        .expect("reply")
        .expect("worker");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"outcome":"saved"}));

    // cli, Download Folders As Zip: `splitty download` over the control
    // socket packs a folder, offers the archive and removes it once saved,
    // or when interrupted while it waits.
    let control_socket = dir.path().join("control.sock");
    let listener = tokio::net::UnixListener::bind(&control_socket).expect("control socket");
    let serving = tokio::spawn({
        let control = control.clone();
        async move { axum::serve(listener, control).await }
    });
    let server = crate::cli::ServerInfo {
        transport: crate::cli::Transport::LocalSocket(
            control_socket.to_string_lossy().into_owned(),
        ),
        source: "test",
    };
    let folder = dir.path().join("build");
    std::fs::create_dir_all(folder.join("logs")).expect("folder");
    std::fs::write(folder.join("logs").join("run.log"), "ok\n").expect("log");
    let store = dir.path().join("cache").join("download");
    let download = |interrupted: tokio::sync::oneshot::Receiver<()>| {
        let (server, folder, store, session) = (
            server.clone(),
            folder.clone(),
            store.clone(),
            session.clone(),
        );
        tokio::spawn(async move {
            crate::cli::download::download_in(&server, &folder, &session, Some(store), async {
                let _ = interrupted.await;
            })
            .await
        })
    };
    let archives = || std::fs::read_dir(&store).expect("store").count();

    let (_keep, never) = tokio::sync::oneshot::channel();
    let saved = download(never);
    let offered = message_type(&mut socket, "file.offer").await;
    assert_eq!(offered["name"], "build.zip");
    assert_eq!(archives(), 1);
    let archive = take_offer(&mut socket, &offered["offerId"]).await;
    tokio::time::timeout(WAIT, saved)
        .await
        .expect("reply")
        .expect("command")
        .expect("saved");
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive)).expect("zip");
    let names: Vec<String> = zip
        .file_names()
        .map(|name| name.expect("name").into_owned())
        .collect();
    assert_eq!(names, ["build/", "build/logs/", "build/logs/run.log"]);
    let mut log = String::new();
    std::io::Read::read_to_string(
        &mut zip.by_name("build/logs/run.log").expect("log"),
        &mut log,
    )
    .expect("read");
    assert_eq!(log, "ok\n");
    assert_eq!(archives(), 0);

    let (interrupt, interrupted) = tokio::sync::oneshot::channel();
    let waiting = download(interrupted);
    let offered = message_type(&mut socket, "file.offer").await;
    assert_eq!(archives(), 1);
    interrupt.send(()).expect("interrupt");
    let error = tokio::time::timeout(WAIT, waiting)
        .await
        .expect("reply")
        .expect("command")
        .expect_err("interrupted");
    assert!(error.to_string().contains("Interrupted"), "{error}");
    assert_eq!(archives(), 0);
    send(
        &mut socket,
        json!({"type":"file.decline","offerId":offered["offerId"]}),
    )
    .await;
    serving.abort();

    // Detached, the window is no recipient.
    send(&mut socket, json!({"type":"detach","id":session})).await;
    send(
        &mut socket,
        json!({"type":"create","shell":"/bin/sh","cols":80,"rows":24,"dir":dir.path()}),
    )
    .await;
    let fence = message_type(&mut socket, "created").await;
    let (status, body) = offer(control.clone(), input.clone()).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "no_client");

    let url = format!("http://127.0.0.1:{}/api/file/offer", served.port);
    let response = reqwest::Client::new()
        .post(url)
        .json(&input)
        .send()
        .await
        .expect("HTTP");
    assert_eq!(response.status().as_u16(), 401);
    let backend = served.state.ws_state.dispatcher.backend();
    backend.close(&session).expect("close");
    backend
        .close(fence["sessionId"].as_str().expect("session"))
        .expect("close");
}
