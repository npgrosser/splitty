use crate::terminal::PtyGrid;
use crate::workspace::chrome::view_tests::with_view;
use crate::workspace::{Destination, Update, Workspace};
use gpui_kit::{Entity, Modifiers, MouseButton, VisualTestContext};
use serde_json::json;
use splitty_client_core::native_workspace::{PtySize, SizePolicy, WindowPresenceCell};
use splitty_client_core::workspace::WindowWorkspace;
use tokio::sync::mpsc;
use uuid::Uuid;

/// The window's local Screen "work" with a pane for each of `sessions`.
fn model(owner: Uuid, sessions: &[&str]) -> WindowWorkspace {
    let mut model = WindowWorkspace::new(owner);
    model.ui_test_local_screen_fixture("Work").unwrap();
    let source = model.lifecycle_token();
    let mut panes = serde_json::Map::new();
    let mut states = serde_json::Map::new();
    for session in sessions {
        let pane = format!("p-{session}");
        panes.insert(pane.clone(), json!({"id":pane,"sessionId":session}));
        states.insert((*session).into(), json!({"id":session,"status":"running"}));
    }
    let slots: Vec<_> = sessions
        .iter()
        .map(|session| json!({"type":"slot","id":format!("slot-{session}"),"paneIds":[format!("p-{session}")]}))
        .collect();
    let sizes = vec![1.0 / sessions.len() as f64; sessions.len()];
    model
        .source_mut(source.source_id())
        .unwrap()
        .apply_snapshot(
            owner,
            &json!({
                "type":"mux.screen_snapshot","screen":{
                    "id":"work","revision":2,
                    "root":{"type":"split","id":"split","axis":"horizontal","sizes":sizes,"children":slots},
                    "panes":panes,
                    "sessions":states
                }
            }),
        )
        .unwrap();
    model
}

fn deliver(window: &mut VisualTestContext, view: &Entity<Workspace>, update: Update) {
    let (_sender, mut receiver) = mpsc::channel(1);
    window.update(|window, cx| {
        view.update(cx, |workspace, cx| {
            workspace.poll(Some(update), &mut receiver, window, cx);
        });
        window.draw(cx).clear(cx);
    });
}

fn pty_of(
    window: &mut VisualTestContext,
    view: &Entity<Workspace>,
    session: &str,
) -> Option<PtyGrid> {
    window.update(|_, cx| {
        let workspace = view.read(cx);
        let terminal = workspace
            .panes
            .values()
            .find(|pane| pane.route.session_id() == session)
            .expect("a view of the session")
            .terminal
            .clone();
        terminal.read(cx).pty_size()
    })
}

#[test]
fn a_size_reaches_the_view_of_its_session_also_one_made_later() {
    with_view(|window, view| {
        let owner = Uuid::new_v4();
        let first = model(owner, &["s-a"]);
        let source = first.lifecycle_token();
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.destination = Destination::Screens;
                workspace.reconcile(first, window, cx);
            });
            window.draw(cx).clear(cx);
        });
        let size = |cols, rows, policy| PtySize { cols, rows, policy };
        deliver(
            window,
            &view,
            Update::PtySize {
                source,
                session: "s-a".into(),
                size: size(100, 30, SizePolicy::Active),
            },
        );
        assert_eq!(
            pty_of(window, &view, "s-a"),
            Some(PtyGrid {
                cols: 100,
                rows: 30,
                takeover: true
            })
        );
        // A session without a view yet, as a pane that streams before its
        // layout arrives, gets the size once its view is made.
        deliver(
            window,
            &view,
            Update::PtySize {
                source,
                session: "s-b".into(),
                size: size(90, 20, SizePolicy::Smallest),
            },
        );
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.reconcile(model(owner, &["s-a", "s-b"]), window, cx);
            });
            window.draw(cx).clear(cx);
        });
        assert_eq!(
            pty_of(window, &view, "s-b"),
            Some(PtyGrid {
                cols: 90,
                rows: 20,
                takeover: false
            })
        );
        assert_eq!(
            pty_of(window, &view, "s-a").map(|grid| grid.cols),
            Some(100)
        );
        // A size of another connection does not reach the view.
        let stranger = WindowWorkspace::new(Uuid::new_v4()).lifecycle_token();
        deliver(
            window,
            &view,
            Update::PtySize {
                source: stranger,
                session: "s-a".into(),
                size: size(40, 10, SizePolicy::Latest),
            },
        );
        assert_eq!(
            pty_of(window, &view, "s-a").map(|grid| grid.cols),
            Some(100)
        );
        // Sessions no route shows any more are forgotten.
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.reconcile(model(owner, &["s-a"]), window, cx);
                assert!(workspace
                    .pty_sizes
                    .keys()
                    .all(|(_, session)| session == "s-a"));
            });
        });
    });
}

#[test]
fn the_users_keys_and_clicks_reach_the_worker_without_a_command() {
    with_view(|window, view| {
        let owner = Uuid::new_v4();
        let model = model(owner, &["s-a"]);
        let presence = WindowPresenceCell::default();
        window.update(|window, cx| {
            window.activate_window();
            view.update(cx, |workspace, cx| {
                workspace.presence = presence.clone();
                workspace.destination = Destination::Screens;
                workspace.reconcile(model, window, cx);
            });
            window.draw(cx).clear(cx);
        });
        // Activating the window may have counted already; start afresh.
        presence.take_acted();
        window.simulate_keystrokes("a");
        assert!(presence.take_acted());
        assert!(!presence.take_acted());
        let center = window.update(|window, _| {
            let size = window.viewport_size();
            gpui_kit::point(size.width / 2., size.height / 2.)
        });
        window.simulate_mouse_down(center, MouseButton::Left, Modifiers::default());
        window.simulate_mouse_up(center, MouseButton::Left, Modifiers::default());
        assert!(presence.take_acted());
    });
}
