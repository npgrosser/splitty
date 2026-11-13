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
pub(crate) mod fake_peer {
    use super::*;
    use splitty_server::native::client::Connector;
    use splitty_transport::{negotiation, peer::Peer, queue};
    use std::{os::unix::fs::PermissionsExt, time::Duration};

    async fn scripted(
        replies: Vec<(u16, String, bool)>,
    ) -> (ApiStream, tokio::task::JoinHandle<()>, std::path::PathBuf) {
        scripted_with_gate(replies, None).await
    }
    pub(crate) async fn scripted_with_gate(
        replies: Vec<(u16, String, bool)>,
        gate: Option<tokio::sync::oneshot::Receiver<()>>,
    ) -> (ApiStream, tokio::task::JoinHandle<()>, std::path::PathBuf) {
        scripted_with_gate_observed(replies, gate, None).await
    }
    pub(crate) async fn scripted_with_gate_observed(
        replies: Vec<(u16, String, bool)>,
        gate: Option<tokio::sync::oneshot::Receiver<()>>,
        observed: Option<tokio::sync::oneshot::Sender<()>>,
    ) -> (ApiStream, tokio::task::JoinHandle<()>, std::path::PathBuf) {
        scripted_operations(replies, gate, observed, None).await
    }
    pub(crate) async fn scripted_operations(
        replies: Vec<(u16, String, bool)>,
        gate: Option<tokio::sync::oneshot::Receiver<()>>,
        observed: Option<tokio::sync::oneshot::Sender<()>>,
        operations: Option<Vec<(&'static str, serde_json::Value)>>,
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
            let mut observed = observed;
            let mut operations = operations.map(|ops| ops.into_iter());
            for (status, body, uncertain) in replies {
                let incoming = peer.recv().await.expect("one passive request");
                let request = incoming
                    .frame
                    .decode_charged::<wire::Request>(&peer.sender.context().budget)
                    .unwrap();
                if let Some(operations) = &mut operations {
                    let (operation, args) = operations.next().expect("expected operation");
                    assert_eq!(request.operation, operation);
                    assert_eq!(serde_json::to_value(&request.args).unwrap(), args);
                } else {
                    assert_eq!(request.operation, "remote.directory.get");
                    assert!(request.args.is_empty());
                }
                if let Some(sender) = observed.take() {
                    sender.send(()).unwrap();
                }
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
        let api = Connector::isolated().local(&info).await.unwrap();
        (api.into(), task, dir)
    }

    fn owner() -> SourceToken {
        crate::workspace::SourceWorkspace::new(Uuid::new_v4()).source_token()
    }
    async fn management(client: &mut HubClient) -> HostManagementResult {
        match tokio::time::timeout(Duration::from_secs(5), client.next_event())
            .await
            .unwrap()
        {
            HubEvent::Management(result) => result,
            _ => panic!("expected identified mutation result"),
        }
    }
    #[tokio::test]
    async fn metadata_outcomes_validate_identity_and_refresh_without_replay() {
        use HostManagementOutcome::*;
        for (status, body, uncertain, expected) in [
            (200, r#"{"target":"vm1","alias":"Build"}"#, false, Applied),
            (
                200,
                r#"{"target":"other","alias":"Build"}"#,
                false,
                Uncertain,
            ),
            (200, r#"{"target":"vm1","alias":"Wrong"}"#, false, Uncertain),
            (
                200,
                r#"{"target":"vm1","alias":"Build","hidden":true}"#,
                false,
                Uncertain,
            ),
            (400, r#"{"error":"bad request"}"#, false, Rejected),
            (500, r#"{"error":"write failed"}"#, false, Uncertain),
            (200, r#"{"target":"vm1","alias":"Build"}"#, true, Uncertain),
        ] {
            let (api, task, dir) = scripted_operations(
                vec![
                    (status, body.into(), uncertain),
                    (200, serde_json::to_string(&sample()).unwrap(), false),
                ],
                None,
                None,
                Some(vec![
                    (
                        "hostPreferences.update",
                        serde_json::json!({"body":{"target":"vm1","alias":"Build","hidden":false}}),
                    ),
                    ("remote.directory.get", serde_json::json!({})),
                ]),
            )
            .await;
            let owner = owner();
            let id = Uuid::new_v4();
            let mut client = HubClient::new(api, owner.connection_id());
            let admitted = client
                .enqueue(
                    owner,
                    id,
                    HostManagementRequest::connect("vm1", Some("Build")).unwrap(),
                )
                .unwrap();
            assert_eq!(admitted.outcome(), Pending);
            let result = management(&mut client).await;
            assert_eq!(result.outcome(), expected);
            assert_eq!(result.owner(), owner);
            assert_eq!(result.request_id(), id);
            assert_eq!(result.target(), "vm1");
            assert!(matches!(
                next(&mut client).await.state,
                HostDirectoryState::Loading { .. }
            ));
            assert!(matches!(
                next(&mut client).await.state,
                HostDirectoryState::Ready(_)
            ));
            client.finish().await.unwrap();
            task.await.unwrap();
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
    #[tokio::test]
    async fn host_order_uses_metadata_only_and_validates_the_exact_acknowledgement() {
        for (body, outcome) in [
            (
                r#"{"version":1,"order":["vm2","vm1"]}"#,
                HostManagementOutcome::Applied,
            ),
            (
                r#"{"version":1,"order":["vm1","vm2"]}"#,
                HostManagementOutcome::Uncertain,
            ),
        ] {
            let (api, task, dir) = scripted_operations(
                vec![
                    (200, body.into(), false),
                    (200, serde_json::to_string(&sample()).unwrap(), false),
                ],
                None,
                None,
                Some(vec![
                    (
                        "hostOrder.update",
                        serde_json::json!({"body":{"order":["vm2","vm1"]}}),
                    ),
                    ("remote.directory.get", serde_json::json!({})),
                ]),
            )
            .await;
            let owner = owner();
            let mut client = HubClient::new(api, owner.connection_id());
            client
                .enqueue(
                    owner,
                    Uuid::new_v4(),
                    HostManagementRequest::set_order("vm2", vec!["vm2".into(), "vm1".into()])
                        .unwrap(),
                )
                .unwrap();
            assert_eq!(management(&mut client).await.outcome(), outcome);
            assert!(matches!(
                next(&mut client).await.state,
                HostDirectoryState::Loading { .. }
            ));
            assert!(matches!(
                next(&mut client).await.state,
                HostDirectoryState::Ready(_)
            ));
            client.finish().await.unwrap();
            task.await.unwrap();
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[tokio::test]
    async fn changed_during_write_does_not_discard_acknowledgement() {
        let (send, wait) = tokio::sync::oneshot::channel();
        let (observed, received) = tokio::sync::oneshot::channel();
        let (api, task, dir) = scripted_operations(
            vec![
                (200, r#"{"target":"vm1"}"#.into(), false),
                (200, serde_json::to_string(&sample()).unwrap(), false),
            ],
            Some(wait),
            Some(observed),
            Some(vec![
                (
                    "hostPreferences.update",
                    serde_json::json!({"body":{"target":"vm1","alias":null}}),
                ),
                ("remote.directory.get", serde_json::json!({})),
            ]),
        )
        .await;
        let owner = owner();
        let mut client = HubClient::new(api, owner.connection_id());
        client
            .enqueue(
                owner,
                Uuid::new_v4(),
                HostManagementRequest::set_alias("vm1", "").unwrap(),
            )
            .unwrap();
        let mut pending = Box::pin(client.next_event());
        tokio::select! { _=&mut pending=>panic!("reply gate"), _=received=>{} }
        drop(pending);
        client.refresh(owner.connection_id()).unwrap();
        client.refresh(owner.connection_id()).unwrap();
        send.send(()).unwrap();
        assert_eq!(
            management(&mut client).await.outcome(),
            HostManagementOutcome::Applied
        );
        assert!(matches!(
            next(&mut client).await.state,
            HostDirectoryState::Loading { .. }
        ));
        assert!(matches!(
            next(&mut client).await.state,
            HostDirectoryState::Ready(_)
        ));
        client.finish().await.unwrap();
        task.await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn admission_is_bounded_exact_and_finish_discards_unsent_writes() {
        let (api, task, dir) = scripted(vec![]).await;
        let owner = owner();
        let mut client = HubClient::new(api, owner.connection_id());
        let id = Uuid::new_v4();
        let first = HostManagementRequest::forget("vm0").unwrap();
        assert!(client
            .enqueue(
                crate::workspace::SourceWorkspace::new(Uuid::new_v4()).source_token(),
                id,
                first.clone()
            )
            .is_err());
        client.enqueue(owner, id, first.clone()).unwrap();
        assert!(client
            .enqueue(
                owner,
                id,
                HostManagementRequest::forget("different").unwrap()
            )
            .is_err());
        assert!(client.enqueue(owner, Uuid::new_v4(), first).is_err());
        for n in 1..16 {
            client
                .enqueue(
                    owner,
                    Uuid::new_v4(),
                    HostManagementRequest::forget(&format!("vm{n}")).unwrap(),
                )
                .unwrap();
        }
        assert!(client
            .enqueue(
                owner,
                Uuid::new_v4(),
                HostManagementRequest::forget("overflow").unwrap()
            )
            .is_err());
        client.finish().await.unwrap();
        task.await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn lost_mutation_reply_rejects_unsent_queue_then_requests_reopen() {
        let (api, task, dir) = scripted_operations(
            vec![(0, String::new(), false)],
            None,
            None,
            Some(vec![(
                "host.forget",
                serde_json::json!({"body":{"target":"vm1"}}),
            )]),
        )
        .await;
        let owner = owner();
        let mut client = HubClient::new(api, owner.connection_id());
        for target in ["vm1", "vm2"] {
            client
                .enqueue(
                    owner,
                    Uuid::new_v4(),
                    HostManagementRequest::forget(target).unwrap(),
                )
                .unwrap();
        }
        assert_eq!(
            management(&mut client).await.outcome(),
            HostManagementOutcome::Uncertain
        );
        assert_eq!(
            management(&mut client).await.outcome(),
            HostManagementOutcome::Rejected
        );
        assert!(matches!(
            next(&mut client).await.state,
            HostDirectoryState::Loading { .. }
        ));
        assert!(client.take_reopen_due());
        let _ = client.finish().await;
        task.await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn mutation_supersedes_old_read_and_yields_to_reconciliation_between_writes() {
        let directory = serde_json::to_string(&sample()).unwrap();
        let (api, task, dir) = scripted_operations(
            vec![
                (200, directory.clone(), false),
                (204, String::new(), false),
                (200, directory.clone(), false),
                (204, String::new(), false),
                (200, directory, false),
            ],
            None,
            None,
            Some(vec![
                ("remote.directory.get", serde_json::json!({})),
                ("host.forget", serde_json::json!({"body":{"target":"vm1"}})),
                ("remote.directory.get", serde_json::json!({})),
                ("host.forget", serde_json::json!({"body":{"target":"vm2"}})),
                ("remote.directory.get", serde_json::json!({})),
            ]),
        )
        .await;
        let owner = owner();
        let mut client = HubClient::new(api, owner.connection_id());
        assert!(matches!(
            next(&mut client).await.state,
            HostDirectoryState::Loading { .. }
        ));
        for target in ["vm1", "vm2"] {
            client
                .enqueue(
                    owner,
                    Uuid::new_v4(),
                    HostManagementRequest::forget(target).unwrap(),
                )
                .unwrap();
        }
        // No old Ready event can appear between admission and the first ack.
        assert_eq!(management(&mut client).await.target(), "vm1");
        assert!(matches!(
            next(&mut client).await.state,
            HostDirectoryState::Loading { .. }
        ));
        assert!(matches!(
            next(&mut client).await.state,
            HostDirectoryState::Ready(_)
        ));
        assert_eq!(management(&mut client).await.target(), "vm2");
        assert!(matches!(
            next(&mut client).await.state,
            HostDirectoryState::Loading { .. }
        ));
        assert!(matches!(
            next(&mut client).await.state,
            HostDirectoryState::Ready(_)
        ));
        client.finish().await.unwrap();
        task.await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn shutdown_joins_submitted_metadata_and_drops_queued_followup() {
        let (release, gate) = tokio::sync::oneshot::channel();
        let (observed, received) = tokio::sync::oneshot::channel();
        let (api, task, dir) = scripted_operations(
            vec![(204, String::new(), false)],
            Some(gate),
            Some(observed),
            Some(vec![(
                "host.forget",
                serde_json::json!({"body":{"target":"vm1"}}),
            )]),
        )
        .await;
        let owner = owner();
        let mut client = HubClient::new(api, owner.connection_id());
        for target in ["vm1", "vm2"] {
            client
                .enqueue(
                    owner,
                    Uuid::new_v4(),
                    HostManagementRequest::forget(target).unwrap(),
                )
                .unwrap();
        }
        let mut update = Box::pin(client.next_event());
        tokio::select! {_=&mut update=>panic!("reply gate"),_=received=>{}}
        drop(update);
        let mut closing = Box::pin(client.finish());
        tokio::select! {_=&mut closing=>panic!("must join admitted mutation"),_=tokio::time::sleep(Duration::from_millis(20))=>{}}
        release.send(()).unwrap();
        closing.await.unwrap();
        task.await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
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
        for first_status in [200, 500] {
            let (send, receive) = tokio::sync::oneshot::channel();
            let (api, task, dir) = scripted_with_gate(
                vec![
                    (
                        first_status,
                        serde_json::to_string(&sample()).unwrap(),
                        false,
                    ),
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

    #[tokio::test]
    async fn pending_read_finish_cancels_and_releases_its_stream_without_late_result() {
        use std::{future::Future, task::Poll};
        let (release, gate) = tokio::sync::oneshot::channel();
        let (observed, request_seen) = tokio::sync::oneshot::channel();
        let (api, server, dir) = scripted_with_gate_observed(
            vec![(0, String::new(), false)],
            Some(gate),
            Some(observed),
        )
        .await;
        let budget = api.sender().context().budget.clone();
        let mut client = HubClient::new(api, Uuid::new_v4());
        assert!(matches!(
            next(&mut client).await.state,
            HostDirectoryState::Loading { .. }
        ));
        let mut pending = Box::pin(client.next_update());
        std::future::poll_fn(|cx| {
            assert!(pending.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        drop(pending);
        request_seen.await.unwrap();
        client.finish().await.unwrap();
        assert_eq!(budget.used(), 0);
        release.send(()).unwrap();
        server.await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn replacement_owner_cannot_inherit_old_snapshot_or_result() {
        let (old_api, old_server, old_dir) = scripted(vec![(
            200,
            serde_json::to_string(&sample()).unwrap(),
            false,
        )])
        .await;
        let old_owner = Uuid::new_v4();
        let mut old = HubClient::new(old_api, old_owner);
        let _ = next(&mut old).await;
        let old_ready = next(&mut old).await;
        let HostDirectoryState::Ready(old_snapshot) = old_ready.state else {
            panic!("old owner ready")
        };
        old.finish().await.unwrap();
        old_server.await.unwrap();

        let (api, server, dir) = scripted(vec![(
            200,
            serde_json::to_string(&Directory::new(vec![], vec![])).unwrap(),
            false,
        )])
        .await;
        let new_owner = Uuid::new_v4();
        let mut replacement =
            HubClient::with_previous(api, new_owner, 1, Some(old_snapshot.clone()));
        assert!(replacement.last_good().is_none());
        assert!(matches!(
            next(&mut replacement).await.state,
            HostDirectoryState::Loading { previous: None }
        ));
        let current = next(&mut replacement).await;
        assert_eq!(current.connection_id, new_owner);
        assert!(
            matches!(current.state, HostDirectoryState::Ready(ref snapshot) if snapshot.connection_id() == new_owner && snapshot.directory().hosts.is_empty())
        );
        assert_eq!(old_snapshot.connection_id(), old_owner);
        replacement.finish().await.unwrap();
        server.await.unwrap();
        std::fs::remove_dir_all(old_dir).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn queued_old_owner_reply_remains_scoped_after_replacement_starts() {
        use std::{future::Future, task::Poll};
        let (release, gate) = tokio::sync::oneshot::channel();
        let (old_api, old_server, old_dir) = scripted_with_gate(
            vec![(200, serde_json::to_string(&sample()).unwrap(), false)],
            Some(gate),
        )
        .await;
        let old_owner = Uuid::new_v4();
        let mut old = HubClient::new(old_api, old_owner);
        let _ = next(&mut old).await;
        let mut old_wait = Box::pin(old.next_update());
        std::future::poll_fn(|cx| {
            assert!(old_wait.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        drop(old_wait);

        let (new_api, new_server, new_dir) = scripted(vec![(
            200,
            serde_json::to_string(&Directory::new(vec![], vec![])).unwrap(),
            false,
        )])
        .await;
        let new_owner = Uuid::new_v4();
        let mut replacement = HubClient::new(new_api, new_owner);
        let _ = next(&mut replacement).await;
        let new_ready = next(&mut replacement).await;
        assert_eq!(new_ready.connection_id, new_owner);
        release.send(()).unwrap();
        let old_ready = next(&mut old).await;
        assert_eq!(old_ready.connection_id, old_owner);
        assert_ne!(old_ready.connection_id, new_ready.connection_id);
        old.finish().await.unwrap();
        replacement.finish().await.unwrap();
        old_server.await.unwrap();
        new_server.await.unwrap();
        std::fs::remove_dir_all(old_dir).unwrap();
        std::fs::remove_dir_all(new_dir).unwrap();
    }
}
