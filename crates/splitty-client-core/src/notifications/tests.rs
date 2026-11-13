use super::*;
use serde_json::json;
use uuid::Uuid;

fn settings() -> NotificationSettings {
    NotificationSettings {
        process_exited: true,
        ..NotificationSettings::default()
    }
}
fn snapshot(
    window: &mut WindowWorkspace,
    owner: SourceToken,
    screen: &str,
    panes: &[(&str, &str)],
) {
    let panes_map = panes
        .iter()
        .map(|(pane, session)| ((*pane).to_owned(), json!({"id":pane,"sessionId":session})))
        .collect::<serde_json::Map<_, _>>();
    let sessions = panes
        .iter()
        .map(|(_, session)| {
            (
                (*session).to_owned(),
                json!({"id":session,"status":"running"}),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    window.source_mut(owner.source_id()).unwrap().apply_snapshot(owner.connection_id(), &json!({
        "type":"mux.screen_snapshot","screen":{"id":screen,"revision":3,
        "root":{"type":"slot","id":format!("slot-{screen}"),"paneIds":panes.iter().map(|(pane,_)|*pane).collect::<Vec<_>>(),"activePaneId":panes.first().map(|(pane,_)|*pane)},
        "panes":panes_map,"sessions":sessions}
    })).unwrap();
}
fn fixture() -> (WindowWorkspace, NotificationCenter, ScreenKey) {
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    let screen = window.ui_test_local_screen_fixture("Work").unwrap();
    let owner = window.lifecycle_token();
    snapshot(&mut window, owner, "work", &[("p0", "s0"), ("p1", "s1")]);
    let mut center = NotificationCenter::default();
    center.reconcile(&window);
    center.apply_settings(&settings());
    (window, center, screen)
}
/// An event the server classified, governed by no switch but `enabled`.
fn event(id: &str, session: &str, severity: &str) -> Value {
    json!({"type":"process.idle","id":id,"sessionId":session,"process":"task",
        "notice":{"kind":"process.idle","severity":severity,"title":format!("event {id}")}})
}
/// `event_type` with the notice `notice`.
fn noticed(event_type: &str, id: &str, notice: Value) -> Value {
    json!({"type":event_type,"id":id,"sessionId":"s0","process":"task","notice":notice})
}
fn admit(
    center: &mut NotificationCenter,
    window: &WindowWorkspace,
    owner: SourceToken,
    value: &Value,
    watched: Option<&PaneKey>,
) -> bool {
    center.reconcile(window);
    center.observe_json(window, owner, value, watched, "Build host")
}

/// mux-protocol, Classified Notification Events and Notifications in the
/// Window: the server's notice decides kind, severity and title; a window
/// adds nothing for a suppressed one, a switch turned off also removes what
/// it governs, and the watched pane's is recorded as read without a notice.
#[test]
fn notices_settings_and_watched_read_share_one_bounded_policy() {
    let (window, mut center, screen) = fixture();
    let owner = window.lifecycle_token();
    let p0 = window.local_source().pane_key("p0");
    let p1 = window.local_source().pane_key("p1");
    for (id, event_type, severity, setting) in [
        ("finished", "process.finished", "info", "processFinished"),
        ("exited", "session.exited", "info", "processExited"),
        ("crashed", "session.exited", "error", "processCrashed"),
    ] {
        let value = noticed(
            event_type,
            id,
            json!({"kind":event_type,"severity":severity,"title":format!("task {id}"),"setting":setting}),
        );
        assert!(admit(&mut center, &window, owner, &value, None));
    }
    assert_eq!(center.records.back().unwrap().severity, Severity::Error);
    assert_eq!(center.unread_titles(&screen).next(), Some("task crashed"));
    assert_eq!(center.unread_count(&screen), 3);
    assert_eq!(center.severity(&p0), Some(Severity::Error));
    assert_eq!(center.take_fresh().len(), 3);

    // Own pane closed, or a program that reports its status: nothing.
    for (id, reason) in [("closed", "user-initiated"), ("covered", "program-status")] {
        let value = noticed(
            "session.exited",
            id,
            json!({"kind":"session.crashed","severity":"error","title":"zsh failed",
                "setting":"processCrashed","suppressed":reason}),
        );
        assert!(!admit(&mut center, &window, owner, &value, None), "{id}");
    }
    assert!(center.take_fresh().is_empty());
    assert_eq!(center.unread_count(&screen), 3);

    // The watched pane's notification arrives read and shows no notice.
    assert!(admit(
        &mut center,
        &window,
        owner,
        &event("watched", "s1", "warning"),
        Some(&p1)
    ));
    assert!(center.take_fresh().is_empty());
    assert_eq!(center.severity(&p1), None);
    assert!(admit(
        &mut center,
        &window,
        owner,
        &event("other", "s1", "success"),
        Some(&p0)
    ));
    assert_eq!(center.unread_titles(&screen).next(), Some("event other"));
    assert!(center.mark_read(&p0));
    assert!(!center.mark_read(&p0));
    assert_eq!(center.unread_count(&screen), 1);
    assert_eq!(center.severity(&p1), Some(Severity::Success));
    assert!(center.mark_read(&p1)); // UI focus/visibility return uses the same operation.
    assert_eq!(center.unread_count(&screen), 0);
    assert!(!admit(
        &mut center,
        &window,
        owner,
        &event("crashed", "s0", "error"),
        None
    ));

    // Switches turned off remove what they govern and stop new ones.
    assert!(admit(
        &mut center,
        &window,
        owner,
        &noticed(
            "process.finished",
            "kept",
            json!({"kind":"process.finished","severity":"info","title":"kept","setting":"processFinished"})
        ),
        None
    ));
    let mut switches = settings();
    switches.process_finished = false;
    switches.process_exited = false;
    switches.process_crashed = false;
    let held = center.records.len();
    assert!(center.apply_settings(&switches));
    assert_eq!(center.records.len(), held - 4); // finished, exited, crashed, kept.
    assert_eq!(center.unread_count(&screen), 0);
    for (id, setting) in [
        ("finished2", "processFinished"),
        ("exited2", "processExited"),
        ("crashed2", "processCrashed"),
    ] {
        let value = noticed(
            "session.exited",
            id,
            json!({"kind":"session.exited","severity":"info","title":id,"setting":setting}),
        );
        assert!(!admit(&mut center, &window, owner, &value, None), "{id}");
    }
    // No switch, or one this window does not know: only `enabled` counts.
    assert!(admit(
        &mut center,
        &window,
        owner,
        &event("idle2", "s0", "info"),
        None
    ));
    assert!(admit(
        &mut center,
        &window,
        owner,
        &noticed(
            "process.finished",
            "future",
            json!({"kind":"later","severity":"nonsense","title":"future","setting":"later"})
        ),
        None
    ));
    assert_eq!(center.records.back().unwrap().severity, Severity::Info);
    assert_eq!(center.unread_count(&screen), 2);
    switches.enabled = false;
    assert!(center.apply_settings(&switches));
    assert!(center.records.is_empty());
    assert!(!admit(
        &mut center,
        &window,
        owner,
        &event("disabled", "s0", "error"),
        None
    ));
    center.apply_settings(&settings());
    assert_eq!(center.unread_count(&screen), 0); // no resurrection.
}

/// mux-protocol, Notification Notices Capability (Older server): an event
/// without a notice gets the generic one, governed by `enabled` only, and
/// a status change or a bell none.
#[test]
fn an_older_server_gets_a_generic_notice() {
    let (window, mut center, screen) = fixture();
    let owner = window.lifecycle_token();
    let mut off = settings();
    off.process_finished = false;
    off.process_crashed = false;
    center.apply_settings(&off);
    for (id, value) in [
        (
            "finished",
            json!({"type":"process.finished","id":"finished","sessionId":"s0","process":"cargo"}),
        ),
        (
            "closed",
            json!({"type":"session.exited","id":"closed","sessionId":"s0","exitCode":1,"userInitiated":true}),
        ),
    ] {
        assert!(admit(&mut center, &window, owner, &value, None), "{id}");
        assert_eq!(center.records.back().unwrap().severity, Severity::Info);
    }
    assert_eq!(
        center.unread_titles(&screen).collect::<Vec<_>>(),
        ["Session activity", "Activity in cargo"]
    );
    for value in [
        json!({"type":"bell","id":"bell","sessionId":"s0"}),
        json!({"type":"program_status.changed","id":"status","sessionId":"s0","notify":true,
            "status":{"state":"blocked","updatedAt":1}}),
    ] {
        assert!(!admit(&mut center, &window, owner, &value, None), "{value}");
    }
}

#[test]
fn limits_reject_ambiguous_ids_and_bound_plain_text_without_payload_copies() {
    let (window, mut center, screen) = fixture();
    let owner = window.lifecycle_token();
    for id in [
        "".to_owned(),
        "x".repeat(MAX_ID_BYTES + 1),
        "a\n".into(),
        "a\u{202e}".into(),
    ] {
        assert!(!admit(
            &mut center,
            &window,
            owner,
            &event(&id, "s0", "error"),
            None
        ));
    }
    assert!(!admit(
        &mut center,
        &window,
        owner,
        &json!({"type":"notification","id":7}),
        None
    ));
    for notice in [
        json!({"title":1}),
        json!("notice"),
        json!({"severity":"error"}),
    ] {
        assert!(!admit(
            &mut center,
            &window,
            owner,
            &noticed("session.exited", "bad", notice),
            None
        ));
    }
    let mut value = event("long", "s0", "warning");
    value["notice"]["title"] = json!(format!("\u{202e}\x1b{}\n", "界".repeat(10_000)));
    value["body"] = json!("discard".repeat(100_000));
    assert!(admit(&mut center, &window, owner, &value, None));
    let title = center.unread_titles(&screen).next().unwrap();
    assert_eq!(title.chars().count(), MAX_TITLE_CHARS);
    assert!(!title.chars().any(hidden));
    assert!(title.len() <= MAX_TITLE_CHARS * 4);
    for index in 0..MAX_RECORDS + 1 {
        assert!(admit(
            &mut center,
            &window,
            owner,
            &event(&index.to_string(), "s0", "info"),
            None
        ));
    }
    assert_eq!(center.records.len(), MAX_RECORDS);
    assert_eq!(center.unread_count(&screen), MAX_RECORDS);
    assert_eq!(center.records.front().unwrap().id, "1");
    assert_eq!(center.unread_titles(&screen).next(), Some("event 100"));
    assert!(admit(
        &mut center,
        &window,
        owner,
        &event("0", "s0", "info"),
        None
    )); // bounded dedup horizon.
}

#[test]
fn sources_offline_facts_and_unknown_sessions_never_cross_bind() {
    let (mut window, mut center, local_screen) = fixture();
    let local = window.lifecycle_token();
    let (remote_screen, remote_route) = window
        .ui_test_remote_terminal_fixture("me@192.0.2.1")
        .unwrap();
    let remote = remote_route.source_token();
    snapshot(&mut window, remote, "work", &[("p0", "s0")]);
    assert!(admit(
        &mut center,
        &window,
        local,
        &event("same", "s0", "info"),
        None
    ));
    assert!(center.observe_json(
        &window,
        remote,
        &event("same", "s0", "error"),
        None,
        "host\u{202e}\n"
    ));
    assert_eq!(center.unread_count(&local_screen), 1);
    assert_eq!(center.unread_count(&remote_screen), 1);
    assert_eq!(
        center.unread_titles(&remote_screen).next(),
        Some("host: event same")
    );
    assert_eq!(
        center.severity(&window.local_source().pane_key("p0")),
        Some(Severity::Info)
    );
    assert_eq!(
        center.severity(&remote_route.pane_key()),
        Some(Severity::Error)
    );
    assert!(admit(
        &mut center,
        &window,
        remote,
        &event("unknown", "s1", "warning"),
        None
    )); // exists only on local source.
    assert!(admit(
        &mut center,
        &window,
        remote,
        &json!({"type":"notification","id":"unscoped","title":"global"}),
        None
    ));
    snapshot(&mut window, remote, "work", &[("p0", "s0"), ("p1", "s1")]);
    center.reconcile(&window);
    assert_eq!(center.unread_count(&remote_screen), 1); // unknown-at-arrival stays unbound.
    window.disconnect(remote.source_id()).unwrap();
    assert_eq!(window.execution_token(remote.source_id()), None);
    assert!(admit(
        &mut center,
        &window,
        remote,
        &event("queued", "s0", "warning"),
        None
    ));
    assert_eq!(center.unread_count(&remote_screen), 2);
    assert!(!admit(
        &mut center,
        &window,
        local,
        &event("same", "s0", "error"),
        None
    ));
}

#[test]
fn reconnect_keeps_history_through_unknown_routes_then_uses_fresh_canonical_proof() {
    let (mut window, mut center, _) = fixture();
    let (screen, route) = window
        .ui_test_remote_terminal_fixture("me@192.0.2.1")
        .unwrap();
    let old = route.source_token();
    assert!(admit(
        &mut center,
        &window,
        old,
        &event("unread", route.session_id(), "error"),
        None
    ));
    assert!(admit(
        &mut center,
        &window,
        old,
        &event("read", route.session_id(), "info"),
        Some(&route.pane_key())
    ));
    // Existing source reset revokes its snapshots and binding, retaining source namespace.
    window
        .source_mut(old.source_id())
        .unwrap()
        .reset_connection(Uuid::new_v4());
    assert!(center.reconcile(&window));
    assert_eq!(center.records.len(), 2);
    assert_eq!(center.unread_count(&screen), 0);
    assert!(!center.mark_read(&route.pane_key()));
    assert!(!admit(
        &mut center,
        &window,
        old,
        &event("late", route.session_id(), "error"),
        None
    ));
    let current = window.source(old.source_id()).unwrap().source_token();
    snapshot(&mut window, current, "work", &[("p0", route.session_id())]);
    assert!(center.reconcile(&window));
    assert_eq!(center.unread_count(&screen), 1); // no event replay.
    assert!(!admit(
        &mut center,
        &window,
        current,
        &event("read", route.session_id(), "error"),
        None
    ));
    assert_eq!(center.unread_count(&screen), 1);
    // A current metadata refresh uses the ordinary source fixture and closes the
    // observation gap; replacement must remove old records, not just hide them.
    let (_, fresh) = window
        .ui_test_remote_terminal_fixture("me@192.0.2.1")
        .unwrap();
    snapshot(
        &mut window,
        fresh.source_token(),
        "work",
        &[("p0", "replacement")],
    );
    assert!(window
        .source(old.source_id())
        .unwrap()
        .route_observation_complete());
    assert!(center.reconcile(&window));
    assert!(center.records.is_empty());
}

#[test]
fn moved_removed_sources_and_local_owner_replacement_have_distinct_cleanup() {
    let (mut window, mut center, old_screen) = fixture();
    let owner = window.lifecycle_token();
    assert!(admit(
        &mut center,
        &window,
        owner,
        &event("move", "s0", "error"),
        None
    ));
    window
        .ui_test_screen_order_fixture(&[("work", "Work"), ("other", "Other")], None)
        .unwrap();
    snapshot(&mut window, owner, "work", &[("p1", "s1")]);
    center.reconcile(&window); // target Screen has no snapshot yet: absence is unknown.
    assert_eq!(center.records.len(), 1);
    snapshot(&mut window, owner, "other", &[("p0", "s0")]);
    center.reconcile(&window);
    let moved = window.local_source().screen_key("other");
    assert_eq!(center.unread_count(&old_screen), 0);
    assert_eq!(center.unread_count(&moved), 1);
    snapshot(&mut window, owner, "other", &[]);
    assert!(window.local_source().route_observation_complete());
    center.reconcile(&window);
    assert!(center.records.is_empty());
    let (_, route) = window
        .ui_test_remote_terminal_fixture("me@192.0.2.1")
        .unwrap();
    assert!(admit(
        &mut center,
        &window,
        route.source_token(),
        &event("disposed", route.session_id(), "error"),
        None
    ));
    window
        .begin_close(route.source_token().source_id())
        .unwrap();
    window
        .finish_close(route.source_token().source_id())
        .unwrap();
    center.reconcile(&window);
    assert!(center.records.is_empty());
    let (replacement, _, screen) = fixture();
    center.reconcile(&replacement);
    assert!(!admit(
        &mut center,
        &replacement,
        replacement.lifecycle_token(),
        &event("before-settings", "s0", "info"),
        None
    ));
    assert_eq!(center.unread_count(&screen), 0);
    center.apply_settings(&settings());
    assert!(admit(
        &mut center,
        &replacement,
        replacement.lifecycle_token(),
        &event("after-settings", "s0", "info"),
        None
    ));
}

#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn real_event_admission_releases_transport_credit_when_event_is_dropped() {
    use crate::{appearance::local::tests::fixture, embedded::EmbeddedService};
    use splitty_core::protocol::ServerEvent;
    let (runtime, path) = fixture();
    let state = runtime.state.clone();
    let service = EmbeddedService::from_runtime(runtime);
    let mut client = service.connect().await.unwrap();
    let budget = client.sender().context().budget;
    drop(client.next_event().await.unwrap().unwrap()); // welcome
    let mut window = WindowWorkspace::new(client.connection_id());
    let screen = window.ui_test_local_screen_fixture("Work").unwrap();
    let owner = window.lifecycle_token();
    snapshot(&mut window, owner, "work", &[("p0", "s0")]);
    let mut center = NotificationCenter::default();
    center.reconcile(&window);
    // Admission before the first accepted settings is deliberately empty.
    state
        .client_state
        .notify_tx
        .send(&ServerEvent::process_finished("s0", "build".into(), None));
    let mut event = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = client.next_event().await.unwrap().unwrap();
            if event
                .json()
                .is_some_and(|value| value["type"] == "process.finished")
            {
                break event;
            }
        }
    })
    .await
    .unwrap();
    assert!(!center.observe(&window, owner, &event, None, ""));
    center.apply_settings(&settings());
    let actual = event.connection_id;
    event.connection_id = Uuid::new_v4();
    assert!(!center.observe(&window, owner, &event, None, ""));
    event.connection_id = actual;
    assert!(center.observe(&window, owner, &event, None, ""));
    assert_eq!(center.unread_count(&screen), 1);
    client.finish().await.unwrap();
    service.close().await.unwrap();
    assert!(budget.used() > 0);
    drop(event);
    assert_eq!(budget.used(), 0);
    assert_eq!(center.unread_titles(&screen).next(), Some("build finished"));
    assert!(!path.exists());
}

