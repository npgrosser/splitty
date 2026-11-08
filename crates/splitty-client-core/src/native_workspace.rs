//! Own the local service and route workspace intents on one ordered connection.
mod remote;
mod source;
use crate::{
    appearance::{AppearanceSnapshot, ColorScheme, DrawerVisibilityResult},
    hub::{
        HostDirectorySnapshot, HostDirectoryState, HostManagementAction, HostManagementOutcome,
        HostManagementRequest, HostManagementResult, HubClient, HubEvent, HubUpdate,
    },
    ports::{
        ForwardOutcome, ForwardRequest, ForwardResult, ForwardSnapshot, PortsClient, PortsEvent,
    },
    shell_profiles::ShellProfilesSnapshot,
    templates::{TemplateApiRequest, TemplateApiResult},
    workspace::{
        Axis, ClosePlan, CloseTarget, FloatBounds, FloatingActionOutcome, FloatingActionResult,
        FloatingTarget, HostActionTarget, MovePlacement, PaneActionTarget, PaneDrag,
        PlacementOutcome, ScreenKey, ScreenTransferOutcome, ScreenTransferResult,
        ScreenTransferTarget, SourceToken, SourceWorkspace, TabStep, TemplateApplyChoice,
        TemplateApplyOutcome, TemplateApplyPlan, TemplateApplyResult, TerminalToken,
        WindowWorkspace, WorkspaceRefresh,
    },
    zoom::{ZoomAction, ZoomResult},
    Event,
};
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
};
use tokio::sync::mpsc;

/// One immutable intent captured on its assigned source connection.
pub struct Command {
    source: Option<SourceToken>,
    kind: CommandKind,
}
impl Command {
    pub fn new(source: SourceToken, kind: CommandKind) -> Self {
        Self {
            source: Some(source),
            kind,
        }
    }
    /// Startup close is channel-owned until the first workspace is discovered.
    pub fn close(plan: ClosePlan, confirmed: bool) -> Self {
        Self {
            source: plan.source_token(),
            kind: CommandKind::RequestClose { plan, confirmed },
        }
    }
    pub fn source_token(&self) -> Option<SourceToken> {
        self.source
    }
    pub fn kind(&self) -> &CommandKind {
        &self.kind
    }
    fn validate_source(&self, current: SourceToken) -> std::io::Result<()> {
        let startup = self.source.is_none()
            && matches!(&self.kind, CommandKind::RequestClose { plan, .. }
                if plan.source_token().is_none() && plan.target == CloseTarget::Window);
        if !startup && self.source != Some(current) {
            return Err(crate::connection::invalid(
                "Command belongs to another source connection",
            ));
        }
        let captured = match &self.kind {
            CommandKind::Input { route, .. } | CommandKind::ResizePane { route, .. } => {
                Some(route.source_token())
            }
            CommandKind::RequestClose { plan, .. } => plan.source_token(),
            CommandKind::ApplyTemplate { plan, .. } => Some(plan.source_token()),
            CommandKind::FloatPane { source, .. } | CommandKind::TransferPane { source, .. } => {
                Some(source.source_token())
            }
            CommandKind::DockFloat { target, .. } | CommandKind::MoveFloat { target, .. } => {
                Some(target.source_token())
            }
            CommandKind::PlaceTab { drag, .. } | CommandKind::MovePane { drag, .. } => {
                Some(drag.source_token())
            }
            _ => self.source,
        };
        if captured != self.source {
            return Err(crate::connection::invalid(
                "Captured target belongs to another source connection",
            ));
        }
        Ok(())
    }
}

pub enum CommandKind {
    ManageForward {
        request_id: uuid::Uuid,
        request: ForwardRequest,
    },
    RefreshForwards,
    CancelForwardOpen {
        request_id: uuid::Uuid,
    },
    ManageHost {
        request_id: uuid::Uuid,
        request: HostManagementRequest,
    },
    WithdrawHostConnect {
        request_id: uuid::Uuid,
    },
    ReleaseHostBrowsing,

    ConnectHost {
        target: String,
    },
    RetryHost {
        target: HostActionTarget,
    },
    CancelHost {
        target: HostActionTarget,
    },
    ReleaseHostBrowse {
        target: HostActionTarget,
    },
    OpenRemoteScreen {
        screen: ScreenKey,
    },
    DetachRemoteScreen {
        screen: ScreenKey,
    },
    RefreshHostDirectory {
        connection: uuid::Uuid,
    },
    TemplateApi {
        connection: uuid::Uuid,
        request_id: uuid::Uuid,
        request: TemplateApiRequest,
    },
    ApplyTemplate {
        plan: Arc<TemplateApplyPlan>,
        choice: TemplateApplyChoice,
        replace_confirmed: bool,
    },
    FloatPane {
        request_id: uuid::Uuid,
        source: PaneActionTarget,
    },
    DockFloat {
        request_id: uuid::Uuid,
        target: FloatingTarget,
    },
    MoveFloat {
        request_id: uuid::Uuid,
        target: FloatingTarget,
        bounds: FloatBounds,
    },
    TransferPane {
        request_id: uuid::Uuid,
        source: PaneActionTarget,
        target: ScreenTransferTarget,
    },
    Zoom {
        connection: uuid::Uuid,
        request_id: uuid::Uuid,
        action: ZoomAction,
    },
    SetColorScheme {
        connection: uuid::Uuid,
        scheme: ColorScheme,
    },
    UpdateSettings {
        connection: uuid::Uuid,
        request_id: uuid::Uuid,
        edit: crate::appearance::SettingsEdit,
    },
    SetDrawerVisibility {
        connection: uuid::Uuid,
        request_id: uuid::Uuid,
        pinned: bool,
    },
    CreateScreen(String),
    SwitchScreen(ScreenKey),
    CycleScreen {
        step: TabStep,
    },
    RenameScreen {
        screen: String,
        connection: uuid::Uuid,
        name: String,
    },
    Split {
        screen: ScreenKey,
        pane: String,
        axis: Axis,
    },
    Tab {
        screen: ScreenKey,
        pane: String,
    },
    ProfileTab {
        connection: uuid::Uuid,
        screen: String,
        pane: String,
        session: String,
        profile_id: String,
    },
    NewTerminal {
        screen: String,
        connection: uuid::Uuid,
    },
    SetMaximized {
        screen: String,
        connection: uuid::Uuid,
        slot: Option<String>,
        pane: Option<String>,
    },
    PlaceTab {
        drag: PaneDrag,
        target: String,
        before: Option<String>,
    },
    CycleTab {
        connection: uuid::Uuid,
        screen: String,
        slot: String,
        step: TabStep,
    },
    CycleTabInSlot {
        connection: uuid::Uuid,
        screen: String,
        slot: String,
        step: TabStep,
    },
    MovePane {
        drag: PaneDrag,
        target: String,
        placement: MovePlacement,
    },
    RequestClose {
        plan: ClosePlan,
        confirmed: bool,
    },
    ResizeSplit {
        screen: ScreenKey,
        revision: u64,
        node: String,
        sizes: Vec<f64>,
    },
    Select {
        screen: ScreenKey,
        pane: String,
    },
    Focus {
        screen: ScreenKey,
        pane: String,
    },
    Input {
        route: TerminalToken,
        bytes: Vec<u8>,
    },
    ResizePane {
        route: TerminalToken,
        screen: ScreenKey,
        route_generation: u64,
        cols: u16,
        rows: u16,
    },
    Visible {
        screen: Option<ScreenKey>,
        route_generation: u64,
        panes: Vec<String>,
    },
}
pub enum Update {
    Forwards(Arc<ForwardSnapshot>),
    Forward(ForwardResult),
    HostManagement(HostManagementResult),
    HostDirectory(HubUpdate),
    TemplateApi(TemplateApiResult),
    TemplateApply(TemplateApplyResult),
    FloatingAction(FloatingActionResult),
    ScreenTransfer(ScreenTransferResult),
    Zoom(ZoomResult),
    SettingsWrite(crate::local_service::SettingsWriteResult),
    SettingsReconciled(crate::local_service::SettingsReconciled),
    DrawerVisibility(DrawerVisibilityResult),
    ShellProfiles(Arc<ShellProfilesSnapshot>),
    ShellProfilesError {
        connection: uuid::Uuid,
        reason: String,
    },
    Appearance(Arc<AppearanceSnapshot>),
    AppearanceError {
        connection: uuid::Uuid,
        reason: String,
    },
    Workspace(Box<WindowWorkspace>),
    Event {
        source: SourceToken,
        event: Box<Event>,
    },
    Attached {
        source: SourceToken,
        session: String,
    },
    Status(String),
    CloseSubmitted {
        request_id: u64,
        source: Option<SourceToken>,
    },
    CloseRejected {
        source: Option<SourceToken>,
        request_id: u64,
        reason: String,
    },
    CloseSettled {
        source: Option<SourceToken>,
        request_id: u64,
    },
    WindowCloseApproved {
        request_id: u64,
        source: Option<SourceToken>,
    },
}

