use super::*;
use splitty_core::native_api::directory::{DirectoryHost, DirectoryScreen, DirectorySource};
use splitty_transport::{
    budget::{ConnectionBudget, ProcessBudget},
    wire,
};

fn budget() -> ConnectionBudget {
    ConnectionBudget::new(
        ProcessBudget::new(wire::CONNECTION_BYTES, wire::CONNECTION_CONTROL_BYTES),
        wire::CONNECTION_BYTES,
        wire::CONNECTION_CONTROL_BYTES,
    )
}

fn sample() -> Directory {
    let mut host = DirectoryHost::new("build.example");
    host.alias = Some("Build".into());
    host.source = Some(DirectorySource::SshConfig);
    host.last_seen = Some(7);
    host.screens
        .push(DirectoryScreen::new("s1", Some("Work".into()), 2));
    Directory::new(vec!["build.example".into()], vec![host])
}

#[test]
fn validates_boundaries_without_a_twenty_host_cap() {
    let budget = budget();
    let mut directory = sample();
    validate(&directory, &budget).unwrap();
    directory
        .hosts
        .extend((0..25).map(|n| DirectoryHost::new(format!("vm{n}.example"))));
    validate(&directory, &budget).unwrap();
    directory.version = 2;
    assert!(validate(&directory, &budget).is_err());
    directory.version = 1;
    directory.hosts.push(DirectoryHost::new("build.example"));
    assert!(validate(&directory, &budget).is_err());
    directory.hosts.pop();
    directory.hosts[0]
        .screens
        .push(DirectoryScreen::new("s1", None, 0));
    assert!(validate(&directory, &budget).is_err());
    directory.hosts[0].screens.pop();
    directory.hosts[0].screens[0].id = "x".repeat(257);
    assert!(validate(&directory, &budget).is_err());
    directory.hosts[0].screens[0].id = "s1".into();
    directory.hosts[0].screens[0].name = Some("x".repeat(201));
    assert!(validate(&directory, &budget).is_err());
    directory.hosts[0].screens[0].name = None;
    directory.hosts[0]
        .screens
        .extend((0..200).map(|n| DirectoryScreen::new(format!("s{n}x"), None, 0)));
    assert!(validate(&directory, &budget).is_err());
    directory.hosts[0].screens.truncate(1);
    directory.order.push(" bad".into());
    assert!(validate(&directory, &budget).is_err());
    assert_eq!(budget.used(), 0);
}

#[test]
fn typed_decode_rejects_bad_replies_and_releases_leases() {
    let budget = budget();
    let good = serde_json::to_string(&sample()).unwrap();
    for body in [
        "{",
        r#"{"version":2,"order":[],"hosts":[]}"#,
        r#"{"version":1,"order":[],"hosts":[{"target":"bad","screens":[],"source":"unknown"}]}"#,
    ] {
        let reply = wire::Reply {
            id: 1,
            status: 200,
            content_type: wire::ContentType::Json,
            body: body.into(),
            uncertain: false,
        };
        if let Ok(directory) = crate::api_stream::decode_with_budget::<Directory>(&budget, &reply) {
            assert!(validate(&directory, &budget).is_err());
        }
        assert_eq!(budget.used(), 0);
    }
    let reply = wire::Reply {
        id: 1,
        status: 200,
        content_type: wire::ContentType::Json,
        body: good,
        uncertain: false,
    };
    let charged = crate::api_stream::decode_with_budget::<Directory>(&budget, &reply).unwrap();
    validate(&charged, &budget).unwrap();
    let snapshot = Arc::new(HostDirectorySnapshot {
        connection_id: Uuid::new_v4(),
        generation: 1,
        directory: charged,
    });
    let held = snapshot.clone();
    drop(snapshot);
    assert!(budget.used() > 0);
    drop(held);
    assert_eq!(budget.used(), 0);

    let oversized = wire::Reply {
        body: "x".repeat(splitty_core::native_api::MAX_API_BODY + 1),
        ..reply.clone()
    };
    assert!(crate::api_stream::decode_with_budget::<Directory>(&budget, &oversized).is_err());
    let available = wire::CONNECTION_BYTES - wire::CONNECTION_CONTROL_BYTES;
    let held = budget.reserve(available, false).unwrap();
    assert!(crate::api_stream::decode_with_budget::<Directory>(&budget, &reply).is_err());
    drop(held);
    assert_eq!(budget.used(), 0);
}

