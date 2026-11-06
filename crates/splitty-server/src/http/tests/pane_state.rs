use super::*;
use splitty_core::{
    ids::{PaneId, ScreenId},
    mux::{Op, SessionStatus},
    pty::testing::{pty_available, wait_until},
};

#[tokio::test]
async fn unattended_exit_uses_the_host_policy_and_keeps_empty_screens() {
    if !pty_available() {
        return;
    }
    for (policy, code, removed) in [
        ("always", 23, true),
        ("graceful", 0, true),
        ("graceful", 23, false),
        ("never", 0, false),
    ] {
        let temp = tempfile::tempdir().expect("temporary directory");
        let app = state(ADDR, temp.path());
        app.config.write().expect("settings").terminal.close_on_exit = policy.into();
        app.mux_store
            .dispatch(Op::CreateScreen {
                name: None,
                screen_id: Some("screen".into()),
                new_pane_id: Some("pane".into()),
                shell_profile_id: None,
            })
            .expect("screen");
        let sid = app
            .mux_store
            .get_screen(&ScreenId::from("screen"))
            .expect("screen")
            .panes[&PaneId::from("pane")]
            .session_id
            .clone()
            .expect("session");
        let backend = app.client_state.dispatcher.backend();
        let session = backend
            .create(command_opts(
                sid.as_str(),
                &format!("printf 'final output'; exit {code}"),
            ))
            .await
            .expect("PTY");
        assert!(
            wait_until(WAIT, || *session.done().borrow()
                && session.output_complete())
            .await
        );
        // Move after process exit but before observation: ownership follows its session.
        app.mux_store
            .dispatch(Op::MovePaneToScreen {
                screen_id: "screen".into(),
                pane_id: "pane".into(),
                target_screen_id: "moved".into(),
                create_screen: true,
                screen_name: None,
            })
            .expect("move");
        let mut observer = crate::service::pane_state::Observer::default();
        let now = std::time::Instant::now();
        observer.refresh(&app.service, now).await;
        assert!(
            app.mux_store
                .get_screen(&"moved".into())
                .expect("screen")
                .panes
                .contains_key(&"pane".into()),
            "final-output grace"
        );
        observer
            .refresh(&app.service, now + Duration::from_secs(4))
            .await;
        let screen = app
            .mux_store
            .get_screen(&"moved".into())
            .expect("empty screen remains");
        assert_eq!(screen.panes.is_empty(), removed, "{policy}/{code}");
        assert_eq!(backend.get(sid.as_str()).is_none(), removed);
        if !removed {
            assert_eq!(screen.sessions[&sid].status, SessionStatus::Exited);
            assert_eq!(screen.sessions[&sid].exit_code, Some(code));
            assert!(!screen.sessions[&sid].busy);
            assert!(String::from_utf8_lossy(&session.scrollback()).contains("final output"));
            backend.close(sid.as_str()).expect("cleanup");
        }
    }
}

#[tokio::test]
async fn unattended_metadata_populates_snapshots_and_cli_pane_titles() {
    if !pty_available() {
        return;
    }
    let temp = tempfile::tempdir().expect("temporary directory");
    let app = state(ADDR, temp.path());
    app.mux_store
        .dispatch(Op::CreateScreen {
            name: None,
            screen_id: Some("screen".into()),
            new_pane_id: Some("pane".into()),
            shell_profile_id: None,
        })
        .expect("screen");
    let sid = app
        .mux_store
        .get_screen(&"screen".into())
        .expect("screen")
        .panes[&PaneId::from("pane")]
        .session_id
        .clone()
        .expect("session");
    let backend = app.client_state.dispatcher.backend();
    let session = backend
        .create(command_opts(
            sid.as_str(),
            r"printf '\033]2;Live title\007'; sleep 30",
        ))
        .await
        .expect("PTY");
    assert!(
        wait_until(WAIT, || session.osc_title_watch().borrow().title
            == "Live title")
        .await
    );
    let mut observer = crate::service::pane_state::Observer::default();
    let publication = app.client_state.effect_runner.publication.lock().await;
    let mut refresh = Box::pin(observer.refresh(&app.service, std::time::Instant::now()));
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut refresh)
            .await
            .is_err(),
        "metadata waits until spawn publication is complete"
    );
    drop(publication);
    refresh.await;
    let screen = app.mux_store.get_screen(&"screen".into()).expect("screen");
    let entry = &screen.sessions[&sid];
    assert!(entry.display_title.is_some());
    assert_eq!(
        screen.panes[&PaneId::from("pane")].title,
        entry.display_title
    );
    assert_eq!(entry.busy, session.title().busy());
    assert_eq!(entry.status, SessionStatus::Running);
    backend.close(sid.as_str()).expect("cleanup");
}

fn command_opts(id: &str, script: &str) -> splitty_core::pty::CreateSessionOpts {
    let mut opts = splitty_core::pty::testing::sh_opts(id, "");
    opts.command = script.into();
    opts.restore_mode = "close".into();
    opts
}
