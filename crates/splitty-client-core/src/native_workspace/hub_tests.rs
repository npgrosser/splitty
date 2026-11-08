use super::*;
use crate::workspace::SourceWorkspace;
use serde_json::json;
use std::time::Duration;
use uuid::Uuid;

#[test]
fn host_directory_admission_requires_the_exact_welcome_capability() {
    assert!(supports_host_directory(
        &json!({"capabilities":["hub-host-decisions-v1"]})
    ));
    for welcome in [
        json!({}),
        json!({"capabilities":[]}),
        json!({"capabilities":["hub-host-decisions-v2"]}),
        json!({"capabilities":"hub-host-decisions-v1"}),
    ] {
        assert!(!supports_host_directory(&welcome));
    }
    let unavailable = HubUpdate::unavailable(Uuid::new_v4());
    assert!(matches!(unavailable.state, HostDirectoryState::Unavailable));
    assert_eq!(unavailable.generation, 0);
}

async fn next(updates: &mut mpsc::Receiver<Update>) -> Update {
    tokio::time::timeout(Duration::from_secs(10), updates.recv())
        .await
        .expect("hub worker timeout")
        .expect("hub worker alive")
}

async fn directory(updates: &mut mpsc::Receiver<Update>) -> HubUpdate {
    loop {
        if let Update::HostDirectory(update) = next(updates).await {
            if matches!(update.state, HostDirectoryState::Ready(_)) {
                return update;
            }
        }
    }
}