fn validate_command_target(model: &SourceWorkspace, command: &CommandKind) -> std::io::Result<()> {
    match command {
        CommandKind::Split { screen, pane, .. }
        | CommandKind::Tab { screen, pane }
        | CommandKind::Select { screen, pane }
        | CommandKind::Focus { screen, pane } => {
            model.validate_shown_key(screen)?;
            if model.screen_for(pane) != Some(screen.screen_id()) {
                return Err(crate::connection::invalid(
                    "Pane no longer belongs to the captured Screen",
                ));
            }
        }
        CommandKind::ResizeSplit {
            screen, revision, ..
        } => {
            model.validate_shown_key(screen)?;
            if model
                .snapshot()
                .is_none_or(|snapshot| snapshot.revision != *revision)
            {
                return Err(crate::connection::invalid(
                    "Divider belongs to an obsolete layout",
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

fn rejected_command(source: Option<SourceToken>, command: &CommandKind, reason: String) -> Update {
    match command {
        CommandKind::ManageForward {
            request_id,
            request,
        } if source.is_some() => Update::Forward(ForwardResult::new(
            source.expect("captured owner"),
            *request_id,
            request,
            ForwardOutcome::Rejected,
            Some(reason),
        )),
        CommandKind::ManageHost {
            request_id,
            request,
        } if source.is_some() => Update::HostManagement(HostManagementResult::new(
            source.expect("captured owner"),
            *request_id,
            request,
            HostManagementOutcome::Rejected,
            Some(reason),
        )),
        CommandKind::RequestClose { plan, .. } => Update::CloseRejected {
            source,
            request_id: plan.request_id,
            reason,
        },
        CommandKind::TemplateApi {
            connection,
            request_id,
            ..
        } => Update::TemplateApi(TemplateApiResult::rejected(
            *connection,
            *request_id,
            reason,
        )),
        CommandKind::ApplyTemplate { plan, .. } => Update::TemplateApply(TemplateApplyResult {
            connection_id: plan.connection_id(),
            request_id: plan.request_id(),
            outcome: TemplateApplyOutcome::Rejected { reason },
        }),
        CommandKind::FloatPane { request_id, source } => {
            Update::FloatingAction(FloatingActionResult {
                connection_id: source.connection_id,
                request_id: *request_id,
                outcome: FloatingActionOutcome::Rejected(reason),
            })
        }
        CommandKind::DockFloat { request_id, target }
        | CommandKind::MoveFloat {
            request_id, target, ..
        } => Update::FloatingAction(FloatingActionResult {
            connection_id: target.connection_id(),
            request_id: *request_id,
            outcome: FloatingActionOutcome::Rejected(reason),
        }),
        CommandKind::TransferPane {
            request_id, source, ..
        } => Update::ScreenTransfer(ScreenTransferResult {
            connection_id: source.connection_id,
            request_id: *request_id,
            outcome: ScreenTransferOutcome::Rejected(reason),
        }),
        CommandKind::Zoom {
            connection,
            request_id,
            ..
        } => Update::Zoom(ZoomResult::rejected(*connection, *request_id, reason)),
        CommandKind::UpdateSettings {
            connection,
            request_id,
            edit,
        } => Update::SettingsWrite(crate::local_service::SettingsWriteResult::rejected(
            *connection,
            *request_id,
            edit.field(),
            reason,
        )),
        CommandKind::SetDrawerVisibility {
            connection,
            request_id,
            ..
        } => Update::DrawerVisibility(DrawerVisibilityResult::rejected(
            *connection,
            *request_id,
            reason,
        )),
        CommandKind::SetColorScheme { connection, .. } => Update::AppearanceError {
            connection: *connection,
            reason,
        },
        _ => Update::Status(reason),
    }
}
pub struct Worker {
    pub stop: Arc<AtomicBool>,
    pub cleanup_complete: Arc<AtomicBool>,
    /// The worker exited and can be joined, even when cleanup failed. This is
    /// deliberately separate from successful service-cleanup evidence.
    pub finished: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<anyhow::Result<()>>>,
}

struct Finished(Arc<AtomicBool>);
impl Drop for Finished {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn profile_fallback_status(
    model: &SourceWorkspace,
    connection: uuid::Uuid,
    pane: &str,
    session: &str,
    event: &serde_json::Value,
) -> Option<String> {
    // Spawn events have no Screen ID. Report only after the workspace has
    // admitted this exact pane/session route from its own connection.
    if connection != model.connection_id()
        || model.session_for(pane) != Some(session)
        || model.pane_for(session) != Some(pane)
    {
        return None;
    }
    let reason = match event["shellProfileFallbackReason"].as_str()? {
        "missing" => "missing",
        "disabled" => "disabled",
        "unavailable" => "unavailable",
        _ => return None,
    };
    let effective = event["shellProfileId"].as_str()?;
    if effective.len() > crate::shell_profiles::MAX_PROFILE_ID_BYTES {
        return None;
    }
    let effective: String = effective.chars().filter(|ch| !ch.is_control()).collect();
    Some(format!(
        "Shell profile {reason}; started {effective} instead"
    ))
}

fn supports_host_directory(welcome: &serde_json::Value) -> bool {
    welcome["capabilities"]
        .as_array()
        .is_some_and(|capabilities| {
            capabilities
                .iter()
                .any(|capability| capability.as_str() == Some("hub-host-decisions-v1"))
        })
}
// Setup and reads stay owned across cancelled select branches. Both streams
// share the embedded service's admission domain, but never consume each other's events.
type LocalServiceSetup<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = std::io::Result<crate::local_service::LocalServiceClient>>
            + Send
            + 'a,
    >,
>;
struct RuntimeLocalService<'a> {
    service: &'a crate::owned_service::OwnedService,
    connection: uuid::Uuid,
    scheme: ColorScheme,
    setup: Option<LocalServiceSetup<'a>>,
    reader: Option<crate::local_service::LocalServiceClient>,
    dirty: bool,
    settings_dirty: bool,
}
impl<'a> RuntimeLocalService<'a> {
    fn new(
        service: &'a crate::owned_service::OwnedService,
        connection: uuid::Uuid,
        scheme: ColorScheme,
    ) -> Self {
        Self {
            service,
            connection,
            scheme,
            setup: None,
            reader: None,
            dirty: true,
            settings_dirty: false,
        }
    }

    fn enqueue_template(
        &mut self,
        connection: uuid::Uuid,
        request_id: uuid::Uuid,
        request: TemplateApiRequest,
    ) -> std::io::Result<()> {
        if connection != self.connection {
            return Err(std::io::Error::other("Template source changed"));
        }
        self.reader
            .as_mut()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "Local service API is not ready",
                )
            })?
            .enqueue_template(connection, request_id, request)
    }
    fn enqueue_zoom(
        &mut self,
        connection: uuid::Uuid,
        request_id: uuid::Uuid,
        action: ZoomAction,
    ) -> std::io::Result<()> {
        if connection != self.connection {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "local settings source changed",
            ));
        }
        self.reader
            .as_mut()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "local settings are not ready",
                )
            })?
            .enqueue_zoom(connection, request_id, action)
    }

    fn enqueue_settings(
        &mut self,
        connection: uuid::Uuid,
        request_id: uuid::Uuid,
        edit: crate::appearance::SettingsEdit,
    ) -> std::io::Result<()> {
        if connection != self.connection {
            return Err(std::io::Error::other("Local settings source changed"));
        }
        self.reader
            .as_mut()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "Local settings are not ready",
                )
            })?
            .enqueue_settings(connection, request_id, edit)
    }

    fn enqueue_drawer_visibility(
        &mut self,
        connection: uuid::Uuid,
        request_id: uuid::Uuid,
        pinned: bool,
    ) -> std::io::Result<()> {
        if connection != self.connection {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "local settings source changed",
            ));
        }
        self.reader
            .as_mut()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "local settings are not ready",
                )
            })?
            .enqueue_drawer_visibility(connection, request_id, pinned)
    }

    fn set_scheme(&mut self, connection: uuid::Uuid, scheme: ColorScheme) -> std::io::Result<()> {
        if connection != self.connection || scheme == self.scheme {
            return Ok(());
        }
        self.scheme = scheme;
        match &mut self.reader {
            Some(reader) => reader.set_scheme(connection, scheme),
            None => {
                self.dirty = true;
                Ok(())
            }
        }
    }

    fn settings_changed(&mut self, connection: uuid::Uuid) -> std::io::Result<()> {
        if connection != self.connection {
            return Ok(());
        }
        match &mut self.reader {
            Some(reader) => reader.refresh_settings(connection),
            None => {
                self.dirty = true;
                self.settings_dirty = true;
                Ok(())
            }
        }
    }

    async fn next_update(&mut self) -> std::io::Result<crate::local_service::LocalServiceUpdate> {
        loop {
            if let Some(reader) = &mut self.reader {
                return Ok(reader.next_update().await);
            }
            if self.setup.is_none() {
                if !self.dirty {
                    std::future::pending::<()>().await;
                }
                self.dirty = false;
                self.setup = Some(Box::pin(
                    self.service
                        .local_service_client(self.connection, self.scheme),
                ));
            }
            let result = self
                .setup
                .as_mut()
                .expect("local settings setup started")
                .await;
            self.setup = None;
            match result {
                Ok(mut reader) => {
                    reader.set_scheme(self.connection, self.scheme)?;
                    if self.settings_dirty {
                        reader.refresh_settings(self.connection)?;
                        self.settings_dirty = false;
                    }
                    self.dirty = false;
                    self.reader = Some(reader);
                }
                Err(_) if self.dirty => continue,
                Err(error) => return Err(error),
            }
        }
    }

    #[cfg(test)]
    async fn finish(self) -> std::io::Result<()> {
        self.finish_with_outcomes().await.map(|_| ())
    }
    async fn finish_with_outcomes(
        mut self,
    ) -> std::io::Result<Vec<crate::local_service::SettingsWriteResult>> {
        // Completing a started, time-bounded handshake lets us await its service
        // cleanup explicitly. Dropping setup would only schedule Drop cleanup.
        if let Some(setup) = self.setup.take() {
            // The connector awaits failed setup cleanup before returning an
            // initialization error. Local settings initialization remains optional
            // when it happens to finish during close, just as during operation.
            self.reader = setup.await.ok();
        }
        match self.reader {
            Some(reader) => reader.finish_with_outcomes().await,
            None => Ok(Vec::new()),
        }
    }
}

type HubSetup<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = std::io::Result<crate::api_stream::ApiStream>> + Send + 'a,
    >,
