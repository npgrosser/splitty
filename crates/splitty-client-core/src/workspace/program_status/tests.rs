use super::*;
use crate::workspace::SourceKind;
use serde_json::json;
use uuid::Uuid;

fn changed(session: &str, pane: Option<&str>, screen: Option<&str>, status: Value) -> Value {
    json!({
        "type":"program_status.changed","id":format!("event-{session}"),"timestamp":1,
        "sessionId":session,"paneId":pane,"screenId":screen,"status":status
    })
}
fn record(state: &str, updated_at: u64) -> Value {
    json!({"state":state,"updatedAt":updated_at})
}
fn observe(
    window: &mut WindowWorkspace,
    id: SourceId,
    event: &Value,
    budget: &ConnectionBudget,
) -> io::Result<bool> {
    let mut source = window.source_mut(id).unwrap();
    let owner = source.source_token();
    source.observe_program_status(owner, event, budget)
}
fn supported(window: &mut WindowWorkspace, id: SourceId) {
    window
        .source_mut(id)
        .unwrap()
        .set_program_status_supported(true);
}
fn snapshot(window: &mut WindowWorkspace, id: SourceId, screen: &str, panes: &[(&str, &str)]) {
    let mut source = window.source_mut(id).unwrap();
    let connection = source.connection_id();
    source
        .apply_snapshot(
            connection,
            &json!({"type":"mux.screen_snapshot","screen":{
                "id":screen,"revision":3,
                "root":{"type":"slot","id":format!("slot-{screen}"),
                    "paneIds":panes.iter().map(|(pane, _)| *pane).collect::<Vec<_>>(),
                    "activePaneId":panes.first().map(|(pane, _)| *pane)},
                "panes":panes.iter().map(|(pane, session)|
                    ((*pane).to_owned(), json!({"id":pane,"sessionId":session}))).collect::<serde_json::Map<_,_>>(),
                "sessions":panes.iter().map(|(_, session)|
                    ((*session).to_owned(), json!({"id":session,"status":"running"}))).collect::<serde_json::Map<_,_>>()
            }}),
        )
        .unwrap();
}
fn local(window: &WindowWorkspace) -> SourceId {
    window.local_source().source_id()
}

#[test]
fn records_need_the_capability_and_the_current_connection() {
    let budget = ConnectionBudget::default();
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    window.ui_test_local_screen_fixture("Work").unwrap();
    let id = local(&window);
    snapshot(&mut window, id, "work", &[("p0", "s0")]);
    let blocked = changed("s0", Some("p0"), Some("work"), record("blocked", 1));
    // An older server's welcome lists no capability: its events change nothing.
    assert!(!observe(&mut window, id, &blocked, &budget).unwrap());
    assert!(window.local_source().pane_program_status("p0").is_none());
    assert_eq!(budget.used(), 0);

    supported(&mut window, id);
    assert!(window.local_source().program_status_supported());
    let foreign = SourceWorkspace::new(Uuid::new_v4()).source_token();
    assert!(!window
        .source_mut(id)
        .unwrap()
        .observe_program_status(foreign, &blocked, &budget)
        .unwrap());
    assert!(observe(&mut window, id, &blocked, &budget).unwrap());
    let status = window.local_source().pane_program_status("p0").unwrap();
    assert_eq!(status.state(), ProgramState::Blocked);
    assert!(budget.used() > 0);
    assert_eq!(
        window.local_source().blocked_screens(),
        HashSet::from(["work"])
    );

    // A replaced connection starts without records and without the capability.
    window
        .source_mut(id)
        .unwrap()
        .reset_connection(Uuid::new_v4());
    assert!(!window.local_source().program_status_supported());
    assert!(window.local_source().session_program_status("s0").is_none());
    assert_eq!(budget.used(), 0);
}

#[test]
fn the_same_session_id_on_two_connections_marks_only_each_sources_pane() {
    let budget = ConnectionBudget::default();
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    window.ui_test_local_screen_fixture("Work").unwrap();
    let local_id = local(&window);
    snapshot(&mut window, local_id, "work", &[("p0", "shared")]);
    let (_, route) = window
        .ui_test_remote_terminal_fixture("me@192.0.2.1")
        .unwrap();
    let remote_id = route.source_token().source_id();
    snapshot(&mut window, remote_id, "work", &[("p0", "shared")]);
    supported(&mut window, local_id);
    supported(&mut window, remote_id);
    assert!(observe(
        &mut window,
        local_id,
        &changed("shared", Some("p0"), Some("work"), record("blocked", 1)),
        &budget
    )
    .unwrap());
    assert!(observe(
        &mut window,
        remote_id,
        &changed("shared", Some("p0"), Some("work"), record("done", 2)),
        &budget
    )
    .unwrap());
    let local_source = window.local_source();
    let remote_source = window.source(remote_id).unwrap();
    assert_eq!(remote_source.kind(), SourceKind::Remote);
    assert_eq!(
        local_source.pane_program_status("p0").unwrap().state(),
        ProgramState::Blocked
    );
    assert_eq!(
        remote_source.pane_program_status("p0").unwrap().state(),
        ProgramState::Done
    );
    assert_eq!(local_source.blocked_screens(), HashSet::from(["work"]));
    assert!(remote_source.blocked_screens().is_empty());
    // A cleared record goes from its own source only.
    assert!(observe(
        &mut window,
        local_id,
        &changed("shared", None, None, Value::Null),
        &budget
    )
    .unwrap());
    assert!(!observe(
        &mut window,
        local_id,
        &changed("shared", None, None, Value::Null),
        &budget
    )
    .unwrap());
    assert!(window.local_source().pane_program_status("p0").is_none());
    assert!(window
        .source(remote_id)
        .unwrap()
        .pane_program_status("p0")
        .is_some());
}

