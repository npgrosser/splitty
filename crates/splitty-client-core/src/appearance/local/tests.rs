use super::*;
use crate::appearance::theme_name;
use crate::local_service::{LocalServiceClient, LocalServiceUpdate};
use splitty_transport::wire;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
use crate::connection::Client as SessionClient;
use serde_json::json;
use splitty_core::{
    config::{HomeConfig, ServerConfig},
    mux::{
        effect_runner::{EffectRunner, EnvContext},
        Store,
    },
    protocol::{dispatcher::Dispatcher, notify::NotificationBus, ServerEvent},
    pty::manager::Manager,
};
use splitty_server::{
    client_session::ClientState,
    hub::{Hub, SshConnector},
    native::client::Connector,
    service::{catalog::Role, runtime::Runtime, State},
};
use std::{
    future::Future,
    path::PathBuf,
    sync::{Arc, RwLock},
    task::Poll,
    time::Duration,
};
use tokio::sync::Notify;

// Build the real service entry points without runtime::build/config::load:
// production startup may persist home-config migrations. No test here writes
// settings, changes process-global paths or starts a terminal process.
pub(crate) fn fixture() -> (Runtime, PathBuf) {
    fixture_with_connector(Box::new(SshConnector))
}

pub(crate) fn fixture_with_connector(
    connector: Box<dyn splitty_server::hub::Connector>,
) -> (Runtime, PathBuf) {
    fixture_with(connector, Store::new())
}

/// A service that starts with `workspace`, as one that restored it.
pub(crate) fn fixture_with_workspace(
    workspace: splitty_core::mux::WorkspaceState,
) -> (Runtime, PathBuf) {
    fixture_with(Box::new(SshConnector), Store::with_state(workspace))
}

/// A service whose settings writes replace `config_path`, for a test that
/// runs in a child process with a private configuration directory.
pub(crate) fn fixture_writing_to(config_path: PathBuf) -> (Runtime, PathBuf) {
    fixture_with_path(Box::new(SshConnector), Store::new(), Some(config_path))
}

fn fixture_with(
    connector: Box<dyn splitty_server::hub::Connector>,
    store: Store,
) -> (Runtime, PathBuf) {
    fixture_with_path(connector, store, None)
}

fn fixture_with_path(
    connector: Box<dyn splitty_server::hub::Connector>,
    store: Store,
    config_path: Option<PathBuf>,
) -> (Runtime, PathBuf) {
    let path = std::env::temp_dir().join(format!("splitty-appearance-{}", Uuid::new_v4()));
    let mut config = HomeConfig::default();
    config.appearance.theme_mode = "auto".into();
    config.appearance.font_size = 18;
    config.appearance.font_family = "test monospace".into();
    let profiles = Arc::new(RwLock::new(
        splitty_core::shell_profiles::state_from_settings(&config.shells),
    ));
    let pty = Arc::new(Manager::with_terminals(
        profiles.clone(),
        Arc::new(splitty_core::pty::terminal::LocalTerminals),
    ));
    let store = Arc::new(store);
    let dispatcher = Arc::new(Dispatcher::new(pty.clone()));
    let effects = Arc::new(EffectRunner::new(pty.clone()));
    let hub = Arc::new(Hub::new(connector, path.clone(), None));
    let clients = Arc::new(
        ClientState::new(
            store.clone(),
            dispatcher,
            profiles.clone(),
            effects,
            EnvContext::default(),
            NotificationBus::default(),
            "embedded".into(),
        )
        .with_hub(hub.clone()),
    );
    let state = Arc::new(State {
        instance: splitty_server::service::new_instance(),
        owner_shutdown: Arc::new(Notify::new()),
        mux_store: store,
        client_state: clients,
        config: Arc::new(RwLock::new(config)),
        config_path: config_path.unwrap_or_else(|| path.join("config.toml")),
        shell_profiles: profiles,
        server_config: Arc::new(RwLock::new(ServerConfig::default())),
        template_store: Arc::new(splitty_core::template::FileStore::new(
            path.join("templates"),
        )),
        themes_dir: path.join("themes"),
        version: "test".into(),
        commit: String::new(),
        build_time: String::new(),
        server_mode: "embedded".into(),
        hub,
    });
    (Runtime { state, pty }, path)
}

async fn next_appearance(reader: &mut LocalServiceClient) -> io::Result<Arc<AppearanceSnapshot>> {
    loop {
        if let LocalServiceUpdate::Appearance(result) = reader.next_update().await {
            return result;
        }
    }
}

async fn snapshot(reader: &mut LocalServiceClient) -> Arc<AppearanceSnapshot> {
    tokio::time::timeout(Duration::from_secs(5), next_appearance(reader))
        .await
        .unwrap()
        .unwrap()
}

