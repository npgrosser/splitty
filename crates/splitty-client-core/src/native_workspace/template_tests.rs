//! Template effects, partial readiness and owned shutdown through the actual service.
use super::*;
use crate::templates::{TemplateApiOutcome, TemplateDraft};
use crate::workspace::{CloseTarget, TemplateApplyTarget};
use serde_json::json;
use splitty_core::pty::Backend as _;
use splitty_core::template::TemplateStore as _;
use uuid::Uuid;
async fn next(rx: &mut mpsc::Receiver<Update>) -> Update {
    tokio::time::timeout(Duration::from_secs(15), rx.recv())
        .await
        .expect("template worker timeout")
        .expect("worker alive")
}
async fn settled(
    rx: &mut mpsc::Receiver<Update>,
    id: Uuid,
) -> (
    Box<LocalWorkspace>,
    Vec<crate::workspace::TemplateLaunchError>,
) {
    let mut latest = None;
    let mut preview = false;
    loop {
        match next(rx).await {
            Update::Workspace(model) => {
                if model.pending_template().is_some() {
                    preview = true;
                    assert!(model.displayed_panes().is_empty());
                }
                latest = Some(model);
            }
            Update::TemplateApply(result) if result.request_id == id => {
                assert!(preview, "immediate pending projection");
                let TemplateApplyOutcome::Confirmed {
                    launch_errors,
                    profile_fallbacks,
                } = result.outcome
                else {
                    panic!("apply failed: {:?}", result.outcome)
                };
                assert_eq!(profile_fallbacks.len(), 1);
                assert_eq!(profile_fallbacks[0].requested_profile_id, "missing");
                assert_eq!(profile_fallbacks[0].effective_profile_id, "base");
                assert_eq!(profile_fallbacks[0].reason, "missing");
                let current = latest.expect("workspace before outcome");
                assert!(current
                    .template_launch_error(&profile_fallbacks[0].pane_id)
                    .is_none());
                return (current, launch_errors);
            }
            _ => {}
        }
    }
}
async fn ready(rx: &mut mpsc::Receiver<Update>, pane: &str) -> Box<LocalWorkspace> {
    loop {
        if let Update::Workspace(m) = next(rx).await {
            if m.session_for(pane).is_some_and(|s| m.session_ready(s)) {
                return m;
            }
        }
    }
}
async fn output(rx: &mut mpsc::Receiver<Update>, session: &str, marker: &[u8]) {
    let mut bytes = Vec::new();
    loop {
        if let Update::Event { event, .. } = next(rx).await {
            if let Some(chunk) = event.output() {
                if chunk.session_id == session {
                    bytes.extend_from_slice(chunk.bytes);
                    if bytes.windows(marker.len()).any(|w| w == marker) {
                        return;
                    }
                }
            }
        }
    }
}
#[tokio::test]
async fn templates_new_replace_partial_launch_keep_other_pty_and_join_cleanup() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let state = runtime.state.clone();
    let pty = runtime.pty.clone();
    *state.shell_profiles.write().unwrap()=serde_json::from_value(json!({"defaultProfileId":"base","profiles":[{"id":"base","name":"Base","command":"/bin/sh","args":[],"enabled":true,"available":true,"target":"local","source":"custom"},{"id":"broken","name":"Broken","command":"/no/such/splitty-test-shell","args":[],"enabled":true,"available":true,"target":"local","source":"custom"}]})).unwrap();
    let template=serde_json::from_value(json!({"name":"Partial","panes":{"a":{"shell_profile":"missing","command":"printf 'TPL_EXECUTED\\n'","restore_mode":"replay"},"b":{"shell_profile":"broken"}},"layout":{"slot":"a"},"floating":[{"layout":{"slot":"b"},"bounds":{"x":0.1,"y":0.1,"w":0.5,"h":0.5}}]})).unwrap();
    state.template_store.save("partial", &template).unwrap();
    let service = crate::EmbeddedService::from_runtime(runtime);
    let (tx, rx) = mpsc::channel(64);
    let (out, mut updates) = mpsc::channel(64);
    let cleanup = Arc::new(AtomicBool::new(false));
    let worker = tokio::spawn(run_service(
        service,
        rx,
        out,
        Arc::new(AtomicBool::new(false)),
        cleanup.clone(),
        ColorScheme::Dark,
    ));
    let (mut model, mut api_ready) = (None, false);
    loop {
        match next(&mut updates).await {
            Update::Workspace(m)
                if m.routes().len() == 1
                    && m.routes().iter().all(|r| m.session_ready(&r.session_id)) =>
            {
                model = Some(m)
            }
            Update::ShellProfiles(_) => api_ready = true,
            _ => {}
        }
        if model.is_some() && api_ready {
            break;
        }
    }
    let initial = model.unwrap();
    let scope = initial.source_token();
    let owner = initial.connection_id();
    let unrelated = initial.routes()[0].clone();
    let process = pty.get(&unrelated.session_id).unwrap();
    let load = Uuid::new_v4();
    tx.send(Command::new(
        scope,
        CommandKind::TemplateApi {
            connection: owner,
            request_id: load,
            request: TemplateApiRequest::Load {
                slug: "partial".into(),
            },
        },
    ))
    .await
    .unwrap();
    let draft: Arc<TemplateDraft> = loop {
        if let Update::TemplateApi(result) = next(&mut updates).await {
            if result.request_id == load {
                let TemplateApiOutcome::Draft(draft) = result.outcome else {
                    panic!("draft load")
                };
                break draft;
            }
        }
    };
    let id = Uuid::new_v4();
    let plan = Arc::new(
        initial
            .prepare_template_apply(
                id,
                draft.clone(),
                TemplateApplyTarget::NewScreen,
                String::new(),
            )
            .unwrap(),
    );
    let target = plan.screen_id().to_owned();
    let successful = plan
        .projection()
        .pane_ids()
        .find(|id| plan.pane_name(id.as_str()) == Some("a"))
        .unwrap()
        .to_string();
    tx.send(Command::new(
        scope,
        CommandKind::ApplyTemplate {
            plan,
            choice: TemplateApplyChoice::RunCommands,
            replace_confirmed: false,
        },
    ))
    .await
    .unwrap();
    let (mut applied, failures) = settled(&mut updates, id).await;
    assert_eq!(failures.len(), 1);
    let failed = failures[0].pane_id.as_ref().unwrap();
    assert!(applied.template_launch_error(failed).is_some());
    assert_eq!(applied.screens().len(), 2);
    assert!(Arc::ptr_eq(
        &process,
        &pty.get(&unrelated.session_id).unwrap()
    ));
    let session = applied.session_for(&successful).unwrap().to_owned();
    if !applied.session_ready(&session) {
        applied = ready(&mut updates, &successful).await;
    }
    output(&mut updates, &session, b"TPL_EXECUTED").await;
    // Failed Starting entries remain visible errors and are not attached in a loop.
    assert!(!applied.session_ready(applied.session_for(failed).unwrap()));
    assert_eq!(pty.sessions().len(), 2);
    let id = Uuid::new_v4();
    let plan = Arc::new(
        applied
            .prepare_template_apply(
                id,
                draft,
                TemplateApplyTarget::Current { screen_id: target },
                String::new(),
            )
            .unwrap(),
    );
    let new_pane = plan
        .projection()
        .pane_ids()
        .find(|id| plan.pane_name(id.as_str()) == Some("a"))
        .unwrap()
        .to_string();
    tx.send(Command::new(
        scope,
        CommandKind::ApplyTemplate {
            plan,
            choice: TemplateApplyChoice::LayoutOnly,
            replace_confirmed: true,
        },
    ))
    .await
    .unwrap();
    let (mut replaced, failures) = settled(&mut updates, id).await;
    assert_eq!(failures.len(), 1);
    assert!(pty.get(&session).is_none());
    assert!(Arc::ptr_eq(
        &process,
        &pty.get(&unrelated.session_id).unwrap()
    ));
    let new_session = replaced.session_for(&new_pane).unwrap().to_owned();
    if !replaced.session_ready(&new_session) {
        replaced = ready(&mut updates, &new_pane).await;
    }
    tx.send(Command::new(
        scope,
        CommandKind::Input {
            route: TerminalToken::new(scope, &new_pane, &new_session),
            bytes: b"printf 'TPL_ALIVE\\n'\n".to_vec(),
        },
    ))
    .await
    .unwrap();
    output(&mut updates, &new_session, b"TPL_ALIVE").await;
    let snapshot = state
        .mux_store
        .get_screen(&splitty_core::ids::ScreenId::from(
            replaced.shown_screen_id().unwrap(),
        ))
        .unwrap();
    assert!(snapshot
        .sessions
        .values()
        .all(|s| s.command.is_none() && s.restore_mode.is_none()));
    tx.send(Command {
        source: Some(scope),
        kind: CommandKind::RequestClose {
            plan: replaced.assess_close(100, CloseTarget::Window).unwrap(),
            confirmed: true,
        },
    })
    .await
    .unwrap();
    loop {
        if matches!(
            next(&mut updates).await,
            Update::WindowCloseApproved {
                request_id: 100,
                ..
            }
        ) {
            break;
        }
    }
    worker.await.unwrap().unwrap();
    assert!(cleanup.load(Ordering::Acquire));
    assert!(pty.sessions().is_empty());
    let _ = std::fs::remove_dir_all(path);
}
