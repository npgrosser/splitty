//! `program_status.changed` through the real source drivers of a local
//! source and a host source, each on its own connection of the embedded
//! service; the events themselves are synthetic.
use super::*;
use crate::workspace::ProgramState;
use serde_json::json;

async fn deliver(
    source: &mut source::SourceRuntime,
    window: &mut WindowWorkspace,
    connection: uuid::Uuid,
    value: serde_json::Value,
) -> source::SourceEffects {
    let event = Event::ui_test_json(connection, &value).await.unwrap();
    source.event(window, Box::new(event)).unwrap()
}
fn welcome(capabilities: &[&str]) -> serde_json::Value {
    json!({"type":"mux.welcome","id":"w","timestamp":1,"clientId":"c","screens":[],
        "capabilities":capabilities})
}
fn blocked(session: &str, screen: &str) -> serde_json::Value {
    json!({"type":"program_status.changed","id":format!("ps-{session}"),"timestamp":2,
        "sessionId":session,"paneId":"p0","screenId":screen,
        "status":{"state":"blocked","kind":"permission","msg":"Allow edit?","updatedAt":2},
        "notify":true})
}
fn record(
    window: &WindowWorkspace,
    id: crate::workspace::SourceId,
    session: &str,
) -> Option<ProgramState> {
    window
        .source(id)?
        .session_program_status(session)
        .map(|status| status.state())
}

#[tokio::test]
async fn program_status_events_route_to_their_own_welcomed_source_only() {
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let service = crate::EmbeddedService::from_runtime(runtime);
    let local_client = service.connect().await.unwrap();
    let local_connection = local_client.connection_id();
    let mut window = WindowWorkspace::new(local_connection);
    window.ui_test_local_screen_fixture("Work").unwrap();
    let mut local = source::SourceRuntime::new(local_client, window.lifecycle_token());
    let local_id = window.lifecycle_token().source_id();
    let id = window
        .admit_remote(window.lifecycle_token(), "older.test")
        .unwrap();
    let host_client = service.connect().await.unwrap();
    let host_connection = host_client.connection_id();
    window.test_bind_remote_client(id, &host_client);
    let token = window.source(id).unwrap().source_token();
    window.confirm_ready(token).unwrap();
    let mut host = source::SourceRuntime::new(host_client, token);

    // The host is older: its welcome does not list the capability.
    deliver(&mut host, &mut window, host_connection, welcome(&[])).await;
    let effects = deliver(
        &mut host,
        &mut window,
        host_connection,
        blocked("shared", "work"),
    )
    .await;
    assert!(!effects.changed);
    assert!(effects
        .updates
        .iter()
        .all(|update| !matches!(update, Update::Status(_))));
    assert_eq!(record(&window, id, "shared"), None);

    let current = welcome(&[crate::workspace::PROGRAM_STATUS_CAPABILITY]);
    deliver(&mut local, &mut window, local_connection, current).await;
    assert!(window.local_source().program_status_supported());
    // The same session id on the host's connection still marks nothing there.
    let effects = deliver(
        &mut local,
        &mut window,
        local_connection,
        blocked("shared", "work"),
    )
    .await;
    assert!(effects.changed);
    assert!(effects
        .updates
        .iter()
        .any(|update| matches!(update, Update::Event { source, .. } if *source == window.lifecycle_token())));
    assert_eq!(
        record(&window, local_id, "shared"),
        Some(ProgramState::Blocked)
    );
    assert_eq!(record(&window, id, "shared"), None);
    // An event of another connection reaches no source.
    let effects = deliver(
        &mut local,
        &mut window,
        host_connection,
        blocked("other", "work"),
    )
    .await;
    assert!(!effects.changed);
    assert_eq!(record(&window, local_id, "other"), None);
    // An invalid event is reported and changes nothing.
    let mut invalid = blocked("shared", "work");
    invalid["status"]["state"] = json!("thinking");
    let effects = deliver(&mut local, &mut window, local_connection, invalid).await;
    assert!(effects.updates.iter().any(
        |update| matches!(update, Update::Status(message) if message == "Invalid program status")
    ));
    assert_eq!(
        record(&window, local_id, "shared"),
        Some(ProgramState::Blocked)
    );

    local.client.finish().await.unwrap();
    host.client.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}