// Poll just once without running the service task. Cancellation of this outer
// receive must retain the in-flight API client and the request's allocation.
async fn begin_read(reader: &mut LocalServiceClient) {
    let mut next = Box::pin(next_appearance(reader));
    std::future::poll_fn(|cx| {
        assert!(next.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
}

#[test]
fn fixed_and_automatic_theme_names_use_only_local_preference() {
    let mut settings = AppearanceSettings::default();
    assert_eq!(theme_name(&settings, ColorScheme::Light), "splitty-dark");
    settings.theme_mode = "auto".into();
    assert_eq!(theme_name(&settings, ColorScheme::Light), "splitty-light");
    assert_eq!(theme_name(&settings, ColorScheme::Dark), "splitty-dark");
}

#[tokio::test]
async fn reads_keep_allocations_charged_and_session_events_separate() {
    let (service, path) = fixture();
    let connector = Connector::isolated();
    let session = connector
        .embedded(service.state.clone(), Role::Main)
        .await
        .unwrap();
    let mut session = SessionClient::from_service(session);
    // Establish the session stream before the API-only stream starts.
    tokio::time::timeout(Duration::from_secs(5), session.next_event())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let api = connector
        .embedded(service.state.clone(), Role::Main)
        .await
        .unwrap();
    let budget = api.sender().context().budget.clone();
    let mut reader = LocalServiceClient::new(api, session.connection_id(), ColorScheme::Light);
    service.state.client_state.broadcast_events(
        &[ServerEvent::settings_changed(
            json!({"appearance":{"theme":"test"}}),
        )],
        None,
    );
    let snapshot = snapshot(&mut reader).await;
    assert_eq!(snapshot.settings().font_size, 18);
    assert_eq!(snapshot.settings().font_family, "test monospace");
    assert!(snapshot.drawer().pinned);
    assert_eq!(snapshot.drawer().sidebar_width, 200);
    assert_eq!(snapshot.os_scheme(), ColorScheme::Light);
    assert_eq!(snapshot.ui_theme(), "splitty-light");
    assert_eq!(snapshot.connection_id(), session.connection_id());
    let event = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = session.next_event().await.unwrap().unwrap();
            if event
                .json()
                .is_some_and(|v| v["type"] == "settings.changed")
            {
                break event;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(event.connection_id, session.connection_id());
    drop(event);
    reader.finish().await.unwrap();
    assert!(
        budget.used() > 0,
        "presentation still owns decoded appearance"
    );
    drop(snapshot);
    assert_eq!(budget.used(), 0, "both wire and decoded leases released");
    session.finish().await.unwrap();
    service.close().await.unwrap();
    assert!(
        !path.exists(),
        "read fixture never creates a settings directory"
    );
}

#[tokio::test]
async fn cancelled_poll_retains_read_and_refresh_discards_superseded_generation() {
    let (service, path) = fixture();
    service.state.config.write().unwrap().drawer.sidebar_width = 260;
    let api = Connector::isolated()
        .embedded(service.state.clone(), Role::Main)
        .await
        .unwrap();
    let owner = Uuid::new_v4();
    let mut reader = LocalServiceClient::new(api, owner, ColorScheme::Dark);
    begin_read(&mut reader).await;
    assert!(reader
        .set_scheme(Uuid::new_v4(), ColorScheme::Light)
        .is_err());
    reader.refresh_settings(owner).unwrap();
    reader.set_scheme(owner, ColorScheme::Light).unwrap();
    service.state.config.write().unwrap().drawer.sidebar_width = 340;
    let result = snapshot(&mut reader).await;
    assert_eq!(result.generation(), 3);
    assert_eq!(result.drawer().sidebar_width, 340);
    assert_eq!(result.os_scheme(), ColorScheme::Light);
    drop(result);
    reader.finish().await.unwrap();
    service.close().await.unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn finish_cancels_pending_request_and_releases_the_owned_stream() {
    let (service, path) = fixture();
    let api = Connector::isolated()
        .embedded(service.state.clone(), Role::Main)
        .await
        .unwrap();
    let budget = api.sender().context().budget.clone();
    let mut reader = LocalServiceClient::new(api, Uuid::new_v4(), ColorScheme::Dark);
    begin_read(&mut reader).await;
    tokio::time::timeout(Duration::from_secs(5), reader.finish())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(budget.used(), 0);
    service.close().await.unwrap();
    assert!(!path.exists());
}

#[test]
fn malformed_or_oversized_reply_is_rejected_without_losing_budget() {
    use crate::local_service::decode_with_budget;
    use splitty_transport::budget::{ConnectionBudget, ProcessBudget};
    // Exhaust only this decoder's private budget, not the process-wide admission
    // pool shared by concurrently running real-service tests.
    let budget = ConnectionBudget::new(
        ProcessBudget::new(wire::CONNECTION_BYTES, wire::CONNECTION_CONTROL_BYTES),
        wire::CONNECTION_BYTES,
        wire::CONNECTION_CONTROL_BYTES,
    );
    for (status, content_type, body) in [
        (500, wire::ContentType::Json, "{}".into()),
        (200, wire::ContentType::Text, "{}".into()),
        (200, wire::ContentType::Json, "{bad".into()),
        (
            200,
            wire::ContentType::Json,
            "x".repeat(MAX_RESPONSE_BYTES + 1),
        ),
    ] {
        let reply = wire::Reply {
            id: 1,
            status,
            content_type,
            body,
            uncertain: false,
        };
        assert!(decode_with_budget::<HomeAppearance>(&budget, &reply).is_err());
    }
    let reply = wire::Reply {
        id: 1,
        status: 200,
        content_type: wire::ContentType::Json,
        body: serde_json::to_string(&json!({"appearance": AppearanceSettings::default(), "drawer": splitty_core::config::DrawerSettings::default()})).unwrap(),
        uncertain: false,
    };
    let available = wire::CONNECTION_BYTES - wire::CONNECTION_CONTROL_BYTES - budget.data.used();
    let held = budget.reserve(available, false).unwrap();
    assert_eq!(
        decode_with_budget::<HomeAppearance>(&budget, &reply)
            .err()
            .unwrap()
            .kind(),
        io::ErrorKind::OutOfMemory
    );
    drop(held);
    assert_eq!(budget.used(), 0);
}

#[tokio::test]
async fn drawer_snapshot_restores_service_values_without_writing() {
    let (service, path) = fixture();
    {
        let mut config = service.state.config.write().unwrap();
        config.drawer.pinned = false;
        config.drawer.sidebar_width = 280;
        config.appearance.font_size = 19;
    }
    let api = Connector::isolated()
        .embedded(service.state.clone(), Role::Main)
        .await
        .unwrap();
    let owner = Uuid::new_v4();
    let mut reader = LocalServiceClient::new(api, owner, ColorScheme::Dark);
    let first = snapshot(&mut reader).await;
    assert!(!first.drawer().pinned);
    assert_eq!(first.drawer().sidebar_width, 280);
    assert_eq!(first.settings().font_size, 19);
    let first_generation = first.generation();
    drop(first);
    service.state.config.write().unwrap().drawer.sidebar_width = 320;
    reader.refresh_settings(owner).unwrap();
    let later = snapshot(&mut reader).await;
    assert!(later.generation() > first_generation);
    assert_eq!(later.drawer().sidebar_width, 320);
    assert_eq!(later.settings().font_size, 19);
    assert!(
        !path.exists(),
        "reading presentation must not save settings"
    );
    drop(later);
    reader.finish().await.unwrap();
    service.close().await.unwrap();
}

#[tokio::test]
async fn read_error_is_reported_once_and_explicit_refresh_recovers() {
    let (service, path) = fixture();
    service.state.config.write().unwrap().appearance.font_family = "x".repeat(4097);
    let api = Connector::isolated()
        .embedded(service.state.clone(), Role::Main)
        .await
        .unwrap();
    let owner = Uuid::new_v4();
    let mut reader = LocalServiceClient::new(api, owner, ColorScheme::Dark);
    let error = tokio::time::timeout(Duration::from_secs(5), next_appearance(&mut reader))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    begin_read(&mut reader).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(20), next_appearance(&mut reader))
            .await
            .is_err(),
        "no automatic failed-read retry loop"
    );
    service.state.config.write().unwrap().appearance.font_family = "recovered".into();
    reader.refresh_settings(owner).unwrap();
    let result = snapshot(&mut reader).await;
    assert_eq!(result.generation(), 2);
    assert_eq!(result.settings().font_family, "recovered");
    drop(result);
    reader.finish().await.unwrap();
    service.close().await.unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn editor_reads_service_defaults_and_limits_without_writes() {
    use crate::settings::{SettingsField as F, SettingsValue as V};
    let (service, path) = fixture();
    let api = Connector::isolated()
        .embedded(service.state.clone(), Role::Main)
        .await
        .unwrap();
    let owner = Uuid::new_v4();
    let mut reader = LocalServiceClient::new(api, owner, ColorScheme::Dark);
    let current = snapshot(&mut reader).await;
    assert_eq!(current.connection_id(), owner);
    assert_eq!(current.value(F::FontSize), Some(V::Number(18.0)));
    assert_eq!(current.value(F::NotificationEnabled), None);
    assert!(current
        .validate_edit(
            &crate::settings::SettingsEdit::new(F::NotificationEnabled, V::Bool(false)).unwrap()
        )
        .is_err());
    let metadata = current.metadata().unwrap();
    assert_eq!(metadata.default_value(F::DrawerPinned), Some(V::Bool(true)));
    assert_eq!(
        metadata.default_value(F::TerminalTheme),
        Some(V::Text("default".into()))
    );
    assert_eq!(
        metadata.choices(F::CloseOnExit).unwrap(),
        &["always", "never", "graceful"]
    );
    let size = metadata.limits(F::FontSize).unwrap();
    assert!(size.values().contains(&0.0));
    assert!(size.values().contains(&8.0));
    assert!(!size.values().contains(&7.0));
    for field in [F::TerminalThemeDark, F::TerminalThemeLight] {
        assert_eq!(
            metadata.default_value(field),
            Some(V::Text("default".into()))
        );
        assert_eq!(current.value(field), Some(V::Text("default".into())));
    }
    // A theme name is free text here: the application's catalog offers the choices.
    for field in [F::ThemeDark, F::TerminalTheme, F::TerminalThemeLight] {
        assert!(current
            .validate_edit(
                &crate::settings::SettingsEdit::new(field, V::Text("any-theme".into())).unwrap()
            )
            .is_ok());
    }
    assert!(current.metadata_error().is_none());
    reader.finish().await.unwrap();
    service.close().await.unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn terminal_palette_names_pass_through_unresolved() {
    let (service, path) = fixture();
    service.state.config.write().unwrap().terminal.theme = "splitty-light".into();
    let api = Connector::isolated()
        .embedded(service.state.clone(), Role::Main)
        .await
        .unwrap();
    let owner = Uuid::new_v4();
    let mut reader = LocalServiceClient::new(api, owner, ColorScheme::Dark);
    let first = snapshot(&mut reader).await;
    assert_eq!(first.ui_theme(), "splitty-dark");
    assert_eq!(first.terminal().theme(), "splitty-light");
    assert_eq!(first.terminal().theme_dark(), "default");
    assert_eq!(first.terminal().theme_light(), "default");
    service.state.config.write().unwrap().terminal.theme = "missing-palette".into();
    reader.refresh_settings(owner).unwrap();
    let missing = snapshot(&mut reader).await;
    assert_eq!(missing.terminal().theme(), "missing-palette");
    assert_eq!(missing.ui_theme(), "splitty-dark");
    assert!(missing.terminal_generation() > first.terminal_generation());
    {
        let mut config = service.state.config.write().unwrap();
        config.terminal.theme = "default".into();
        config.terminal.theme_dark = "night-palette".into();
        config.terminal.theme_light = "day-palette".into();
    }
    reader.refresh_settings(owner).unwrap();
    let pair = snapshot(&mut reader).await;
    assert_eq!(pair.terminal().theme(), "default");
    assert_eq!(pair.terminal().theme_dark(), "night-palette");
    assert_eq!(pair.terminal().theme_light(), "day-palette");
    reader.set_scheme(owner, ColorScheme::Light).unwrap();
    let light = snapshot(&mut reader).await;
    assert_eq!(light.os_scheme(), ColorScheme::Light);
    assert_eq!(light.ui_theme(), "splitty-light");
    assert_eq!(light.terminal().theme_dark(), "night-palette");
    assert_eq!(light.terminal().theme_light(), "day-palette");
    assert!(light.terminal_generation() > pair.terminal_generation());
    reader.finish().await.unwrap();
    service.close().await.unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn superseded_read_does_not_replace_current_palette_name() {
    let (service, path) = fixture();
    let api = Connector::isolated()
        .embedded(service.state.clone(), Role::Main)
        .await
        .unwrap();
    let owner = Uuid::new_v4();
    let mut reader = LocalServiceClient::new(api, owner, ColorScheme::Dark);
    let first = snapshot(&mut reader).await;
    service.state.config.write().unwrap().terminal.theme = "splitty-light".into();
    reader.refresh_settings(owner).unwrap();
    begin_read(&mut reader).await;
    service.state.config.write().unwrap().terminal.theme = "splitty-dark".into();
    reader.refresh_settings(owner).unwrap();
    let latest = snapshot(&mut reader).await;
    assert_eq!(latest.generation(), first.generation() + 2);
    assert_eq!(
        latest.terminal_generation(),
        first.terminal_generation() + 2
    );
    assert_eq!(latest.terminal().theme(), "splitty-dark");
    reader.finish().await.unwrap();
    service.close().await.unwrap();
    assert!(!path.exists());
}
