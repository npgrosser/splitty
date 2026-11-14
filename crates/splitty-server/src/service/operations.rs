//! Shared operation implementations. Browser/native adapters only decode and authorize.
use super::reply::{IntoReply, Payload, Reply};
use super::State as ServiceState;
use crate::hub::Hub;
use http::StatusCode;
use serde::Deserialize;
use serde_json::{json, Value};
use splitty_core::ids::*;
use splitty_core::mux::{self, Direction, ScreenOp};
use splitty_core::native_api::hosts as native_hosts;
use splitty_core::template::TemplateStore;
use std::collections::HashMap;

pub(crate) fn remember_screens(hub: &Hub, document: Value) -> Reply {
    remembered_reply(hub.remember_screens(document))
}

/// One host's remembered Screens have the error contract of the whole list.
pub(crate) fn remember_host_screens(hub: &Hub, document: Value) -> Reply {
    remembered_reply(hub.remember_host_screens(document))
}

fn remembered_reply(result: Result<crate::hub::Remembered, crate::hub::RememberError>) -> Reply {
    use crate::hub::RememberError;

    match result {
        Ok(remembered) => Payload(remembered).into_reply(),
        Err(error @ RememberError::Invalid(_)) => (
            StatusCode::BAD_REQUEST,
            Payload(json!({"error": error.to_string()})),
        )
            .into_reply(),
        Err(error @ RememberError::Write(_)) => {
            tracing::warn!("remote screens: {error}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Payload(json!({"error": error.to_string()})),
            )
                .into_reply()
        }
    }
}

/// A catalog update has the same error contract as remembered screens.
pub(crate) fn update_catalog(hub: &Hub, document: Value) -> Reply {
    use crate::hub::CatalogError;

    match hub.update_catalog(document) {
        Ok(host) => Payload(host).into_reply(),
        Err(error @ CatalogError::Invalid(_)) => (
            StatusCode::BAD_REQUEST,
            Payload(json!({"error": error.to_string()})),
        )
            .into_reply(),
        Err(error @ CatalogError::Hidden) => (
            StatusCode::CONFLICT,
            Payload(json!({"error": error.to_string()})),
        )
            .into_reply(),
        Err(error @ CatalogError::Write(_)) => {
            tracing::warn!("remote catalog: {error}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Payload(json!({"error": error.to_string()})),
            )
                .into_reply()
        }
    }
}

/// Host metadata replies are the shared reply types
/// (`native_api::hosts`), not the hub's file records.
pub(crate) fn host_order_reply(order: crate::hub::HostOrder) -> native_hosts::HostOrder {
    native_hosts::HostOrder {
        version: order.version,
        order: order.order,
    }
}

fn host_preference_reply(row: crate::hub::HostPreference) -> native_hosts::HostPreference {
    native_hosts::HostPreference {
        target: row.target,
        alias: row.alias,
        hidden: row.hidden,
    }
}

pub(crate) fn host_preferences_reply(
    all: splitty_remote::hosts::HostPreferences,
) -> native_hosts::HostPreferences {
    native_hosts::HostPreferences {
        version: all.version,
        hosts: all.hosts.into_iter().map(host_preference_reply).collect(),
    }
}

