use super::*;
use crate::settings::{SettingsEdit, SettingsField as F, SettingsValue as V};

fn exchange(operation: &'static str, body: String) -> Exchange {
    Exchange {
        operation,
        status: 200,
        uncertain: false,
        body,
    }
}
fn appearance_exchanges() -> Vec<Exchange> {
    vec![
        exchange("home.get", home(1.0)),
        exchange(
            "home.metadata",
            splitty_core::config::settings_metadata().to_string(),
        ),
    ]
}
async fn loaded(api: Client, owner: Uuid) -> LocalServiceClient {
    let mut client = LocalServiceClient::new(api, owner, ColorScheme::Dark);
    client.dirty = [true, false, false];
    assert!(matches!(
        next(&mut client).await,
        LocalServiceUpdate::Appearance(Ok(_))
    ));
    client
}
fn edit(field: F) -> SettingsEdit {
    SettingsEdit::new(
        field,
        match field {
            F::ThemeMode => V::Text("auto".into()),
            F::Theme
            | F::ThemeDark
            | F::ThemeLight
            | F::TerminalTheme
            | F::TerminalThemeDark
            | F::TerminalThemeLight => V::Text("splitty-light".into()),
            F::FontSize => V::Number(18.0),
            F::FontFamily => V::Text("Owned Test Font".into()),
            F::Zoom => V::Number(1.2),
            F::ZoomScope => V::Text("window".into()),
            F::CloseOnExit => V::Text("never".into()),
            F::ScrollToBottom => V::Bool(false),
            F::DrawerPinned => V::Bool(false),
            F::ReconnectHosts => V::List(vec!["vm1".into()]),
            F::AutoForwardPorts => V::Bool(true),
            F::TabsShowSplit | F::TabsShowFloat | F::TabsShowMaximize => V::Bool(false),
            F::TabsCloseButton => V::Text("active".into()),
            F::TabsLocked => V::Bool(true),
            F::EditorFile | F::EditorFolder => V::Text("zed".into()),
            F::DrawerWidth => V::Number(240.0),
            F::NotificationEnabled
            | F::NotificationToasts
            | F::NotificationDesktop
            | F::NotificationProcessFinished
            | F::NotificationProcessExited
            | F::NotificationProcessCrashed
            | F::NotificationProgramStatus => V::Bool(false),
        },
    )
    .unwrap()
}
#[tokio::test]
async fn admission_is_local_bounded_field_exclusive_and_close_discards_unsent() {
    let mut exchanges = appearance_exchanges();
    exchanges.extend(notification_exchanges());
    let (api, task, dir) = scripted(exchanges).await;
    let owner = Uuid::new_v4();
    let mut client = loaded_domains(api, owner).await;
    assert!(client
        .enqueue_settings(Uuid::new_v4(), Uuid::new_v4(), edit(F::Theme))
        .is_err());
    for field in F::ALL {
        client
            .enqueue_settings(owner, Uuid::new_v4(), edit(field))
            .unwrap();
    }
    assert_eq!(client.settings.len(), MAX_QUEUED_SETTINGS);
    assert!(client.settings.len() <= MAX_QUEUED_SETTINGS);
    assert!(client
        .enqueue_settings(owner, Uuid::new_v4(), edit(F::Theme))
        .is_err());
    assert!(client
        .enqueue_zoom(owner, Uuid::new_v4(), ZoomAction::In)
        .is_err());
    assert!(client
        .enqueue_drawer_visibility(owner, Uuid::new_v4(), true)
        .is_err());
    client.finish().await.unwrap();
    task.await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn fifo_writes_release_fields_and_never_publish_optimistic_values() {
    let mut exchanges = appearance_exchanges();
    exchanges.push(exchange("home.update", home(1.0)));
    // Same-service refresh also schedules an independent notification read.
    exchanges.push(exchange("home.get", home(1.0)));
    exchanges.push(exchange("home.update", home(1.0)));
    let (api, task, dir) = scripted(exchanges).await;
    let owner = Uuid::new_v4();
    let mut client = loaded(api, owner).await;
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    client
        .enqueue_settings(owner, first, edit(F::FontSize))
        .unwrap();
    client
        .enqueue_settings(owner, second, edit(F::DrawerWidth))
        .unwrap();
    for (id, field) in [(first, F::FontSize), (second, F::DrawerWidth)] {
        loop {
            match next(&mut client).await {
                LocalServiceUpdate::SettingsWrite(result) => {
                    assert_eq!(result.request_id(), id);
                    assert_eq!(result.field(), field);
                    assert_eq!(result.outcome(), &SettingsWriteOutcome::Confirmed);
                    assert!(
                        result.generation() > client.last_appearance.as_ref().unwrap().generation(),
                        "ack cannot complete against a pre-write snapshot"
                    );
                    if id == first {
                        // The service notification may also be consumed after
                        // the acknowledgement. It must only supersede the read.
                        client.refresh_settings(owner).unwrap();
                        client.dirty[1] = false;
                        assert_eq!(result.outcome(), &SettingsWriteOutcome::Confirmed);
                    }
                    break;
                }
                LocalServiceUpdate::Appearance(Ok(snapshot)) => {
                    assert_eq!(snapshot.drawer().sidebar_width, 200)
                }
                _ => panic!("unexpected update"),
            }
        }
    }
    client.finish().await.unwrap();
    task.await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn rejection_and_uncertainty_reconcile_without_replay_even_when_metadata_fails() {
    for (status, uncertain, body, blocked) in [
        (400, false, "{}", false),
        (200, true, "{}", true),
        (200, false, "{", true),
    ] {
        let mut exchanges = appearance_exchanges();
        exchanges.push(Exchange {
            operation: "home.update",
            status,
            uncertain,
            body: body.into(),
        });
        exchanges.push(exchange("home.get", home(1.0)));
        exchanges.push(Exchange {
            operation: "home.metadata",
            status: 500,
            uncertain: false,
            body: "{}".into(),
        });
        let (api, task, dir) = scripted(exchanges).await;
        let owner = Uuid::new_v4();
        let id = Uuid::new_v4();
        let mut client = loaded(api, owner).await;
        client
            .enqueue_settings(owner, id, edit(F::FontSize))
            .unwrap();
        let LocalServiceUpdate::SettingsWrite(result) = next(&mut client).await else {
            panic!("write outcome missing")
        };
        assert_eq!(
            matches!(result.outcome(), SettingsWriteOutcome::Uncertain { .. }),
            blocked
        );
        assert_eq!(client.field_pending(F::FontSize), blocked);
        assert_eq!(
            client
                .last_appearance
                .as_ref()
                .unwrap()
                .settings()
                .font_size,
            0
        );
        if blocked {
            assert!(client
                .enqueue_settings(owner, Uuid::new_v4(), edit(F::FontSize))
                .is_err());
            let LocalServiceUpdate::SettingsReconciled(result) = next(&mut client).await else {
                panic!("reconciliation missing")
            };
            assert_eq!(result.requests(), &[(F::FontSize, id)]);
            assert_eq!(result.connection_id(), owner);
            assert!(!client.field_pending(F::FontSize));
        }
        let LocalServiceUpdate::Appearance(Ok(snapshot)) = next(&mut client).await else {
            panic!("refreshed appearance missing")
        };
        assert!(snapshot.metadata_error().is_some());
        drop(snapshot);
        client.finish().await.unwrap();
        task.await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
#[tokio::test]
async fn close_reports_active_uncertainty_and_discards_next_field() {
    for field in [F::FontSize, F::NotificationEnabled] {
        let (admitted, received) = tokio::sync::oneshot::channel();
        let (api, task, dir) = scripted_tail(
            appearance_exchanges()
                .into_iter()
                .chain(notification_exchanges())
                .collect(),
            Tail::Hold(admitted),
        )
        .await;
        let owner = Uuid::new_v4();
        let id = Uuid::new_v4();
        let mut client = loaded_domains(api, owner).await;
        client.enqueue_settings(owner, id, edit(field)).unwrap();
        client
            .enqueue_settings(owner, Uuid::new_v4(), edit(F::DrawerWidth))
            .unwrap();
        tokio::select! { result = client.next_update() => { drop(result); panic!("write should remain pending") }, _ = received => {} }
        let outcomes = client.finish_with_outcomes().await.unwrap();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].request_id(), id);
        assert!(matches!(
            outcomes[0].outcome(),
            SettingsWriteOutcome::Uncertain { .. }
        ));
        task.await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
#[test]
fn patches_are_single_fields_and_font_gap_is_not_admitted() {
    for field in F::ALL {
        let args = edit(field).args();
        let body = args["body"].as_object().unwrap();
        assert_eq!(body.len(), 1);
        assert_eq!(body.values().next().unwrap().as_object().unwrap().len(), 1);
    }
    for value in [1.0, 7.0, 8.5, 25.0, f64::NAN] {
        assert!(SettingsEdit::new(F::FontSize, V::Number(value)).is_err());
    }
}

#[tokio::test]
async fn missing_metadata_preserves_home_and_disables_dependent_writes() {
    for missing_metadata in [true, false] {
        let mut exchanges = appearance_exchanges();
        if missing_metadata {
            exchanges[1].status = 404;
            exchanges[1].body = "{}".into();
        }
        let (api, task, dir) = scripted(exchanges).await;
        let owner = Uuid::new_v4();
        let mut client = loaded(api, owner).await;
        let snapshot = client.last_appearance.as_ref().unwrap();
        assert_eq!(snapshot.settings().zoom, 1.0);
        assert_eq!(snapshot.metadata_error().is_some(), missing_metadata);
        for field in [F::Theme, F::TerminalThemeDark, F::FontSize] {
            assert_eq!(
                client
                    .enqueue_settings(owner, Uuid::new_v4(), edit(field))
                    .is_err(),
                missing_metadata
            );
        }
        client.finish().await.unwrap();
        task.await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test]
async fn named_terminal_palettes_pass_through_without_a_theme_request() {
    // The script is the whole conversation: a theme request would not match it.
    let mut exchanges = appearance_exchanges();
    let mut home: serde_json::Value = serde_json::from_str(&exchanges[0].body).unwrap();
    home["terminal"]["theme"] = "unknown-palette".into();
    home["terminal"]["themeDark"] = "splitty-dark".into();
    home["terminal"]["themeLight"] = "splitty-light".into();
    exchanges[0].body = home.to_string();
    let (api, task, dir) = scripted(exchanges).await;
    let owner = Uuid::new_v4();
    let client = loaded(api, owner).await;
    let snapshot = client.last_appearance.as_ref().unwrap();
    assert_eq!(snapshot.ui_theme(), "splitty-dark");
    assert_eq!(snapshot.terminal().theme(), "unknown-palette");
    assert_eq!(snapshot.terminal().theme_dark(), "splitty-dark");
    assert_eq!(snapshot.terminal().theme_light(), "splitty-light");
    assert_eq!(
        snapshot.value(F::TerminalTheme),
        Some(V::Text("unknown-palette".into()))
    );
    client.finish().await.unwrap();
    task.await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn uncertain_write_stays_uncertain_and_queued_old_read_cannot_publish() {
    let mut exchanges = appearance_exchanges();
    let mut lost_reply = exchange("home.update", home(1.0));
    lost_reply.uncertain = true;
    exchanges.push(lost_reply);
    exchanges.push(exchange("home.get", home(1.0)));
    exchanges.extend(appearance_exchanges());
    let mut latest = appearance_exchanges();
    latest[0].body = home(1.5);
    exchanges.push(exchange("home.get", home(1.0)));
    exchanges.extend(latest);
    let (api, task, dir) = scripted(exchanges).await;
    let owner = Uuid::new_v4();
    let id = Uuid::new_v4();
    let mut client = loaded(api, owner).await;
    client
        .enqueue_settings(owner, id, edit(F::FontSize))
        .unwrap();
    super::super::tests::begin(&mut client).await;
    client.refresh_settings(owner).unwrap();
    client.dirty[1] = false;
    let LocalServiceUpdate::SettingsWrite(result) = next(&mut client).await else {
        panic!("write outcome missing")
    };
    assert!(matches!(
        result.outcome(),
        SettingsWriteOutcome::Uncertain { .. }
    ));
    let LocalServiceUpdate::SettingsReconciled(reconciled) = next(&mut client).await else {
        panic!("read reconciliation missing")
    };
    assert_eq!(reconciled.requests(), &[(F::FontSize, id)]);
    // The old appearance is queued behind its reconciliation event. A fresh
    // notification before publication must invalidate this retained result too.
    client.refresh_settings(owner).unwrap();
    client.dirty[1] = false;
    let LocalServiceUpdate::Appearance(Ok(current)) = next(&mut client).await else {
        panic!("latest appearance missing")
    };
    assert!(current.generation() > reconciled.generation());
    assert_eq!(current.settings().zoom, 1.5);
    client.finish().await.unwrap();
    task.await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

// HomeUpdate uses a process-global config path. Run the in-process service in
// this test's own child and private directory without changing the parent env.
#[test]
fn own_settings_event_before_ack_requires_fresh_read_without_uncertainty() {
    const CHILD: &str = "SPLITTY_SETTINGS_ACK_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let directory =
            std::env::temp_dir().join(format!("splitty-settings-ack-{}", Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "local_service::zoom_tests::settings_tests::own_settings_event_before_ack_requires_fresh_read_without_uncertainty", "--nocapture"])
            .env(CHILD, "1")
            .env("SPLITTY_CONFIG_DIR", &directory)
            .output().unwrap();
        std::fs::remove_dir_all(directory).unwrap();
        assert!(
            result.status.success(),
            "child failed: {}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let (service, path) = crate::appearance::local::tests::fixture_writing_to(
                splitty_core::config::config_path(),
            );
            let connector = Connector::isolated();
            let session = connector
                .embedded(service.state.clone(), Role::Main)
                .await
                .unwrap();
            let mut session = crate::connection::Client::from_service(session);
            tokio::time::timeout(Duration::from_secs(5), session.next_event())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let owner = session.connection_id();
            let api = connector
                .embedded(service.state.clone(), Role::Main)
                .await
                .unwrap();
            let mut client = LocalServiceClient::new(api, owner, ColorScheme::Dark);
            let before = loop {
                if let LocalServiceUpdate::Appearance(Ok(snapshot)) = next(&mut client).await {
                    break snapshot;
                }
            };
            let id = Uuid::new_v4();
            client
                .enqueue_settings(
                    owner,
                    id,
                    SettingsEdit::new(F::FontSize, V::Number(20.0)).unwrap(),
                )
                .unwrap();
            super::super::tests::begin(&mut client).await;
            assert!(client.active_settings.is_some());
            // Deliberately consume the writer's actual service broadcast before
            // polling its retained API acknowledgement to completion.
            let event = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let event = session.next_event().await.unwrap().unwrap();
                    if event.json().is_some_and(|json| {
                        json["type"] == "settings.changed"
                            && json["settings"]["appearance"]["fontSize"] == 20
                    }) {
                        break event;
                    }
                }
            })
            .await
            .unwrap();
            assert_eq!(event.connection_id, owner);
            client.refresh_settings(event.connection_id).unwrap();
            drop(event);
            let LocalServiceUpdate::SettingsWrite(result) = next(&mut client).await else {
                panic!("write acknowledgement missing")
            };
            assert_eq!(result.request_id(), id);
            assert_eq!(result.outcome(), &SettingsWriteOutcome::Confirmed);
            assert!(result.generation() > before.generation());
            assert_eq!(result.generation(), client.generations[0]);
            assert_eq!(
                client
                    .last_appearance
                    .as_ref()
                    .unwrap()
                    .settings()
                    .font_size,
                before.settings().font_size,
                "acknowledgement must not publish reply settings"
            );
            let current = loop {
                match next(&mut client).await {
                    LocalServiceUpdate::Appearance(Ok(snapshot)) => break snapshot,
                    LocalServiceUpdate::SettingsReconciled(_) => {
                        panic!("acknowledged write is not an uncertain mutation")
                    }
                    _ => {}
                }
            };
            assert!(current.generation() >= result.generation());
            assert_eq!(current.settings().font_size, 20);
            assert!(client.uncertain_settings.is_empty());
            // The same real writer now changes only a notification field. Other
            // preferences survive, and an OS change cannot invalidate its ack.
            // Seed persisted preferences through the existing real API: HomeUpdate
            // reloads disk, so mutating only the fixture's in-memory config would
            // not establish the preservation precondition.
            let reply = client
                .client
                .as_mut()
                .unwrap()
                .request(
                    splitty_core::native_api::Operation::HomeUpdate,
                    serde_json::json!({"body":{"notifications":{"toasts":true,"desktop":true}}})
                        .as_object()
                        .unwrap()
                        .clone(),
                )
                .await
                .unwrap();
            assert_eq!(reply.status, 200);
            drop(reply);
            client.refresh_settings(owner).unwrap();
            let before = loop {
                if let LocalServiceUpdate::NotificationSettings(read) = raw_next(&mut client).await
                {
                    break read;
                }
            };
            assert!(before.result().unwrap().enabled);
            let id = Uuid::new_v4();
            client
                .enqueue_settings(owner, id, edit(F::NotificationEnabled))
                .unwrap();
            super::super::tests::begin(&mut client).await;
            let event = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let event = session.next_event().await.unwrap().unwrap();
                    if event.json().is_some_and(|json| {
                        json["type"] == "settings.changed"
                            && json["settings"]["notifications"]["enabled"] == false
                    }) {
                        break event;
                    }
                }
            })
            .await
            .unwrap();
            client.refresh_settings(event.connection_id).unwrap();
            client.set_scheme(owner, ColorScheme::Light).unwrap();
            drop(event);
            let LocalServiceUpdate::SettingsWrite(result) = raw_next(&mut client).await else {
                panic!("notification ack missing")
            };
            assert_eq!(result.request_id(), id);
            assert_eq!(result.outcome(), &SettingsWriteOutcome::Confirmed);
            assert_eq!(result.generation(), client.generations[2]);
            assert!(
                client
                    .last_notifications
                    .as_ref()
                    .unwrap()
                    .result()
                    .unwrap()
                    .enabled
            );
            let current = loop {
                if let LocalServiceUpdate::NotificationSettings(read) = raw_next(&mut client).await
                {
                    break read;
                }
            };
            assert!(current.generation() >= result.generation());
            let settings = current.result().unwrap();
            assert!(!settings.enabled);
            assert!(settings.toasts && settings.desktop);
            assert_eq!(
                settings.process_finished,
                before.result().unwrap().process_finished
            );
            assert_eq!(
                settings.process_exited,
                before.result().unwrap().process_exited
            );
            assert_eq!(
                settings.process_crashed,
                before.result().unwrap().process_crashed
            );
            assert_eq!(
                service.state.config.read().unwrap().appearance.font_size,
                20
            );

            client.finish().await.unwrap();
            session.finish().await.unwrap();
            service.close().await.unwrap();
            assert!(!path.exists());
        });
}

