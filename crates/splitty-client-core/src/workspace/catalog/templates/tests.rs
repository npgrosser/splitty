use super::*;
use serde_json::json;
fn model() -> SourceWorkspace {
    let mut m = SourceWorkspace::new(Uuid::new_v4());
    m.set_catalog_supported(true);
    m.set_templates_supported(true);
    m.catalog_in_flight = Some(0);
    m.apply_catalog(
        m.connection_id,
        &json!({"type":"mux.workspace_snapshot","screens":[{"id":"old"}],"activeScreenId":"old"}),
    )
    .unwrap();
    m.apply_snapshot(m.connection_id,&json!({"type":"mux.screen_snapshot","screen":{"id":"old","revision":4,"root":{"type":"slot","id":"slot","paneIds":["p"]},"panes":{"p":{"id":"p","sessionId":"s"}},"sessions":{"s":{"id":"s","status":"running"}}}})).unwrap();
    m
}
fn plan(m: &SourceWorkspace, new: bool) -> Arc<TemplateApplyPlan> {
    Arc::new(
        m.prepare_template_apply(
            Uuid::new_v4(),
            crate::templates::tests::draft(m.connection_id),
            if new {
                TemplateApplyTarget::NewScreen
            } else {
                TemplateApplyTarget::Current {
                    screen_id: "old".into(),
                }
            },
            "/project".into(),
        )
        .unwrap(),
    )
}
#[test]
fn replacement_consent_capacity_queue_rejection_and_single_admission() {
    let mut m = model();
    let p = plan(&m, false);
    let routes = m.routes();
    assert!(m
        .apply_template_with(
            p.clone(),
            TemplateApplyChoice::LayoutOnly,
            false,
            |_, _| panic!("no consent")
        )
        .is_err());
    assert!(m
        .apply_template_with(
            p.clone(),
            TemplateApplyChoice::LayoutOnly,
            true,
            |_, _| Err(io::ErrorKind::WouldBlock.into())
        )
        .is_err());
    assert!(m.pending_template().is_none());
    assert_eq!(m.routes(), routes);
    m.apply_template_with(
        p.clone(),
        TemplateApplyChoice::LayoutOnly,
        true,
        |screen, r| {
            assert_eq!(screen, "old");
            assert!(!r.create_screen);
            assert!(r
                .template
                .panes
                .values()
                .all(|p| p.command.is_empty() && p.restore_mode.is_empty()));
            Ok(())
        },
    )
    .unwrap();
    assert!(m.pending_template().is_some());
    assert!(m.displayed_panes().is_empty());
    assert!(!m.is_displayed_pane("p"));
    assert_eq!(m.routes(), routes);
    assert!(m
        .apply_template_with(
            p.clone(),
            TemplateApplyChoice::RunCommands,
            true,
            |_, _| panic!("duplicate")
        )
        .is_err());
    m.finish_template(Some("uncertain".into()));
    assert!(m
        .apply_template_with(p, TemplateApplyChoice::RunCommands, true, |_, _| panic!(
            "replayed decision"
        ))
        .is_err());
    let mut m = model();
    let p = plan(&m, false);
    Arc::make_mut(m.models.get_mut("old").unwrap().snapshot.as_mut().unwrap())
        .sessions
        .get_mut(&SessionId::from("s"))
        .unwrap()
        .busy = true;
    assert!(m
        .apply_template_with(p, TemplateApplyChoice::RunCommands, true, |_, _| panic!(
            "stale risk"
        ))
        .is_err());
    m.set_templates_supported(false);
    assert!(m
        .prepare_template_apply(
            Uuid::new_v4(),
            crate::templates::tests::draft(m.connection_id),
            TemplateApplyTarget::NewScreen,
            String::new()
        )
        .is_err());
}
#[test]
fn pending_close_identity_navigation_and_authoritative_partial_outcome() {
    let mut m = model();
    let oldclose = m.assess_close(9, CloseTarget::Window).unwrap();
    let p = plan(&m, true);
    m.apply_template_with(
        p.clone(),
        TemplateApplyChoice::RunCommands,
        false,
        |_, _| Ok(()),
    )
    .unwrap();
    assert!(m.validate_close(&oldclose, true).is_err());
    assert!(
        m.assess_close(10, CloseTarget::Window)
            .unwrap()
            .unresolved_count
            > 0
    );
    let mutation = m.mutation;
    assert!(m.settle_template_after_refresh(mutation - 1).is_none());
    m.select_tab("p");
    assert!(m.pending_template().is_none());
    assert!(m.template_apply_pending());
    assert_eq!(
        m.screen_count(),
        2,
        "navigation cannot release a pending Screen reservation"
    );
    assert_eq!(
        m.admission_count(),
        3,
        "pending template panes remain reserved behind the shown Screen"
    );
    let (state, _) = mux::reduce_workspace(
        mux::WorkspaceState::default(),
        mux::Op::ApplyTemplate {
            screen_id: splitty_core::ids::ScreenId::from(p.screen_id()),
            request: Box::new(p.request.clone()),
        },
    )
    .unwrap();
    m.catalog_in_flight = Some(mutation);
    m.apply_catalog_inner(
        m.connection_id,
        &json!({"type":"mux.workspace_snapshot","screens":[{"id":"old"},{"id":p.screen_id()}]}),
    )
    .unwrap();
    let screen = state
        .screens
        .get(&splitty_core::ids::ScreenId::from(p.screen_id()))
        .unwrap()
        .clone();
    m.models
        .get_mut(p.screen_id())
        .unwrap()
        .apply_screen(Arc::new(screen))
        .unwrap();
    let successful = p.request.pane_ids["a"].as_str();
    let session = m.session_for(successful).unwrap().to_owned();
    m.observe_template_spawn(m.connection_id, successful, &session, &Value::Null);
    let failed = p.request.pane_ids["b"].to_string();
    m.observe_template_failure(m.connection_id,&json!({"type":"mux.op_failed","op":"apply_template","screenId":p.screen_id(),"paneId":failed,"error":"launch failed"}));
    let result = m.settle_template_after_refresh(mutation).unwrap();
    let TemplateApplyOutcome::Confirmed { launch_errors, .. } = result.outcome else {
        panic!("partial layout is committed")
    };
    assert_eq!(launch_errors.len(), 1);
    assert_eq!(m.shown_screen_id(), Some("old"));
    assert!(m.template_launch_error(&failed).is_some());
    assert_eq!(m.failed_template_sessions().count(), 1);
}
#[test]
fn nonmatching_state_is_uncertain_unless_rejection_and_original_identity_are_proven() {
    let mut m = model();
    let p = plan(&m, false);
    m.apply_template_with(p, TemplateApplyChoice::RunCommands, true, |_, _| Ok(()))
        .unwrap();
    assert!(matches!(
        m.finish_template(None).unwrap().outcome,
        TemplateApplyOutcome::Unverified { .. }
    ));
    let p = plan(&m, false);
    m.apply_template_with(p, TemplateApplyChoice::RunCommands, true, |_, _| Ok(()))
        .unwrap();
    m.observe_template_failure(
        m.connection_id,
        &json!({"type":"mux.op_failed","op":"apply_template","screenId":"old","error":"rejected"}),
    );
    assert!(matches!(
        m.finish_template(None).unwrap().outcome,
        TemplateApplyOutcome::Rejected { .. }
    ));
}