>;
type HubClosing = std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>> + Send>>;
struct RuntimeHub<'a> {
    service: &'a crate::owned_service::OwnedService,
    connection: uuid::Uuid,
    generation: u64,
    dirty: bool,
    reopen: bool,
    setup: Option<HubSetup<'a>>,
    setup_context: Option<splitty_transport::queue::Context>,
    closing: Option<HubClosing>,
    cleanup_error: Option<std::io::Error>,
    setup_loading_due: bool,
    reader: Option<HubClient>,
    previous: Option<Arc<HostDirectorySnapshot>>,
    admitted: std::collections::VecDeque<uuid::Uuid>,
}
impl<'a> RuntimeHub<'a> {
    fn new(service: &'a crate::owned_service::OwnedService, connection: uuid::Uuid) -> Self {
        Self {
            service,
            connection,
            generation: 1,
            dirty: true,
            reopen: false,
            setup: None,
            setup_context: None,
            closing: None,
            cleanup_error: None,
            setup_loading_due: false,
            reader: None,
            previous: None,
            admitted: std::collections::VecDeque::new(),
        }
    }
    fn refresh(&mut self, owner: uuid::Uuid) -> std::io::Result<()> {
        if owner != self.connection {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "hub directory source changed",
            ));
        }
        if let Some(reader) = &mut self.reader {
            self.previous = reader.last_good().cloned();
            reader.refresh(owner)?;
            self.generation = reader.generation();
        } else {
            self.generation = self
                .generation
                .checked_add(1)
                .ok_or_else(|| std::io::Error::other("hub generation exhausted"))?;
            self.dirty = true;
            self.setup_loading_due = true;
        }
        Ok(())
    }
    fn enqueue(
        &mut self,
        owner: SourceToken,
        id: uuid::Uuid,
        request: HostManagementRequest,
    ) -> std::io::Result<HostManagementResult> {
        if self.admitted.contains(&id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "Host request was already admitted",
            ));
        }
        let result = self
            .reader
            .as_mut()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "Host metadata service is not ready",
                )
            })?
            .enqueue(owner, id, request)?;
        Ok(result)
    }
    #[cfg(test)]
    async fn next_update(&mut self) -> HubUpdate {
        match self.next_event().await {
            HubEvent::Directory(update) => update,
            HubEvent::Management(_) => panic!("unexpected mutation update"),
        }
    }
    async fn next_event(&mut self) -> HubEvent {
        loop {
            if self.reopen {
                self.reopen = false;
                if let Some(reader) = self.reader.take() {
                    self.previous = reader.last_good().cloned();
                    self.closing = Some(Box::pin(reader.finish()));
                }
            }
            if let Some(closing) = &mut self.closing {
                let result = closing.await;
                self.closing = None;
                if let Err(error) = result {
                    self.dirty = false;
                    let reason = format!("Host directory stream cleanup failed: {error}");
                    self.cleanup_error = Some(error);
                    return HubEvent::Directory(HubUpdate {
                        connection_id: self.connection,
                        generation: self.generation,
                        state: HostDirectoryState::Failed {
                            previous: self.previous.clone(),
                            reason,
                        },
                    });
                }
            }
            if let Some(reader) = &mut self.reader {
                let update = reader.next_event().await;
                self.generation = reader.generation();
                if let HubEvent::Management(result) = &update {
                    if self.admitted.len() == 256 {
                        self.admitted.pop_front();
                    }
                    self.admitted.push_back(result.request_id());
                }
                if reader.take_reopen_due() {
                    self.reopen = true;
                    self.dirty = true;
                }
                if let HubEvent::Directory(HubUpdate {
                    state: HostDirectoryState::Ready(snapshot),
                    ..
                }) = &update
                {
                    self.previous = Some(snapshot.clone());
                }
                return update;
            }
            if self.setup.is_none() {
                if !self.dirty {
                    std::future::pending::<()>().await;
                }
                self.dirty = false;
                let context = splitty_transport::queue::Context::default();
                self.setup = Some(Box::pin(
                    self.service.api_stream_with_context(context.clone()),
                ));
                self.setup_context = Some(context);
            }
            if self.setup_loading_due {
                self.setup_loading_due = false;
                return HubEvent::Directory(HubUpdate {
                    connection_id: self.connection,
                    generation: self.generation,
                    state: HostDirectoryState::Loading {
                        previous: self.previous.clone(),
                    },
                });
            }
            let result = self.setup.as_mut().expect("hub setup retained").await;
            self.setup = None;
            self.setup_context = None;
            match result {
                Ok(client) => {
                    self.reader = Some(HubClient::with_previous(
                        client,
                        self.connection,
                        self.generation,
                        self.previous.clone(),
                    ));
                    self.dirty = false;
                }
                Err(_) if self.dirty => continue,
                Err(_) => {
                    return HubEvent::Directory(HubUpdate {
                        connection_id: self.connection,
                        generation: self.generation,
                        state: HostDirectoryState::Failed {
                            previous: self.previous.clone(),
                            reason: "Host directory API unavailable".into(),
                        },
                    });
                }
            }
        }
    }
    async fn finish(mut self) -> std::io::Result<()> {
        if let Some(context) = self.setup_context.take() {
            context.close();
        }
        if let Some(setup) = self.setup.take() {
            if let Ok(client) = setup.await {
                self.reader = Some(HubClient::with_previous(
                    client,
                    self.connection,
                    self.generation,
                    self.previous.clone(),
                ));
            }
        }
        if let Some(closing) = self.closing.take() {
            if let Err(error) = closing.await {
                self.cleanup_error.get_or_insert(error);
            }
        }
        if let Some(reader) = self.reader.take() {
            if let Err(error) = reader.finish().await {
                self.cleanup_error.get_or_insert(error);
            }
        }
        self.cleanup_error.map_or(Ok(()), Err)
    }
}

struct PendingClose {
    source: Option<SourceToken>,
    request_id: u64,
    target: CloseTarget,
    barrier_generation: u64,
    catalog_barrier_seen: bool,
    failure: Option<String>,
}
impl PendingClose {
    fn is_duplicate(&self, request_id: u64) -> bool {
        self.request_id == request_id
    }
    fn observe_catalog(&mut self, generation: Option<u64>) -> bool {
        if generation.is_some_and(|generation| generation >= self.barrier_generation) {
            self.catalog_barrier_seen = true;
            true
        } else {
            false
        }
    }
    fn observe_failure(&mut self, json: &serde_json::Value) {
        let matching = match &self.target {
            CloseTarget::Pane { screen, .. } => {
                json["op"] == "close_pane" && json["screenId"] == *screen
            }
            CloseTarget::Screen { screen } => {
                json["op"] == "close_screen" && json["screenId"] == *screen
            }
            CloseTarget::Window => false,
        };
        if matching {
            self.failure = Some(format!("Close rejected: {json}"));
        }
    }
    fn settlement(
        &self,
        screen_exists: bool,
        accepted_pane_snapshot: bool,
        pane_exists: bool,
    ) -> Option<Result<(), String>> {
        if !self.catalog_barrier_seen {
            return None;
        }
        if let Some(reason) = &self.failure {
            return Some(Err(reason.clone()));
        }
        match self.target {
            CloseTarget::Screen { .. } => Some(if screen_exists {
                Err("Screen close was not confirmed".into())
            } else {
                Ok(())
            }),
            CloseTarget::Pane { .. } if !screen_exists => Some(Ok(())),
            CloseTarget::Pane { .. } if accepted_pane_snapshot => Some(if pane_exists {
                Err("Pane close was not confirmed".into())
            } else {
                Ok(())
            }),
            _ => None,
        }
    }
}

fn validate_pre_discovery_close(plan: &ClosePlan, confirmed: bool) -> std::io::Result<()> {
    if confirmed && *plan == ClosePlan::startup_window(plan.request_id) {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "workspace changed while confirming close",
        ))
    }
}

async fn finish_owned_run(
    service: crate::owned_service::OwnedService,
    result: anyhow::Result<()>,
    prior_cleanup: anyhow::Result<()>,
    cleanup_complete: &AtomicBool,
    stop: &AtomicBool,
) -> anyhow::Result<()> {
    let cleanup = service.close().await;
    if cleanup.is_ok() && prior_cleanup.is_ok() {
        cleanup_complete.store(true, Ordering::Release);
    }
    prior_cleanup?;
    cleanup?;
    if stop.load(Ordering::Acquire) {
        Ok(())
    } else {
        result
    }
}
impl Worker {
    #[cfg(feature = "ui-test-support")]
    #[doc(hidden)]
    pub fn ui_test_fixture() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            cleanup_complete: Arc::new(AtomicBool::new(true)),
            finished: Arc::new(AtomicBool::new(true)),
            thread: None,
        }
    }
    pub fn start(commands: mpsc::Receiver<Command>, updates: mpsc::Sender<Update>) -> Self {
        Self::start_with_scheme(commands, updates, ColorScheme::Dark)
    }

    pub fn start_with_scheme(
        commands: mpsc::Receiver<Command>,
        updates: mpsc::Sender<Update>,
        scheme: ColorScheme,
    ) -> Self {
        Self::start_with_backend(
            commands,
            updates,
            scheme,
            crate::owned_service::LocalBackend::default(),
        )
    }

    pub fn start_with_backend(
        commands: mpsc::Receiver<Command>,
        updates: mpsc::Sender<Update>,
        scheme: ColorScheme,
        backend: crate::owned_service::LocalBackend,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let cleanup_complete = Arc::new(AtomicBool::new(false));
        let completed = cleanup_complete.clone();
        let finished = Arc::new(AtomicBool::new(false));
        let completion = finished.clone();
        let thread = thread::spawn(move || {
            // Declared before the runtime: its drop (including cleanup tasks)
            // precedes this completion signal on every return/unwind path.
            let _finished = Finished(completion);
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build();
            let runtime = match runtime {
                Ok(runtime) => runtime,
                Err(error) => {
                    completed.store(true, Ordering::Release); // no owned service existed
                    return Err(error.into());
                }
            };
            runtime.block_on(async {
                let result = run(
                    commands,
                    updates.clone(),
                    stopping,
                    completed,
                    scheme,
                    backend,
                )
                .await;
                if let Err(error) = &result {
                    let _ = updates.try_send(Update::Status(format!("Local service: {error:#}")));
                }
                result
            })
        });
        Self {
            stop,
            cleanup_complete,
            finished,
            thread: Some(thread),
        }
    }
    pub fn shutdown(mut self) -> anyhow::Result<()> {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| anyhow::anyhow!("Local service worker panicked"))??;
        }
        Ok(())
    }
}

async fn publish(
    updates: &mpsc::Sender<Update>,
    update: Update,
    stop: &AtomicBool,
) -> anyhow::Result<()> {
    loop {
        if stop.load(Ordering::Acquire) {
            anyhow::bail!("Local workspace is closing");
        }
        tokio::select! {
            permit = updates.reserve() => { permit?.send(update); return Ok(()); }
            _ = tokio::time::sleep(Duration::from_millis(16)) => {}
        }
    }
}

