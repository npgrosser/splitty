//! mux-protocol, Program Status Capability and Program Status Events, and
//! the greeting halves of program-status's "Status while nobody streams" and
//! "Record rebuilt after a restart": sessions of the server's own backend
//! report through their output, and connections of the WebSocket or the
//! native transport receive the root records. In-process, since the router,
//! the transports and the watcher are the server's own.
use super::*;
use splitty_core::ids::{PaneId, ScreenId};
use splitty_core::mux::Op;
use splitty_core::pty::terminal::{Feed, SessionSpec, Terminal, TerminalFactory};
use splitty_core::pty::testing::{pty_available, wait_until};
use splitty_core::pty::{ProcessResult, PtyError, SessionHandle};

use crate::client_session::program_status::INTERVAL;

/// A program that writes each line of its input as a status report.
const REPORTER: &str = "while IFS= read -r line; do printf '\\033]7501;%s\\007' \"$line\"; done";

/// The next `program_status.changed`, answering pings meanwhile.
async fn next_status(socket: &mut Socket) -> serde_json::Value {
    loop {
        match next(socket).await {
            Message::Text(text) => {
                let value: serde_json::Value = serde_json::from_str(&text).expect("JSON");
                if value["type"] == "program_status.changed" {
                    return value;
                }
            }
            Message::Ping(bytes) => socket.send(Message::Pong(bytes)).await.expect("pong"),
            _ => {}
        }
    }
}

/// The next `program_status.changed` on both connections, which is the same
/// event.
async fn both(a: &mut Socket, b: &mut Socket) -> serde_json::Value {
    let event = next_status(a).await;
    assert_eq!(next_status(b).await, event);
    event
}

/// The message after `mux.welcome` and the listening ports, where the
/// system lists them.
async fn after_greetings(socket: &mut Socket) -> serde_json::Value {
    let message = json_of(next(socket).await);
    if message["type"] == "ports.changed" {
        return json_of(next(socket).await);
    }
    message
}

async fn send(socket: &mut Socket, value: serde_json::Value) {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .expect("send");
}

/// The screen `screen` with the pane `pane`, whose session the backend
/// starts with `opts` of that session's id.
async fn pane_session(
    state: &TestState,
    screen: &str,
    pane: &str,
    opts: impl FnOnce(&str) -> splitty_core::pty::CreateSessionOpts,
) -> (String, Arc<dyn SessionHandle>) {
    state
        .mux_store
        .dispatch(Op::CreateScreen {
            name: None,
            screen_id: Some(screen.into()),
            new_pane_id: Some(pane.into()),
            shell_profile_id: None,
        })
        .expect("screen");
    let sid = state
        .mux_store
        .get_screen(&ScreenId::from(screen))
        .expect("screen")
        .panes[&PaneId::from(pane)]
        .session_id
        .clone()
        .expect("session")
        .to_string();
    let session = state
        .client_state
        .dispatcher
        .backend()
        .create(opts(&sid))
        .await
        .expect("session");
    (sid, session)
}

async fn report(session: &Arc<dyn SessionHandle>, report: &str) {
    session
        .write(format!("{report}\n").as_bytes())
        .await
        .expect("input");
}

/// mux-protocol, Program Status Capability (Current server) and Program
/// Status Events: Status on connect, Every connection receives the change,
/// Paced changes, Record cleared, Child records only, Pane moved to another
/// screen and Session removed; program-status, Status while nobody streams
/// (the greeting half).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_records_reach_every_connection() {
    run_root_records_reach_every_connection(false).await;
    run_root_records_reach_every_connection(true).await;
}

