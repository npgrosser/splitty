//! Regression tests for immutable terminal callback and pending resize authority.

#[cfg(not(target_os = "windows"))]
mod callbacks {
    use super::super::{retry_pending_resize, terminal_commands, PendingResize, ResizeCapture};
    use crate::terminal;
    use splitty_client_core::{
        native_workspace::{Command as WorkerCommand, CommandKind},
        workspace::{ScreenKey, SourceWorkspace, TerminalToken},
        EmbeddedService,
    };
    use std::sync::{Arc, Mutex};
    use tokio::{
        sync::mpsc,
        time::{timeout, Duration},
    };

    async fn ready_model(client: &mut splitty_client_core::Client) -> SourceWorkspace {
        let owner = client.connection_id();
        let mut model = SourceWorkspace::new(owner);
        model.set_catalog_supported(true);
        model.mark_catalog_dirty();
        model.flush_catalog(client).unwrap();
        timeout(Duration::from_secs(5), async {
            loop {
                let event = client.next_event().await.unwrap().unwrap();
                if event
                    .json()
                    .is_some_and(|json| json["type"] == "mux.workspace_snapshot")
                {
                    model.apply_catalog(owner, event.json().unwrap()).unwrap();
                    break;
                }
            }
        })
        .await
        .unwrap();
        model
    }

    async fn tokens() -> (
        TerminalToken,
        ScreenKey,
        TerminalToken,
        TerminalToken,
        ScreenKey,
        TerminalToken,
        ScreenKey,
    ) {
        let service = EmbeddedService::isolated();
        let mut first = service.connect().await.unwrap();
        let mut first_model = ready_model(&mut first).await;
        let first_ids = first_model.create_screen(&first, "First").unwrap();
        first_model.observe_spawn(first.connection_id(), &first_ids.pane_id, "session-old");
        let old = first_model.terminal_token(&first_ids.pane_id).unwrap();
        let old_screen = first_model.screen_key(&first_ids.screen_id);
        first_model.observe_spawn(first.connection_id(), &first_ids.pane_id, "session-new");
        let new_session = first_model.terminal_token(&first_ids.pane_id).unwrap();
        assert_ne!(old, new_session);

        let mut second = service.connect().await.unwrap();
        let mut second_model = ready_model(&mut second).await;
        let second_ids = second_model.create_screen(&second, "Second").unwrap();
        second_model.observe_spawn(
            second.connection_id(),
            &second_ids.pane_id,
            "session-second",
        );
        let new_source = second_model.terminal_token(&second_ids.pane_id).unwrap();
        let new_screen = second_model.screen_key(&second_ids.screen_id);
        assert_ne!(old.source_token(), new_source.source_token());

        first_model.reset_connection(second.connection_id());
        first_model.set_catalog_supported(true);
        first_model.mark_catalog_dirty();
        first_model.flush_catalog(&second).unwrap();
        timeout(Duration::from_secs(5), async {
            loop {
                let event = second.next_event().await.unwrap().unwrap();
                if event
                    .json()
                    .is_some_and(|json| json["type"] == "mux.workspace_snapshot")
                {
                    first_model
                        .apply_catalog(second.connection_id(), event.json().unwrap())
                        .unwrap();
                    break;
                }
            }
        })
        .await
        .unwrap();
        let replacement_ids = first_model.create_screen(&second, "Rebound").unwrap();
        first_model.observe_spawn(
            second.connection_id(),
            &replacement_ids.pane_id,
            "session-rebound",
        );
        let new_connection = first_model
            .terminal_token(&replacement_ids.pane_id)
            .unwrap();
        let replacement_screen = first_model.screen_key(&replacement_ids.screen_id);
        assert_eq!(
            old.source_token().source_id(),
            new_connection.source_token().source_id()
        );
        assert_ne!(
            old.source_token().connection_id(),
            new_connection.source_token().connection_id()
        );

        first.finish().await.unwrap();
        second.finish().await.unwrap();
        service.close().await.unwrap();
        (
            old,
            old_screen,
            new_session,
            new_source,
            new_screen,
            new_connection,
            replacement_screen,
        )
    }

    fn input(bytes: &[u8]) -> terminal::Command {
        terminal::Command::Input {
            generation: 0,
            bytes: bytes.to_vec(),
        }
    }

    fn expect_input(message: WorkerCommand, expected: &TerminalToken, bytes: &[u8]) {
        assert_eq!(message.source_token(), Some(expected.source_token()));
        match message.kind() {
            CommandKind::Input {
                route,
                bytes: actual,
            } => {
                assert_eq!(route, expected);
                assert_eq!(actual, bytes);
            }
            _ => panic!("expected captured terminal input"),
        }
    }

    fn expect_resize(message: WorkerCommand, expected: &ResizeCapture, cols: u16, rows: u16) {
        assert_eq!(message.source_token(), Some(expected.route.source_token()));
        match message.kind() {
            CommandKind::ResizePane {
                route,
                screen,
                route_generation,
                cols: actual_cols,
                rows: actual_rows,
            } => {
                assert_eq!(route, &expected.route);
                assert_eq!(screen, &expected.screen);
                assert_eq!(*route_generation, expected.route_generation);
                assert_eq!((*actual_cols, *actual_rows), (cols, rows));
            }
            _ => panic!("expected captured terminal resize"),
        }
    }

