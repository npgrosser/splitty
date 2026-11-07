//! Real-service profile launches retain captured terminal identity and service defaults.
use super::*;
use crate::workspace::SourceWorkspace;
use serde_json::json;
use splitty_core::{
    ids::{ScreenId, SessionId},
    mux::Op,
    pty::Backend as _,
};

async fn next(updates: &mut mpsc::Receiver<Update>) -> Update {
    tokio::time::timeout(Duration::from_secs(10), updates.recv())
        .await
        .expect("profile launch update timeout")
        .expect("profile launch update channel")
}

async fn initial(
    updates: &mut mpsc::Receiver<Update>,
) -> (Box<SourceWorkspace>, Arc<ShellProfilesSnapshot>) {
    let (mut model, mut profiles) = (None, None);
    loop {
        match next(updates).await {
            Update::Workspace(workspace)
                if workspace.local_source().catalog_initialized()
                    && !workspace.local_source().has_pending_create()
                    && workspace.local_source().snapshot().is_some()
                    && workspace.local_source().routes().len() == 1 =>
            {
                model = Some(local_projection(&workspace));
            }
            Update::ShellProfiles(snapshot) => profiles = Some(snapshot),
            Update::ShellProfilesError { reason, .. } => panic!("profile read failed: {reason}"),
            _ => {}
        }
        if let Some(workspace) = model.take() {
            if let Some(snapshot) = profiles.take() {
                return (workspace, snapshot);
            }
            model = Some(workspace);
        }
    }
}

async fn routes(updates: &mut mpsc::Receiver<Update>, count: usize) -> Box<SourceWorkspace> {
    loop {
        if let Update::Workspace(model) = next(updates).await {
            let model = local_projection(&model);
            if model.routes().len() == count {
                return model;
            }
        }
    }
}

async fn rejected(updates: &mut mpsc::Receiver<Update>) {
    loop {
        if let Update::Status(status) = next(updates).await {
            if status.contains("Shell profile choice belongs to a previous terminal") {
                return;
            }
        }
    }
}

async fn output_contains(updates: &mut mpsc::Receiver<Update>, session: &str, marker: &[u8]) {
    let mut output = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Update::Event { event, .. } = next(updates).await {
                if let Some(chunk) = event.output() {
                    if chunk.session_id == session {
                        output.extend_from_slice(chunk.bytes);
                        if output.windows(marker.len()).any(|bytes| bytes == marker) {
                            return;
                        }
                    }
                }
            }
        }
    })
    .await
    .expect("profile terminal output");
}