async fn run(
    commands: mpsc::Receiver<Command>,
    updates: mpsc::Sender<Update>,
    stop: Arc<AtomicBool>,
    cleanup_complete: Arc<AtomicBool>,
    scheme: ColorScheme,
    backend: crate::owned_service::LocalBackend,
) -> anyhow::Result<()> {
    let service = crate::owned_service::OwnedService::start(backend, &stop).await?;
    run_service(service, commands, updates, stop, cleanup_complete, scheme).await
}

/// Pending metadata and recent source admissions share one request identity.
/// Only an actual admitted Connect can acquire browse demand.
#[derive(Default)]
struct HostConnects {
    pending: HashMap<uuid::Uuid, SourceToken>,
    started: std::collections::VecDeque<(uuid::Uuid, HostActionTarget)>,
}
impl HostConnects {
    fn insert(&mut self, id: uuid::Uuid, owner: SourceToken) {
        self.pending.insert(id, owner);
    }
    fn complete(&mut self, result: &HostManagementResult) -> bool {
        self.pending.remove(&result.request_id()) == Some(result.owner())
            && result.outcome() == HostManagementOutcome::Applied
    }
    fn apply(
        &mut self,
        result: &mut HostManagementResult,
        window: &mut WindowWorkspace,
        remotes: &mut remote::Remotes<'_>,
    ) -> bool {
        if result.owner() != window.lifecycle_token() || !self.complete(result) {
            return false;
        }
        let already_browsing = window
            .hosts()
            .any(|host| host.action_target().target() == result.target() && host.is_browsing());
        match remotes.connect_explicit(window, result.owner(), result.target()) {
            Ok(()) => {
                if !already_browsing {
                    if let Some(action) = window
                        .hosts()
                        .find(|host| host.action_target().target() == result.target())
                        .map(|host| host.action_target().clone())
                    {
                        self.started(result.request_id(), action);
                    }
                }
                true
            }
            Err(error) => {
                result.connection_failed(error.to_string());
                false
            }
        }
    }
    fn started(&mut self, id: uuid::Uuid, action: HostActionTarget) {
        self.retain_other_targets(action.target());
        if self.started.len() == 16 {
            self.started.pop_front();
        }
        self.started.push_back((id, action));
    }
    fn retain_other_targets(&mut self, target: &str) {
        self.started.retain(|(_, action)| action.target() != target);
    }
    fn withdraw(&mut self, id: uuid::Uuid) -> Option<HostActionTarget> {
        self.pending.remove(&id);
        let index = self
            .started
            .iter()
            .position(|(request, _)| *request == id)?;
        self.started.remove(index).map(|(_, action)| action)
    }
    fn clear(&mut self) {
        self.pending.clear();
        self.started.clear();
    }
}