#[tokio::test]
async fn passive_directory_refresh_keeps_local_worker_and_cleanup_independent() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let (runtime, path) = crate::appearance::local::tests::fixture();
    std::fs::create_dir_all(&path).unwrap();
    let state = runtime.state.clone();
    state
        .hub
        .update_host_preference(json!({"target":"vm1.example","alias":"Build"}))
        .unwrap();
    state.hub.update_catalog(json!({"target":"vm1.example","screens":[{"id":"cached-screen","name":"Deploy","panes":3}]})).unwrap();
    let service =
        crate::owned_service::OwnedService::from(crate::EmbeddedService::from_runtime(runtime));
    let (commands, receiver) = mpsc::channel(64);
    let (sender, mut updates) = mpsc::channel(128);
    let stop = Arc::new(AtomicBool::new(false));
    let cleanup = Arc::new(AtomicBool::new(false));
    let worker = tokio::spawn(run_service(
        service,
        receiver,
        sender,
        stop.clone(),
        cleanup.clone(),
        ColorScheme::Dark,
    ));
    let mut initial = None;
    let mut route = None;
    let mut attached = None;
    let mut established_owner = None;
    while initial.is_none()
        || route.is_none()
        || attached.as_ref().is_none_or(|session: &String| {
            route
                .as_ref()
                .is_none_or(|(_, routed): &(String, String)| routed != session)
        })
    {
        match next(&mut updates).await {
            Update::HostDirectory(update) => {
                assert_eq!(
                    established_owner,
                    Some(update.connection_id),
                    "workspace owner must publish before Hub state"
                );
                if matches!(update.state, HostDirectoryState::Ready(_)) {
                    initial = Some(update);
                }
            }
            Update::Workspace(model) => {
                let model = local_projection(&model);
                established_owner = Some(model.connection_id());
                if let Some(first) = model.routes().first() {
                    route = Some((first.pane_id.clone(), first.session_id.clone()));
                }
            }
            Update::Attached { session, .. } => attached = Some(session),
            _ => {}
        }
    }
    let initial = initial.unwrap();
    let owner = initial.connection_id;
    let scope = SourceWorkspace::new(owner).source_token();
    let HostDirectoryState::Ready(snapshot) = initial.state else {
        unreachable!()
    };
    assert_eq!(snapshot.directory().hosts[0].target, "vm1.example");
    assert_eq!(
        snapshot.directory().hosts[0].alias.as_deref(),
        Some("Build")
    );
    assert_eq!(snapshot.directory().hosts[0].screens[0].id, "cached-screen");
    assert_eq!(
        snapshot.directory().hosts[0].screens[0].name.as_deref(),
        Some("Deploy")
    );
    assert_eq!(snapshot.directory().hosts[0].screens[0].panes, 3);
    commands
        .send(Command::new(
            scope,
            CommandKind::RefreshHostDirectory {
                connection: Uuid::new_v4(),
            },
        ))
        .await
        .unwrap();
    commands
        .send(Command::new(
            scope,
            CommandKind::RefreshHostDirectory { connection: owner },
        ))
        .await
        .unwrap();
    let (pane, session) = route.unwrap();
    commands
        .send(Command::new(
            scope,
            CommandKind::Input {
                route: TerminalToken::new(scope, &pane, &session),
                bytes: b"printf 'HOSTS_%s\\n' 'PASSIVE_ALIVE'\n".to_vec(),
            },
        ))
        .await
        .unwrap();
    commands
        .send(Command::new(
            scope,
            CommandKind::SetColorScheme {
                connection: owner,
                scheme: ColorScheme::Light,
            },
        ))
        .await
        .unwrap();
    let mut refreshed = None;
    let mut output = Vec::new();
    let mut light = false;
    while refreshed.is_none()
        || !output
            .windows(b"HOSTS_PASSIVE_ALIVE".len())
            .any(|part| part == b"HOSTS_PASSIVE_ALIVE")
        || !light
    {
        match next(&mut updates).await {
            Update::HostDirectory(update)
                if matches!(update.state, HostDirectoryState::Ready(_))
                    && update.generation > initial.generation =>
            {
                refreshed = Some(update)
            }
            Update::Event { event, .. } => {
                if let Some(chunk) = event.output() {
                    if chunk.session_id == session {
                        output.extend_from_slice(chunk.bytes);
                    }
                }
            }
            Update::Appearance(appearance)
                if appearance.connection_id() == owner
                    && appearance.theme().variant == splitty_core::theme::Variant::Light =>
            {
                light = true
            }
            _ => {}
        }
    }
    let refreshed = refreshed.unwrap();
    assert_eq!(refreshed.connection_id, owner);
    assert!(refreshed.generation > initial.generation);
    state
        .hub
        .update_host_preference(json!({"target":"vm1.example","alias":"Changed"}))
        .unwrap();
    let changed = directory(&mut updates).await;
    let HostDirectoryState::Ready(changed_snapshot) = changed.state else {
        unreachable!()
    };
    assert_eq!(
        changed_snapshot.directory().hosts[0].alias.as_deref(),
        Some("Changed")
    );
    assert!(changed.generation > refreshed.generation);
    drop(snapshot);
    drop(changed_snapshot);
    stop.store(true, Ordering::Release);
    drop(commands);
    worker.await.unwrap().unwrap();
    assert!(cleanup.load(Ordering::Acquire));
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn runtime_hub_retains_setup_across_cancelled_receive_and_finishes() {
    use std::{future::Future, task::Poll};
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let service =
        crate::owned_service::OwnedService::from(crate::EmbeddedService::from_runtime(runtime));
    let owner = Uuid::new_v4();
    let mut hub = RuntimeHub::new(&service, owner);
    let mut pending = Box::pin(hub.next_update());
    std::future::poll_fn(|cx| {
        assert!(pending.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(pending);
    hub.refresh(owner).unwrap();
    hub.finish().await.unwrap();
    service.close().await.unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn prior_stream_cleanup_failure_is_not_reported_as_successful_shutdown() {
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let service =
        crate::owned_service::OwnedService::from(crate::EmbeddedService::from_runtime(runtime));
    let completed = AtomicBool::new(false);
    let stop = AtomicBool::new(true);
    let result = finish_owned_run(
        service,
        Ok(()),
        Err(anyhow::anyhow!("hub stream cleanup failed")),
        &completed,
        &stop,
    )
    .await;
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("hub stream cleanup failed"));
    assert!(!completed.load(Ordering::Acquire));
    assert!(!path.exists());
}

#[tokio::test]
async fn cancelled_close_wait_retains_failure_through_final_finish() {
    use std::{future::Future, task::Poll};
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let service =
        crate::owned_service::OwnedService::from(crate::EmbeddedService::from_runtime(runtime));
    let mut hub = RuntimeHub::new(&service, Uuid::new_v4());
    let (release, gate) = tokio::sync::oneshot::channel::<()>();
    hub.dirty = false;
    hub.closing = Some(Box::pin(async move {
        gate.await.unwrap();
        Err(std::io::Error::other("gated close failed"))
    }));
    let mut wait = Box::pin(hub.next_update());
    std::future::poll_fn(|cx| {
        assert!(wait.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(wait);
    let mut finish = Box::pin(hub.finish());
    std::future::poll_fn(|cx| {
        assert!(finish.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    release.send(()).unwrap();
    assert!(finish
        .await
        .unwrap_err()
        .to_string()
        .contains("gated close failed"));
    service.close().await.unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn final_finish_revokes_retained_setup_before_joining_it() {
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let service =
        crate::owned_service::OwnedService::from(crate::EmbeddedService::from_runtime(runtime));
    let mut hub = RuntimeHub::new(&service, Uuid::new_v4());
    let context = splitty_transport::queue::Context::default();
    let check = context.clone();
    let mut closed = context.closing();
    hub.setup_context = Some(context);
    hub.setup = Some(Box::pin(async move {
        if !*closed.borrow() {
            closed.changed().await.unwrap();
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "setup revoked",
        ))
    }));
    hub.finish().await.unwrap();
    assert!(check.is_closed());
    service.close().await.unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn published_close_failure_remains_shutdown_failure() {
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let service =
        crate::owned_service::OwnedService::from(crate::EmbeddedService::from_runtime(runtime));
    let mut hub = RuntimeHub::new(&service, Uuid::new_v4());
    hub.dirty = false;
    hub.closing = Some(Box::pin(async {
        Err(std::io::Error::other("close failure"))
    }));
    let update = hub.next_update().await;
    assert!(matches!(update.state, HostDirectoryState::Failed { .. }));
    assert!(hub
        .finish()
        .await
        .unwrap_err()
        .to_string()
        .contains("close failure"));
    service.close().await.unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn runtime_shutdown_during_pending_directory_read_joins_stream_and_service() {
    use std::{future::Future, task::Poll};
    let (release, gate) = tokio::sync::oneshot::channel();
    let (observed, seen) = tokio::sync::oneshot::channel();
    let (api, peer, dir) = crate::hub::tests::fake_peer::scripted_with_gate_observed(
        vec![(0, String::new(), false)],
        Some(gate),
        Some(observed),
    )
    .await;
    let budget = api.sender().context().budget.clone();
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let service =
        crate::owned_service::OwnedService::from(crate::EmbeddedService::from_runtime(runtime));
    let owner = Uuid::new_v4();
    let mut hub = RuntimeHub::new(&service, owner);
    hub.reader = Some(HubClient::new(api, owner));
    hub.dirty = false;
    assert!(matches!(
        hub.next_update().await.state,
        HostDirectoryState::Loading { .. }
    ));
    let mut wait = Box::pin(hub.next_update());
    std::future::poll_fn(|cx| {
        assert!(wait.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(wait);
    seen.await.unwrap();
    let completed = AtomicBool::new(false);
    let stop = AtomicBool::new(true);
    let hub_cleanup = hub.finish().await.map_err(anyhow::Error::from);
    let result = finish_owned_run(service, Ok(()), hub_cleanup, &completed, &stop).await;
    assert!(result.is_ok());
    assert!(completed.load(Ordering::Acquire));
    assert_eq!(budget.used(), 0);
    release.send(()).unwrap();
    peer.await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn pending_failed_hub_read_does_not_block_settings_or_terminal_output() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    use std::{future::Future, task::Poll};
    let (release, gate) = tokio::sync::oneshot::channel();
    let (observed, seen) = tokio::sync::oneshot::channel();
    let (api, peer, dir) = crate::hub::tests::fake_peer::scripted_with_gate_observed(
        vec![(500, "{}".into(), false)],
        Some(gate),
        Some(observed),
    )
    .await;
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let service =
        crate::owned_service::OwnedService::from(crate::EmbeddedService::from_runtime(runtime));
    let mut main = service.connect().await.unwrap();
    let owner = main.connection_id();
    let mut settings = RuntimeLocalService::new(&service, owner, ColorScheme::Dark);
    let mut hub = RuntimeHub::new(&service, owner);
    hub.reader = Some(HubClient::new(api, owner));
    hub.dirty = false;
    assert!(matches!(
        hub.next_update().await.state,
        HostDirectoryState::Loading { .. }
    ));
    let mut pending = Box::pin(hub.next_update());
    std::future::poll_fn(|cx| {
        assert!(pending.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(pending);
    seen.await.unwrap();

    let build = tokio::time::timeout(Duration::from_secs(5), settings.next_update())
        .await
        .unwrap()
        .unwrap();
    let crate::local_service::LocalServiceUpdate::BackendVersion { connection, result } = build
    else {
        panic!("assigned backend read must progress independently of the hub");
    };
    assert_eq!(connection, owner);
    assert_eq!(result.unwrap().as_str(), "test");
    let setting = tokio::time::timeout(Duration::from_secs(5), settings.next_update())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        setting,
        crate::local_service::LocalServiceUpdate::Appearance(Ok(_))
            | crate::local_service::LocalServiceUpdate::ShellProfiles(Ok(_))
    ));
    main.create_screen("Local while Hub pending").unwrap();
    let session = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = main.next_event().await.unwrap().unwrap();
            if let Some(session) = event.json().and_then(|json| {
                (json["type"] == "mux.pane_spawned")
                    .then(|| json["sessionId"].as_str())
                    .flatten()
            }) {
                break session.to_owned();
            }
        }
    })
    .await
    .unwrap();
    main.attach(&session).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = main.next_event().await.unwrap().unwrap();
            if event
                .json()
                .is_some_and(|json| json["type"] == "attached" && json["sessionId"] == session)
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    main.input(&session, "printf 'HUB_%s\\n' 'PENDING_OK'\n")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut output = Vec::new();
        loop {
            let event = main.next_event().await.unwrap().unwrap();
            if let Some(chunk) = event.output() {
                if chunk.session_id == session {
                    output.extend_from_slice(chunk.bytes);
                }
            }
            if output
                .windows(b"HUB_PENDING_OK".len())
                .any(|part| part == b"HUB_PENDING_OK")
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    release.send(()).unwrap();
    let failed = hub.next_update().await;
    assert!(matches!(
        failed.state,
        HostDirectoryState::Failed { previous: None, .. }
    ));
    settings.finish().await.unwrap();
    hub.finish().await.unwrap();
    main.finish().await.unwrap();
    peer.await.unwrap();
    service.close().await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    assert!(!path.exists());
}

#[test]
fn entered_connections_require_live_identified_success_and_withdrawal_is_selective() {
    let owner = SourceWorkspace::new(Uuid::new_v4()).source_token();
    let foreign = SourceWorkspace::new(Uuid::new_v4()).source_token();
    let request = HostManagementRequest::connect("new.test", None).unwrap();
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let mut demand = HostConnects::default();
    demand.insert(first, owner);
    demand.insert(second, owner);
    demand.withdraw(first);
    let result = |id, token, outcome| HostManagementResult::new(token, id, &request, outcome, None);
    assert!(!demand.complete(&result(first, owner, HostManagementOutcome::Applied)));
    assert!(demand.complete(&result(second, owner, HostManagementOutcome::Applied)));
    assert!(!demand.complete(&result(second, owner, HostManagementOutcome::Applied)));
    for outcome in [
        HostManagementOutcome::Rejected,
        HostManagementOutcome::Uncertain,
    ] {
        demand.insert(first, owner);
        assert!(!demand.complete(&result(first, owner, outcome)));
    }
    demand.insert(first, owner);
    assert!(!demand.complete(&result(first, foreign, HostManagementOutcome::Applied)));
    demand.insert(first, owner);
    demand.insert(second, owner);
    demand.clear();
    assert!(!demand.complete(&result(first, owner, HostManagementOutcome::Applied)));
    assert!(!demand.complete(&result(second, owner, HostManagementOutcome::Applied)));
    assert!(local_command(&CommandKind::ManageHost {
        request_id: first,
        request: request.clone()
    }));
    assert!(local_command(&CommandKind::ReleaseHostBrowsing));
    let Update::HostManagement(rejected) = rejected_command(
        Some(foreign),
        &CommandKind::ManageHost {
            request_id: first,
            request,
        },
        "owner changed".into(),
    ) else {
        panic!("identified stale-owner result")
    };
    assert_eq!(rejected.owner(), foreign);
    assert_eq!(rejected.request_id(), first);
    assert_eq!(rejected.outcome(), HostManagementOutcome::Rejected);
}

#[tokio::test]
async fn worker_metadata_acknowledgements_survive_hub_changes_and_reconcile_alias_and_forget() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let state = runtime.state.clone();
    state
        .hub
        .update_host_preference(json!({"target":"managed.test","alias":"Original"}))
        .unwrap();
    let service =
        crate::owned_service::OwnedService::from(crate::EmbeddedService::from_runtime(runtime));
    let (commands, receiver) = mpsc::channel(64);
    let (sender, mut updates) = mpsc::channel(128);
    let stop = Arc::new(AtomicBool::new(false));
    let cleanup = Arc::new(AtomicBool::new(false));
    let worker = tokio::spawn(run_service(
        service,
        receiver,
        sender,
        stop.clone(),
        cleanup.clone(),
        ColorScheme::Dark,
    ));
    let initial = directory(&mut updates).await;
    let owner = SourceWorkspace::new(initial.connection_id).source_token();
    let mut previous = None;
    for request in [
        HostManagementRequest::set_alias("managed.test", "New name").unwrap(),
        HostManagementRequest::set_alias("managed.test", "").unwrap(),
        HostManagementRequest::forget("managed.test").unwrap(),
    ] {
        let id = Uuid::new_v4();
        commands
            .send(Command::new(
                owner,
                CommandKind::ManageHost {
                    request_id: id,
                    request: request.clone(),
                },
            ))
            .await
            .unwrap();
        let mut applied = false;
        let mut pending = false;
        let mut refreshed = false;
        while !applied || !refreshed {
            match next(&mut updates).await {
                Update::HostManagement(result) if result.request_id() == id => {
                    assert_eq!(result.owner(), owner);
                    assert_eq!(result.target(), "managed.test");
                    match result.outcome() {
                        HostManagementOutcome::Pending => pending = true,
                        HostManagementOutcome::Applied => {
                            assert!(pending);
                            applied = true;
                        }
                        other => panic!("unexpected outcome {other:?}: {:?}", result.reason()),
                    }
                }
                Update::HostDirectory(update) if applied => {
                    if let HostDirectoryState::Ready(snapshot) = update.state {
                        let row = snapshot
                            .directory()
                            .hosts
                            .iter()
                            .find(|host| host.target == "managed.test");
                        if request.action() == HostManagementAction::Forget || previous.is_some() {
                            assert!(row.is_none());
                        } else {
                            assert_eq!(row.unwrap().alias.as_deref(), Some("New name"));
                        }
                        refreshed = true;
                    }
                }
                Update::Workspace(window) => {
                    assert!(window.hosts().next().is_none(), "metadata never dials")
                }
                _ => {}
            }
        }
        previous = Some(id);
    }
    assert!(state
        .hub
        .host_preferences()
        .hosts
        .iter()
        .any(|host| host.target == "managed.test" && host.hidden));
    stop.store(true, Ordering::Release);
    tokio::time::timeout(Duration::from_secs(10), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(cleanup.load(Ordering::Acquire));
    let _ = std::fs::remove_dir_all(path);
}
