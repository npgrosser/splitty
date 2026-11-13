use crate::workspace::chrome::view_tests::with_view;
use crate::workspace::{Axis, Command, Destination, Update, WorkerCommand, Workspace};
use gpui_kit::{Entity, VisualTestContext};
use serde_json::{json, Value};
use splitty_client_core::workspace::{SourceToken, WindowWorkspace};
use splitty_client_core::Event;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use uuid::Uuid;

fn model(owner: Uuid) -> WindowWorkspace {
    let mut model = WindowWorkspace::new(owner);
    model.ui_test_local_screen_fixture("Work").unwrap();
    let source = model.lifecycle_token();
    model.source_mut(source.source_id()).unwrap().apply_snapshot(owner, &json!({
        "type":"mux.screen_snapshot","screen":{
            "id":"work","revision":2,
            "root":{"type":"split","id":"split","axis":"horizontal","sizes":[0.5,0.5],"children":[
                {"type":"slot","id":"left","paneIds":["p0"]},
                {"type":"slot","id":"right","paneIds":["p1"]}
            ]},
            "panes":{"p0":{"id":"p0","sessionId":"s-p0"},"p1":{"id":"p1","sessionId":"s-p1"}},
            "sessions":{"s-p0":{"id":"s-p0","status":"running"},"s-p1":{"id":"s-p1","status":"running"}}
        }
    })).unwrap();
    model
        .source_mut(source.source_id())
        .unwrap()
        .focus_pane("p0");
    model
}

/// Deliver a forwarded command and return what the window sent for it.
fn forward(
    rt: &Runtime,
    window: &mut VisualTestContext,
    view: &Entity<Workspace>,
    commands: &mut mpsc::Receiver<WorkerCommand>,
    source: SourceToken,
    connection: Uuid,
    command: Value,
) -> Vec<WorkerCommand> {
    while commands.try_recv().is_ok() {}
    let event = rt
        .block_on(Event::ui_test_json(
            connection,
            &json!({"type":"command","command":command}),
        ))
        .unwrap();
    let (_sender, mut receiver) = mpsc::channel(1);
    window.update(|window, cx| {
        view.update(cx, |workspace, cx| {
            workspace.poll(
                Some(Update::Event {
                    source,
                    event: Box::new(event),
                }),
                &mut receiver,
                window,
                cx,
            );
        });
    });
    std::iter::from_fn(|| commands.try_recv().ok())
        .filter(|sent| matches!(sent.kind(), Command::Split { .. } | Command::Select { .. }))
        .collect()
}

#[test]
fn forwarded_commands_act_on_a_pane_of_the_shown_screen_of_their_service() {
    let rt = Runtime::new().unwrap();
    with_view(|window, view| {
        let model = model(Uuid::new_v4());
        let owner = model.lifecycle_token();
        let screen = model.local_source().screen_key("work");
        let (sender, mut commands) = mpsc::channel(64);
        window.update(|window, cx| {
            window.activate_window();
            view.update(cx, |workspace, cx| {
                workspace.commands = sender;
                workspace.destination = Destination::Screens;
                workspace.reconcile(model, window, cx);
            });
            window.draw(cx).clear(cx);
        });
        let connection = owner.connection_id();
        let mut send = |window: &mut VisualTestContext, connection, command| {
            forward(
                &rt,
                window,
                &view,
                &mut commands,
                owner,
                connection,
                command,
            )
        };

        let sent = send(
            window,
            connection,
            json!({"type":"pane.split","paneId":"p1","direction":"below"}),
        );
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].source_token(), Some(owner));
        match sent[0].kind() {
            Command::Split {
                screen: target,
                pane,
                axis,
            } => {
                assert_eq!(target, &screen);
                assert_eq!(pane, "p1");
                assert_eq!(*axis, Axis::Vertical);
            }
            _ => unreachable!(),
        }

        // Without a pane, the focused one.
        let sent = send(
            window,
            connection,
            json!({"type":"pane.split","direction":"right"}),
        );
        assert!(matches!(
            sent[0].kind(),
            Command::Split { pane, axis: Axis::Horizontal, .. } if pane == "p0"
        ));

        let sent = send(
            window,
            connection,
            json!({"type":"pane.activate","paneId":"p1"}),
        );
        assert!(matches!(
            sent[0].kind(),
            Command::Select { pane, .. } if pane == "p1"
        ));

        for ignored in [
            json!({"type":"pane.split","paneId":"elsewhere","direction":"right"}),
            json!({"type":"pane.split","paneId":"p1","direction":"up"}),
            json!({"type":"pane.split","paneId":7,"direction":"right"}),
            json!({"type":"pane.explode","paneId":"p1"}),
        ] {
            assert!(send(window, connection, ignored).is_empty());
        }
        // An earlier connection of the service no longer commands the window.
        assert!(send(
            window,
            Uuid::new_v4(),
            json!({"type":"pane.split","paneId":"p1","direction":"right"}),
        )
        .is_empty());

        send(
            window,
            connection,
            json!({"type":"pane.close","paneId":"p1"}),
        );
        // The close first waits for its service's assessment.
        window.update(|_, cx| assert!(view.read(cx).assessing_close.is_some()));
    });
}