async fn run_service(
    service: impl Into<crate::owned_service::OwnedService>,
    mut commands: mpsc::Receiver<Command>,
    updates: mpsc::Sender<Update>,
    stop: Arc<AtomicBool>,
    cleanup_complete: Arc<AtomicBool>,
    scheme: ColorScheme,
) -> anyhow::Result<()> {
    let service = service.into();
    let client = match service.connect().await {
        Ok(client) => client,
        Err(error) => {
            return finish_owned_run(service, Err(error.into()), Ok(()), &cleanup_complete, &stop)
                .await
        }
    };
    let mut window = WindowWorkspace::new(client.connection_id());
    let mut local = source::SourceRuntime::new(client, window.lifecycle_token());
    let mut settings = None::<RuntimeLocalService<'_>>;
    let mut ports = None::<PortsClient<'_>>;
    let mut hub = None::<RuntimeHub<'_>>;
    let mut remotes = remote::Remotes::new(&service);
    let mut known = None::<Arc<HostDirectorySnapshot>>;
    let mut host_connects = HostConnects::default();
    let result = async {
        let mut tick = tokio::time::interval(Duration::from_millis(16));
        enum Next {
            Settings(std::io::Result<crate::local_service::LocalServiceUpdate>),
            Hub(HubEvent),
            Ports(PortsEvent),
            Event(std::io::Result<Option<Event>>),
            Remote(remote::Step),
            Command(Option<Command>),
            Tick,
        }
        loop {
            let next = tokio::select! {
                result = async { match &mut settings { Some(reader)=>reader.next_update().await, None=>std::future::pending().await } } => Next::Settings(result),
                update = async { match &mut hub { Some(reader)=>reader.next_event().await, None=>std::future::pending().await } } => Next::Hub(update),
                event = async { match &mut ports { Some(ports)=>ports.next_event().await,None=>std::future::pending().await } } => Next::Ports(event),
                event = local.client.next_event() => Next::Event(event),
                remote = remotes.next() => Next::Remote(remote),
                command = commands.recv() => Next::Command(command),
                _ = tick.tick() => Next::Tick,
            };
            if stop.load(Ordering::Acquire) { break; }
            let owner = window.lifecycle_token();
            match next {
                Next::Ports(event) => {
                    if let Some(ports)=&ports {ports.revalidate(&window);}
                    let update=match event {PortsEvent::Snapshot(snapshot)=>Update::Forwards(snapshot),PortsEvent::Result(result)=>Update::Forward(*result)};
                    publish(&updates,update,&stop).await?;
                }
                Next::Hub(HubEvent::Management(mut result)) => {
                    if result.owner()==owner {
                        if result.action()==HostManagementAction::Connect && host_connects.apply(&mut result,&mut window,&mut remotes) {
                            publish(&updates,Update::Workspace(Box::new(window.clone())),&stop).await?;
                        }
                        publish(&updates,Update::HostManagement(result),&stop).await?;
                    }
                }
                Next::Hub(HubEvent::Directory(update)) => {
                    if update.connection_id == local.client.connection_id() {
                        let snapshot=match &update.state {HostDirectoryState::Ready(snapshot)=>Some(snapshot.clone()),HostDirectoryState::Loading {previous}|HostDirectoryState::Failed {previous,..}=>previous.clone(),HostDirectoryState::Unavailable=>None};
                        if let Some(snapshot)=snapshot.filter(|snapshot|known.as_ref().is_none_or(|old|snapshot.generation()>=old.generation())) {
                            window.set_host_order(owner,&snapshot.directory().order)?;
                            known=Some(snapshot);
                            publish(&updates,Update::Workspace(Box::new(window.clone())),&stop).await?;
                        }
                        publish(&updates, Update::HostDirectory(update), &stop).await?;
                    }
                }
                Next::Settings(result) => {
                    use crate::local_service::LocalServiceUpdate;
                    let connection = local.client.connection_id();
                    let update = match result {
                        Ok(LocalServiceUpdate::SettingsWrite(result)) => Update::SettingsWrite(result),
                        Ok(LocalServiceUpdate::SettingsReconciled(result)) => Update::SettingsReconciled(result),
                        Ok(LocalServiceUpdate::Zoom(result)) => Update::Zoom(result),
                        Ok(LocalServiceUpdate::DrawerVisibility(result)) => Update::DrawerVisibility(result),
                        Ok(LocalServiceUpdate::Template(result)) => Update::TemplateApi(result),
                        Ok(LocalServiceUpdate::Appearance(Ok(snapshot))) => Update::Appearance(snapshot),
                        Ok(LocalServiceUpdate::Appearance(Err(error))) => Update::AppearanceError { connection, reason:error.to_string() },
                        Ok(LocalServiceUpdate::ShellProfiles(Ok(snapshot))) => Update::ShellProfiles(snapshot),
                        Ok(LocalServiceUpdate::ShellProfiles(Err(error))) => Update::ShellProfilesError { connection, reason:error.to_string() },
                        Err(error) => {
                            publish(&updates, Update::ShellProfilesError { connection,reason:error.to_string() }, &stop).await?;
                            Update::AppearanceError { connection,reason:error.to_string() }
                        }
                    };
                    publish(&updates, update, &stop).await?;
                }
                Next::Remote(step) => {
                    let effects=remotes.handle(&mut window,step);
                    publish_source_effects(&mut window,effects,&updates,&stop).await?;
                }
                Next::Tick => {}
                Next::Command(None) => break,
                Next::Command(Some(command)) => {
                    let expected=if local_command(&command.kind) {Some(owner)} else {command.source.and_then(|captured|window.execution_token(captured.source_id()))};
                    let admission=expected.ok_or_else(||crate::connection::invalid("Command belongs to an unavailable source connection")).and_then(|expected|command.validate_source(expected));
                    if let Err(error) = admission {
                        publish(&updates,rejected_command(command.source,&command.kind,error.to_string()),&stop).await?;
                        continue;
                    }
                    let lifecycle=match &command.kind {
                        CommandKind::RetryHost {target}=>Some((target,remotes.retry(&mut window,target))),
                        CommandKind::CancelHost {target}=>Some((target,remotes.cancel(&mut window,target))),
                        CommandKind::ReleaseHostBrowse {target}=>Some((target,remotes.release_browse(&mut window,target))),
                        _=>None,
                    };
                    if let Some((target,result))=lifecycle {
                        let changed = match result { Ok(())=>true, Err(error)=>remotes.reject_action(&mut window,target,&error) };
                        if changed { publish(&updates,Update::Workspace(Box::new(window.clone())),&stop).await?; }
                        continue;
                    }
                    let captured = command.source;
                    match command.kind {
                        CommandKind::ManageForward {request_id,request} => {
                            let result=ports.as_mut().ok_or_else(||std::io::Error::new(std::io::ErrorKind::WouldBlock,"Forwarding service is not ready")).and_then(|ports|ports.enqueue(owner,request_id,request.clone(),&window));
                            let result=match result {Ok(result)=>result,Err(error) if error.kind()==std::io::ErrorKind::AlreadyExists=>continue,Err(error)=>ForwardResult::new(owner,request_id,&request,ForwardOutcome::Rejected,Some(error.to_string()))};
                            publish(&updates,Update::Forward(result),&stop).await?;
                        }
                        CommandKind::RefreshForwards => {if let Some(ports)=&mut ports {ports.refresh();}}
                        CommandKind::CancelForwardOpen {request_id} => {if let Some(ports)=&mut ports {ports.cancel_open(request_id);}}
                        CommandKind::ManageHost {request_id,request} => {
                            let result=hub.as_mut().ok_or_else(||std::io::Error::new(std::io::ErrorKind::WouldBlock,"Host metadata service is not ready")).and_then(|hub|hub.enqueue(owner,request_id,request.clone()));
                            let result=match result {
                                Ok(result)=>{if request.action()==HostManagementAction::Connect {host_connects.insert(request_id,owner);} result},
                                Err(error) if error.kind()==std::io::ErrorKind::AlreadyExists=>continue,
                                Err(error)=>HostManagementResult::new(owner,request_id,&request,HostManagementOutcome::Rejected,Some(error.to_string())),
                            };
                            publish(&updates,Update::HostManagement(result),&stop).await?;
                        }
                        CommandKind::WithdrawHostConnect {request_id} => {
                            if let Some(action)=host_connects.withdraw(request_id) { let _=remotes.release_browse(&mut window,&action); }
                            publish(&updates,Update::Workspace(Box::new(window.clone())),&stop).await?;
                        }
                        CommandKind::ReleaseHostBrowsing => {
                            host_connects.clear();
                            remotes.release_all_browse(&mut window);
                            publish(&updates,Update::Workspace(Box::new(window.clone())),&stop).await?;
                        }
                        CommandKind::RefreshHostDirectory { connection } => {
                            if let Some(reader)=&mut hub {
                                if let Err(error)=reader.refresh(connection) { publish(&updates,Update::Status(error.to_string()),&stop).await?; }
                            } else if connection==owner.connection_id() { publish(&updates,Update::HostDirectory(HubUpdate::unavailable(connection)),&stop).await?; }
                        }
                        CommandKind::TemplateApi { connection,request_id,request } => {
                            let result = if window.local_source().connection_id()!=connection || !window.local_source().templates_supported() { Err(std::io::Error::other("Template source or capability unavailable")) }
                            else { settings.as_mut().ok_or_else(||std::io::Error::new(std::io::ErrorKind::WouldBlock,"Local service API is not ready")).and_then(|settings|settings.enqueue_template(connection,request_id,request)) };
                            if let Err(error)=result { if error.kind()!=std::io::ErrorKind::AlreadyExists { publish(&updates,Update::TemplateApi(TemplateApiResult::rejected(connection,request_id,error.to_string())),&stop).await?; } }
                        }
                        CommandKind::Zoom { connection,request_id,action } => {
                            let result=settings.as_mut().ok_or_else(||std::io::Error::new(std::io::ErrorKind::WouldBlock,"local settings are not ready")).and_then(|settings|settings.enqueue_zoom(connection,request_id,action));
                            if let Err(error)=result { publish(&updates,Update::Zoom(ZoomResult::rejected(connection,request_id,error.to_string())),&stop).await?; }
                        }
                        CommandKind::UpdateSettings { connection,request_id,edit } => {
                            let field = edit.field();
                            let result=settings.as_mut().ok_or_else(||std::io::Error::new(std::io::ErrorKind::WouldBlock,"Local settings are not ready")).and_then(|settings|settings.enqueue_settings(connection,request_id,edit));
                            if let Err(error)=result { if error.kind()!=std::io::ErrorKind::AlreadyExists { publish(&updates,Update::SettingsWrite(crate::local_service::SettingsWriteResult::rejected(connection,request_id,field,error.to_string())),&stop).await?; } }
                        }
                        CommandKind::SetDrawerVisibility { connection,request_id,pinned } => {
                            let result=settings.as_mut().ok_or_else(||std::io::Error::new(std::io::ErrorKind::WouldBlock,"local settings are not ready")).and_then(|settings|settings.enqueue_drawer_visibility(connection,request_id,pinned));
                            if let Err(error)=result { publish(&updates,Update::DrawerVisibility(DrawerVisibilityResult::rejected(connection,request_id,error.to_string())),&stop).await?; }
                        }
                        CommandKind::SetColorScheme { connection,scheme } => {
                            if let Some(reader)=&mut settings { if let Err(error)=reader.set_scheme(connection,scheme) { publish(&updates,Update::AppearanceError { connection,reason:error.to_string() },&stop).await?; } }
                        }
                        CommandKind::SwitchScreen(key) => {
                            match select_window_screen(&mut window,&mut local,&mut remotes,&key) {
                                Ok(())=>publish(&updates,Update::Workspace(Box::new(window.clone())),&stop).await?,
                                Err(error)=>publish(&updates,Update::Status(error.to_string()),&stop).await?,
                            }
                        }
                        CommandKind::CycleScreen { step } => {
                            if let Some(key)=adjacent_screen(&window,step) {
                                match select_window_screen(&mut window,&mut local,&mut remotes,&key) {
                                    Ok(())=>publish(&updates,Update::Workspace(Box::new(window.clone())),&stop).await?,
                                    Err(error)=>publish(&updates,Update::Status(error.to_string()),&stop).await?,
                                }
                            }
                        }
                        CommandKind::ConnectHost { target } => {
                            host_connects.retain_other_targets(&target);
                            let result=known.as_deref().ok_or_else(||crate::connection::invalid("Host directory has not been read")).and_then(|known|remotes.connect(&mut window,owner,&target,known));
                            match result {Ok(())=>publish(&updates,Update::Workspace(Box::new(window.clone())),&stop).await?,Err(error)=>publish(&updates,Update::Status(error.to_string()),&stop).await?}
                        }
                        CommandKind::RetryHost { .. } | CommandKind::CancelHost { .. } | CommandKind::ReleaseHostBrowse { .. } => unreachable!("host lifecycle is dispatched before moving command"),
                        CommandKind::OpenRemoteScreen { screen } => {
                            match remotes.open(&mut window,captured.expect("ready source capture"),&screen) {Ok(())=>{local.cancel_created_follow();remotes.cancel_created_follow();publish(&updates,Update::Workspace(Box::new(window.clone())),&stop).await?;},Err(error)=>publish(&updates,Update::Status(error.to_string()),&stop).await?}
                        }
                        CommandKind::DetachRemoteScreen { screen } => {
                            match remotes.detach(&mut window,owner,&screen) {Ok(())=>publish(&updates,Update::Workspace(Box::new(window.clone())),&stop).await?,Err(error)=>publish(&updates,Update::Status(error.to_string()),&stop).await?}
                        }
                        kind => {
                            let command=Command {source:captured,kind};
                            let effects=if captured.is_none_or(|scope|scope.source_id()==local.id()) {local.command(&mut window,command)?} else {remotes.command(&mut window,command)?};
                            if publish_source_effects(&mut window,effects,&updates,&stop).await? {break;}
                        }
                    }
                }
                Next::Event(event) => {
                    let event=Box::new(event?.ok_or_else(||anyhow::anyhow!("Local connection closed"))?);
                    if event.json().is_none() {
                        // Binary delivery remains charged and never clones navigation state.
                        publish(&updates,Update::Event { source:local.owner(),event },&stop).await?;
                    } else {
                        let json=event.json().expect("JSON event");
                        let welcome=!local.welcomed && json["type"]=="mux.welcome";
                        let supports_hub=welcome && supports_host_directory(json);
                        if json["type"]=="settings.changed" {
                            if let Some(reader)=&mut settings { if let Err(error)=reader.settings_changed(event.connection_id) {publish(&updates,Update::AppearanceError {connection:event.connection_id,reason:error.to_string()},&stop).await?;} }
                        }
                        if json["type"]=="hub.changed" {
                            if let Some(ports)=&mut ports {ports.refresh();}
                            if let Some(reader)=&mut hub {if let Err(error)=reader.refresh(event.connection_id) {publish(&updates,Update::Status(error.to_string()),&stop).await?;} }
                        }
                        let effects=local.event(&mut window,event)?;
                        publish_source_effects(&mut window,effects,&updates,&stop).await?;
                        if welcome {
                            // Owner publication precedes any independently owned API result.
                            if supports_hub {
                                hub=Some(RuntimeHub::new(&service,local.client.connection_id()));
                                publish(&updates,Update::HostDirectory(HubUpdate { connection_id:local.client.connection_id(),generation:1,state:HostDirectoryState::Loading {previous:None} }),&stop).await?;
                            } else {publish(&updates,Update::HostDirectory(HubUpdate::unavailable(local.client.connection_id())),&stop).await?;}
                            ports=Some(PortsClient::new(&service,window.lifecycle_token(),supports_hub));
                            settings=Some(RuntimeLocalService::new(&service,local.client.connection_id(),scheme));
                        }
                    }
                }
            }
            if let Some(ports)=&ports {ports.revalidate(&window);}
            let effects=local.reconcile(&mut window)?;
            publish_source_effects(&mut window,effects,&updates,&stop).await?;
            let effects=remotes.reconcile(&mut window)?;
            publish_source_effects(&mut window,effects,&updates,&stop).await?;
        }
        Ok::<(),anyhow::Error>(())
    }.await;
    let (settings_cleanup, hub_cleanup, client_cleanup, remote_cleanup, ports_cleanup) = tokio::join!(
        async {
            match settings.take() {
                Some(reader) => match reader.finish_with_outcomes().await {
                    Ok(outcomes) => {
                        // Closing must not wait for a UI that has stopped draining.
                        // Preserve final write uncertainty while a sink is available.
                        for outcome in outcomes {
                            let _ = updates.try_send(Update::SettingsWrite(outcome));
                        }
                        Ok(())
                    }
                    Err(error) => Err(anyhow::Error::from(error)),
                },
                None => Ok(()),
            }
        },
        async {
            match hub.take() {
                Some(reader) => reader.finish().await.map_err(anyhow::Error::from),
                None => Ok(()),
            }
        },
        async { local.client.finish().await.map_err(anyhow::Error::from) },
        async {
            remotes
                .finish(&mut window)
                .await
                .map_err(anyhow::Error::from)
        },
        async {
            match ports.take() {
                Some(ports) => ports.finish().await.map_err(anyhow::Error::from),
                None => Ok(()),
            }
        },
    );
    drop(ports);
    drop(settings);
    drop(hub);
    finish_owned_run(
        service,
        result,
        settings_cleanup
            .and(hub_cleanup)
            .and(client_cleanup)
            .and(remote_cleanup)
            .and(ports_cleanup),
        &cleanup_complete,
        &stop,
    )
    .await
}