#[test]
fn a_host_screen_not_opened_is_marked_through_the_events_screen_only_when_listed() {
    let budget = ConnectionBudget::default();
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    window.ui_test_local_screen_fixture("Work").unwrap();
    let (_, route) = window
        .ui_test_remote_terminal_fixture("me@192.0.2.1")
        .unwrap();
    let id = route.source_token().source_id();
    window
        .ui_test_remote_catalog(id, &["work", "other"])
        .unwrap();
    assert!(window
        .opened()
        .iter()
        .all(|entry| entry.key().screen_id() != "other"));
    supported(&mut window, id);
    for (session, screen) in [("s-other", "other"), ("s-gone", "gone")] {
        assert!(observe(
            &mut window,
            id,
            &changed(
                session,
                Some(&format!("pane-{session}")),
                Some(screen),
                json!({"state":"blocked","kind":"question","updatedAt":5})
            ),
            &budget
        )
        .unwrap());
    }
    let source = window.source(id).unwrap();
    assert_eq!(source.blocked_screens(), HashSet::from(["other"]));
    assert_eq!(source.program_status_screen("s-other"), Some("other"));
    assert_eq!(source.program_status_pane("s-other"), Some("pane-s-other"));
    assert_eq!(source.program_status_screen("s-gone"), None);
    // A pane the window has bound marks its own Screen, whatever the event names.
    assert!(observe(
        &mut window,
        id,
        &changed(
            route.session_id(),
            Some("p0"),
            Some("other"),
            record("blocked", 6)
        ),
        &budget
    )
    .unwrap());
    let source = window.source(id).unwrap();
    assert_eq!(source.blocked_screens(), HashSet::from(["other", "work"]));
    assert_eq!(
        source.program_status_screen(route.session_id()),
        Some("work")
    );
    // Other states mark no Screen.
    assert!(observe(
        &mut window,
        id,
        &changed("s-other", None, Some("other"), record("working", 7)),
        &budget
    )
    .unwrap());
    assert_eq!(
        window.source(id).unwrap().blocked_screens(),
        HashSet::from(["work"])
    );
}

#[test]
fn a_seen_done_is_not_marked_again_after_a_reconnect_but_a_newer_one_is() {
    let budget = ConnectionBudget::default();
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    window.ui_test_local_screen_fixture("Work").unwrap();
    let id = local(&window);
    snapshot(&mut window, id, "work", &[("p0", "s0"), ("p1", "s1")]);
    supported(&mut window, id);
    let mut seen = ProgramStatusSeen::default();
    for (session, state) in [("s0", "done"), ("s1", "working")] {
        assert!(observe(
            &mut window,
            id,
            &changed(session, None, None, record(state, 10)),
            &budget
        )
        .unwrap());
    }
    assert!(seen.shown(window.local_source(), "p0").is_some());
    assert!(
        !seen.see(window.local_source(), "p1"),
        "only done and error"
    );
    assert!(seen.shown(window.local_source(), "p1").is_some());
    assert!(seen.see(window.local_source(), "p0"));
    assert!(!seen.see(window.local_source(), "p0"));
    assert!(seen.shown(window.local_source(), "p0").is_none());

    // The connection is replaced and greets with the same record.
    window
        .source_mut(id)
        .unwrap()
        .reset_connection(Uuid::new_v4());
    seen.reconcile(&window);
    window.ui_test_local_screen_fixture("Work").unwrap();
    snapshot(&mut window, id, "work", &[("p0", "s0")]);
    supported(&mut window, id);
    assert!(observe(
        &mut window,
        id,
        &changed("s0", None, None, record("done", 10)),
        &budget
    )
    .unwrap());
    assert!(seen.shown(window.local_source(), "p0").is_none());
    for (state, updated_at) in [("error", 11), ("done", 12)] {
        assert!(observe(
            &mut window,
            id,
            &changed("s0", None, None, record(state, updated_at)),
            &budget
        )
        .unwrap());
        assert_eq!(
            seen.shown(window.local_source(), "p0")
                .unwrap()
                .updated_at(),
            updated_at,
            "a newer record is marked"
        );
    }
    // A window without the source forgets what it saw there.
    seen.reconcile(&WindowWorkspace::new(Uuid::new_v4()));
    assert!(seen.seen.is_empty());
}