pub(crate) fn host_preference_response(
    result: Result<Option<crate::hub::HostPreference>, crate::hub::PreferenceError>,
) -> Reply {
    use crate::hub::PreferenceError;
    match result {
        Ok(Some(row)) => Payload(host_preference_reply(row)).into_reply(),
        Ok(None) => StatusCode::NO_CONTENT.into_reply(),
        Err(error) => {
            let status = match &error {
                PreferenceError::Invalid(_) => StatusCode::BAD_REQUEST,
                PreferenceError::Write(_) => {
                    tracing::warn!("remote host metadata: {error}");
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            };
            (status, Payload(json!({"error": error.to_string()}))).into_reply()
        }
    }
}

/// The body of `POST` and `DELETE /api/remote/forwards`.
#[derive(Deserialize)]
struct ForwardRequest {
    target: String,
    port: i64,
}

#[derive(Deserialize)]
struct OpenForwardRequest {
    target: String,
    port: i64,
    #[serde(default, deserialize_with = "supplied")]
    local: Option<i64>,
    #[serde(default)]
    automatic: bool,
    #[serde(default, rename = "preserveRestore")]
    preserve_restore: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveForwardRequest {
    target: String,
    port: i64,
    #[serde(default, deserialize_with = "supplied")]
    local: Option<i64>,
    #[serde(default, deserialize_with = "supplied")]
    label: Option<String>,
    #[serde(default, deserialize_with = "supplied")]
    enabled: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ForgetForwardRequest {
    target: String,
    port: i64,
}

/// Optional means omitted, not null. Supplied fields retain their types.
pub(crate) fn supplied<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

pub(crate) async fn save_forward(hub: &Hub, body: Value) -> Reply {
    let request: SaveForwardRequest = match serde_json::from_value(body) {
        Ok(request) => request,
        Err(error) => return refuse_forward_request(&error),
    };
    match hub
        .save_forward_with_restore(
            &request.target,
            request.port,
            request.local,
            request.label,
            request.enabled,
        )
        .await
    {
        Ok(row) => Payload(row).into_reply(),
        Err(error) => forward_failed(&request.target, error),
    }
}

pub(crate) async fn forget_forward(hub: &Hub, body: Value) -> Reply {
    let request: ForgetForwardRequest = match serde_json::from_value(body) {
        Ok(request) => request,
        Err(error) => return refuse_forward_request(&error),
    };
    match hub.forget_forward(&request.target, request.port).await {
        Ok(()) => StatusCode::NO_CONTENT.into_reply(),
        Err(error) => forward_failed(&request.target, error),
    }
}

/// The 400 for a body that is no [`ForwardRequest`]. The body is taken as any
/// JSON first, so a wrong shape is a 400 with its reason, not the extractor's
/// 422.
pub(crate) fn refuse_forward_request(error: &serde_json::Error) -> Reply {
    (
        StatusCode::BAD_REQUEST,
        Payload(json!({"error": format!("expected {{\"target\", \"port\"}}: {error}")})),
    )
        .into_reply()
}

/// `POST /api/remote/forwards`: 200 with the forward once it is verified, or
/// the one that exists.
pub(crate) async fn open_forward(hub: &Hub, body: Value) -> Reply {
    let request: OpenForwardRequest = match serde_json::from_value(body) {
        Ok(request) => request,
        Err(error) => return refuse_forward_request(&error),
    };
    match hub
        .open_forward_preserving_restore(
            &request.target,
            request.port,
            request.local,
            request.automatic,
            request.preserve_restore,
        )
        .await
    {
        Ok(forward) => Payload(forward).into_reply(),
        Err(error) => forward_failed(&request.target, error),
    }
}

/// `DELETE /api/remote/forwards`: 204 once the forward's `ssh` has ended.
pub(crate) async fn stop_forward(hub: &Hub, body: Value) -> Reply {
    let request: ForwardRequest = match serde_json::from_value(body) {
        Ok(request) => request,
        Err(error) => return refuse_forward_request(&error),
    };
    match hub.stop_forward(&request.target, request.port).await {
        Ok(()) => StatusCode::NO_CONTENT.into_reply(),
        Err(error) => forward_failed(&request.target, error),
    }
}

/// The body of `POST /api/remote/reverse-forwards`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenReverseRequest {
    target: String,
    port: i64,
    local: i64,
}

/// The body of `DELETE` on both reverse-forward APIs.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReverseKey {
    target: String,
    port: i64,
}

/// The body of `PUT /api/remote/reverse-forward-preferences`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveReverseRequest {
    target: String,
    port: i64,
    #[serde(default, deserialize_with = "supplied")]
    local: Option<i64>,
}

/// The 400 for a reverse-forward body without the expected fields.
pub(crate) fn refuse_reverse_request(expected: &str, error: &serde_json::Error) -> Reply {
    (
        StatusCode::BAD_REQUEST,
        Payload(json!({"error": format!("expected {expected}: {error}")})),
    )
        .into_reply()
}