async fn run_root_records_reach_every_connection(native: bool) {
    if !pty_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let served = Served::start(hub(&host, dir.path()), dir.path()).await;
    let state = &served.state;

    // Before anyone connects, nobody streams the session.
    let (sid, session) = pane_session(state, "s1", "p1", |id| command_opts(id, REPORTER)).await;
    report(&session, "state=blocked:kind=permission").await;
    assert!(
        wait_until(WAIT, || session
            .program_status()
            .record
            .is_some_and(|record| record.state.as_str() == "blocked"))
        .await
    );

    let mut a = served.open_local_for(native).await;
    let welcome = json_of(next(&mut a).await);
    assert_eq!(welcome["type"], "mux.welcome", "{welcome}");
    for capability in ["program-status-v1", "notification-notices-v1"] {
        assert!(
            welcome["capabilities"]
                .as_array()
                .expect("capabilities")
                .contains(&json!(capability)),
            "{welcome}"
        );
    }
    let greeting = after_greetings(&mut a).await;
    assert_eq!(greeting["type"], "program_status.changed", "{greeting}");
    assert_eq!(greeting["sessionId"], sid.as_str());
    assert_eq!(greeting["paneId"], "p1");
    assert_eq!(greeting["screenId"], "s1");
    assert_eq!(greeting["status"]["state"], "blocked");
    assert_eq!(greeting["status"]["kind"], "permission");
    assert!(greeting.get("notify").is_none(), "{greeting}");
    assert!(greeting.get("notice").is_none(), "{greeting}");
    assert!(greeting["id"].is_string() && greeting["timestamp"].is_i64());

    // `a` streams the session, `b` does not.
    send(&mut a, json!({"type":"attach","id":sid})).await;
    let mut b = served.open_local_for(native).await;
    assert_eq!(json_of(next(&mut b).await)["type"], "mux.welcome");
    let greeting = after_greetings(&mut b).await;
    assert_eq!(greeting["type"], "program_status.changed", "{greeting}");
    assert_eq!(greeting["status"]["state"], "blocked");

    report(&session, "state=working:app=claude-code").await;
    let working = both(&mut a, &mut b).await;
    assert_eq!(working["sessionId"], sid.as_str());
    assert_eq!(working["status"]["state"], "working");
    assert_eq!(working["status"]["app"], "claude-code");
    assert!(working.get("notify").is_none(), "{working}");
    assert!(working.get("notice").is_none(), "{working}");

    // 100 reports within about a second.
    for progress in 1..100 {
        report(&session, &format!("state=working:progress={progress}")).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    report(&session, "state=done").await;
    let mut events = Vec::new();
    loop {
        let event = next_status(&mut b).await;
        let done = event["status"]["state"] == "done";
        events.push(event);
        if done {
            break;
        }
    }
    assert!(events.len() < 100, "{} events", events.len());
    let times: Vec<i64> = events
        .iter()
        .map(|event| event["timestamp"].as_i64().expect("timestamp"))
        .collect();
    assert!(
        times.windows(2).all(|pair| pair[1] - pair[0] >= 100),
        "events closer than 100 ms: {times:?}"
    );
    // `a` got the same events.
    while next_status(&mut a).await["status"]["state"] != "done" {}

    report(&session, "state=clear").await;
    let cleared = both(&mut a, &mut b).await;
    assert_eq!(cleared["sessionId"], sid.as_str());
    assert!(
        cleared
            .get("status")
            .is_some_and(serde_json::Value::is_null),
        "{cleared}"
    );

    // mux-protocol, Classified Notification Events (Blocked program): a
    // marked event carries the notice, its screen named by the catalog.
    state
        .mux_store
        .dispatch(Op::RenameScreen {
            screen_id: "s1".into(),
            name: "build".into(),
        })
        .expect("rename");
    report(
        &session,
        "state=blocked:kind=permission:msg=UnVuIHRlc3RzPw==",
    )
    .await;
    let blocked = both(&mut a, &mut b).await;
    assert_eq!(blocked["notify"], true, "{blocked}");
    assert_eq!(
        blocked["notice"],
        json!({
            "kind": "program-status",
            "severity": "warning",
            "title": "build: Waiting for permission: Run tests?",
            "setting": "programStatus",
        })
    );

    // A child record alone sends nothing: the next event is the root's.
    report(&session, "state=working:id=task").await;
    tokio::time::sleep(INTERVAL * 3).await;
    report(&session, "state=done").await;
    let done = both(&mut a, &mut b).await;
    assert_eq!(done["status"]["state"], "done", "{done}");

    state
        .mux_store
        .dispatch(Op::MovePaneToScreen {
            screen_id: "s1".into(),
            pane_id: "p1".into(),
            target_screen_id: "s2".into(),
            create_screen: true,
            screen_name: None,
        })
        .expect("move");
    let moved = both(&mut a, &mut b).await;
    assert_eq!(moved["paneId"], "p1");
    assert_eq!(moved["screenId"], "s2");
    assert_eq!(moved["status"]["state"], "done");
    assert!(moved.get("notify").is_none(), "{moved}");
    assert!(moved.get("notice").is_none(), "{moved}");

    state
        .client_state
        .dispatcher
        .backend()
        .close(&sid)
        .expect("close");
    let removed = both(&mut a, &mut b).await;
    assert_eq!(removed["sessionId"], sid.as_str());
    assert!(
        removed
            .get("status")
            .is_some_and(serde_json::Value::is_null),
        "{removed}"
    );
}

/// A terminal a starting server adopts: its keeper had buffered a `blocked`
/// report, which the session reads again. No program runs on it.
struct Restarted;

impl TerminalFactory for Restarted {
    fn start(&self, _: &SessionSpec, feed: Arc<dyn Feed>) -> Result<Box<dyn Terminal>, PtyError> {
        feed.record_again(b"\x1b]7501;state=blocked:kind=permission\x07");
        Ok(Box::new(Kept(tokio::sync::watch::channel(false).0)))
    }
}

/// The adopted terminal, ended only by closing it.
struct Kept(tokio::sync::watch::Sender<bool>);

#[async_trait::async_trait]
impl Terminal for Kept {
    fn pid(&self) -> Option<u32> {
        None
    }
    async fn write(&self, _: &[u8]) -> std::io::Result<()> {
        Ok(())
    }
    fn resize(&self, _: u16, _: u16) -> std::io::Result<()> {
        Ok(())
    }
    fn close(&self) -> std::io::Result<()> {
        self.0.send_replace(true);
        Ok(())
    }
    async fn stop(&self) -> std::io::Result<ProcessResult> {
        self.close()?;
        Ok(ProcessResult::default())
    }
    fn done(&self) -> tokio::sync::watch::Receiver<bool> {
        self.0.subscribe()
    }
    fn result(&self) -> Option<ProcessResult> {
        self.0.borrow().then(ProcessResult::default)
    }
    fn foreground_pgid(&self) -> Option<u32> {
        None
    }
}

/// program-status, Record rebuilt after a restart (the greeting half): a
/// record from output read again is sent unmarked, to a connected client
/// and to one that connects later.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_read_again_is_sent_without_notify() {
    run_a_record_read_again_is_sent_without_notify(false).await;
    run_a_record_read_again_is_sent_without_notify(true).await;
}