#[tokio::test]
async fn opaque_ui_event_fixture_joins_peers_before_releasing_held_credit() {
    let (window, mut center, screen) = fixture();
    let owner = window.lifecycle_token();
    let event = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        Event::ui_test_json(owner.connection_id(), &event("fixture", "s0", "error")),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(event.connection_id, owner.connection_id());
    assert!(center.observe(&window, owner, &event, None, ""));
    drop(event);
    assert_eq!(center.unread_count(&screen), 1);
}

/// A `program_status.changed` marked `notify`, with the notice the server
/// built for it.
fn status_event(id: &str, session: &str, severity: &str, title: &str) -> Value {
    json!({"type":"program_status.changed","id":id,"timestamp":1,"sessionId":session,
        "paneId":"p9","screenId":"work","status":{"state":"blocked","updatedAt":5},"notify":true,
        "notice":{"kind":"program-status","severity":severity,"title":title,"setting":"programStatus"}})
}

/// program-status, Status Notifications in the Window: a marked event
/// notifies with the server's notice, prefixed with the host; the watched
/// pane's is read, the switch stops new ones and keeps the others, and a
/// pane with a record rings and finishes commands without notifying.
#[test]
fn program_status_notifies_by_its_notice_and_switch() {
    let (mut window, mut center, local_screen) = fixture();
    let local = window.lifecycle_token();
    let (remote_screen, route) = window
        .ui_test_remote_terminal_fixture("me@192.0.2.1")
        .unwrap();
    let remote = route.source_token();
    let title = "Review: Waiting for permission: Allow edit?";

    // A host pane on a Screen the window has not opened.
    let unopened = status_event("blocked", "s-review", "warning", title);
    assert!(admit(&mut center, &window, remote, &unopened, None));
    let fresh = center.take_fresh();
    assert_eq!(fresh.len(), 1);
    assert_eq!(fresh[0].severity, Severity::Warning);
    assert_eq!(fresh[0].title, format!("Build host: {title}"));
    assert_eq!(fresh[0].screen, None);
    // A bound host pane counts as unread on its opened Screen.
    let bound = status_event("bound", route.session_id(), "warning", "work: Waiting");
    assert!(admit(&mut center, &window, remote, &bound, None));
    assert_eq!(
        center.unread_titles(&remote_screen).next(),
        Some("Build host: work: Waiting")
    );
    assert_eq!(center.severity(&route.pane_key()), Some(Severity::Warning));
    assert_eq!(center.take_fresh().len(), 1);
    // The watched pane's arrives read, without a notice.
    let watched = status_event("watched", route.session_id(), "error", "work: Failed");
    assert!(admit(
        &mut center,
        &window,
        remote,
        &watched,
        Some(&route.pane_key())
    ));
    assert!(center.take_fresh().is_empty());
    assert_eq!(center.severity(&route.pane_key()), Some(Severity::Warning));
    // Unmarked events carry no notice and add none.
    let mut unmarked = status_event("unmarked", "s0", "info", "x");
    let object = unmarked.as_object_mut().unwrap();
    object.remove("notify");
    object.remove("notice");
    assert!(!admit(&mut center, &window, local, &unmarked, None));
    assert!(admit(
        &mut center,
        &window,
        local,
        &status_event("done", "s0", "success", "Work: Done: Edit"),
        None
    ));
    assert_eq!(center.unread_count(&local_screen), 1);

    // A pane holding a root record: its bell is no notification, and the
    // server suppresses its finished commands.
    assert!(!admit(
        &mut center,
        &window,
        local,
        &json!({"type":"bell","id":"bell","sessionId":"s0"}),
        None
    ));
    assert!(!admit(
        &mut center,
        &window,
        local,
        &json!({"type":"process.idle","id":"idle","sessionId":"s0","process":"claude",
            "notice":{"kind":"process.idle","severity":"info","title":"claude is waiting","suppressed":"program-status"}}),
        None
    ));

    // Control and bidirectional characters never reach the text.
    let hostile = status_event(
        "hostile",
        "s-review",
        "warning",
        "Review: Waiting: ok\u{202e}txt\u{1b}[2J\u{2066}.exe",
    );
    assert!(admit(&mut center, &window, remote, &hostile, None));
    let text = &center.records.back().unwrap().title;
    assert_eq!(text, "Build host: Review: Waiting: oktxt[2J.exe");
    assert!(!text.chars().any(hidden));

    // The switch off: no notification, earlier ones go.
    let mut off = settings();
    off.program_status = false;
    assert!(center.apply_settings(&off));
    assert!(center
        .records
        .iter()
        .all(|record| record.setting != Some(Setting::ProgramStatus)));
    assert!(!admit(
        &mut center,
        &window,
        remote,
        &status_event("off", "s-review", "warning", title),
        None
    ));
    // The pane's tab still shows its status: the workspace keeps the record.
    observe_status(
        &mut window,
        local,
        &status_event("tab", "s0", "warning", title),
    );
    assert!(window.local_source().session_program_status("s0").is_some());
}

fn observe_status(window: &mut WindowWorkspace, owner: SourceToken, value: &Value) {
    assert!(window
        .ui_test_program_status(owner.source_id(), value)
        .unwrap());
}
