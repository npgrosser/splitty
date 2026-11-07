use super::*;
use crate::appearance::DrawerVisibilityOutcome;
use crate::zoom::{Zoom, ZoomAction, ZoomOutcome};
use splitty_server::{native::client::Connector, service::catalog::Role};
use splitty_transport::{negotiation, peer::Peer, queue, wire};
use std::{os::unix::fs::PermissionsExt, time::Duration};

fn home(level: f64) -> String {
    let mut settings = splitty_core::config::HomeConfig::default();
    settings.appearance.zoom = level;
    settings.appearance.zoom_scope = "terminals".into();
    serde_json::to_string(&settings).unwrap()
}
struct Exchange {
    operation: &'static str,
    status: u16,
    uncertain: bool,
    body: String,
}
enum Tail {
    Closed,
    Disconnect,
    Hold(tokio::sync::oneshot::Sender<()>),
    HoldTemplate(tokio::sync::oneshot::Sender<()>),
}
async fn scripted(
    exchanges: Vec<Exchange>,
) -> (Client, tokio::task::JoinHandle<()>, std::path::PathBuf) {
    scripted_tail(exchanges, Tail::Closed).await
}
async fn scripted_tail(
    exchanges: Vec<Exchange>,
    tail: Tail,
) -> (Client, tokio::task::JoinHandle<()>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("sz-{}", Uuid::new_v4().simple()));
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
        for exchange in exchanges {
            let message = peer.recv().await.expect("expected request");
            assert_eq!(message.frame.kind, wire::Kind::Request);
            let request = message
                .frame
                .decode_charged::<wire::Request>(&peer.sender.context().budget)
                .unwrap();
            assert_eq!(request.operation, exchange.operation);
            if request.operation == "home.update" {
                // No unrelated appearance or home fields are echoed back.
                assert_eq!(request.args.len(), 1);
                assert_eq!(request.args["body"].as_object().unwrap().len(), 1);
                if let Some(appearance) = request.args["body"]["appearance"].as_object() {
                    assert_eq!(appearance.len(), 1);
                    assert!(appearance["zoom"].is_number());
                } else {
                    let drawer = request.args["body"]["drawer"].as_object().unwrap();
                    assert_eq!(drawer.len(), 1);
                    assert!(drawer["pinned"].is_boolean());
                }
            }
            peer.sender
                .json(
                    wire::Kind::Reply,
                    &wire::Reply {
                        id: request.id,
                        status: exchange.status,
                        content_type: wire::ContentType::Json,
                        body: exchange.body,
                        uncertain: exchange.uncertain,
                    },
                )
                .unwrap();
        }
        match tail {
            Tail::Closed => assert!(
                peer.recv().await.is_none(),
                "mutation was retried without a new intent"
            ),
            Tail::Disconnect | Tail::Hold(_) | Tail::HoldTemplate(_) => {
                let message = peer
                    .recv()
                    .await
                    .expect("update admitted before disconnect/cancel");
                let request = message
                    .frame
                    .decode_charged::<wire::Request>(&peer.sender.context().budget)
                    .unwrap();
                assert_eq!(
                    request.operation,
                    if matches!(&tail, Tail::HoldTemplate(_)) {
                        "template.delete"
                    } else {
                        "home.update"
                    }
                );
                drop(request);
                drop(message);
                if let Tail::Hold(observed) | Tail::HoldTemplate(observed) = tail {
                    observed.send(()).unwrap();
                    while let Some(message) = peer.recv().await {
                        assert_eq!(message.frame.kind, wire::Kind::Cancel);
                    }
                }
            }
        }
        peer.finish().await.unwrap();
    });
    let client = Connector::default().local(&info).await.unwrap();
    (client.into(), task, dir)
}
async fn next(client: &mut LocalServiceClient) -> LocalServiceUpdate {
    tokio::time::timeout(Duration::from_secs(5), client.next_update())
        .await
        .unwrap()
}