#[test]
fn template_capacity_credits_replaced_panes_but_not_unrelated_routes() {
    let mut m = model();
    let mut snapshot = serde_json::to_value(m.snapshot().unwrap()).unwrap();
    let mut panes = serde_json::Map::new();
    let mut sessions = serde_json::Map::new();
    let mut ids = Vec::new();
    for i in 0..MAX_ATTACHMENTS {
        let pane = format!("p-{i}");
        let session = format!("s-{i}");
        ids.push(pane.clone());
        panes.insert(pane.clone(), json!({"id":pane,"sessionId":session}));
        sessions.insert(session.clone(), json!({"id":session,"status":"running"}));
    }
    snapshot["panes"] = Value::Object(panes);
    snapshot["sessions"] = Value::Object(sessions);
    snapshot["root"]["paneIds"] = json!(ids);
    m.apply_snapshot(
        m.connection_id,
        &json!({"type":"mux.screen_snapshot","screen":snapshot}),
    )
    .unwrap();
    let draft = crate::templates::tests::draft(m.connection_id);
    assert!(m
        .prepare_template_apply(
            Uuid::new_v4(),
            draft.clone(),
            TemplateApplyTarget::NewScreen,
            String::new()
        )
        .is_err());
    assert!(m
        .prepare_template_apply(
            Uuid::new_v4(),
            draft,
            TemplateApplyTarget::Current {
                screen_id: "old".into()
            },
            String::new()
        )
        .is_ok());
}