fn local_command(kind: &CommandKind) -> bool {
    matches!(
        kind,
        CommandKind::ConnectHost { .. }
            | CommandKind::ManageForward { .. }
            | CommandKind::RefreshForwards
            | CommandKind::CancelForwardOpen { .. }
            | CommandKind::ManageHost { .. }
            | CommandKind::WithdrawHostConnect { .. }
            | CommandKind::ReleaseHostBrowsing
            | CommandKind::RetryHost { .. }
            | CommandKind::CancelHost { .. }
            | CommandKind::ReleaseHostBrowse { .. }
            | CommandKind::DetachRemoteScreen { .. }
            | CommandKind::SwitchScreen(_)
            | CommandKind::CycleScreen { .. }
            | CommandKind::RefreshHostDirectory { .. }
            | CommandKind::TemplateApi { .. }
            | CommandKind::ApplyTemplate { .. }
            | CommandKind::Zoom { .. }
            | CommandKind::SetColorScheme { .. }
            | CommandKind::UpdateSettings { .. }
            | CommandKind::SetDrawerVisibility { .. }
            | CommandKind::CreateScreen(_)
            | CommandKind::RenameScreen { .. }
            | CommandKind::ProfileTab { .. }
            | CommandKind::NewTerminal { .. }
            | CommandKind::TransferPane { .. }
            | CommandKind::FloatPane { .. }
            | CommandKind::DockFloat { .. }
            | CommandKind::MoveFloat { .. }
    ) || matches!(kind,CommandKind::RequestClose {plan,..} if plan.target==CloseTarget::Window)
}
fn select_window_screen(
    window: &mut WindowWorkspace,
    local: &mut source::SourceRuntime,
    remotes: &mut remote::Remotes<'_>,
    key: &ScreenKey,
) -> std::io::Result<()> {
    if key.source_id() == local.id() {
        window
            .source_mut(local.id())
            .expect("local source")
            .switch_screen(&local.client, key.screen_id())?;
    }
    window.select(window.lifecycle_token(), key)?;
    local.cancel_created_follow();
    remotes.cancel_created_follow();
    local.withdraw_visibility();
    Ok(())
}

fn adjacent_screen(window: &WindowWorkspace, step: TabStep) -> Option<ScreenKey> {
    let current = window.shown_key()?;
    let opened = window.opened();
    let index = opened.iter().position(|entry| entry.key() == &current)?;
    let next = match step {
        TabStep::Previous => index.checked_sub(1)?,
        TabStep::Next => index.checked_add(1)?,
    };
    opened.get(next).map(|entry| entry.key().clone())
}

async fn publish_source_effects(
    window: &mut WindowWorkspace,
    mut effects: source::SourceEffects,
    updates: &mpsc::Sender<Update>,
    stop: &AtomicBool,
) -> anyhow::Result<bool> {
    if let Some(key) = effects.select_created {
        window.select(window.lifecycle_token(), &key)?;
        effects.changed = true;
    }
    if effects.changed {
        publish(updates, Update::Workspace(Box::new(window.clone())), stop).await?;
    }
    for update in effects.updates {
        publish(updates, update, stop).await?;
    }
    Ok(effects.close_window)
}

#[cfg(all(test, not(target_os = "windows")))]
fn local_projection(window: &WindowWorkspace) -> Box<crate::workspace::SourceWorkspace> {
    let source = window.local_source();
    assert_eq!(window.lifecycle_token(), source.source_token());
    assert_eq!(
        window.shown_key(),
        source.shown_screen_id().map(|id| source.screen_key(id)),
        "local-only worker publishes a coherent global selection"
    );
    assert!(window
        .opened()
        .iter()
        .all(|entry| entry.key().source_id() == source.source_id()));
    Box::new(source.clone())
}

#[cfg(all(test, not(target_os = "windows")))]
mod close_tests {
    use super::*;
    use crate::workspace::SourceWorkspace;
    use serde_json::json;
    use std::time::Duration;

    async fn next_update(updates: &mut mpsc::Receiver<Update>) -> Update {
        tokio::time::timeout(Duration::from_secs(10), updates.recv())
            .await
            .expect("native close update timeout")
            .expect("native close update channel")
    }

    async fn stable_model(updates: &mut mpsc::Receiver<Update>) -> Box<SourceWorkspace> {
        loop {
            if let Update::Workspace(model) = next_update(updates).await {
                let model = local_projection(&model);
                if model.catalog_initialized()
                    && model.screens().len() == 1
                    && !model.has_pending_create()
                    && model.snapshot().is_some()
                    && !model.routes().is_empty()
                {
                    return model;
                }
            }
        }
    }