/// `POST /api/remote/reverse-forwards`: 200 with the reverse forward once the
/// host's relay listens, or the one that exists.
pub(crate) async fn open_reverse(hub: &Hub, body: Value) -> Reply {
    let request: OpenReverseRequest = match serde_json::from_value(body) {
        Ok(request) => request,
        Err(error) => return refuse_reverse_request(r#"{"target", "port", "local"}"#, &error),
    };
    match hub
        .open_reverse(&request.target, request.port, request.local)
        .await
    {
        Ok(reverse) => Payload(reverse).into_reply(),
        Err(error) => forward_failed(&request.target, error),
    }
}

/// `DELETE /api/remote/reverse-forwards`: 204 once the reverse forward's
/// `ssh` has ended.
pub(crate) async fn stop_reverse(hub: &Hub, body: Value) -> Reply {
    let request: ReverseKey = match serde_json::from_value(body) {
        Ok(request) => request,
        Err(error) => return refuse_reverse_request(r#"{"target", "port"}"#, &error),
    };
    match hub.stop_reverse(&request.target, request.port).await {
        Ok(()) => StatusCode::NO_CONTENT.into_reply(),
        Err(error) => forward_failed(&request.target, error),
    }
}

pub(crate) async fn save_reverse(hub: &Hub, body: Value) -> Reply {
    let request: SaveReverseRequest = match serde_json::from_value(body) {
        Ok(request) => request,
        Err(error) => return refuse_reverse_request(r#"{"target", "port", "local"?}"#, &error),
    };
    match hub
        .save_reverse(&request.target, request.port, request.local)
        .await
    {
        Ok(row) => Payload(row).into_reply(),
        Err(error) => forward_failed(&request.target, error),
    }
}

pub(crate) async fn forget_reverse(hub: &Hub, body: Value) -> Reply {
    let request: ReverseKey = match serde_json::from_value(body) {
        Ok(request) => request,
        Err(error) => return refuse_reverse_request(r#"{"target", "port"}"#, &error),
    };
    match hub.forget_reverse(&request.target, request.port).await {
        Ok(()) => StatusCode::NO_CONTENT.into_reply(),
        Err(error) => forward_failed(&request.target, error),
    }
}

/// A failed forward request: 400 for an invalid target or port, 404 for a
/// forward that does not exist, 409 for a host at its limit, and for a
/// forward that could not be opened 502 (500 for a local problem) with the
/// code and hint of the hub's errors.
pub(crate) fn forward_failed(target: &str, error: crate::hub::ForwardError) -> Reply {
    use crate::hub::{Code, ForwardError};

    if matches!(&error, ForwardError::Failed(error) if error.busy()) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Payload(json!({"error":error.to_string(), "code":"busy"})),
        )
            .into_reply();
    }
    let status = match &error {
        ForwardError::Target(_) | ForwardError::Port(_) | ForwardError::Invalid(_) => {
            StatusCode::BAD_REQUEST
        }
        ForwardError::NotFound { .. } => StatusCode::NOT_FOUND,
        ForwardError::Limit { .. }
        | ForwardError::PreferenceLimit
        | ForwardError::LocalConflict { .. }
        | ForwardError::ActiveConflict { .. }
        | ForwardError::Paused
        | ForwardError::HostPortConflict { .. }
        | ForwardError::LocalPortReserved { .. } => StatusCode::CONFLICT,
        ForwardError::Storage(_) | ForwardError::OperationEnded => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
        ForwardError::Failed(error) if error.unsupported_environment() => {
            StatusCode::NOT_IMPLEMENTED
        }
        ForwardError::Failed(_) => StatusCode::BAD_GATEWAY,
    };
    let ForwardError::Failed(failed) = &error else {
        let mut body = json!({"error": error.to_string()});
        if let Some(code) = error.code() {
            body["code"] = json!(code);
        }
        return (status, Payload(body)).into_reply();
    };
    let failure = failed.failure(target);
    tracing::info!(
        "remote screen hub: a forward to {target:?} failed ({}): {}",
        failure.code.as_str(),
        failure.message
    );
    let status = if failure.code == Code::Internal {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        status
    };
    let mut body = json!({"error": failure.message, "code": failure.code});
    if let Some(hint) = failure.hint {
        body["hint"] = json!(hint);
    }
    (status, Payload(body)).into_reply()
}

// ── Theme ───────────────────────────────────────────────────────────────────

pub(crate) async fn get_resolved_theme(
    state: &ServiceState,
    params: HashMap<String, String>,
) -> Payload<Value> {
    let (name, [named, dark, light]) = {
        let config = state.config.read().unwrap();
        let name = params
            .get("name")
            .cloned()
            .unwrap_or_else(|| config.appearance.theme.clone());
        let terminal = &config.terminal;
        let palettes = [&terminal.theme, &terminal.theme_dark, &terminal.theme_light];
        (name, palettes.map(String::clone))
    };
    // The terminal colors are those of the palette the settings choose.
    let theme = splitty_core::theme::resolve_theme_with_palette(
        &name,
        &named,
        &dark,
        &light,
        &state.themes_dir,
    );
    Payload(serde_json::to_value(&theme).unwrap_or(json!({})))
}

pub(crate) async fn list_available_themes(state: &ServiceState) -> Payload<Vec<String>> {
    Payload(splitty_core::theme::list_themes(&state.themes_dir))
}

// ── Home Settings ────────────────────────────────────────────────────────────

pub(crate) async fn get_home_settings(state: &ServiceState) -> Payload<Value> {
    let cfg = state.config.read().unwrap();
    Payload(serde_json::to_value(&*cfg).unwrap_or(json!({})))
}

pub(crate) fn get_home_metadata() -> Payload<Value> {
    Payload(splitty_core::config::settings_metadata())
}

pub(crate) async fn patch_home_settings(state: &ServiceState, partial: Value) -> Reply {
    match splitty_core::config::update_at(&state.config_path, partial) {
        Ok(updated) => {
            apply_home_settings(state, updated.clone());
            state.client_state.broadcast_events(
                &[splitty_core::protocol::ServerEvent::settings_changed(
                    serde_json::to_value(&updated).unwrap_or(json!({})),
                )],
                None,
            );
            Payload(serde_json::to_value(&updated).unwrap_or(json!({}))).into_reply()
        }
        Err(splitty_core::config::ConfigError::InvalidField { field, message }) => (
            StatusCode::BAD_REQUEST,
            Payload(json!({"error": message, "field": field})),
        )
            .into_reply(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Payload(json!({"error": e.to_string()})),
        )
            .into_reply(),
    }
}

/// Settings that were saved take effect in the running server: the shell
/// profiles, the settings themselves and the PTY size policy.
pub(crate) fn apply_home_settings(state: &ServiceState, updated: splitty_core::config::HomeConfig) {
    let shell_state = splitty_core::shell_profiles::state_from_settings(&updated.shells);
    *state.shell_profiles.write().unwrap() = shell_state;
    state.client_state.dispatcher.configure_sizes(
        splitty_core::protocol::sizing::SizeConfig::from_settings(
            &updated.terminal.size_policy,
            updated.terminal.size_idle_seconds,
        ),
    );
    *state.config.write().unwrap() = updated;
}

// ── Shell Profiles ─────────────────────────────────────────────────────────

pub(crate) async fn get_shell_profiles(state: &ServiceState) -> Payload<Value> {
    let profiles = state.shell_profiles.read().unwrap();
    Payload(serde_json::to_value(&*profiles).unwrap_or(json!({})))
}

pub(crate) async fn put_shell_profiles(
    state: &ServiceState,
    update: splitty_core::shell_profiles::ShellProfilesState,
) -> Reply {
    let detection = splitty_core::shell_profiles::detect_shell_profiles(
        &splitty_core::shell_profiles::DetectionEnvironment::current(),
    );
    let mut candidate = state.config.read().unwrap().clone();
    let shell_state = match splitty_core::shell_profiles::apply_shell_profiles_update(
        &mut candidate.shells,
        update,
        &detection,
    ) {
        Ok(profiles) => profiles,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Payload(json!({"error": e.to_string()})),
            )
                .into_reply();
        }
    };

    if let Err(e) = splitty_core::config::save_to(&candidate, &state.config_path) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Payload(json!({"error": e.to_string()})),
        )
            .into_reply();
    }

    *state.config.write().unwrap() = candidate;
    *state.shell_profiles.write().unwrap() = shell_state.clone();
    state.client_state.broadcast_events(
        &[splitty_core::protocol::ServerEvent::settings_changed(
            serde_json::to_value(&*state.config.read().unwrap()).unwrap_or(json!({})),
        )],
        None,
    );
    Payload(serde_json::to_value(shell_state).unwrap_or(json!({}))).into_reply()
}