#[cfg(unix)]
mod fake_peer {
    use super::*;
    use splitty_server::native::client::Connector;
    use splitty_transport::{negotiation, peer::Peer, queue};
    use std::{os::unix::fs::PermissionsExt, time::Duration};

    async fn scripted(
        replies: Vec<(u16, String, bool)>,
    ) -> (ApiStream, tokio::task::JoinHandle<()>, std::path::PathBuf) {
        scripted_with_gate(replies, None).await
    }
    async fn scripted_with_gate(
        replies: Vec<(u16, String, bool)>,
        gate: Option<tokio::sync::oneshot::Receiver<()>>,
    ) -> (ApiStream, tokio::task::JoinHandle<()>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("hub-read-{}", Uuid::new_v4().simple()));
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.join("client.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let protocol = splitty_core::protocol::PROTOCOL_VERSION;
        let info = splitty_core::daemon::DaemonInfo {
            pid: std::process::id(),
            format: Some(2),
            client_socket: Some(path.to_string_lossy().into()),
            instance: Some("a".repeat(32)),
            transport_version: Some(u32::from(wire::VERSION)),
            port: None,
            socket: String::new(),
            started_at: String::new(),
            version: "test".into(),
            protocol,
            keeps_sessions: true,
        };
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (mut read, mut write) = stream.into_split();
            let context = queue::Context::default();
            negotiation::server(&mut read, &mut write, &context.budget)
                .await
                .unwrap();
            let ready = wire::Ready {
                generation: 1,
                instance: "a".repeat(32),
                mux_protocol: protocol,
                purpose: wire::Purpose::Direct,
                limits: wire::Limits::default(),
            };
            negotiation::offer(&mut read, &mut write, &context.budget, &ready)
                .await
                .unwrap();
            let mut peer = Peer::start(read, write, &ready, context);
            let mut gate = gate;
            for (status, body, uncertain) in replies {
                let incoming = peer.recv().await.expect("one passive request");
                let request = incoming
                    .frame
                    .decode_charged::<wire::Request>(&peer.sender.context().budget)
                    .unwrap();
                assert_eq!(request.operation, "remote.directory.get");
                assert!(request.args.is_empty());
                if let Some(wait) = gate.take() {
                    wait.await.unwrap();
                }
                if status == 0 {
                    // Controlled EOF after a read was admitted.
                    return;
                }
                peer.sender
                    .json(
                        wire::Kind::Reply,
                        &wire::Reply {
                            id: request.id,
                            status,
                            content_type: wire::ContentType::Json,
                            body,
                            uncertain,
                        },
                    )
                    .unwrap();
            }
            assert!(peer.recv().await.is_none(), "unexpected automatic retry");
            peer.finish().await.unwrap();
        });
        let api = Connector::default().local(&info).await.unwrap();
        (api.into(), task, dir)
    }

    async fn next(client: &mut HubClient) -> HubUpdate {
        tokio::time::timeout(Duration::from_secs(5), client.next_update())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn loading_ready_then_last_good_failure_and_empty_replacement() {
        let (api, task, dir) = scripted(vec![
            (200, serde_json::to_string(&sample()).unwrap(), false),
            (500, "{}".into(), false),
            (
                200,
                serde_json::to_string(&Directory::new(vec![], vec![])).unwrap(),
                false,
            ),
        ])
        .await;
        let owner = Uuid::new_v4();
        let budget = api.sender().context().budget.clone();
        let mut client = HubClient::new(api, owner);
        assert!(matches!(
            next(&mut client).await.state,
            HostDirectoryState::Loading { previous: None }
        ));
        let first = next(&mut client).await;
        let HostDirectoryState::Ready(snapshot) = first.state else {
            panic!("ready")
        };
        assert_eq!(first.generation, 1);
        assert_eq!(snapshot.directory().hosts.len(), 1);
        assert!(client.refresh(Uuid::new_v4()).is_err());
        client.refresh(owner).unwrap();
        assert!(matches!(
            next(&mut client).await.state,
            HostDirectoryState::Loading { previous: Some(_) }
        ));
        let failed = next(&mut client).await;
        assert_eq!(failed.generation, 2);
        assert!(matches!(
            &failed.state,
            HostDirectoryState::Failed {
                previous: Some(_),
                ..
            }
        ));
        client.refresh(owner).unwrap();
        assert!(matches!(
            next(&mut client).await.state,
            HostDirectoryState::Loading { previous: Some(_) }
        ));
        let empty = next(&mut client).await;
        let HostDirectoryState::Ready(empty) = empty.state else {
            panic!("empty directory")
        };
        assert_eq!(empty.generation(), 3);
        assert!(empty.directory().hosts.is_empty());
        drop(snapshot);
        drop(empty);
        drop(failed);
        client.finish().await.unwrap();
        task.await.unwrap();
        assert_eq!(budget.used(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn first_failure_is_distinct_from_empty_and_waits_for_retry() {
        let (api, task, dir) = scripted(vec![
            (200, "{invalid".into(), false),
            (
                200,
                serde_json::to_string(&Directory::new(vec![], vec![])).unwrap(),
                false,
            ),
        ])
        .await;
        let owner = Uuid::new_v4();
        let mut client = HubClient::new(api, owner);
        assert!(matches!(
            next(&mut client).await.state,
            HostDirectoryState::Loading { previous: None }
        ));
        assert!(matches!(
            next(&mut client).await.state,
            HostDirectoryState::Failed { previous: None, .. }
        ));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), client.next_update())
                .await
                .is_err()
        );
        client.refresh(owner).unwrap();
        assert!(matches!(
            next(&mut client).await.state,
            HostDirectoryState::Loading { previous: None }
        ));
        assert!(matches!(next(&mut client).await.state,
            HostDirectoryState::Ready(ref snapshot) if snapshot.directory().hosts.is_empty()));
        client.finish().await.unwrap();
        task.await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn cancelled_receives_retain_read_and_coalesce_superseded_replies() {
        use std::{future::Future, task::Poll};
        let (send, receive) = tokio::sync::oneshot::channel();
        let (api, task, dir) = scripted_with_gate(
            vec![
                (200, serde_json::to_string(&sample()).unwrap(), false),
                (
                    200,
                    serde_json::to_string(&Directory::new(vec![], vec![])).unwrap(),
                    false,
                ),
            ],
            Some(receive),
        )
        .await;
        let owner = Uuid::new_v4();
        let mut client = HubClient::new(api, owner);
        assert!(matches!(
            next(&mut client).await.state,
            HostDirectoryState::Loading { previous: None }
        ));
        for _ in 0..3 {
            let mut pending = Box::pin(client.next_update());
            std::future::poll_fn(|cx| {
                assert!(pending.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
        }
        client.refresh(owner).unwrap();
        client.refresh(owner).unwrap();
        send.send(()).unwrap();
        let loading = next(&mut client).await;
        assert_eq!(loading.generation, 3);
        assert!(matches!(
            loading.state,
            HostDirectoryState::Loading { previous: None }
        ));
        let ready = next(&mut client).await;
        assert_eq!(ready.generation, 3);
        assert!(
            matches!(ready.state, HostDirectoryState::Ready(ref snapshot) if snapshot.directory().hosts.is_empty())
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(20), client.next_update())
                .await
                .is_err()
        );
        drop(ready);
        client.finish().await.unwrap();
        task.await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn refresh_during_eof_requests_one_reopen_without_reusing_broken_stream() {
        let (send, receive) = tokio::sync::oneshot::channel();
        let (api, task, dir) =
            scripted_with_gate(vec![(0, String::new(), false)], Some(receive)).await;
        let owner = Uuid::new_v4();
        let mut client = HubClient::new(api, owner);
        assert!(matches!(
            next(&mut client).await.state,
            HostDirectoryState::Loading { .. }
        ));
        client.refresh(owner).unwrap();
        send.send(()).unwrap();
        let update = next(&mut client).await;
        assert_eq!(update.generation, 2);
        assert!(matches!(
            update.state,
            HostDirectoryState::Loading { previous: None }
        ));
        assert!(client.is_broken());
        assert!(client.take_reopen_due());
        assert!(!client.take_reopen_due());
        let _ = client.finish().await;
        task.await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