#[tokio::test]
async fn captured_profile_launch_and_stale_choices_use_owned_service() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let state = runtime.state.clone();
    let pty = runtime.pty.clone();
    *state.shell_profiles.write().unwrap() = serde_json::from_value(json!({
        "defaultProfileId":"base",
        "profiles":[
            {"id":"base","name":"Base","command":"/bin/sh","args":[],
             "enabled":true,"available":true,"target":"local","source":"custom"},
            {"id":"chosen","name":"Chosen","command":"/bin/sh","args":[],
             "enabled":true,"available":true,"target":"local","source":"custom"}
        ]
    }))
    .unwrap();
    let cwd = path.join("cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let cwd = cwd.to_string_lossy().into_owned();
    let service = crate::EmbeddedService::from_runtime(runtime);
    let (commands, receiver) = mpsc::channel(32);
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

    let (model, profiles) = initial(&mut updates).await;
    let scope = model.source_token();
    let connection = model.connection_id();
    assert_eq!(profiles.connection_id(), connection);
    assert!(profiles
        .state()
        .profiles
        .iter()
        .any(|profile| profile.id == "chosen" && profile.available));
    let screen = model.shown_screen_id().unwrap().to_owned();
    let first = model.routes().pop().unwrap();
    assert_eq!(
        pty.get(&first.session_id).unwrap().shell_profile_id(),
        Some("base")
    );
    state
        .mux_store
        .dispatch(Op::UpdateSessionMetadata {
            session_id: SessionId::from(first.session_id.as_str()),
            display_title: "base shell".into(),
            cwd: cwd.clone(),
            busy: false,
            exited: false,
            exit_code: None,
            close: false,
        })
        .unwrap();

    let choice = |connection, screen: String, pane: String, session: String| {
        Command::new(
            scope,
            CommandKind::ProfileTab {
                connection,
                screen,
                pane,
                session,
                profile_id: "chosen".into(),
            },
        )
    };
    for invalid in [
        choice(
            uuid::Uuid::new_v4(),
            screen.clone(),
            first.pane_id.clone(),
            first.session_id.clone(),
        ),
        choice(
            connection,
            "wrong-screen".into(),
            first.pane_id.clone(),
            first.session_id.clone(),
        ),
        choice(
            connection,
            screen.clone(),
            first.pane_id.clone(),
            "replaced-session".into(),
        ),
        choice(
            connection,
            screen.clone(),
            "different-pane".into(),
            first.session_id.clone(),
        ),
    ] {
        commands.send(invalid).await.unwrap();
        rejected(&mut updates).await;
        assert_eq!(
            state
                .mux_store
                .get_screen(&ScreenId::from(screen.as_str()))
                .unwrap()
                .panes
                .len(),
            1,
            "rejected captured choice must not launch a PTY"
        );
    }

    commands
        .send(choice(
            connection,
            screen.clone(),
            first.pane_id.clone(),
            first.session_id.clone(),
        ))
        .await
        .unwrap();
    let model = routes(&mut updates, 2).await;
    let chosen = model
        .routes()
        .into_iter()
        .find(|route| route.pane_id != first.pane_id)
        .unwrap();
    assert_eq!(
        pty.get(&chosen.session_id).unwrap().shell_profile_id(),
        Some("chosen")
    );
    let service_screen = state
        .mux_store
        .get_screen(&ScreenId::from(screen.as_str()))
        .unwrap();
    assert_eq!(
        service_screen.sessions[&SessionId::from(chosen.session_id.as_str())]
            .cwd
            .as_deref(),
        Some(cwd.as_str()),
        "service-owned CWD inheritance survives explicit profile selection"
    );
    let wrong_route = json!({"shellProfileFallbackReason":"disabled", "shellProfileId":"base"});
    assert!(profile_fallback_status(
        &model,
        uuid::Uuid::new_v4(),
        &chosen.pane_id,
        &chosen.session_id,
        &wrong_route
    )
    .is_none());
    assert!(profile_fallback_status(
        &model,
        connection,
        &chosen.pane_id,
        &first.session_id,
        &wrong_route
    )
    .is_none());
    assert!(profile_fallback_status(
        &model,
        connection,
        &chosen.pane_id,
        &chosen.session_id,
        &json!({"shellProfileFallbackReason":"disabled", "shellProfileId":"x".repeat(257)})
    )
    .is_none());

    // The original tab now has a valid route but is hidden by the selected tab.
    commands
        .send(choice(
            connection,
            screen.clone(),
            first.pane_id.clone(),
            first.session_id.clone(),
        ))
        .await
        .unwrap();
    rejected(&mut updates).await;
    assert_eq!(
        state
            .mux_store
            .get_screen(&ScreenId::from(screen.as_str()))
            .unwrap()
            .panes
            .len(),
        2
    );

    // The user still sees a cached launchable profile while the service has
    // disabled it. Service policy falls back to base and reports that choice.
    state
        .shell_profiles
        .write()
        .unwrap()
        .profiles
        .iter_mut()
        .find(|profile| profile.id == "chosen")
        .unwrap()
        .enabled = false;
    commands
        .send(choice(
            connection,
            screen.clone(),
            chosen.pane_id.clone(),
            chosen.session_id.clone(),
        ))
        .await
        .unwrap();
    let mut fallback = false;
    let mut final_model = None;
    let mut attached = HashSet::new();
    while !fallback || final_model.is_none() {
        match next(&mut updates).await {
            Update::Status(status)
                if status.contains("Shell profile disabled; started base instead") =>
            {
                fallback = true
            }
            Update::Workspace(model) if model.local_source().routes().len() == 3 => {
                final_model = Some(local_projection(&model))
            }
            Update::Attached { session, .. } => {
                attached.insert(session);
            }
            _ => {}
        }
    }
    let final_model = final_model.unwrap();
    let fallback_route = final_model
        .routes()
        .into_iter()
        .find(|route| route.pane_id != first.pane_id && route.pane_id != chosen.pane_id)
        .unwrap();
    assert_eq!(
        pty.get(&fallback_route.session_id)
            .unwrap()
            .shell_profile_id(),
        Some("base")
    );
    assert_eq!(
        state
            .mux_store
            .get_screen(&ScreenId::from(screen.as_str()))
            .unwrap()
            .panes
            .len(),
        3
    );

    // A live profile-picked terminal still accepts bounded input through the
    // same route after its spawn and attachment notifications settle.
    while !attached.contains(&fallback_route.session_id) {
        if let Update::Attached { session, .. } = next(&mut updates).await {
            attached.insert(session);
        }
    }
    commands
        .send(Command::new(
            scope,
            CommandKind::Input {
                route: TerminalToken::new(
                    scope,
                    &fallback_route.pane_id,
                    &fallback_route.session_id,
                ),
                bytes: b"printf 'PROFILE_ROUTE_READY\\n'\n".to_vec(),
            },
        ))
        .await
        .unwrap();
    output_contains(
        &mut updates,
        &fallback_route.session_id,
        b"PROFILE_ROUTE_READY",
    )
    .await;

    stop.store(true, Ordering::Release);
    worker.await.unwrap().unwrap();
    assert!(cleanup.load(Ordering::Acquire));
    for session in [
        &first.session_id,
        &chosen.session_id,
        &fallback_route.session_id,
    ] {
        assert!(pty.get(session).is_none(), "owned session must be closed");
    }
    std::fs::remove_dir_all(path).unwrap();
}