// ── Templates ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExtractTemplateRequest {
    screen_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    root: String,
}

/// `template.extract`, deprecated: windows of earlier releases extract their
/// local Screens with it, current ones use `mux.extract_template`
/// (`control-api`, Template Extraction Endpoint).
pub(crate) async fn extract_template(
    state: &ServiceState,
    request: ExtractTemplateRequest,
) -> Reply {
    match state.client_state.effect_runner.extract_screen(
        &state.mux_store,
        &ScreenId::from(request.screen_id.as_str()),
        request.name,
        &request.root,
    ) {
        Ok(template) => Payload(template).into_reply(),
        Err(splitty_core::mux::MuxError::ScreenNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Payload(json!({"error": "screen not found"})),
        )
            .into_reply(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Payload(json!({"error": error.to_string()})),
        )
            .into_reply(),
    }
}

pub(crate) async fn list_templates(state: &ServiceState) -> Reply {
    match state.template_store.list() {
        Ok(summaries) => {
            Payload(serde_json::to_value(&summaries).unwrap_or(json!([]))).into_reply()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Payload(json!({"error": e.to_string()})),
        )
            .into_reply(),
    }
}

pub(crate) async fn get_template(state: &ServiceState, slug: String) -> Reply {
    match state.template_store.get(&slug) {
        Ok(template) => Payload(serde_json::to_value(&template).unwrap_or(json!({}))).into_reply(),
        Err(splitty_core::template::TemplateError::NotFound(_)) => (
            StatusCode::NOT_FOUND,
            Payload(json!({"error": "template not found"})),
        )
            .into_reply(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Payload(json!({"error": e.to_string()})),
        )
            .into_reply(),
    }
}

pub(crate) async fn save_template(state: &ServiceState, slug: String, body: Value) -> Reply {
    let template: splitty_core::template::Template = match serde_json::from_value(body) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Payload(json!({"error": e.to_string()})),
            )
                .into_reply()
        }
    };

    match state.template_store.save(&slug, &template) {
        Ok(()) => StatusCode::NO_CONTENT.into_reply(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Payload(json!({"error": e.to_string()})),
        )
            .into_reply(),
    }
}

/// A save by name (`templates`, Saved Template Names): the server derives the
/// file name and checks and saves in one step.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateTemplateRequest {
    template: splitty_core::template::Template,
    #[serde(default)]
    overwrite: bool,
}

pub(crate) async fn create_template(state: &ServiceState, body: Value) -> Reply {
    use splitty_core::template::TemplateError;
    let request: CreateTemplateRequest = match serde_json::from_value(body) {
        Ok(request) => request,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Payload(json!({"error": e.to_string()})),
            )
                .into_reply()
        }
    };
    match state
        .template_store
        .save_named(&request.template, request.overwrite)
    {
        Ok(saved) => Payload(json!({"slug": saved.slug, "replaced": saved.replaced})).into_reply(),
        Err(TemplateError::Exists(slug)) => (
            StatusCode::CONFLICT,
            Payload(json!({"error": "template exists", "slug": slug})),
        )
            .into_reply(),
        Err(e @ (TemplateError::Io(_) | TemplateError::Yaml(_))) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Payload(json!({"error": e.to_string()})),
        )
            .into_reply(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Payload(json!({"error": e.to_string()})),
        )
            .into_reply(),
    }
}

pub(crate) async fn delete_template(state: &ServiceState, slug: String) -> Reply {
    match state.template_store.delete(&slug) {
        Ok(()) => StatusCode::NO_CONTENT.into_reply(),
        Err(splitty_core::template::TemplateError::NotFound(_)) => (
            StatusCode::NOT_FOUND,
            Payload(json!({"error": "template not found"})),
        )
            .into_reply(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Payload(json!({"error": e.to_string()})),
        )
            .into_reply(),
    }
}

// ── Build info ──────────────────────────────────────────────────────────────

pub(crate) async fn get_build(state: &ServiceState) -> Payload<Value> {
    let distro = std::env::var("WSL_DISTRO_NAME").ok();
    Payload(build_body(
        &state.version,
        &state.commit,
        &state.build_time,
        distro.as_deref(),
    ))
}

/// `GET /api/build`. Inside WSL it names the distribution, through which the
/// UI opens a local folder in VS Code (`wsl+<distro>`).
pub(crate) fn build_body(
    version: &str,
    commit: &str,
    build_time: &str,
    wsl_distro: Option<&str>,
) -> Value {
    let mut build = json!({
        "version": version,
        "commit": commit,
        "buildTime": build_time,
    });
    if let Some(distro) = wsl_distro.filter(|d| !d.is_empty()) {
        build["wslDistro"] = Value::String(distro.to_owned());
    }
    build
}