#[test]
fn records_are_bounded_charged_and_validated_before_they_replace_anything() {
    let budget = ConnectionBudget::default();
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    window.ui_test_local_screen_fixture("Work").unwrap();
    let id = local(&window);
    supported(&mut window, id);
    for index in 0..MAX_STATUS_SESSIONS {
        assert!(observe(
            &mut window,
            id,
            &changed(&format!("s{index}"), None, None, record("working", 1)),
            &budget
        )
        .unwrap());
    }
    let full = budget.used();
    let extra = changed("extra", None, None, record("blocked", 2));
    assert_eq!(
        observe(&mut window, id, &extra, &budget)
            .unwrap_err()
            .kind(),
        io::ErrorKind::OutOfMemory
    );
    assert_eq!(budget.used(), full);
    // A known session is replaced; a cleared one makes room.
    assert!(observe(
        &mut window,
        id,
        &changed("s0", None, None, record("done", 3)),
        &budget
    )
    .unwrap());
    assert!(observe(
        &mut window,
        id,
        &changed("s1", None, None, Value::Null),
        &budget
    )
    .unwrap());
    assert!(observe(&mut window, id, &extra, &budget).unwrap());

    let before = window
        .local_source()
        .session_program_status("s0")
        .cloned()
        .unwrap();
    let mut long_title = record("blocked", 4);
    long_title["title"] = json!("t".repeat(MAX_TITLE_BYTES + 1));
    let mut long_message = record("blocked", 4);
    long_message["msg"] = json!("m".repeat(MAX_MESSAGE_BYTES + 1));
    for invalid in [
        changed("s0", None, None, record("thinking", 4)),
        changed("s0", None, None, json!({"state":"done"})),
        changed("s0", None, None, long_title),
        changed("s0", None, None, long_message),
        changed("s0", Some(""), None, record("done", 4)),
        changed("s0", None, Some("a\nb"), record("done", 4)),
        json!({"type":"program_status.changed","sessionId":"s0"}),
        json!({"type":"program_status.changed","status":record("done", 4)}),
    ] {
        assert!(observe(&mut window, id, &invalid, &budget).is_err());
        assert_eq!(
            window.local_source().session_program_status("s0"),
            Some(&before)
        );
    }
    window
        .source_mut(id)
        .unwrap()
        .reset_connection(Uuid::new_v4());
    assert_eq!(budget.used(), 0);
}

#[test]
fn values_outside_their_state_count_as_absent_and_texts_stay_plain() {
    let status = ProgramStatus::parse(&json!({
        "state":"working","kind":"permission","progress":40,"app":"claude-code",
        "title":"Build","msg":"ok\u{202e}txt\u{1b}[31m\u{200b}.exe","updatedAt":7
    }))
    .unwrap();
    assert_eq!(status.kind(), None);
    assert_eq!(status.progress(), Some(40));
    assert_eq!(status.app(), Some("claude-code"));
    assert_eq!(status.message(), Some("oktxt[31m.exe"));
    assert_eq!(status.label(), "Working, 40%");
    assert_eq!(status.summary(), "Working, 40%: oktxt[31m.exe");
    let blocked = ProgramStatus::parse(&json!({
        "state":"blocked","kind":"review","progress":150,"title":"Edit","updatedAt":8
    }))
    .unwrap();
    assert_eq!((blocked.kind(), blocked.progress()), (None, None));
    assert_eq!(blocked.summary(), "Waiting: Edit");
    for (kind, label) in [
        ("permission", "Waiting for permission"),
        ("question", "Waiting for an answer"),
        ("auth", "Waiting for sign-in"),
    ] {
        let status = ProgramStatus::parse(&json!({
            "state":"blocked","kind":kind,"msg":"","updatedAt":1
        }))
        .unwrap();
        assert_eq!(status.kind().map(BlockedKind::as_str), Some(kind));
        assert_eq!(status.message(), None, "an empty text is absent");
        assert_eq!(status.summary(), label);
    }
    for (state, label) in [("done", "Done"), ("error", "Failed"), ("idle", "Idle")] {
        let status = ProgramStatus::parse(&record(state, 1)).unwrap();
        assert_eq!(status.state().as_str(), state);
        assert_eq!(status.label(), label);
    }
    let long = ProgramStatus::parse(&json!({
        "state":"done","msg":"界".repeat(STATUS_TEXT_CHARS + 5),"updatedAt":1
    }))
    .unwrap();
    let text = long.summary();
    let text = text.strip_prefix("Done: ").unwrap();
    assert_eq!(text.chars().count(), STATUS_TEXT_CHARS);
    assert!(text.ends_with('…'));
}