#[tokio::test]
async fn uncertain_write_is_not_retried_and_later_intent_reads_again() {
    let exchanges = vec![
        Exchange {
            operation: "home.get",
            status: 200,
            uncertain: false,
            body: home(1.0),
        },
        Exchange {
            operation: "home.update",
            status: 200,
            uncertain: true,
            body: home(1.1),
        },
        // Reconciliation is a read job between the two writes. Theme decoding
        // fails independently; it must neither hide nor retry either mutation.
        Exchange {
            operation: "home.get",
            status: 200,
            uncertain: false,
            body: home(1.1),
        },
        Exchange {
            operation: "theme.get",
            status: 500,
            uncertain: false,
            body: "{}".into(),
        },
        Exchange {
            operation: "home.get",
            status: 200,
            uncertain: false,
            body: home(1.1),
        },
        Exchange {
            operation: "home.update",
            status: 200,
            uncertain: false,
            body: home(1.25),
        },
    ];
    let (api, task, dir) = scripted(exchanges).await;
    let owner = Uuid::new_v4();
    let budget = api.sender().context().budget.clone();
    let mut client = LocalServiceClient::new(api, owner, ColorScheme::Dark);
    client.dirty = [false; 2];
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    client.enqueue_zoom(owner, first, ZoomAction::In).unwrap();
    client.enqueue_zoom(owner, second, ZoomAction::In).unwrap();
    super::tests::begin(&mut client).await;
    // A read-generation change cannot suppress an accepted write outcome.
    client.set_scheme(owner, ColorScheme::Light).unwrap();
    let LocalServiceUpdate::Zoom(result) = next(&mut client).await else {
        panic!("missing outcome")
    };
    assert_eq!(result.request_id, first);
    assert!(matches!(result.outcome, ZoomOutcome::Uncertain { .. }));
    assert!(matches!(
        next(&mut client).await,
        LocalServiceUpdate::Appearance(Err(_))
    ));
    let LocalServiceUpdate::Zoom(result) = next(&mut client).await else {
        panic!("missing second outcome")
    };
    assert_eq!(result.request_id, second);
    assert_eq!(
        result.outcome,
        ZoomOutcome::Confirmed(Zoom::from_values(1.25, "terminals"))
    );
    client.finish().await.unwrap();
    task.await.unwrap();
    assert_eq!(budget.used(), 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn rejected_and_malformed_write_replies_are_distinct() {
    for (status, body, uncertain) in [(403, "{}", false), (200, "{", false), (403, "{}", true)] {
        let (api, task, dir) = scripted(vec![
            Exchange {
                operation: "home.get",
                status: 200,
                uncertain: false,
                body: home(1.0),
            },
            Exchange {
                operation: "home.update",
                status,
                uncertain,
                body: body.into(),
            },
        ])
        .await;
        let owner = Uuid::new_v4();
        let mut client = LocalServiceClient::new(api, owner, ColorScheme::Dark);
        client.dirty = [false; 2];
        client
            .enqueue_zoom(owner, Uuid::new_v4(), ZoomAction::In)
            .unwrap();
        let LocalServiceUpdate::Zoom(result) = next(&mut client).await else {
            panic!("outcome")
        };
        assert_eq!(
            matches!(result.outcome, ZoomOutcome::Rejected { .. }),
            status == 403 && !uncertain
        );
        client.finish().await.unwrap();
        task.await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test]
async fn capacity_owner_saturation_and_pending_cancel_are_bounded() {
    let (service, path) = crate::appearance::local::tests::fixture();
    service.state.config.write().unwrap().appearance.zoom = 3.0;
    let api = Connector::default()
        .embedded(service.state.clone(), Role::Main)
        .await
        .unwrap();
    let budget = api.sender().context().budget.clone();
    let owner = Uuid::new_v4();
    let first = Uuid::new_v4();
    let mut client = LocalServiceClient::new(api, owner, ColorScheme::Dark);
    assert!(client
        .enqueue_zoom(Uuid::new_v4(), first, ZoomAction::In)
        .is_err());
    client.enqueue_zoom(owner, first, ZoomAction::In).unwrap();
    assert!(client.enqueue_zoom(owner, first, ZoomAction::In).is_err());
    for _ in 1..MAX_QUEUED_ZOOM {
        client
            .enqueue_zoom(owner, Uuid::new_v4(), ZoomAction::In)
            .unwrap();
    }
    assert_eq!(
        client
            .enqueue_zoom(owner, Uuid::new_v4(), ZoomAction::In)
            .unwrap_err()
            .kind(),
        io::ErrorKind::WouldBlock
    );
    // A read must progress even with a full mutation queue.
    assert!(matches!(
        next(&mut client).await,
        LocalServiceUpdate::Appearance(Ok(_))
    ));
    let LocalServiceUpdate::Zoom(result) = next(&mut client).await else {
        panic!("write starved")
    };
    assert_eq!(result.request_id, first);
    assert_eq!(
        result.outcome,
        ZoomOutcome::Confirmed(Zoom::from_values(3.0, "window"))
    );
    assert!(matches!(
        next(&mut client).await,
        LocalServiceUpdate::ShellProfiles(Ok(_))
    ));
    client
        .enqueue_zoom(owner, Uuid::new_v4(), ZoomAction::Out)
        .unwrap();
    super::tests::begin(&mut client).await;
    assert!(client.active_zoom.is_some());
    client.finish().await.unwrap();
    assert_eq!(budget.used(), 0);
    assert_eq!(service.state.config.read().unwrap().appearance.zoom, 3.0);
    service.close().await.unwrap();
    assert!(!path.exists());
}

fn theme() -> String {
    serde_json::to_string(&splitty_core::theme::resolve_theme(
        "splitty-dark",
        std::path::Path::new("/nonexistent-splitty-zoom-test"),
    ))
    .unwrap()
}
#[tokio::test]
async fn uncertain_outcome_refresh_has_newer_generation_without_broadcast() {
    let (api, task, dir) = scripted(vec![
        Exchange {
            operation: "home.get",
            status: 200,
            uncertain: false,
            body: home(1.0),
        },
        Exchange {
            operation: "theme.get",
            status: 200,
            uncertain: false,
            body: theme(),
        },
        Exchange {
            operation: "home.get",
            status: 200,
            uncertain: false,
            body: home(1.0),
        },
        Exchange {
            operation: "home.update",
            status: 503,
            uncertain: true,
            body: "{}".into(),
        },
        Exchange {
            operation: "home.get",
            status: 200,
            uncertain: false,
            body: home(1.1),
        },
        Exchange {
            operation: "theme.get",
            status: 200,
            uncertain: false,
            body: theme(),
        },
    ])
    .await;
    let owner = Uuid::new_v4();
    let mut client = LocalServiceClient::new(api, owner, ColorScheme::Dark);
    client.dirty[1] = false;
    let LocalServiceUpdate::Appearance(Ok(before)) = next(&mut client).await else {
        panic!("initial appearance")
    };
    client
        .enqueue_zoom(owner, Uuid::new_v4(), ZoomAction::In)
        .unwrap();
    assert!(matches!(
        next(&mut client).await,
        LocalServiceUpdate::Zoom(ZoomResult {
            outcome: ZoomOutcome::Uncertain { .. },
            ..
        })
    ));
    let LocalServiceUpdate::Appearance(Ok(after)) = next(&mut client).await else {
        panic!("reconciled appearance")
    };
    assert!(after.generation() > before.generation());
    assert_eq!(after.settings().zoom, 1.1);
    assert!(
        tokio::time::timeout(Duration::from_millis(15), client.next_update())
            .await
            .is_err(),
        "no idle retry"
    );
    drop(before);
    drop(after);
    client.finish().await.unwrap();
    task.await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn lost_mutation_reply_is_uncertain_and_pending_write_cancellation_joins() {
    for cancel in [false, true] {
        let (sent, received) = tokio::sync::oneshot::channel();
        let tail = if cancel {
            Tail::Hold(sent)
        } else {
            Tail::Disconnect
        };
        let (api, task, dir) = scripted_tail(
            vec![Exchange {
                operation: "home.get",
                status: 200,
                uncertain: false,
                body: home(1.0),
            }],
            tail,
        )
        .await;
        let budget = api.sender().context().budget.clone();
        let owner = Uuid::new_v4();
        let mut client = LocalServiceClient::new(api, owner, ColorScheme::Dark);
        client.dirty = [false; 2];
        client
            .enqueue_zoom(owner, Uuid::new_v4(), ZoomAction::In)
            .unwrap();
        if cancel {
            tokio::time::timeout(Duration::from_secs(5), async {
                tokio::select! {
                    result=received=>result.unwrap(),
                    _=client.next_update()=>panic!("held write should remain pending"),
                }
            })
            .await
            .unwrap();
            assert!(client.pending.is_some());
        } else {
            assert!(matches!(
                next(&mut client).await,
                LocalServiceUpdate::Zoom(ZoomResult {
                    outcome: ZoomOutcome::Uncertain { .. },
                    ..
                })
            ));
        }
        client.finish().await.unwrap();
        task.await.unwrap();
        assert_eq!(budget.used(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test]
async fn drawer_visibility_confirms_only_service_value_and_refreshes_without_replay() {
    let mut saved = splitty_core::config::HomeConfig::default();
    saved.drawer.pinned = false;
    saved.drawer.sidebar_width = 280;
    saved.appearance.font_size = 19;
    let mut after = saved.clone();
    after.drawer.pinned = true;
    let (api, task, dir) = scripted(vec![
        Exchange {
            operation: "home.update",
            status: 200,
            uncertain: false,
            body: serde_json::to_string(&after).unwrap(),
        },
        Exchange {
            operation: "home.get",
            status: 200,
            uncertain: false,
            body: serde_json::to_string(&after).unwrap(),
        },
        Exchange {
            operation: "theme.get",
            status: 200,
            uncertain: false,
            body: theme(),
        },
    ])
    .await;
    let owner = Uuid::new_v4();
    let request = Uuid::new_v4();
    let budget = api.sender().context().budget.clone();
    let mut client = LocalServiceClient::new(api, owner, ColorScheme::Dark);
    client.dirty = [false; 2];
    client
        .enqueue_drawer_visibility(owner, request, true)
        .unwrap();
    let LocalServiceUpdate::DrawerVisibility(result) = next(&mut client).await else {
        panic!("visibility outcome")
    };
    assert_eq!(result.connection_id, owner);
    assert_eq!(result.request_id, request);
    assert_eq!(result.outcome, DrawerVisibilityOutcome::Confirmed(true));
    let LocalServiceUpdate::Appearance(Ok(snapshot)) = next(&mut client).await else {
        panic!("refreshed presentation")
    };
    assert!(snapshot.drawer().pinned);
    assert_eq!(snapshot.drawer().sidebar_width, 280);
    assert_eq!(snapshot.settings().font_size, 19);
    assert_eq!(snapshot.generation(), 2);
    drop(snapshot);
    client.finish().await.unwrap();
    task.await.unwrap();
    assert_eq!(budget.used(), 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn drawer_visibility_admits_one_and_distinguishes_failure_from_uncertainty() {
    for (status, uncertain, body, rejected) in [
        (403, false, "{}", true),
        (200, true, "{}", false),
        (200, false, "{", false),
    ] {
        let (api, task, dir) = scripted(vec![
            Exchange {
                operation: "home.update",
                status,
                uncertain,
                body: body.into(),
            },
            Exchange {
                operation: "home.get",
                status: 200,
                uncertain: false,
                body: home(1.0),
            },
            Exchange {
                operation: "theme.get",
                status: 200,
                uncertain: false,
                body: theme(),
            },
        ])
        .await;
        let owner = Uuid::new_v4();
        let request = Uuid::new_v4();
        let mut client = LocalServiceClient::new(api, owner, ColorScheme::Dark);
        client.dirty = [false; 2];
        assert!(client
            .enqueue_drawer_visibility(Uuid::new_v4(), request, true)
            .is_err());
        client
            .enqueue_drawer_visibility(owner, request, true)
            .unwrap();
        assert_eq!(
            client
                .enqueue_drawer_visibility(owner, request, false)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            client
                .enqueue_drawer_visibility(owner, Uuid::new_v4(), false)
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        super::tests::begin(&mut client).await;
        assert_eq!(
            client
                .enqueue_drawer_visibility(owner, Uuid::new_v4(), false)
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        let LocalServiceUpdate::DrawerVisibility(result) = next(&mut client).await else {
            panic!("visibility outcome")
        };
        assert_eq!(result.request_id, request);
        assert_eq!(
            matches!(result.outcome, DrawerVisibilityOutcome::Rejected { .. }),
            rejected
        );
        assert!(matches!(
            next(&mut client).await,
            LocalServiceUpdate::Appearance(Ok(_))
        ));
        client.finish().await.unwrap();
        task.await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test]
async fn drawer_visibility_shutdown_joins_pending_write() {
    let (sent, received) = tokio::sync::oneshot::channel();
    let (api, task, dir) = scripted_tail(vec![], Tail::Hold(sent)).await;
    let budget = api.sender().context().budget.clone();
    let owner = Uuid::new_v4();
    let mut client = LocalServiceClient::new(api, owner, ColorScheme::Dark);
    client.dirty = [false; 2];
    client
        .enqueue_drawer_visibility(owner, Uuid::new_v4(), false)
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        tokio::select! {
            result = received => result.unwrap(),
            _ = client.next_update() => panic!("held write completed"),
        }
    })
    .await
    .unwrap();
    assert!(client.pending.is_some());
    client.finish().await.unwrap();
    task.await.unwrap();
    assert_eq!(budget.used(), 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn template_mutation_uncertainty_is_identified_and_never_replayed() {
    let (mut client, server, path) = scripted(vec![
        Exchange {
            operation: "template.get",
            status: 404,
            uncertain: false,
            body: "{}".into(),
        },
        Exchange {
            operation: "template.save",
            status: 204,
            uncertain: true,
            body: String::new(),
        },
    ])
    .await;
    let owner = Uuid::new_v4();
    let request = Uuid::new_v4();
    let result = crate::templates::execute(
        &mut client,
        owner,
        request,
        crate::templates::TemplateApiRequest::Save {
            draft: crate::templates::tests::draft(owner),
            slug: "uncertain".into(),
            name: "Saved".into(),
            included_commands: vec![],
            overwrite_confirmed: false,
        },
        &Notify::new(),
    )
    .await;
    assert_eq!(result.request_id, request);
    assert!(matches!(
        result.outcome,
        crate::templates::TemplateApiOutcome::Uncertain { .. }
    ));
    client.finish().await.unwrap();
    server.await.unwrap();
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn template_cancelled_poll_retains_request_and_finish_joins_cancel() {
    let (seen, observed) = tokio::sync::oneshot::channel();
    let (api, server, path) = scripted_tail(vec![], Tail::HoldTemplate(seen)).await;
    let budget = api.sender().context().budget.clone();
    let owner = Uuid::new_v4();
    let mut client = LocalServiceClient::new(api, owner, ColorScheme::Dark);
    client
        .enqueue_template(
            owner,
            Uuid::new_v4(),
            crate::templates::TemplateApiRequest::Delete {
                slug: "captured".into(),
            },
        )
        .unwrap();
    super::tests::begin(&mut client).await;
    observed.await.unwrap();
    assert!(client.active_template.is_some());
    client.finish().await.unwrap();
    server.await.unwrap();
    assert_eq!(budget.used(), 0);
    std::fs::remove_dir_all(path).unwrap();
}
#[tokio::test]
async fn template_catalog_limit_rejects_atomically_without_hot_retry() {
    let body = serde_json::to_string(
        &(0..257)
            .map(|i| serde_json::json!({"slug":format!("item-{i}"),"displayName":"Item"}))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let (mut client, server, path) = scripted(vec![Exchange {
        operation: "template.list",
        status: 200,
        uncertain: false,
        body,
    }])
    .await;
    let result = crate::templates::execute(
        &mut client,
        Uuid::new_v4(),
        Uuid::new_v4(),
        crate::templates::TemplateApiRequest::List,
        &Notify::new(),
    )
    .await;
    assert!(matches!(
        result.outcome,
        crate::templates::TemplateApiOutcome::Rejected { .. }
    ));
    client.finish().await.unwrap();
    server.await.unwrap();
    std::fs::remove_dir_all(path).unwrap();
}