// ── Command broadcast ───────────────────────────────────────────────────────

pub(crate) async fn post_command(
    state: &ServiceState,
    cmd: splitty_core::protocol::ClientCommand,
) -> Payload<Value> {
    let sent = state.client_state.broadcast_command(cmd).await;
    Payload(json!({ "ok": true, "sent": sent }))
}

// ── Mux API ─────────────────────────────────────────────────────────────────

/// Scratchpads are a window's alone (`session-scratchpad`): neither this API
/// nor the control socket, which every pane reaches, hands their text out.
pub(crate) fn without_scratchpads(screen: &mut mux::ScreenState) {
    for session in screen.sessions.values_mut() {
        session.scratchpad.clear();
        session.scratchpad_version = 0;
    }
}

/// A viewer pane's annotations are a window's alone as well
/// (`viewer-annotations`): an agent reads only what the user handed over, and
/// only through the comment endpoints (`service::review`). Their counter and
/// a wait for a review stay out too.
fn without_annotations(screen: &mut mux::ScreenState) {
    for viewer in screen
        .panes
        .values_mut()
        .filter_map(|pane| pane.viewer.as_mut())
    {
        viewer.annotations.clear();
        viewer.last_annotation_number = 0;
        viewer.review_wait = None;
    }
}

/// Each session's root program status record as `programStatus`
/// (`program-status`): these replies carry it, the workspace state and its
/// snapshots do not.
fn with_program_status(state: &ServiceState, screen: &mut Value) {
    let Some(sessions) = screen.get_mut("sessions").and_then(Value::as_object_mut) else {
        return;
    };
    let backend = state.client_state.dispatcher.backend();
    for (id, session) in sessions.iter_mut() {
        let Some(record) = backend
            .get(id)
            .and_then(|session| session.program_status().record)
        else {
            continue;
        };
        if let (Some(session), Ok(record)) = (session.as_object_mut(), serde_json::to_value(record))
        {
            session.insert("programStatus".into(), record);
        }
    }
}

pub(crate) async fn get_mux_state(state: &ServiceState) -> Payload<Value> {
    // Nor the text a viewer pane shows (`viewer-panes`); windows read it with
    // `viewer.read` over their own connection.
    let mut ws = state.mux_store.get_workspace_state().without_viewer_texts();
    for screen in ws.screens.values_mut() {
        without_scratchpads(screen);
        without_annotations(screen);
    }
    let mut ws = serde_json::to_value(&ws).unwrap_or(json!({}));
    if let Some(screens) = ws.get_mut("screens").and_then(Value::as_object_mut) {
        for screen in screens.values_mut() {
            with_program_status(state, screen);
        }
    }
    Payload(ws)
}

