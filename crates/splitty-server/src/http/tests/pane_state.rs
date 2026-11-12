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

/// program-status, Status Record Lifetime (Agent killed without clearing,
/// Done survives the prompt): a program started from a pane's shell reports
/// and is killed; once the observer sees the shell in the foreground again,
/// a `blocked` record is gone, while a `done` record of a second pane
/// survives the same sequence and the shell's prompt mark.
#[tokio::test]
async fn a_killed_programs_block_ends_when_its_shell_is_back_but_done_stays() {
    use splitty_core::pty::status::StatusState;
    if !pty_available() {
        return;
    }
    let temp = tempfile::tempdir().expect("temporary directory");
    let app = state(ADDR, temp.path());
    let backend = app.client_state.dispatcher.backend();
    let mut sessions = Vec::new();
    // The shell of the first pane has no integration and marks no prompt.
    for (pane, report, prompt) in [
        ("blocked", "state=blocked:kind=permission", ""),
        ("done", "state=done", "printf '\\033]133;A\\007'; "),
    ] {
        app.mux_store
            .dispatch(Op::CreateScreen {
                name: None,
                screen_id: Some(format!("screen-{pane}").as_str().into()),
                new_pane_id: Some(pane.into()),
                shell_profile_id: None,
            })
            .expect("screen");
        let sid = app
            .mux_store
            .get_screen(&ScreenId::from(format!("screen-{pane}").as_str()))
            .expect("screen")
            .panes[&PaneId::from(pane)]
            .session_id
            .clone()
            .expect("session");
        // Job control puts the program in a process group of its own, the
        // terminal's foreground, as an interactive shell does.
        let script = format!(
            "set -m; sh -c 'printf \"\\033]7501;{report}\\007\"; sleep 1.5; kill -9 $$'; \
             {prompt}read line"
        );
        let session = backend
            .create(command_opts(sid.as_str(), &script))
            .await
            .expect("PTY");
        sessions.push((sid, session));
    }
    for (_, session) in &sessions {
        assert!(
            wait_until(WAIT, || session.program_status().record.is_some()
                && session.title().busy())
            .await,
            "the program reported and runs in the foreground"
        );
    }
    let mut observer = crate::service::pane_state::Observer::default();
    observer
        .refresh(&app.service, std::time::Instant::now())
        .await;
    for (sid, _) in &sessions {
        let screen = app
            .mux_store
            .get_workspace_state()
            .screens
            .into_values()
            .find(|screen| screen.sessions.contains_key(sid))
            .expect("screen");
        assert!(screen.sessions[sid].busy, "the observer saw it busy");
    }
    for (_, session) in &sessions {
        assert!(wait_until(WAIT, || !session.title().busy()).await, "killed");
    }
    let state = |index: usize| {
        let session: &Arc<dyn splitty_core::pty::SessionHandle> = &sessions[index].1;
        session.program_status().record.map(|record| record.state)
    };
    assert_eq!(
        state(0),
        Some(StatusState::Blocked),
        "until the observer sees it"
    );
    observer
        .refresh(&app.service, std::time::Instant::now())
        .await;
    assert_eq!(state(0), None, "the block ended with the program");
    assert_eq!(state(1), Some(StatusState::Done), "done survives");
    for (sid, _) in &sessions {
        backend.close(sid.as_str()).expect("cleanup");
    }
}

/// control-api, Workspace and Screen Queries (Program status included), on
/// both channels: the session whose program reported `working` carries its
/// root record, a session without one carries none.
#[tokio::test]
async fn the_mux_state_replies_carry_the_root_program_status() {
    if !pty_available() {
        return;
    }
    let temp = tempfile::tempdir().expect("temporary directory");
    let host = ScriptedHost::new(temp.path());
    let app = state(ADDR, temp.path());
    let backend = app.client_state.dispatcher.backend();
    let mut sessions = Vec::new();
    for (screen, script) in [
        (
            "working",
            r"printf '\033]7501;state=working:app=claude-code\007'; read line",
        ),
        ("quiet", "read line"),
    ] {
        app.mux_store
            .dispatch(Op::CreateScreen {
                name: None,
                screen_id: Some(screen.into()),
                new_pane_id: Some(screen.into()),
                shell_profile_id: None,
            })
            .expect("screen");
        let sid = app
            .mux_store
            .session_id_for_pane(&PaneId::from(screen))
            .expect("session");
        let session = backend
            .create(command_opts(sid.as_str(), script))
            .await
            .expect("PTY");
        sessions.push((sid, session));
    }
    assert!(
        wait_until(WAIT, || sessions[0].1.program_status().record.is_some()).await,
        "the program reported"
    );
    let channels = [
        ("HTTP", router(&app, hub(&host, temp.path()))),
        (
            "control socket",
            crate::http::build_control_router(app.clone()),
        ),
    ];
    for (channel, router) in channels {
        let (code, workspace) = exchange(&router, api("GET", "/api/mux/state", None)).await;
        assert_eq!(code, StatusCode::OK, "{channel}: {workspace}");
        for (screen, (sid, _)) in ["working", "quiet"].into_iter().zip(&sessions) {
            let path = format!("/api/mux/screen/{screen}");
            let (code, reply) = exchange(&router, api("GET", &path, None)).await;
            assert_eq!(code, StatusCode::OK, "{channel}: {reply}");
            for record in [
                &workspace["screens"][screen]["sessions"][sid.as_str()],
                &reply["sessions"][sid.as_str()],
            ] {
                assert!(record.is_object(), "{channel}: {record}");
                let status = record.get("programStatus");
                if screen == "working" {
                    let status = status.expect("programStatus");
                    assert_eq!(status["state"], "working", "{channel}");
                    assert_eq!(status["app"], "claude-code", "{channel}");
                    assert!(status["updatedAt"].is_u64(), "{channel}: {status}");
                } else {
                    assert!(status.is_none(), "{channel}: {record}");
                }
            }
        }
    }
    for (sid, _) in &sessions {
        backend.close(sid.as_str()).expect("cleanup");
    }
}