fn notification_exchanges() -> Vec<Exchange> {
    vec![
        exchange("home.get", home(1.0)),
        exchange(
            "home.metadata",
            splitty_core::config::settings_metadata().to_string(),
        ),
    ]
}
async fn raw_next(client: &mut LocalServiceClient) -> LocalServiceUpdate {
    tokio::time::timeout(Duration::from_secs(5), client.next_update())
        .await
        .unwrap()
}
async fn loaded_domains(api: Client, owner: Uuid) -> LocalServiceClient {
    let mut client = LocalServiceClient::new(api, owner, ColorScheme::Dark);
    client.dirty = [true, false, true];
    assert!(matches!(
        raw_next(&mut client).await,
        LocalServiceUpdate::Appearance(Ok(_))
    ));
    assert!(
        matches!(raw_next(&mut client).await, LocalServiceUpdate::NotificationSettings(read) if read.result().is_ok())
    );
    client
}

#[tokio::test]
async fn notifications_keep_values_and_edit_choices_when_appearance_or_reset_metadata_fails() {
    let mut metadata = splitty_core::config::settings_metadata();
    metadata["defaults"]["notifications"]["enabled"] = false.into();
    metadata["defaults"]["notifications"]["processFinished"] = false.into();
    metadata["defaults"]["notifications"]["processExited"] = true.into();
    metadata["defaults"]["notifications"]["processCrashed"] = false.into();
    for (status, body, expected) in [
        (200, metadata.to_string(), Some(false)),
        (404, "{}".into(), None),
        (200, "{".into(), None),
        (
            200,
            "{\"defaults\":{\"notifications\":{\"enabled\":\"false\"}},\"limits\":{}}".into(),
            None,
        ),
    ] {
        let mut exchanges = vec![Exchange {
            operation: "home.get",
            status: 500,
            uncertain: false,
            body: "{}".into(),
        }];
        exchanges.push(exchange("home.get", home(1.0)));
        exchanges.push(Exchange {
            operation: "home.metadata",
            status,
            uncertain: false,
            body,
        });
        let (api, task, dir) = scripted(exchanges).await;
        let budget = api.sender().context().budget.clone();
        let owner = Uuid::new_v4();
        let mut client = LocalServiceClient::new(api, owner, ColorScheme::Dark);
        client.dirty = [true, false, true];
        assert!(matches!(
            raw_next(&mut client).await,
            LocalServiceUpdate::Appearance(Err(_))
        ));
        let LocalServiceUpdate::NotificationSettings(read) = raw_next(&mut client).await else {
            panic!("notification read missing")
        };
        assert!(read.result().unwrap().enabled);
        assert_eq!(read.value(F::NotificationEnabled), Some(V::Bool(true)));
        assert_eq!(read.value(F::FontSize), None);
        assert_eq!(
            read.default_value(F::NotificationEnabled),
            expected.map(V::Bool)
        );
        assert_eq!(read.metadata_error().is_some(), expected.is_none());
        if expected.is_some() {
            assert_eq!(
                read.default_value(F::NotificationProcessExited),
                Some(V::Bool(true))
            );
        }
        assert!(client.last_appearance.is_none());
        assert!(client
            .enqueue_settings(Uuid::new_v4(), Uuid::new_v4(), edit(F::NotificationEnabled))
            .is_err());
        client
            .enqueue_settings(owner, Uuid::new_v4(), edit(F::NotificationEnabled))
            .unwrap();
        // Finish discards the unsent edit; the retained notification read carries no lease.
        client.finish().await.unwrap();
        task.await.unwrap();
        assert_eq!(budget.used(), 0);
        assert!(read.result().is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test]
async fn an_older_server_without_the_program_status_switch_reads_it_as_on() {
    let mut home: serde_json::Value = serde_json::from_str(&home(1.0)).unwrap();
    let mut metadata = splitty_core::config::settings_metadata();
    for notifications in [
        &mut home["notifications"],
        &mut metadata["defaults"]["notifications"],
    ] {
        assert!(notifications
            .as_object_mut()
            .unwrap()
            .remove("programStatus")
            .is_some());
    }
    metadata["defaults"]["notifications"]["processExited"] = true.into();
    let (api, task, dir) = scripted(vec![
        exchange("home.get", home.to_string()),
        exchange("home.metadata", metadata.to_string()),
    ])
    .await;
    let mut client = LocalServiceClient::new(api, Uuid::new_v4(), ColorScheme::Dark);
    client.dirty = [false, false, true];
    let LocalServiceUpdate::NotificationSettings(read) = raw_next(&mut client).await else {
        panic!("notification read missing")
    };
    assert!(read.result().unwrap().program_status);
    assert_eq!(
        read.value(F::NotificationProgramStatus),
        Some(V::Bool(true))
    );
    assert_eq!(
        read.default_value(F::NotificationProgramStatus),
        Some(V::Bool(true))
    );
    assert_eq!(
        read.default_value(F::NotificationProcessExited),
        Some(V::Bool(true)),
        "the other defaults are the server's"
    );
    assert!(read.metadata_error().is_none());
    client.finish().await.unwrap();
    task.await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn mixed_domain_fifo_acknowledges_against_only_its_own_projection() {
    let mut exchanges = appearance_exchanges();
    exchanges.extend(notification_exchanges());
    exchanges.push(exchange("home.update", home(1.0)));
    exchanges.extend(appearance_exchanges());
    // A notification reply need not contain any appearance/theme fields.
    let notification_home =
        serde_json::json!({"notifications": splitty_core::config::NotificationSettings::default()})
            .to_string();
    exchanges.push(exchange("home.update", notification_home));
    exchanges.extend(notification_exchanges());
    let (api, task, dir) = scripted(exchanges).await;
    let owner = Uuid::new_v4();
    let mut client = loaded_domains(api, owner).await;
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    client
        .enqueue_settings(owner, first, edit(F::FontSize))
        .unwrap();
    client
        .enqueue_settings(owner, second, edit(F::NotificationEnabled))
        .unwrap();
    assert!(client
        .enqueue_settings(owner, Uuid::new_v4(), edit(F::NotificationEnabled))
        .is_err());
    assert!(client
        .enqueue_settings(owner, second, edit(F::DrawerWidth))
        .is_err());
    assert!(client
        .enqueue_zoom(owner, Uuid::new_v4(), ZoomAction::In)
        .is_ok());
    // Exercise settings FIFO, without sending the independent queued zoom.
    client.zoom.clear();
    let LocalServiceUpdate::SettingsWrite(appearance) = raw_next(&mut client).await else {
        panic!("first write missing")
    };
    assert_eq!(appearance.request_id(), first);
    assert_eq!(appearance.outcome(), &SettingsWriteOutcome::Confirmed);
    assert_eq!(client.generations, [2, 1, 1]);
    assert!(matches!(
        raw_next(&mut client).await,
        LocalServiceUpdate::Appearance(Ok(_))
    ));
    let terminal_generation = client.terminal_generation;
    let LocalServiceUpdate::SettingsWrite(notification) = raw_next(&mut client).await else {
        panic!("second write missing")
    };
    assert_eq!(notification.request_id(), second);
    assert_eq!(notification.outcome(), &SettingsWriteOutcome::Confirmed);
    assert_eq!(client.generations, [2, 1, 2]);
    assert_eq!(client.terminal_generation, terminal_generation);
    assert!(client.last_notifications.as_ref().unwrap().generation() < notification.generation());
    let LocalServiceUpdate::NotificationSettings(current) = raw_next(&mut client).await else {
        panic!("readback missing")
    };
    assert_eq!(current.generation(), notification.generation());
    client.finish().await.unwrap();
    task.await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn notification_uncertainty_needs_its_own_valid_home_but_not_metadata() {
    let mut exchanges = appearance_exchanges();
    exchanges.extend(notification_exchanges());
    exchanges.push(exchange("home.update", "{}".into())); // malformed notification acknowledgement
    exchanges.push(exchange("home.get", home(1.0)));
    exchanges.push(exchange("home.get", "{}".into())); // invalid notification refresh
    exchanges.push(exchange("home.get", home(1.0)));
    exchanges.push(Exchange {
        operation: "home.metadata",
        status: 404,
        uncertain: false,
        body: "{}".into(),
    });
    let (api, task, dir) = scripted(exchanges).await;
    let owner = Uuid::new_v4();
    let id = Uuid::new_v4();
    let mut client = loaded_domains(api, owner).await;
    client
        .enqueue_settings(owner, id, edit(F::NotificationEnabled))
        .unwrap();
    let LocalServiceUpdate::SettingsWrite(outcome) = raw_next(&mut client).await else {
        panic!("outcome missing")
    };
    assert!(matches!(
        outcome.outcome(),
        SettingsWriteOutcome::Uncertain { .. }
    ));
    client.set_scheme(owner, ColorScheme::Light).unwrap();
    // A valid appearance read does not settle a notification write.
    assert!(matches!(
        raw_next(&mut client).await,
        LocalServiceUpdate::Appearance(Ok(_))
    ));
    assert!(client.field_pending(F::NotificationEnabled));
    let LocalServiceUpdate::NotificationSettings(failed) = raw_next(&mut client).await else {
        panic!("failed read missing")
    };
    assert!(failed.result().is_err());
    assert!(client.field_pending(F::NotificationEnabled));
    assert!(
        client
            .last_notifications
            .as_ref()
            .unwrap()
            .result()
            .unwrap()
            .enabled
    );
    assert!(client
        .enqueue_settings(owner, Uuid::new_v4(), edit(F::NotificationEnabled))
        .is_err());
    client.refresh_settings(owner).unwrap();
    client.dirty = [false, false, true];
    let LocalServiceUpdate::SettingsReconciled(reconciled) = raw_next(&mut client).await else {
        panic!("reconciliation missing")
    };
    assert_eq!(reconciled.domain(), SettingsDomain::Notifications);
    assert_eq!(reconciled.requests(), &[(F::NotificationEnabled, id)]);
    assert!(!client.field_pending(F::NotificationEnabled));
    // A newer OS-only appearance generation must not discard queued notification delivery.
    client.set_scheme(owner, ColorScheme::Dark).unwrap();
    let LocalServiceUpdate::NotificationSettings(current) = raw_next(&mut client).await else {
        panic!("queued notification missing")
    };
    assert_eq!(current.generation(), reconciled.generation());
    assert!(current.metadata_error().is_some());
    client
        .enqueue_settings(owner, Uuid::new_v4(), edit(F::NotificationEnabled))
        .unwrap();
    client.finish().await.unwrap();
    task.await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn superseded_notification_observation_and_queued_delivery_are_discarded() {
    let mut exchanges = notification_exchanges();
    exchanges.extend(notification_exchanges()); // superseded in-flight read
    exchanges.extend(notification_exchanges()); // accepted fresh read
    exchanges.push(Exchange {
        operation: "home.update",
        status: 200,
        uncertain: true,
        body: "{}".into(),
    });
    exchanges.extend(notification_exchanges()); // reconciliation and deferred result
    let mut changed = notification_exchanges();
    let mut home: serde_json::Value = serde_json::from_str(&changed[0].body).unwrap();
    home["notifications"]["enabled"] = false.into();
    changed[0].body = home.to_string();
    exchanges.extend(changed);
    let (api, task, dir) = scripted(exchanges).await;
    let owner = Uuid::new_v4();
    let mut client = LocalServiceClient::new(api, owner, ColorScheme::Dark);
    client.dirty = [false, false, true];
    assert!(matches!(
        raw_next(&mut client).await,
        LocalServiceUpdate::NotificationSettings(_)
    ));
    client.refresh_settings(owner).unwrap();
    client.dirty = [false, false, true];
    super::super::tests::begin(&mut client).await;
    client.refresh_settings(owner).unwrap();
    client.dirty = [false, false, true];
    let LocalServiceUpdate::NotificationSettings(read) = raw_next(&mut client).await else {
        panic!("fresh read missing")
    };
    assert_eq!(read.generation(), 3);
    client
        .enqueue_settings(owner, Uuid::new_v4(), edit(F::NotificationEnabled))
        .unwrap();
    assert!(matches!(
        raw_next(&mut client).await,
        LocalServiceUpdate::SettingsWrite(_)
    ));
    let LocalServiceUpdate::SettingsReconciled(reconciled) = raw_next(&mut client).await else {
        panic!("reconciled missing")
    };
    client.refresh_settings(owner).unwrap();
    client.dirty = [false, false, true];
    let LocalServiceUpdate::NotificationSettings(read) = raw_next(&mut client).await else {
        panic!("fresh deferred replacement missing")
    };
    assert!(read.generation() > reconciled.generation());
    assert!(!read.result().unwrap().enabled);
    client.finish().await.unwrap();
    task.await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