pub(crate) async fn get_mux_screen(state: &ServiceState, id: String) -> Reply {
    let screen_id = ScreenId::from(id.as_str());
    match state.mux_store.get_screen(&screen_id) {
        Some(screen) => {
            let mut screen = screen.without_viewer_texts();
            without_scratchpads(&mut screen);
            without_annotations(&mut screen);
            let mut screen = serde_json::to_value(&screen).unwrap_or(json!({}));
            with_program_status(state, &mut screen);
            Payload(screen).into_reply()
        }
        None => (
            StatusCode::NOT_FOUND,
            Payload(json!({"error": "screen not found"})),
        )
            .into_reply(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MuxOpRequest {
    screen_id: String,
    op: Value,
}

pub(crate) async fn post_mux_op(state: &ServiceState, req: MuxOpRequest) -> Reply {
    let _publication = state.client_state.effect_runner.publication.lock().await;
    if let Some(
        op @ ("set_scratchpad" | "put_annotations" | "remove_annotations" | "put_replies"
        | "remove_replies" | "mark_sent" | "apply_template"),
    ) = req.op.get("type").and_then(|t| t.as_str())
    {
        // Only a window sets a scratchpad (`session-scratchpad`), annotates a
        // viewer pane, replies as the user or hands entries over
        // (`viewer-annotations`). Agents reply through `service::review`.
        return (
            StatusCode::BAD_REQUEST,
            Payload(json!({"ok": false, "error": format!("{op} is only accepted from a window")})),
        )
            .into_reply();
    }
    if let Some(result) = state
        .client_state
        .jobs
        .mux(&req.screen_id, req.op.clone())
        .await
    {
        return match result {
            Ok(access) => super::reply::job(access, StatusCode::OK),
            Err(error) => error.into_reply(),
        };
    }
    let mut screen_id = ScreenId::from(req.screen_id.as_str());
    let op_type = req
        .op
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_owned();

    // The target of a pane moved to another screen, nudged with its source.
    let mut moved_to = None;
    // The pane an `open_viewer` showed its document in, and whether it existed.
    let mut viewer = None;
    let mut screen_order_changed = false;
    // Workspace-level ops vs screen-level ops
    let result = match op_type.as_str() {
        "move_pane_to_screen" => {
            let field = |name: &str| req.op.get(name).and_then(|v| v.as_str());
            let (Some(pane_id), Some(target)) = (field("paneId"), field("targetScreenId")) else {
                return (
                    StatusCode::BAD_REQUEST,
                    Payload(json!({"ok": false, "error": "invalid op: paneId and targetScreenId are required"})),
                )
                    .into_reply();
            };
            let target = ScreenId::from(target);
            moved_to = Some(target.clone());
            state.mux_store.dispatch(mux::Op::MovePaneToScreen {
                screen_id: screen_id.clone(),
                pane_id: splitty_core::ids::PaneId::from(pane_id),
                target_screen_id: target,
                create_screen: req.op.get("createScreen").and_then(|v| v.as_bool()) == Some(true),
                screen_name: field("screenName").map(String::from),
            })
        }
        "move_tab_group_to_screen" => {
            let field = |name: &str| req.op.get(name).and_then(|v| v.as_str());
            let (Some(slot_id), Some(target)) = (field("slotId"), field("targetScreenId")) else {
                return (
                    StatusCode::BAD_REQUEST,
                    Payload(json!({"ok": false, "error": "invalid op: slotId and targetScreenId are required"})),
                )
                    .into_reply();
            };
            let target = ScreenId::from(target);
            moved_to = Some(target.clone());
            state.mux_store.dispatch(mux::Op::MoveTabGroupToScreen {
                screen_id: screen_id.clone(),
                slot_id: splitty_core::ids::NodeId::from(slot_id),
                active_pane_id: field("activePaneId").map(splitty_core::ids::PaneId::from),
                target_screen_id: target,
                create_screen: req.op.get("createScreen").and_then(|v| v.as_bool()) == Some(true),
                screen_name: field("screenName").map(String::from),
            })
        }
        "create_screen" => {
            let name = req
                .op
                .get("name")
                .and_then(|v| v.as_str())
                .map(String::from);
            let sid = req
                .op
                .get("screenId")
                .and_then(|v| v.as_str())
                .map(ScreenId::from)
                .unwrap_or_else(|| ScreenId::with_prefix("screen"));
            screen_id = sid.clone();
            let pid = req
                .op
                .get("newPaneId")
                .and_then(|v| v.as_str())
                .map(PaneId::from);
            let shell_profile_id = req
                .op
                .get("shellProfileId")
                .and_then(|v| v.as_str())
                .map(String::from)
                .or_else(|| {
                    state
                        .shell_profiles
                        .read()
                        .unwrap()
                        .default_profile()
                        .map(|p| p.id.clone())
                });
            if req.op.get("onlyIfEmpty").and_then(|v| v.as_bool()) == Some(true) {
                state.mux_store.dispatch(mux::Op::CreateFirstScreen {
                    name,
                    screen_id: Some(sid),
                    new_pane_id: pid,
                    shell_profile_id,
                })
            } else {
                state.mux_store.dispatch(mux::Op::CreateScreen {
                    name,
                    screen_id: Some(sid),
                    new_pane_id: pid,
                    shell_profile_id,
                })
            }
        }
        "close_screen" => state.mux_store.dispatch(mux::Op::CloseScreen {
            screen_id: screen_id.clone(),
        }),
        "rename_screen" => {
            let name = req
                .op
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            state.mux_store.dispatch(mux::Op::RenameScreen {
                screen_id: screen_id.clone(),
                name,
            })
        }
        "set_active_screen" => state.mux_store.dispatch(mux::Op::SetActiveScreen {
            screen_id: screen_id.clone(),
        }),
        "reorder_screens" => state.mux_store.dispatch(mux::Op::ReorderScreens {
            screen_order: req.op["screenOrder"]
                .as_array()
                .map(|order| {
                    order
                        .iter()
                        .filter_map(|id| id.as_str().map(ScreenId::from))
                        .collect()
                })
                .unwrap_or_default(),
        }),
        "move_screen" => {
            let before_screen_id = match req.op.get("beforeScreenId") {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(id)) => Ok(Some(ScreenId::from(id.as_str()))),
                Some(_) => Err(mux::MuxError::InvalidScreenPlacementAnchor),
            };
            before_screen_id.and_then(|before_screen_id| {
                state
                    .mux_store
                    .dispatch_inspect(
                        mux::Op::MoveScreen {
                            screen_id: screen_id.clone(),
                            before_screen_id,
                        },
                        |before, after| before.screen_order != after.screen_order,
                    )
                    .map(|(effects, changed)| {
                        screen_order_changed = changed;
                        effects
                    })
            })
        }
        _ => {
            // A shell whose pane moved still names its old screen
            // ($SPLITTY_SCREEN_ID): the op follows the pane.
            if let Some(pane) = req.op.get("paneId").and_then(|v| v.as_str()) {
                let pane = splitty_core::ids::PaneId::from(pane);
                let holds = state
                    .mux_store
                    .get_screen(&screen_id)
                    .is_some_and(|screen| screen.panes.contains_key(&pane));
                if !holds {
                    if let Some(actual) = state.mux_store.screen_of_movable_pane(&pane) {
                        screen_id = actual;
                    }
                }
            }
            let screen_op: ScreenOp = match serde_json::from_value(req.op) {
                Ok(op) => op,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Payload(json!({"ok": false, "error": format!("invalid op: {}", e)})),
                    )
                        .into_reply();
                }
            };
            if matches!(screen_op, ScreenOp::OpenViewer { .. }) {
                let id = screen_id.clone();
                state
                    .mux_store
                    .dispatch_inspect(
                        mux::Op::ModifyScreen {
                            screen_id: screen_id.clone(),
                            op: screen_op,
                        },
                        |before, after| {
                            let after = after.screens.get(&id)?;
                            let (pane, reused) =
                                mux::shown_viewer(before.screens.get(&id)?, after)?;
                            let revision = after.panes.get(&pane)?.viewer.as_ref()?.revision;
                            Some((pane, reused, revision))
                        },
                    )
                    .map(|(effects, shown)| {
                        viewer = shown;
                        effects
                    })
            } else {
                state.mux_store.dispatch_screen(&screen_id, screen_op)
            }
        }
    };

    match result {
        Ok(effects) => {
            if let Err(error) = state.client_state.jobs.stop_effects(&effects).await {
                return error.into_reply();
            }
            let outcomes = state
                .client_state
                .effect_runner
                .execute(effects, &state.client_state.env)
                .await;
            let spawned: Vec<_> = outcomes.iter().filter_map(|o| o.spawned()).collect();
            let mut batch = Vec::new();
            for (_, pane, session) in &spawned {
                let handle = state
                    .client_state
                    .dispatcher
                    .backend()
                    .get(session.as_str());
                if let Some(ref handle) = handle {
                    state
                        .mux_store
                        .set_resolved_shell_profile(session, handle.shell_profile_id());
                }
                batch.push(
                    splitty_core::protocol::ServerEvent::mux_pane_spawned_with_profile(
                        pane.to_string(),
                        session.to_string(),
                        handle
                            .as_ref()
                            .and_then(|h| h.shell_profile_id())
                            .map(str::to_owned),
                        handle
                            .as_ref()
                            .and_then(|h| h.shell_profile_fallback_reason())
                            .map(str::to_owned),
                    ),
                );
            }
            if let Some(target) = &moved_to {
                batch.push(splitty_core::protocol::ServerEvent::mux_nudge(
                    "cli_op".into(),
                    Some(target.to_string()),
                ));
            }
            if op_type == "move_screen" {
                if screen_order_changed {
                    batch.push(splitty_core::protocol::ServerEvent::mux_nudge(
                        "cli_op".into(),
                        None,
                    ));
                }
            } else {
                batch.push(splitty_core::protocol::ServerEvent::mux_nudge(
                    "cli_op".into(),
                    Some(screen_id.to_string()),
                ));
            }
            if !batch.is_empty() {
                state.client_state.broadcast_events(&batch, None);
            }
            if let Some(failed) = outcomes.iter().find(|o| o.result.is_err()) {
                let mut value = json!({"ok":false,"code":"spawn_failed","error":failed.result.as_ref().err().map(ToString::to_string),"screenId":screen_id});
                if let splitty_core::mux::Effect::SpawnSession {
                    pane_id,
                    session_id,
                    ..
                } = &failed.effect
                {
                    value["paneId"] = json!(pane_id);
                    value["sessionId"] = json!(session_id);
                }
                return (StatusCode::INTERNAL_SERVER_ERROR, Payload(value)).into_reply();
            }

            let screen = state.mux_store.get_screen(&screen_id);
            let revision = screen.map(|s| s.revision).unwrap_or(0);

            let mut body = json!({
                "ok": true,
                "revision": revision,
                "screenId": screen_id,
                "created": spawned.iter().map(|(_,pane,session)|json!({"paneId":pane,"sessionId":session})).collect::<Vec<_>>(),
            });
            if let Some((pane, reused, revision)) = viewer {
                body["paneId"] = json!(pane);
                body["reused"] = json!(reused);
                body["viewerRevision"] = json!(revision);
            }
            Payload(body).into_reply()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Payload(json!({"ok": false, "error": e.to_string()})),
        )
            .into_reply(),
    }
}