    async fn output_contains(updates: &mut mpsc::Receiver<Update>, session: &str, marker: &[u8]) {
        let mut output = Vec::new();
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if let Update::Event { event, .. } = next_update(updates).await {
                    if let Some(chunk) = event.output() {
                        if chunk.session_id == session {
                            output.extend_from_slice(chunk.bytes);
                            if output.windows(marker.len()).any(|part| part == marker) {
                                break;
                            }
                        }
                    }
                }
            }
        })
        .await
        .expect("terminal output marker");
    }

    async fn session_kind(client: &mut crate::Client, kind: &str) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = client.next_event().await.unwrap().expect("live claimant");
                if event.json().is_some_and(|json| json["type"] == kind) {
                    break;
                }
            }
        })
        .await
        .expect("claimant session event");
    }

    async fn next_appearance(
        reader: &mut RuntimeLocalService<'_>,
    ) -> std::io::Result<Arc<AppearanceSnapshot>> {
        loop {
            if let crate::local_service::LocalServiceUpdate::Appearance(result) =
                reader.next_update().await?
            {
                return result;
            }
        }
    }

    #[tokio::test]
    async fn appearance_runtime_rejects_foreign_refresh_and_finishes_started_setup() {
        use std::{future::Future, task::Poll};
        let (runtime, path) = crate::appearance::local::tests::fixture();
        let service = crate::owned_service::OwnedService::from(
            crate::embedded::EmbeddedService::from_runtime(runtime),
        );
        let connection = uuid::Uuid::new_v4();
        let mut appearance = RuntimeLocalService::new(&service, connection, ColorScheme::Light);
        // Start setup, cancel just the receiving branch, then change the local
        // preference. The owned handshake survives and yields only the latest choice.
        let mut read = Box::pin(next_appearance(&mut appearance));
        std::future::poll_fn(|cx| {
            assert!(read.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        drop(read);
        appearance
            .set_scheme(connection, ColorScheme::Dark)
            .unwrap();
        appearance
            .set_scheme(uuid::Uuid::new_v4(), ColorScheme::Light)
            .unwrap();
        let snapshot =
            tokio::time::timeout(Duration::from_secs(5), next_appearance(&mut appearance))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(snapshot.connection_id(), connection);
        assert_eq!(snapshot.scheme(), ColorScheme::Dark);
        appearance.settings_changed(uuid::Uuid::new_v4()).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), next_appearance(&mut appearance))
                .await
                .is_err()
        );
        drop(snapshot);
        // The runtime also joins cleanup after a refresh has started an API
        // request, rather than only covering shutdown during initial setup.
        appearance.settings_changed(connection).unwrap();
        let mut read = Box::pin(next_appearance(&mut appearance));
        std::future::poll_fn(|cx| {
            assert!(read.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        drop(read);
        appearance.finish().await.unwrap();

        // Shutdown immediately after beginning a second setup must await that
        // handshake and close the recovered API stream, even without a snapshot.
        let mut appearance = RuntimeLocalService::new(&service, connection, ColorScheme::Light);
        let mut read = Box::pin(next_appearance(&mut appearance));
        std::future::poll_fn(|cx| {
            assert!(read.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        drop(read);
        tokio::time::timeout(Duration::from_secs(5), appearance.finish())
            .await
            .unwrap()
            .unwrap();
        service.close().await.unwrap();
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn appearance_setup_failure_during_close_is_not_a_shutdown_failure() {
        use std::{future::Future, task::Poll};
        let (mut runtime, path) = crate::appearance::local::tests::fixture();
        // Fail native identity validation inside the actual embedded handshake.
        Arc::get_mut(&mut runtime.state).unwrap().instance = "invalid".into();
        let service = crate::owned_service::OwnedService::from(
            crate::embedded::EmbeddedService::from_runtime(runtime),
        );
        let mut appearance =
            RuntimeLocalService::new(&service, uuid::Uuid::new_v4(), ColorScheme::Dark);
        let mut read = Box::pin(next_appearance(&mut appearance));
        std::future::poll_fn(|cx| {
            assert!(read.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        drop(read);
        tokio::time::timeout(Duration::from_secs(5), appearance.finish())
            .await
            .unwrap()
            .unwrap();
        service.close().await.unwrap();
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn appearance_worker_failure_keeps_output_and_recovers_from_local_service() {
        if !splitty_core::pty::testing::pty_available() {
            return;
        }
        let (runtime, path) = crate::appearance::local::tests::fixture();
        let state = runtime.state.clone();
        state.config.write().unwrap().appearance.font_family = "x".repeat(4097);
        let service = crate::owned_service::OwnedService::from(
            crate::embedded::EmbeddedService::from_runtime(runtime),
        );
        let (commands, receiver) = mpsc::channel(32);
        let (sender, mut updates) = mpsc::channel(64);
        let stop = Arc::new(AtomicBool::new(false));
        let cleanup = Arc::new(AtomicBool::new(false));
        let worker = tokio::spawn(run_service(
            service,
            receiver,
            sender,
            stop.clone(),
            cleanup.clone(),
            ColorScheme::Light,
        ));
        let mut owner = None;
        let mut ready = None;
        let mut failed = false;
        let mut profile_generation = None;
        while ready.is_none() || !failed || profile_generation.is_none() {
            match next_update(&mut updates).await {
                Update::Workspace(model) => {
                    let model = local_projection(&model);
                    owner = Some(model.connection_id());
                    if model.snapshot().is_some() && !model.routes().is_empty() {
                        ready = Some(model);
                    }
                }
                Update::AppearanceError { connection, reason } => {
                    assert_eq!(
                        Some(connection),
                        owner,
                        "Workspace establishes owner before appearance"
                    );
                    assert!(reason.contains("native limit"));
                    failed = true;
                }
                Update::ShellProfiles(snapshot) => {
                    assert_eq!(
                        Some(snapshot.connection_id()),
                        owner,
                        "workspace owner precedes profiles"
                    );
                    profile_generation = Some(snapshot.generation());
                }
                Update::ShellProfilesError { reason, .. } => {
                    panic!("independent profiles failed: {reason}")
                }
                Update::Appearance(_) => panic!("invalid initial settings were published"),
                _ => {}
            }
        }
        let model = ready.unwrap();
        let scope = model.source_token();
        let pane = model.routes()[0].pane_id.clone();
        let session = model.routes()[0].session_id.clone();
        commands
            .send(Command::new(
                scope,
                CommandKind::Input {
                    route: TerminalToken::new(scope, &pane, &session),
                    bytes: b"printf 'APPEARANCE_%s\\n' 'ALIVE'\n".into(),
                },
            ))
            .await
            .unwrap();
        output_contains(&mut updates, &session, b"APPEARANCE_ALIVE").await;
        state.config.write().unwrap().appearance.font_family = "recovered".into();
        state.shell_profiles.write().unwrap().default_profile_id = "refresh-observed".into();
        state.client_state.broadcast_events(
            &[splitty_core::protocol::ServerEvent::settings_changed(
                json!({}),
            )],
            None,
        );
        let mut refreshed_appearance = None;
        let mut refreshed_profiles = false;
        while refreshed_appearance.is_none() || !refreshed_profiles {
            match next_update(&mut updates).await {
                Update::Appearance(snapshot) => refreshed_appearance = Some(snapshot),
                Update::ShellProfiles(snapshot) => {
                    assert_eq!(snapshot.connection_id(), model.connection_id());
                    assert!(snapshot.generation() > profile_generation.unwrap());
                    assert_eq!(snapshot.state().default_profile_id, "refresh-observed");
                    refreshed_profiles = true;
                }
                Update::ShellProfilesError { reason, .. } => {
                    panic!("profile refresh failed: {reason}")
                }
                Update::AppearanceError { reason, .. } => panic!("refresh failed: {reason}"),
                _ => {}
            }
        }
        let snapshot = refreshed_appearance.unwrap();
        assert_eq!(snapshot.connection_id(), model.connection_id());
        assert_eq!(snapshot.settings().font_family, "recovered");
        assert_eq!(snapshot.scheme(), ColorScheme::Light);
        drop(snapshot);
        // A foreign scheme intent followed by a valid owner change must never
        // replace this window's local preference with the foreign choice.
        commands
            .send(Command::new(
                scope,
                CommandKind::SetColorScheme {
                    connection: uuid::Uuid::new_v4(),
                    scheme: ColorScheme::Light,
                },
            ))
            .await
            .unwrap();
        commands
            .send(Command::new(
                scope,
                CommandKind::SetColorScheme {
                    connection: model.connection_id(),
                    scheme: ColorScheme::Dark,
                },
            ))
            .await
            .unwrap();
        loop {
            if let Update::Appearance(snapshot) = next_update(&mut updates).await {
                assert_eq!(snapshot.scheme(), ColorScheme::Dark);
                break;
            }
        }
        // A later failed refresh reports status without replacing the last
        // successful snapshot or interrupting the established terminal route.
        state.config.write().unwrap().appearance.font_family = "x".repeat(4097);
        state.client_state.broadcast_events(
            &[splitty_core::protocol::ServerEvent::settings_changed(
                json!({}),
            )],
            None,
        );
        loop {
            match next_update(&mut updates).await {
                Update::AppearanceError { connection, .. } => {
                    assert_eq!(connection, model.connection_id());
                    break;
                }
                Update::Appearance(_) => panic!("failed refresh replaced last valid appearance"),
                _ => {}
            }
        }
        commands
            .send(Command::new(
                scope,
                CommandKind::Input {
                    route: TerminalToken::new(scope, &pane, &session),
                    bytes: b"printf 'REFRESH_%s\\n' 'ALIVE'\n".into(),
                },
            ))
            .await
            .unwrap();
        output_contains(&mut updates, &session, b"REFRESH_ALIVE").await;
        stop.store(true, Ordering::Release);
        tokio::time::timeout(Duration::from_secs(10), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(cleanup.load(Ordering::Acquire));
        assert!(state.client_state.clients.lock().unwrap().is_empty());
        drop(updates);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn worker_maximize_filters_stale_visibility_and_size_reports_without_losing_output() {
        if !splitty_core::pty::testing::pty_available() {
            return;
        }
        let (commands, receiver) = mpsc::channel(32);
        let (sender, mut updates) = mpsc::channel(64);
        let service = crate::EmbeddedService::start();
        let mut other = service.connect().await.unwrap();
        session_kind(&mut other, "mux.welcome").await;
        // Register the competing client before the worker. Its later size
        // report is not legacy activity, so the worker remains the most recent
        // active claimant under both Active and Latest policies.
        other.activity(false).unwrap();
        other.request_workspace().unwrap();
        session_kind(&mut other, "mux.workspace_snapshot").await;
        let stop = Arc::new(AtomicBool::new(false));
        let cleanup = Arc::new(AtomicBool::new(false));
        let worker = tokio::spawn(run_service(
            service,
            receiver,
            sender,
            stop.clone(),
            cleanup.clone(),
            ColorScheme::Dark,
        ));
        let first = stable_model(&mut updates).await;
        let scope = first.source_token();
        let screen = first.shown_screen_id().unwrap().to_owned();
        let connection = first.connection_id();
        let pane_a = first.routes()[0].pane_id.clone();
        commands
            .send(Command::new(
                scope,
                CommandKind::Split {
                    screen: first.screen_key(&screen),
                    pane: pane_a.clone(),
                    axis: Axis::Horizontal,
                },
            ))
            .await
            .unwrap();
        let split = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if let Update::Workspace(model) = next_update(&mut updates).await {
                    let model = local_projection(&model);
                    if model.routes().len() == 2
                        && model
                            .routes()
                            .iter()
                            .all(|route| model.session_ready(&route.session_id))
                    {
                        break model;
                    }
                }
            }
        })
        .await
        .expect("split and attachments");
        let pane_b = split
            .routes()
            .into_iter()
            .find(|route| route.pane_id != pane_a)
            .unwrap();
        let slot_a = split
            .screen(&screen)
            .unwrap()
            .snapshot()
            .unwrap()
            .root
            .find_slot(&splitty_core::ids::PaneId::from(pane_a.as_str()))
            .unwrap()
            .id()
            .to_string();
        assert_eq!(split.displayed_panes().len(), 2);
        commands
            .send(Command::new(
                scope,
                CommandKind::Input {
                    route: TerminalToken::new(scope, &pane_a, &pane_b.session_id),
                    bytes: vec![0, 0x80, 0xff],
                },
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Update::Status(status) = next_update(&mut updates).await {
                    if status.contains("Terminal is not attached") {
                        break;
                    }
                }
            }
        })
        .await
        .expect("raw input with a stale pane/session pair is rejected");
        other.attach(&pane_b.session_id).unwrap();
        session_kind(&mut other, "attached").await;
        other.resize(&pane_b.session_id, 120, 40, true).unwrap();
        // The same ordered stream makes the snapshot a barrier for this size
        // report before the worker publishes its smaller 73x19 claim.
        other.resync(&screen).unwrap();
        session_kind(&mut other, "mux.screen_snapshot").await;
        for (pane, session, cols, rows) in [
            (&pane_a, split.session_for(&pane_a).unwrap(), 91, 29),
            (&pane_b.pane_id, pane_b.session_id.as_str(), 73, 19),
        ] {
            commands
                .send(Command::new(
                    scope,
                    CommandKind::ResizePane {
                        route_generation: 0,
                        screen: first.screen_key(&screen),
                        route: TerminalToken::new(scope, pane, session),
                        cols,
                        rows,
                    },
                ))
                .await
                .unwrap();
        }
        commands
            .send(Command::new(
                scope,
                CommandKind::Visible {
                    route_generation: split.route_generation(),
                    screen: Some(first.screen_key(&screen)),
                    panes: vec![pane_a.clone(), pane_b.pane_id.clone()],
                },
            ))
            .await
            .unwrap();
        commands
            .send(Command::new(
                scope,
                CommandKind::Input {
                    route: TerminalToken::new(scope, &pane_b.pane_id, &pane_b.session_id),
                    bytes: b"printf 'BEFORE_%s\\n' \"$(stty size)\"\n".into(),
                },
            ))
            .await
            .unwrap();
        output_contains(&mut updates, &pane_b.session_id, b"BEFORE_19 73").await;

        commands
            .send(Command::new(
                scope,
                CommandKind::SetMaximized {
                    screen: screen.clone(),
                    connection,
                    slot: Some(slot_a.clone()),
                    pane: Some(pane_a.clone()),
                },
            ))
            .await
            .unwrap();
        loop {
            if let Update::Workspace(model) = next_update(&mut updates).await {
                let model = local_projection(&model);
                if model.maximized_slot_id() == Some(slot_a.as_str()) {
                    assert_eq!(model.displayed_panes(), vec![pane_a.clone()]);
                    assert_eq!(
                        model.session_for(&pane_b.pane_id),
                        Some(pane_b.session_id.as_str())
                    );
                    break;
                }
            }
        }
        // Delayed pre-maximize UI reports may arrive, but cannot reclaim or
        // resize the hidden group. Its session remains attached for output.
        commands
            .send(Command::new(
                scope,
                CommandKind::Visible {
                    route_generation: split.route_generation(),
                    screen: Some(first.screen_key(&screen)),
                    panes: vec![pane_a.clone(), pane_b.pane_id.clone()],
                },
            ))
            .await
            .unwrap();
        commands
            .send(Command::new(
                scope,
                CommandKind::ResizePane {
                    route_generation: 0,
                    screen: first.screen_key(&screen),
                    route: TerminalToken::new(scope, &pane_b.pane_id, &pane_b.session_id),
                    cols: 15,
                    rows: 15,
                },
            ))
            .await
            .unwrap();
        commands
            .send(Command::new(
                scope,
                CommandKind::Input {
                    route: TerminalToken::new(scope, &pane_b.pane_id, &pane_b.session_id),
                    bytes: b"printf 'HIDDEN_%s\\n' \"$(stty size)\"\n".into(),
                },
            ))
            .await
            .unwrap();
        // With no competing claim the PTY could retain 73x19 even if hide was
        // omitted. 120x40 proves this worker really withdrew its claim while
        // keeping hidden output attached and rejecting stale 15x15 reports.
        output_contains(&mut updates, &pane_b.session_id, b"HIDDEN_40 120").await;
        commands
            .send(Command::new(
                scope,
                CommandKind::SetMaximized {
                    screen: screen.clone(),
                    connection,
                    slot: None,
                    pane: None,
                },
            ))
            .await
            .unwrap();
        loop {
            if let Update::Workspace(model) = next_update(&mut updates).await {
                let model = local_projection(&model);
                if model.maximized_slot_id().is_none() && model.displayed_panes().len() == 2 {
                    break;
                }
            }
        }
        commands
            .send(Command::new(
                scope,
                CommandKind::Visible {
                    screen: Some(first.screen_key(&screen)),
                    route_generation: split.route_generation(),
                    panes: vec![pane_a, pane_b.pane_id.clone()],
                },
            ))
            .await
            .unwrap();
        commands
            .send(Command::new(
                scope,
                CommandKind::Input {
                    route: TerminalToken::new(scope, &pane_b.pane_id, &pane_b.session_id),
                    bytes: b"printf 'RESTORED_%s\\n' \"$(stty size)\"\n".into(),
                },
            ))
            .await
            .unwrap();
        output_contains(&mut updates, &pane_b.session_id, b"RESTORED_19 73").await;
        other.finish().await.unwrap();
        stop.store(true, Ordering::Release);
        tokio::time::timeout(Duration::from_secs(10), worker)
            .await
            .expect("owned worker cleanup")
            .unwrap()
            .unwrap();
        assert!(cleanup.load(Ordering::Acquire));
    }

    #[test]
    fn pre_discovery_close_requires_exact_confirmed_unknown_window_scope() {
        let plan = ClosePlan::startup_window(3);
        assert!(validate_pre_discovery_close(&plan, true).is_ok());
        assert!(validate_pre_discovery_close(&plan, false).is_err());
        let mut changed = plan;
        changed.target = CloseTarget::Screen {
            screen: "other".into(),
        };
        assert!(validate_pre_discovery_close(&changed, true).is_err());
    }

    #[tokio::test]
    async fn early_run_failure_still_awaits_real_owned_service_cleanup() {
        let service = crate::EmbeddedService::start();
        let completed = AtomicBool::new(false);
        let stop = AtomicBool::new(false);
        let error = finish_owned_run(
            service.into(),
            Err(anyhow::anyhow!("connection failed before client admission")),
            Ok(()),
            &completed,
            &stop,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("connection failed"));
        assert!(
            completed.load(Ordering::Acquire),
            "actual service.close completed before failure returned"
        );
    }

    #[tokio::test]
    async fn rejected_close_keeps_worker_available_for_new_approved_close() {
        let (commands, receiver) = mpsc::channel(16);
        let (sender, mut updates) = mpsc::channel(64);
        let worker = Worker::start(receiver, sender);
        let model = stable_model(&mut updates).await;
        let scope = model.source_token();
        commands
            .send(Command {
                source: None,
                kind: CommandKind::RequestClose {
                    plan: ClosePlan::startup_window(1),
                    confirmed: false,
                },
            })
            .await
            .unwrap();
        loop {
            if let Update::CloseRejected { request_id, .. } = next_update(&mut updates).await {
                assert_eq!(request_id, 1);
                break;
            }
        }
        let plan = model.assess_close(2, CloseTarget::Window).unwrap();
        commands
            .send(Command {
                source: Some(scope),
                kind: CommandKind::RequestClose {
                    plan,
                    confirmed: true,
                },
            })
            .await
            .unwrap();
        loop {
            match next_update(&mut updates).await {
                Update::WindowCloseApproved { request_id, .. } => {
                    assert_eq!(request_id, 2);
                    break;
                }
                Update::CloseRejected { reason, .. } => {
                    panic!("healthy worker rejected fresh close: {reason}")
                }
                _ => {}
            }
        }
        worker.shutdown().unwrap();
    }

    #[test]
    fn old_snapshot_and_unscoped_late_failure_wait_for_ordered_close_barrier() {
        let target = CloseTarget::Pane {
            screen: "screen".into(),
            pane: "pane".into(),
        };
        let mut close = PendingClose {
            source: None,
            request_id: 7,
            target: target.clone(),
            barrier_generation: 4,
            catalog_barrier_seen: false,
            failure: None,
        };
        assert!(close.is_duplicate(7));
        assert!(!close.is_duplicate(8));
        assert!(
            close.settlement(true, true, false).is_none(),
            "old removed-pane snapshot cannot settle"
        );
        close.observe_failure(&json!({"op":"close_pane","screenId":"screen"}));
        assert!(
            close.failure.is_some(),
            "paneId is not required in server failure"
        );
        assert!(!close.observe_catalog(Some(3)));
        assert!(
            close.settlement(true, true, false).is_none(),
            "old catalog cannot release failure"
        );
        assert!(close.observe_catalog(Some(4)));
        assert!(close.settlement(true, false, true).unwrap().is_err());
        let mut next = PendingClose {
            source: None,
            request_id: 8,
            target,
            barrier_generation: 5,
            catalog_barrier_seen: false,
            failure: None,
        };
        assert!(!next.observe_catalog(Some(4)));
        assert!(next.settlement(true, true, false).is_none());
        assert!(next.observe_catalog(Some(5)));
        assert!(
            next.settlement(true, false, false).is_none(),
            "rejected or absent snapshot cannot prove pane removal"
        );
        assert!(next.settlement(true, true, false).unwrap().is_ok());
    }

    #[tokio::test]
    async fn stale_close_request_cannot_lower_worker_high_water() {
        let (commands, receiver) = mpsc::channel(16);
        let (sender, mut updates) = mpsc::channel(64);
        let worker = Worker::start(receiver, sender);
        let model = loop {
            if let Update::Workspace(model) = next_update(&mut updates).await {
                let model = local_projection(&model);
                break model;
            }
        };
        let invalid = |request_id| {
            let mut plan = model.assess_close(request_id, CloseTarget::Window).unwrap();
            plan.affected.clear();
            Command::close(plan, true)
        };
        commands.send(invalid(10)).await.unwrap();
        let mut rejections = Vec::new();
        for request in [2, 5, 10] {
            commands.send(invalid(request)).await.unwrap();
        }
        while rejections.len() < 4 {
            if let Update::CloseRejected {
                request_id, reason, ..
            } = next_update(&mut updates).await
            {
                rejections.push((request_id, reason));
            }
        }
        assert_eq!(
            rejections.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![10, 2, 5, 10]
        );
        assert!(rejections[1..]
            .iter()
            .all(|(_, reason)| reason.contains("stale")));
        assert!(model.connection_id().to_string().len() > 10);
        worker.shutdown().unwrap();
    }

    #[tokio::test]
    async fn approved_window_close_completes_owned_service_cleanup() {
        let (commands, receiver) = mpsc::channel(16);
        let (sender, mut updates) = mpsc::channel(64);
        let worker = Worker::start(receiver, sender);
        let model = stable_model(&mut updates).await;
        let scope = model.source_token();
        let plan = model.assess_close(1, CloseTarget::Window).unwrap();
        commands
            .send(Command {
                source: Some(scope),
                kind: CommandKind::RequestClose {
                    plan: plan.clone(),
                    confirmed: true,
                },
            })
            .await
            .unwrap();
        commands
            .send(Command::new(
                scope,
                CommandKind::CreateScreen("must not run".into()),
            ))
            .await
            .unwrap();
        loop {
            if let Update::WindowCloseApproved { request_id, source } =
                next_update(&mut updates).await
            {
                assert_eq!(request_id, 1);
                assert_eq!(source, plan.source_token());
                break;
            }
        }
        let completed = worker.cleanup_complete.clone();
        loop {
            match tokio::time::timeout(Duration::from_secs(10), updates.recv())
                .await
                .expect("owned worker did not finish after approval")
            {
                Some(Update::Workspace(model)) => {
                    let model = local_projection(&model);
                    assert!(
                        model
                            .screens()
                            .iter()
                            .all(|screen| screen.name.as_deref() != Some("must not run")),
                        "a mutation queued after approved close must not execute"
                    );
                }
                Some(_) => {}
                None => break,
            }
        }
        assert!(
            completed.load(Ordering::Acquire),
            "owned cleanup completed before update channel closure"
        );
        worker.shutdown().unwrap();
    }
}

#[cfg(all(test, not(target_os = "windows")))]
mod profile_launch_tests;

#[cfg(all(test, not(target_os = "windows")))]
mod zoom_tests;

#[cfg(all(test, not(target_os = "windows")))]
mod transfer_tests;

#[cfg(all(test, not(target_os = "windows")))]
mod navigation_tests;

#[cfg(all(test, not(target_os = "windows")))]
mod hub_tests;

#[cfg(all(test, not(target_os = "windows")))]
mod floating_tests;
#[cfg(all(test, not(target_os = "windows")))]
mod template_tests;

#[cfg(all(test, not(target_os = "windows")))]
mod source_tests;

#[cfg(all(test, target_os = "windows"))]
mod windows_startup_tests {
    use super::*;
    #[test]
    fn missing_explicit_backend_finishes_but_does_not_claim_successful_cleanup() {
        let (_commands, receiver) = mpsc::channel(4);
        let (updates, _receiver) = mpsc::channel(4);
        let worker = Worker::start(receiver, updates);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !worker.finished.load(Ordering::Acquire) {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!worker.cleanup_complete.load(Ordering::Acquire));
        assert!(worker.shutdown().is_err());
    }
}

#[cfg(all(test, not(target_os = "windows")))]
mod ports_tests;