async fn run_a_record_read_again_is_sent_without_notify(native: bool) {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let served = Served::start_with(hub(&host, dir.path()), |addr| {
        state_on(addr, dir.path(), None, Arc::new(Restarted))
    })
    .await;
    let state = &served.state;

    let mut connected = served.open_local_for(native).await;
    assert_eq!(json_of(next(&mut connected).await)["type"], "mux.welcome");
    let (sid, session) = pane_session(state, "s1", "p1", |id| {
        splitty_core::pty::testing::sh_opts(id, "")
    })
    .await;
    assert!(session.program_status().read_again);
    let event = next_status(&mut connected).await;
    assert_eq!(event["sessionId"], sid.as_str());
    assert_eq!(event["status"]["state"], "blocked");
    assert!(event.get("notify").is_none(), "{event}");

    let mut later = served.open_local_for(native).await;
    assert_eq!(json_of(next(&mut later).await)["type"], "mux.welcome");
    let greeting = after_greetings(&mut later).await;
    assert_eq!(greeting["type"], "program_status.changed", "{greeting}");
    assert_eq!(greeting["sessionId"], sid.as_str());
    assert_eq!(greeting["paneId"], "p1");
    assert_eq!(greeting["screenId"], "s1");
    assert_eq!(greeting["status"]["state"], "blocked");
    assert!(greeting.get("notify").is_none(), "{greeting}");
    state.client_state.dispatcher.backend().close_all();
}