pub(crate) async fn get_mux_neighbor(
    state: &ServiceState,
    params: HashMap<String, String>,
) -> Reply {
    let screen_id = params.get("screen").cloned().unwrap_or_default();
    let pane_id = params.get("pane").cloned().unwrap_or_default();
    let direction = params.get("dir").cloned().unwrap_or_default();

    if screen_id.is_empty() || pane_id.is_empty() || direction.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Payload(json!({"error": "screen, pane, and dir params required"})),
        )
            .into_reply();
    }

    let screen_id = ScreenId::from(screen_id.as_str());
    let screen = match state.mux_store.get_screen(&screen_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Payload(json!({"error": "screen not found"})),
            )
                .into_reply()
        }
    };

    let dir = match direction.as_str() {
        "left" => Direction::Left,
        "right" => Direction::Right,
        "above" | "up" => Direction::Above,
        "below" | "down" => Direction::Below,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Payload(json!({"error": "invalid direction"})),
            )
                .into_reply()
        }
    };

    let pane = PaneId::from(pane_id.as_str());
    let neighbor = mux::find_neighbor(&screen.root, &pane, &dir);
    let neighbor_str = neighbor.map(|p| p.to_string()).unwrap_or_default();

    Payload(json!({"paneId": neighbor_str})).into_reply()
}

// ── Mux status & screens summary ────────────────────────────────────────

pub(crate) async fn get_mux_screens(state: &ServiceState) -> Payload<Value> {
    let screens = state.mux_store.list_screen_infos();
    let ws = state.mux_store.get_workspace_state();
    let result: Vec<Value> = screens
        .iter()
        .map(|s| {
            let pane_count = ws.screens.get(&s.id).map(|sc| sc.panes.len()).unwrap_or(0);
            json!({
                "id": s.id.to_string(),
                "name": s.name,
                "paneCount": pane_count,
                "active": ws.active_screen_id.as_ref() == Some(&s.id),
            })
        })
        .collect();
    Payload(json!(result))
}