    #[tokio::test]
    async fn retained_callbacks_keep_original_source_connection_session_and_geometry() {
        let (old, screen, new_session, new_source, new_screen, new_connection, replacement_screen) =
            tokens().await;
        let (sender, mut receiver) = mpsc::channel(8);
        let old_capture = ResizeCapture {
            route: old.clone(),
            screen: screen.clone(),
            route_generation: 7,
        };
        let old_callback = terminal_commands(
            sender.clone(),
            old_capture.clone(),
            Arc::new(Mutex::new(None)),
        );
        let session_callback = terminal_commands(
            sender.clone(),
            ResizeCapture {
                route: new_session.clone(),
                screen: screen.clone(),
                route_generation: 8,
            },
            Arc::new(Mutex::new(None)),
        );
        let source_callback = terminal_commands(
            sender.clone(),
            ResizeCapture {
                route: new_source.clone(),
                screen: new_screen,
                route_generation: 9,
            },
            Arc::new(Mutex::new(None)),
        );
        let connection_callback = terminal_commands(
            sender.clone(),
            ResizeCapture {
                route: new_connection.clone(),
                screen: replacement_screen,
                route_generation: 10,
            },
            Arc::new(Mutex::new(None)),
        );
        old_callback(input(b"old route")).unwrap();
        session_callback(input(b"new session")).unwrap();
        source_callback(input(b"new source")).unwrap();
        connection_callback(input(b"new connection")).unwrap();
        expect_input(receiver.try_recv().unwrap(), &old, b"old route");
        expect_input(receiver.try_recv().unwrap(), &new_session, b"new session");
        expect_input(receiver.try_recv().unwrap(), &new_source, b"new source");
        expect_input(
            receiver.try_recv().unwrap(),
            &new_connection,
            b"new connection",
        );
        old_callback(terminal::Command::Resize(101, 31)).unwrap();
        expect_resize(receiver.try_recv().unwrap(), &old_capture, 101, 31);
        let new_geometry = ResizeCapture {
            route: old.clone(),
            screen,
            route_generation: 12,
        };
        let new_callback =
            terminal_commands(sender, new_geometry.clone(), Arc::new(Mutex::new(None)));
        old_callback(input(b"input survives geometry change")).unwrap();
        new_callback(terminal::Command::Resize(111, 41)).unwrap();
        expect_input(
            receiver.try_recv().unwrap(),
            &old,
            b"input survives geometry change",
        );
        expect_resize(receiver.try_recv().unwrap(), &new_geometry, 111, 41);
        assert!(old_callback(terminal::Command::Input {
            generation: 1,
            bytes: b"stale engine".to_vec()
        })
        .is_err());
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn saturated_queue_keeps_atomic_input_and_retries_only_original_resize_capture() {
        let (old, screen, _, new_source, new_screen, _, _) = tokens().await;
        let (sender, mut receiver) = mpsc::channel(2);
        let old_capture = ResizeCapture {
            route: old.clone(),
            screen,
            route_generation: 4,
        };
        let old_pending = Arc::new(Mutex::new(None));
        let old_callback =
            terminal_commands(sender.clone(), old_capture.clone(), old_pending.clone());
        let new_capture = ResizeCapture {
            route: new_source.clone(),
            screen: new_screen,
            route_generation: 8,
        };
        let new_pending = Arc::new(Mutex::new(None));
        let new_callback =
            terminal_commands(sender.clone(), new_capture.clone(), new_pending.clone());
        let paste = b"\x1b[200~one\ntwo\x1b[201~";
        old_callback(input(paste)).unwrap();
        new_callback(input(b"second source")).unwrap();
        assert!(old_callback(input(b"rejected")).is_err());
        assert!(old_callback(terminal::Command::Resize(120, 40)).is_err());
        assert!(old_callback(terminal::Command::Resize(130, 41)).is_err());
        assert_eq!(
            *old_pending.lock().unwrap(),
            Some(PendingResize {
                capture: old_capture.clone(),
                cols: 130,
                rows: 41
            })
        );
        assert_eq!(*new_pending.lock().unwrap(), None);
        assert!(new_callback(terminal::Command::Resize(140, 42)).is_err());
        assert_eq!(
            *new_pending.lock().unwrap(),
            Some(PendingResize {
                capture: new_capture.clone(),
                cols: 140,
                rows: 42
            })
        );
        expect_input(receiver.try_recv().unwrap(), &old, paste);
        expect_input(receiver.try_recv().unwrap(), &new_source, b"second source");
        retry_pending_resize(&sender, &new_capture, &old_pending).unwrap();
        assert!(
            receiver.try_recv().is_err(),
            "old pending Arc cannot upgrade to new binding"
        );
        retry_pending_resize(&sender, &new_capture, &new_pending).unwrap();
        expect_resize(receiver.try_recv().unwrap(), &new_capture, 140, 42);
        assert_eq!(*new_pending.lock().unwrap(), None);

        old_callback(input(b"fill")).unwrap();
        new_callback(input(b"fill second")).unwrap();
        assert!(old_callback(terminal::Command::Resize(150, 43)).is_err());
        expect_input(receiver.try_recv().unwrap(), &old, b"fill");
        expect_input(receiver.try_recv().unwrap(), &new_source, b"fill second");
        retry_pending_resize(&sender, &old_capture, &old_pending).unwrap();
        expect_resize(receiver.try_recv().unwrap(), &old_capture, 150, 43);
        assert_eq!(*old_pending.lock().unwrap(), None);
        drop(receiver);
        assert!(old_callback(input(b"closed")).is_err());
    }
}
