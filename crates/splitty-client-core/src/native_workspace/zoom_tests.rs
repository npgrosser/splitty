//! HomeUpdate persists through a process-global config path. Run this real
//! service test in a child with a private path, never changing the parent's env.
use super::*;
use crate::appearance::DrawerVisibilityOutcome;
use crate::zoom::{Zoom, ZoomOutcome};
use uuid::Uuid;

#[test]
fn persisted_zoom_keeps_input_alive_and_owned_shutdown_joins() {
    const CHILD: &str = "SPLITTY_ZOOM_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let dir = std::env::temp_dir().join(format!("splitty-zoom-{}", Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let result=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","native_workspace::zoom_tests::persisted_zoom_keeps_input_alive_and_owned_shutdown_joins","--nocapture"])
            .env(CHILD,"1").env("SPLITTY_CONFIG_DIR",&dir).output().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        assert!(
            result.status.success(),
            "child failed: {}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(real_service());
}
async fn next(updates: &mut mpsc::Receiver<Update>) -> Update {
    tokio::time::timeout(Duration::from_secs(15), updates.recv())
        .await
        .expect("zoom worker timeout")
        .expect("zoom worker alive")
}
async fn real_service() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let state = runtime.state.clone();
    state.config.write().unwrap().appearance.zoom_scope = "terminals".into();
    let config = state.config.read().unwrap().clone();
    splitty_core::config::save(&config).unwrap();
    let service =
        crate::owned_service::OwnedService::from(crate::EmbeddedService::from_runtime(runtime));
    let (commands, receiver) = mpsc::channel(64);
    let (sender, mut updates) = mpsc::channel(64);
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
    let mut ready = None;
    let mut appearance = false;
    let mut attached = None;
    while ready.is_none()
        || !appearance
        || attached.as_ref().is_none_or(|session: &String| {
            ready.as_deref().is_none_or(|model: &LocalWorkspace| {
                model
                    .routes()
                    .first()
                    .is_none_or(|route| &route.session_id != session)
            })
        })
    {
        match next(&mut updates).await {
            Update::Workspace(model) if model.routes().len() == 1 && model.snapshot().is_some() => {
                ready = Some(model)
            }
            Update::Appearance(_) => appearance = true,
            Update::Attached { session, .. } => attached = Some(session),
            _ => {}
        }
    }
    let model = ready.unwrap();
    let scope = model.source_token();
    let owner = model.connection_id();
    let route = model.routes()[0].clone();
    let foreign = Uuid::new_v4();
    let foreign_id = Uuid::new_v4();
    commands
        .send(Command::new(
            scope,
            CommandKind::Zoom {
                connection: foreign,
                request_id: foreign_id,
                action: ZoomAction::In,
            },
        ))
        .await
        .unwrap();
    let actions = [
        ZoomAction::In,
        ZoomAction::In,
        ZoomAction::Reset,
        ZoomAction::Out,
    ];
    let ids = actions.map(|_| Uuid::new_v4());
    for (request_id, action) in ids.into_iter().zip(actions) {
        commands
            .send(Command::new(
                scope,
                CommandKind::Zoom {
                    connection: owner,
                    request_id,
                    action,
                },
            ))
            .await
            .unwrap();
    }
    commands
        .send(Command::new(
            scope,
            CommandKind::Input {
                route: TerminalToken::new(scope, &route.pane_id, &route.session_id),
                bytes: b"printf 'ZOOM_%s\\n' 'ALIVE'\n".to_vec(),
            },
        ))
        .await
        .unwrap();
    let mut results = Vec::new();
    let mut output = Vec::new();
    let mut rejected = false;
    while results.len() < 4 || !rejected || !output.windows(10).any(|b| b == b"ZOOM_ALIVE") {
        match next(&mut updates).await {
            Update::Zoom(result) if result.request_id == foreign_id => {
                assert_eq!(result.connection_id, foreign);
                assert!(matches!(result.outcome, ZoomOutcome::Rejected { .. }));
                rejected = true;
            }
            Update::Zoom(result) => results.push(result),
            Update::Event { event, .. } => {
                if let Some(chunk) = event.output() {
                    if chunk.session_id == route.session_id {
                        output.extend_from_slice(chunk.bytes);
                    }
                }
            }
            Update::Workspace(current) => assert_eq!(
                current.session_for(&route.pane_id),
                Some(route.session_id.as_str())
            ),
            _ => {}
        }
    }
    for ((result, id), level) in results.into_iter().zip(ids).zip([1.1, 1.25, 1.0, 0.9]) {
        assert_eq!(result.connection_id, owner);
        assert_eq!(result.request_id, id);
        assert_eq!(
            result.outcome,
            ZoomOutcome::Confirmed(Zoom::from_values(level, "terminals"))
        );
    }
    // Drawer visibility shares the API scheduler but has its own identified
    // outcome. An invalid owner cannot change local presentation preferences.
    let foreign_drawer = Uuid::new_v4();
    let drawer_request = Uuid::new_v4();
    for (connection, request_id, pinned) in [
        (foreign, foreign_drawer, true),
        (owner, drawer_request, false),
    ] {
        commands
            .send(Command::new(
                scope,
                CommandKind::SetDrawerVisibility {
                    connection,
                    request_id,
                    pinned,
                },
            ))
            .await
            .unwrap();
    }
    let mut drawer_confirmed = false;
    let mut drawer_rejected = false;
    let mut drawer_refreshed = false;
    while !drawer_confirmed || !drawer_rejected || !drawer_refreshed {
        match next(&mut updates).await {
            Update::DrawerVisibility(result) if result.request_id == foreign_drawer => {
                assert_eq!(result.connection_id, foreign);
                assert!(matches!(
                    result.outcome,
                    DrawerVisibilityOutcome::Rejected { .. }
                ));
                drawer_rejected = true;
            }
            Update::DrawerVisibility(result) if result.request_id == drawer_request => {
                assert_eq!(result.connection_id, owner);
                assert_eq!(result.outcome, DrawerVisibilityOutcome::Confirmed(false));
                drawer_confirmed = true;
            }
            Update::Appearance(snapshot) if !snapshot.drawer().pinned => {
                assert_eq!(snapshot.connection_id(), owner);
                drawer_refreshed = true;
            }
            _ => {}
        }
    }
    let saved = splitty_core::config::load_from(&splitty_core::config::config_path());
    assert!(!saved.drawer.pinned);
    assert_eq!(saved.drawer.sidebar_width, config.drawer.sidebar_width);
    assert_eq!(saved.appearance.zoom, 0.9);
    assert_eq!(saved.appearance.zoom_scope, "terminals");
    assert_eq!(saved.appearance.font_family, config.appearance.font_family);
    assert_eq!(saved.shells, config.shells);
    // Close with admitted queued work. No detached settings task may retain the
    // service or mutate its state after joined cleanup reports completion.
    for _ in 0..20 {
        commands
            .send(Command::new(
                scope,
                CommandKind::Zoom {
                    connection: owner,
                    request_id: Uuid::new_v4(),
                    action: ZoomAction::Reset,
                },
            ))
            .await
            .unwrap();
    }
    stop.store(true, Ordering::Release);
    drop(commands);
    tokio::time::timeout(Duration::from_secs(15), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(cleanup.load(Ordering::Acquire));
    assert!(!path.exists());
    drop(updates);
    let persisted = splitty_core::config::load_from(&splitty_core::config::config_path());
    let restarted = crate::EmbeddedService::start();
    let mut settings = restarted
        .local_service_client(Uuid::new_v4(), ColorScheme::Dark)
        .await
        .unwrap();
    let snapshot = loop {
        if let crate::local_service::LocalServiceUpdate::Appearance(snapshot) =
            settings.next_update().await
        {
            break snapshot.unwrap();
        }
    };
    assert_eq!(snapshot.settings().zoom, persisted.appearance.zoom);
    assert!(!snapshot.drawer().pinned);
    drop(snapshot);
    settings.finish().await.unwrap();
    restarted.close().await.unwrap();
}

#[tokio::test]
async fn zoom_before_settings_setup_is_rejected_without_a_forwarding_queue() {
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let service =
        crate::owned_service::OwnedService::from(crate::EmbeddedService::from_runtime(runtime));
    let owner = Uuid::new_v4();
    let mut settings = RuntimeLocalService::new(&service, owner, ColorScheme::Dark);
    assert_eq!(
        settings
            .enqueue_drawer_visibility(owner, Uuid::new_v4(), false)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert_eq!(
        settings
            .enqueue_drawer_visibility(Uuid::new_v4(), Uuid::new_v4(), false)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidInput
    );
    assert_eq!(
        settings
            .enqueue_zoom(owner, Uuid::new_v4(), ZoomAction::In)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert_eq!(
        settings
            .enqueue_zoom(Uuid::new_v4(), Uuid::new_v4(), ZoomAction::In)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidInput
    );
    settings.finish().await.unwrap();
    service.close().await.unwrap();
    assert!(!path.exists());
}