pub(crate) async fn get_mux_status(state: &ServiceState) -> Payload<Value> {
    let ws = state.mux_store.get_workspace_state();
    let total_sessions: usize = ws.screens.values().map(|s| s.sessions.len()).sum();
    Payload(json!({
        "serverMode": state.server_mode,
        "capabilities": [splitty_core::protocol::capability::MANAGED_JOBS],
        "version": state.version,
        "screens": ws.screens.len(),
        "sessions": total_sessions,
        "activeScreen": ws.active_screen_id.as_ref().map(|id| id.to_string()),
        "listenAddr": null,
        "instance": state.instance,
        "transportVersion": 1,
    }))
}

// ── Session send ───────────────────────────────────────────────────────

#[derive(Deserialize)]
pub(crate) struct SessionSendRequest {
    data: String,
}

pub(crate) async fn post_session_send(
    state: &ServiceState,
    id: String,
    req: SessionSendRequest,
) -> Reply {
    let backend = state.client_state.dispatcher.backend();
    match backend.get(&id) {
        Some(session) => match session.write(req.data.as_bytes()).await {
            Ok(()) => Payload(json!({"ok": true})).into_reply(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Payload(json!({"error": format!("write failed: {e}")})),
            )
                .into_reply(),
        },
        None => (
            StatusCode::NOT_FOUND,
            Payload(json!({"error": "session not found"})),
        )
            .into_reply(),
    }
}

pub(crate) async fn post_file_offer(
    state: &ServiceState,
    input: crate::client_session::files::OfferRequest,
) -> Reply {
    use crate::client_session::editor::{validate_path, Failure};
    fn error((status, code, message): Failure) -> Reply {
        (
            StatusCode::from_u16(status).expect("fixed HTTP status"),
            Payload(json!({"code":code,"error":message})),
        )
            .into_reply()
    }
    if input.session_id.is_empty()
        || input.session_id.len() > 256
        || state
            .client_state
            .dispatcher
            .backend()
            .get(&input.session_id)
            .is_none()
    {
        return error((
            404,
            "session_not_found",
            "The originating terminal session is no longer available",
        ));
    }
    if let Err(e) = validate_path(&input.path) {
        return error(e);
    }
    let path = std::path::PathBuf::from(&input.path);
    let file = match tokio::task::spawn_blocking(move || crate::client_session::files::open(&path))
        .await
    {
        Ok(Ok(file)) => file,
        Ok(Err(e)) => return error(e),
        Err(_) => return error((500, "read_failed", "The file could not be opened")),
    };
    let offers = state.client_state.file_offers.clone();
    match offers.offer(&input, file).await {
        Ok(()) => Payload(json!({"outcome":"saved"})).into_reply(),
        Err(e) => error(e),
    }
}

pub(crate) async fn post_editor_open(
    state: &ServiceState,
    input: crate::client_session::editor::OpenRequest,
) -> Reply {
    use crate::client_session::editor::{validate_path, Failure};
    fn error((status, code, message): Failure) -> Reply {
        (
            StatusCode::from_u16(status).expect("fixed HTTP status"),
            Payload(json!({"code":code,"error":message})),
        )
            .into_reply()
    }
    if input.session_id.is_empty()
        || input.session_id.len() > 256
        || state
            .client_state
            .dispatcher
            .backend()
            .get(&input.session_id)
            .is_none()
    {
        return error((
            404,
            "session_not_found",
            "The originating terminal session is no longer available",
        ));
    }
    if let Err(e) = validate_path(&input.path) {
        return error(e);
    }
    let metadata = match tokio::fs::metadata(&input.path).await {
        Ok(m) => m,
        Err(_) => {
            return error((
                400,
                "invalid_path",
                "The requested path does not exist or is inaccessible",
            ))
        }
    };
    let kind = if metadata.is_dir() {
        crate::client_session::editor::PathKind::Folder
    } else if metadata.is_file() {
        crate::client_session::editor::PathKind::File
    } else {
        return error((
            400,
            "invalid_path",
            "Only regular files and folders can be opened",
        ));
    };
    let requests = state.client_state.editor_requests.clone();
    match requests.request(input, kind).await {
        Ok(()) => Payload(json!({"outcome":"dispatched"})).into_reply(),
        Err(e) => error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::{HostOrder, HostPreference};
    use serde_json::json;

    /// The host metadata replies keep the JSON the hub's records had.
    #[test]
    fn host_replies_keep_their_json() {
        fn value(reply: &impl serde::Serialize) -> serde_json::Value {
            serde_json::to_value(reply).expect("serialize")
        }
        let row = |row: HostPreference| {
            value(&host_preferences_reply(
                splitty_remote::hosts::HostPreferences {
                    version: 1,
                    hosts: vec![row],
                },
            ))["hosts"][0]
                .clone()
        };
        assert_eq!(
            value(&host_order_reply(HostOrder {
                version: 1,
                order: vec!["vm1".into(), "vm2".into()],
            })),
            json!({"version":1,"order":["vm1","vm2"]})
        );
        assert_eq!(
            row(HostPreference {
                target: "vm1".into(),
                alias: Some("Build".into()),
                hidden: false,
            }),
            json!({"target":"vm1","alias":"Build"})
        );
        assert_eq!(
            row(HostPreference {
                target: "vm2".into(),
                alias: None,
                hidden: true,
            }),
            json!({"target":"vm2","hidden":true})
        );
    }
}
