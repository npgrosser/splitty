//! Own the local service and route workspace intents on one ordered connection.
use crate::{
    appearance::{AppearanceSnapshot, ColorScheme},
    shell_profiles::ShellProfilesSnapshot,
    workspace::{
        Axis, ClosePlan, CloseTarget, LocalWorkspace, MovePlacement, PaneDrag, PlacementOutcome,
        TabStep,
    },
    zoom::{ZoomAction, ZoomResult},
    Event,
};
#[cfg(not(target_os = "windows"))]
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

pub enum Command {
    Zoom {
        connection: uuid::Uuid,
        request_id: uuid::Uuid,
        action: ZoomAction,
    },
    SetColorScheme {
        connection: uuid::Uuid,
        scheme: ColorScheme,
    },
    CreateScreen(String),
    SwitchScreen(String),
    RenameScreen {
        screen: String,
        connection: String,
        name: String,
    },
    Split {
        pane: String,
        axis: Axis,
    },
    Tab(String),
    ProfileTab {
        connection: uuid::Uuid,
        screen: String,
        pane: String,
        session: String,
        profile_id: String,
    },
    NewTerminal {
        screen: String,
        connection: String,
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
        node: String,
        sizes: Vec<f64>,
    },
    Select(String),
    Focus(String),
    Input {
        pane: String,
        session: String,
        bytes: Vec<u8>,
    },
    ResizePane {
        pane: String,
        session: String,
        cols: u16,
        rows: u16,
    },
    Visible {
        screen: Option<String>,
        panes: Vec<String>,
    },
}
pub enum Update {
    Zoom(ZoomResult),
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
    Workspace(Box<LocalWorkspace>),
    Event(Box<Event>),
    Attached(String),
    Status(String),
    CloseSubmitted {
        request_id: u64,
        connection_id: Option<uuid::Uuid>,
    },
    CloseRejected {
        request_id: u64,
        reason: String,
    },
    CloseSettled {
        request_id: u64,
    },
    WindowCloseApproved {
        request_id: u64,
        connection_id: Option<uuid::Uuid>,
    },
}
pub struct Worker {
    pub stop: Arc<AtomicBool>,
    pub cleanup_complete: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<anyhow::Result<()>>>,
}

#[cfg(not(target_os = "windows"))]
fn profile_fallback_status(
    model: &LocalWorkspace,
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
// Setup and reads stay owned across cancelled select branches. Both streams
// share the embedded service's admission domain, but never consume each other's events.
#[cfg(not(target_os = "windows"))]
type LocalSettingsSetup<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = std::io::Result<crate::local_settings::LocalSettingsClient>,
            > + Send
            + 'a,
    >,
>;
#[cfg(not(target_os = "windows"))]
struct RuntimeLocalSettings<'a> {
    service: &'a crate::EmbeddedService,
    connection: uuid::Uuid,
    scheme: ColorScheme,
    setup: Option<LocalSettingsSetup<'a>>,
    reader: Option<crate::local_settings::LocalSettingsClient>,
    dirty: bool,
    settings_dirty: bool,
}
#[cfg(not(target_os = "windows"))]
impl<'a> RuntimeLocalSettings<'a> {
    fn new(
        service: &'a crate::EmbeddedService,
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

    async fn next_update(&mut self) -> std::io::Result<crate::local_settings::LocalSettingsUpdate> {
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
                        .local_settings_client(self.connection, self.scheme),
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

    async fn finish(mut self) -> std::io::Result<()> {
        // Completing a started, time-bounded handshake lets us await its service
        // cleanup explicitly. Dropping setup would only schedule Drop cleanup.
        if let Some(setup) = self.setup.take() {
            // The connector awaits failed setup cleanup before returning an
            // initialization error. Local settings initialization remains optional
            // when it happens to finish during close, just as during operation.
            self.reader = setup.await.ok();
        }
        match self.reader {
            Some(reader) => reader.finish().await,
            None => Ok(()),
        }
    }
}

#[cfg(not(target_os = "windows"))]
struct PendingClose {
    request_id: u64,
    target: CloseTarget,
    barrier_generation: u64,
    catalog_barrier_seen: bool,
    failure: Option<String>,
}
#[cfg(not(target_os = "windows"))]
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

#[cfg(not(target_os = "windows"))]
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

#[cfg(not(target_os = "windows"))]
async fn finish_owned_run(
    service: crate::EmbeddedService,
    result: anyhow::Result<()>,
    cleanup_complete: &AtomicBool,
    stop: &AtomicBool,
) -> anyhow::Result<()> {
    let cleanup = service.close().await;
    if cleanup.is_ok() {
        cleanup_complete.store(true, Ordering::Release);
    }
    cleanup?;
    if stop.load(Ordering::Acquire) {
        Ok(())
    } else {
        result
    }
}
impl Worker {
    pub fn start(commands: mpsc::Receiver<Command>, updates: mpsc::Sender<Update>) -> Self {
        Self::start_with_scheme(commands, updates, ColorScheme::Dark)
    }

    pub fn start_with_scheme(
        commands: mpsc::Receiver<Command>,
        updates: mpsc::Sender<Update>,
        scheme: ColorScheme,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let cleanup_complete = Arc::new(AtomicBool::new(false));
        let completed = cleanup_complete.clone();
        let thread = thread::spawn(move || {
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
                let result = run(commands, updates.clone(), stopping, completed, scheme).await;
                if let Err(error) = &result {
                    let _ = updates.try_send(Update::Status(format!("Local service: {error:#}")));
                }
                result
            })
        });
        Self {
            stop,
            cleanup_complete,
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

#[cfg(not(target_os = "windows"))]
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

#[cfg(not(target_os = "windows"))]
async fn run(
    commands: mpsc::Receiver<Command>,
    updates: mpsc::Sender<Update>,
    stop: Arc<AtomicBool>,
    cleanup_complete: Arc<AtomicBool>,
    scheme: ColorScheme,
) -> anyhow::Result<()> {
    run_service(
        crate::EmbeddedService::start(),
        commands,
        updates,
        stop,
        cleanup_complete,
        scheme,
    )
    .await
}

#[cfg(not(target_os = "windows"))]
async fn run_service(
    service: crate::EmbeddedService,
    mut commands: mpsc::Receiver<Command>,
    updates: mpsc::Sender<Update>,
    stop: Arc<AtomicBool>,
    cleanup_complete: Arc<AtomicBool>,
    scheme: ColorScheme,
) -> anyhow::Result<()> {
    use crate::workspace::WorkspaceDelta;
    let mut client = match service.connect().await {
        Ok(client) => client,
        Err(error) => {
            return finish_owned_run(service, Err(error.into()), &cleanup_complete, &stop).await
        }
    };
    let mut settings = None::<RuntimeLocalSettings<'_>>;
    let result = async {
        let mut model = None::<LocalWorkspace>;
        let mut attached = HashSet::<String>::new();
        let mut visible = HashSet::<String>::new();
        let mut desired_visible = HashSet::<String>::new();
        let mut owned_sessions = HashSet::<String>::new();
        let mut closed_sessions = HashSet::<String>::new();
        let mut pending_hide = HashSet::<String>::new();
        let mut pending_resize = HashSet::<String>::new();
        let mut activity_pending = false;
        let mut sizes = HashMap::<String, (u16, u16)>::new();
        let mut refresh_error = None::<String>;
        let mut control_error = None::<String>;
        let mut tick = tokio::time::interval(Duration::from_millis(16));
        let mut last_close_request = 0_u64;
        let mut pending_close = None::<PendingClose>;
        enum Next {
            Settings(std::io::Result<crate::local_settings::LocalSettingsUpdate>),
            Event(std::io::Result<Option<Box<Event>>>),
            Command(Option<Command>),
            Tick,
        }
        loop {
            let next = tokio::select! {
                result = async {
                    match &mut settings {
                        Some(reader) => reader.next_update().await,
                        None => std::future::pending().await,
                    }
                } => Next::Settings(result),
                event = client.next_event() => Next::Event(event.map(|event| event.map(Box::new))),
                command = commands.recv() => Next::Command(command),
                _ = tick.tick() => Next::Tick,
            };
            if stop.load(Ordering::Acquire) {
                break;
            }
            let mut close_window = false;
            match next {
                Next::Settings(result) => {
                    use crate::local_settings::LocalSettingsUpdate;
                    let connection = client.connection_id();
                    let update = match result {
                        Ok(LocalSettingsUpdate::Zoom(result)) => Update::Zoom(result),
                        Ok(LocalSettingsUpdate::Appearance(Ok(snapshot))) => Update::Appearance(snapshot),
                        Ok(LocalSettingsUpdate::Appearance(Err(error))) => Update::AppearanceError { connection, reason: error.to_string() },
                        Ok(LocalSettingsUpdate::ShellProfiles(Ok(snapshot))) => Update::ShellProfiles(snapshot),
                        Ok(LocalSettingsUpdate::ShellProfiles(Err(error))) => Update::ShellProfilesError { connection, reason: error.to_string() },
                        Err(error) => {
                            // Setup belongs to both reads; report each feature once.
                            publish(&updates, Update::ShellProfilesError { connection, reason: error.to_string() }, &stop).await?;
                            Update::AppearanceError { connection, reason: error.to_string() }
                        }
                    };
                    publish(&updates, update, &stop).await?;
                }
                Next::Tick => {}
                Next::Command(None) => break,
                Next::Command(Some(command)) => {
                    if let Command::RequestClose { plan, confirmed } = command {
                        let request_id = plan.request_id;
                        if pending_close.as_ref().is_some_and(|close| close.is_duplicate(request_id)) {
                            // The original is still awaiting authoritative settlement.
                            // Rejecting this duplicate would clear the UI's matching pending request.
                            continue;
                        }
                        let result = if pending_close.is_some() {
                            Err(std::io::Error::new(
                                std::io::ErrorKind::WouldBlock,
                                "previous close still settling",
                            ))
                        } else if request_id <= last_close_request {
                            Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidInput,
                                "stale close request",
                            ))
                        } else if let Some(model) = &mut model {
                            model.validate_close(&plan, confirmed).and_then(|()| {
                                match &plan.target {
                                    CloseTarget::Pane { screen, pane } => {
                                        model.close_pane_on_screen(&client, screen, pane)
                                    }
                                    CloseTarget::Screen { screen } => {
                                        model.close_screen(&client, screen)
                                    }
                                    CloseTarget::Window => Ok(()),
                                }
                            })
                        } else {
                            validate_pre_discovery_close(&plan, confirmed)
                        };
                        last_close_request = last_close_request.max(request_id);
                        match result {
                            Ok(()) => {
                                if plan.target == CloseTarget::Window {
                                    close_window = true;
                                    publish(
                                        &updates,
                                        Update::WindowCloseApproved {
                                            request_id,
                                            connection_id: plan.connection_id,
                                        },
                                        &stop,
                                    )
                                    .await?;
                                } else {
                                    let generation = model
                                        .as_ref()
                                        .map_or(0, LocalWorkspace::mutation_generation);
                                    pending_close = Some(PendingClose {
                                        request_id,
                                        target: plan.target,
                                        barrier_generation: generation,
                                        catalog_barrier_seen: false,
                                        failure: None,
                                    });
                                    publish(
                                        &updates,
                                        Update::CloseSubmitted {
                                            request_id,
                                            connection_id: plan.connection_id,
                                        },
                                        &stop,
                                    )
                                    .await?;
                                }
                            }
                            Err(error) => {
                                publish(
                                    &updates,
                                    Update::CloseRejected {
                                        request_id,
                                        reason: error.to_string(),
                                    },
                                    &stop,
                                )
                                .await?;
                            }
                        }
                        if close_window {
                            break;
                        }
                        continue;
                    }
                    if let Command::Zoom { connection, request_id, action } = command {
                        let result = settings.as_mut()
                            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::WouldBlock, "local settings are not ready"))
                            .and_then(|settings| settings.enqueue_zoom(connection, request_id, action));
                        if let Err(error) = result {
                            publish(&updates, Update::Zoom(ZoomResult::rejected(connection, request_id, error.to_string())), &stop).await?;
                        }
                        continue;
                    }
                    if let Command::SetColorScheme { connection, scheme } = command {
                        if let Some(reader) = &mut settings {
                            // A delayed command from another owner cannot choose app settings.
                            if let Err(error) = reader.set_scheme(connection, scheme) {
                                publish(&updates, Update::AppearanceError { connection, reason: error.to_string() }, &stop).await?;
                            }
                        }
                        continue;
                    }
                    let Some(model) = &mut model else {
                        continue;
                    };
                    let result: std::io::Result<()> = match command {
                        Command::Zoom { .. } | Command::SetColorScheme { .. } | Command::RequestClose { .. } => {
                            unreachable!("close is handled before normal commands")
                        }
                        Command::CreateScreen(name) => {
                            let result = model.create_screen(&client, &name).map(|_| ());
                            if result.is_ok() {
                                publish(
                                    &updates,
                                    Update::Workspace(Box::new(model.clone())),
                                    &stop,
                                )
                                .await?;
                            }
                            result
                        }
                        Command::SwitchScreen(screen) => {
                            match model.switch_screen(&client, &screen) {
                                Ok(changed) => {
                                    if changed {
                                        // Reconcile withdraws old claims before processing the
                                        // next event/command. Old UI reports carry their screen.
                                        desired_visible.clear();
                                        publish(
                                            &updates,
                                            Update::Workspace(Box::new(model.clone())),
                                            &stop,
                                        )
                                        .await?;
                                    }
                                    Ok(())
                                }
                                Err(error) => Err(error),
                            }
                        }
                        Command::RenameScreen {
                            screen,
                            connection,
                            name,
                        } => {
                            if model.connection_id().to_string() == connection {
                                model.rename_screen(&client, &screen, &name)
                            } else {
                                Err(std::io::Error::new(
                                    std::io::ErrorKind::InvalidInput,
                                    "Rename belongs to a previous connection",
                                ))
                            }
                        }
                        Command::Split { pane, axis } => {
                            let result = model.split(&client, &pane, axis).map(|_| ());
                            if result.is_ok() {
                                publish(&updates, Update::Workspace(Box::new(model.clone())), &stop).await?;
                            }
                            result
                        }
                        Command::Tab(pane) => model.new_tab(&client, &pane).map(|_| ()),
                        Command::ProfileTab { connection, screen, pane, session, profile_id } => {
                            if connection != model.connection_id()
                                || model.shown_screen_id() != Some(screen.as_str())
                                || model.screen_for(&pane) != Some(screen.as_str())
                                || model.session_for(&pane) != Some(session.as_str())
                                || !model.is_displayed_pane(&pane)
                            {
                                Err(std::io::Error::new(
                                    std::io::ErrorKind::InvalidInput,
                                    "Shell profile choice belongs to a previous terminal",
                                ))
                            } else {
                                model.new_tab_with_profile(&client, &pane, &profile_id).map(|_| ())
                            }
                        }
                        Command::PlaceTab { drag, target, before } => {
                            let result = model.place_tab(&client, drag, target, before);
                            if result.as_ref().is_ok_and(|moved| *moved) {
                                publish(&updates, Update::Workspace(Box::new(model.clone())), &stop).await?;
                            }
                            result.map(|_| ())
                        }
                        Command::CycleTab { connection, screen, slot, step } => {
                            match model.cycle_tab(connection, &screen, &slot, step) {
                                Ok(true) => {
                                    let result = model.sync_selected_tabs(&client);
                                    publish(&updates, Update::Workspace(Box::new(model.clone())), &stop).await?;
                                    result
                                }
                                Ok(false) => Ok(()),
                                Err(error) => Err(error),
                            }
                        }
                        Command::MovePane {
                            drag,
                            target,
                            placement,
                        } => {
                            let result = model.move_pane(&client, drag, target, placement);
                            if result.as_ref().is_ok_and(|moved| *moved) {
                                publish(&updates, Update::Workspace(Box::new(model.clone())), &stop).await?;
                            }
                            result.map(|_| ())
                        }
                        Command::SetMaximized { screen, connection, slot, pane } => {
                            if model.connection_id() == connection
                                && model.shown_screen_id() == Some(screen.as_str())
                                && model.set_maximized_slot(slot.as_deref(), pane.as_deref())
                            {
                                publish(&updates, Update::Workspace(Box::new(model.clone())), &stop).await?;
                            }
                            Ok(())
                        }
                        Command::NewTerminal { screen, connection } => {
                            if model.connection_id().to_string() == connection
                                && model.shown_screen_id() == Some(screen.as_str())
                            {
                                model.new_terminal(&client).map(|_| ())
                            } else {
                                Ok(())
                            }
                        }
                        Command::ResizeSplit { node, sizes } => {
                            model.resize_split(&client, &node, sizes)
                        }
                        Command::Select(pane) => {
                            model.select_tab(&pane);
                            let result = model.sync_selected_tabs(&client);
                            publish(&updates, Update::Workspace(Box::new(model.clone())), &stop)
                                .await?;
                            result
                        }
                        Command::Focus(pane) => {
                            model.focus_pane(&pane);
                            publish(&updates, Update::Workspace(Box::new(model.clone())), &stop)
                                .await?;
                            Ok(())
                        }
                        Command::Input {
                            pane,
                            session,
                            bytes,
                        } => {
                            if model.session_for(&pane) == Some(session.as_str())
                                && attached.contains(&session)
                            {
                                client.input_bytes(&session, &bytes)
                            } else {
                                Err(std::io::Error::new(
                                    std::io::ErrorKind::WouldBlock,
                                    "Terminal is not attached",
                                ))
                            }
                        }
                        Command::ResizePane {
                            pane,
                            session,
                            cols,
                            rows,
                        } => {
                            if model.session_for(&pane) == Some(session.as_str())
                                && model.is_displayed_pane(&pane)
                            {
                                sizes.insert(session.clone(), (cols, rows));
                                pending_resize.insert(session);
                                Ok(())
                            } else {
                                Ok(())
                            }
                        }
                        Command::Visible { screen, panes } => {
                            if screen.as_deref() != model.shown_screen_id() {
                                Ok(())
                            } else {
                                desired_visible = panes
                                    .into_iter()
                                    .filter(|pane| model.is_displayed_pane(pane))
                                    .collect();
                                Ok(())
                            }
                        }
                    };
                    if let Err(error) = result {
                        publish(&updates, Update::Status(error.to_string()), &stop).await?;
                    }
                }
                Next::Event(event) => {
                    let event = event?.ok_or_else(|| anyhow::anyhow!("Local connection closed"))?;
                    let mut delta = WorkspaceDelta::default();
                    let mut accepted_close_snapshot = false;
                    if let Some(json) = event.json() {
                        match json["type"].as_str() {
                            Some("settings.changed") => {
                                if let Some(reader) = &mut settings {
                                    if let Err(error) = reader.settings_changed(event.connection_id) {
                                        publish(&updates, Update::AppearanceError { connection: event.connection_id, reason: error.to_string() }, &stop).await?;
                                    }
                                }
                            }
                            Some("mux.welcome") if model.is_none() => {
                                let mut workspace = LocalWorkspace::new(client.connection_id());
                                let supports_tab_selection =
                                    json["capabilities"].as_array().is_some_and(|capabilities| {
                                        capabilities.iter().any(|capability| {
                                            capability.as_str() == Some("tab-select-v1")
                                        })
                                    });
                                workspace.set_tab_selection_supported(supports_tab_selection);
                                workspace.set_tab_placement_supported(json["capabilities"].as_array().is_some_and(|capabilities| {
                                    capabilities.iter().any(|capability| capability.as_str() == Some("tab-placement-v1"))
                                }));
                                let supports_catalog =
                                    json["capabilities"].as_array().is_some_and(|capabilities| {
                                        capabilities.iter().any(|capability| {
                                            capability.as_str() == Some("workspace-catalog-v1")
                                        })
                                    });
                                workspace.set_catalog_supported(supports_catalog);
                                if supports_catalog {
                                    workspace.mark_catalog_dirty();
                                } else {
                                    publish(
                                        &updates,
                                        Update::Status(
                                            "Local service does not support workspace-catalog-v1"
                                                .into(),
                                        ),
                                        &stop,
                                    )
                                    .await?;
                                }
                                publish(
                                    &updates,
                                    Update::Workspace(Box::new(workspace.clone())),
                                    &stop,
                                )
                                .await?;
                                model = Some(workspace);
                                // Establish the UI owner before any settings can be published.
                                settings = Some(RuntimeLocalSettings::new(
                                    &service, client.connection_id(), scheme,
                                ));
                            }
                            Some("mux.workspace_snapshot") => {
                                if let Some(model) = &mut model {
                                    let response_generation = model.catalog_request_generation();
                                    let first = !model.catalog_initialized();
                                    match model.apply_catalog(event.connection_id, json) {
                                        Ok(applied) => {
                                            delta = applied;
                                            if let Some(close) = &mut pending_close {
                                                if close.observe_catalog(response_generation) {
                                                    if let CloseTarget::Pane { screen, .. } =
                                                        &close.target
                                                    {
                                                        if model.screen(screen).is_some() {
                                                            model.mark_screen_dirty(screen);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        Err(error) => {
                                            if event.connection_id == model.connection_id() {
                                                if let Some(close) = &mut pending_close {
                                                    if close.observe_catalog(response_generation) {
                                                        close.failure = Some(format!("Close could not be verified: {error}"));
                                                    }
                                                }
                                            }
                                            publish(&updates, Update::Workspace(Box::new(model.clone())), &stop).await?;
                                            publish(
                                                &updates,
                                                Update::Status(format!(
                                                    "Workspace catalog rejected: {error}"
                                                )),
                                                &stop,
                                            )
                                            .await?;
                                        }
                                    }
                                    if first
                                        && model.catalog_initialized()
                                        && model.screens().is_empty()
                                    {
                                        match model.create_screen(&client, "Screen 1") {
                                            Ok(_) => delta.snapshot_changed = true,
                                            Err(error) => {
                                                publish(
                                                    &updates,
                                                    Update::Status(format!(
                                                        "Initial screen not created: {error}"
                                                    )),
                                                    &stop,
                                                )
                                                .await?
                                            }
                                        }
                                    }
                                }
                            }
                            Some("mux.screen_snapshot") => {
                                if let Some(model) = &mut model {
                                    match model.apply_snapshot(event.connection_id, json) {
                                        Ok(applied) => {
                                            delta = applied;
                                            accepted_close_snapshot = delta.snapshot_changed;
                                        }
                                        Err(error) => {
                                            if event.connection_id == model.connection_id() {
                                                if let Some(close) = &mut pending_close {
                                                    if close.catalog_barrier_seen
                                                        && matches!(&close.target, CloseTarget::Pane { screen, .. } if json["screen"]["id"] == *screen)
                                                    {
                                                        close.failure = Some(format!("Close could not be verified: {error}"));
                                                    }
                                                }
                                            }
                                            publish(&updates, Update::Workspace(Box::new(model.clone())), &stop).await?;
                                            publish(
                                                &updates,
                                                Update::Status(format!(
                                                    "Screen snapshot rejected: {error}"
                                                )),
                                                &stop,
                                            )
                                            .await?
                                        }
                                    }
                                }
                            }
                            Some("mux.pane_spawned") => {
                                if let (Some(model), Some(pane), Some(session)) = (
                                    &mut model,
                                    json["paneId"].as_str(),
                                    json["sessionId"].as_str(),
                                ) {
                                    delta = model.observe_spawn(event.connection_id, pane, session);
                                    if let Some(message) = profile_fallback_status(model, event.connection_id, pane, session, json) {
                                        publish(&updates, Update::Status(message), &stop).await?;
                                    }
                                }
                            }
                            Some("mux.nudge") => {
                                if let Some(model) = &mut model {
                                    let id = json["screenId"].as_str();
                                    if matches!(json["reason"].as_str(), Some("mux_op" | "cli_op"))
                                        || id.is_none_or(|id| model.screen(id).is_none())
                                    {
                                        model.mark_catalog_dirty();
                                    }
                                    if let Some(id) = id {
                                        model.mark_screen_dirty(id);
                                    }
                                }
                            }
                            Some("attached") => {
                                if let Some(session) = json["sessionId"].as_str() {
                                    if model.as_mut().is_some_and(|model| {
                                        model.observe_attached(event.connection_id, session)
                                    }) {
                                        attached.insert(session.to_owned());
                                        activity_pending = true;
                                        if model
                                            .as_ref()
                                            .and_then(|model| model.pane_for(session))
                                            .is_some_and(|pane| desired_visible.contains(pane))
                                        {
                                            pending_resize.insert(session.to_owned());
                                        } else {
                                            pending_hide.insert(session.to_owned());
                                        }
                                        if let Some(model) = &model {
                                            publish(
                                                &updates,
                                                Update::Workspace(Box::new(model.clone())),
                                                &stop,
                                            )
                                            .await?;
                                        }
                                        publish(
                                            &updates,
                                            Update::Attached(session.to_owned()),
                                            &stop,
                                        )
                                        .await?;
                                    }
                                }
                            }
                            Some("closed") => {
                                if let Some(session) = json["sessionId"].as_str() {
                                    attached.remove(session);
                                    // Client already forgets this attachment on `closed`.
                                    owned_sessions.remove(session);
                                    closed_sessions.insert(session.to_owned());
                                }
                            }
                            Some("mux.op_failed") | Some("error") => {
                                if json["type"] == "mux.op_failed" {
                                    if let Some(model) = &mut model {
                                        model.observe_placement_failure(event.connection_id, json);
                                    }
                                    if let Some(close) = &mut pending_close {
                                        close.observe_failure(json);
                                    }
                                }
                                if json["type"] == "error" {
                                    if let Some(session) = json["sessionId"].as_str() {
                                        if !attached.contains(session) {
                                            // A server-rejected pending attach is removed by
                                            // Client. Tombstone it until its route changes so
                                            // an absent session does not hot-loop reattach.
                                            owned_sessions.remove(session);
                                            closed_sessions.insert(session.to_owned());
                                            if let Some(model) = &mut model {
                                                if let Some(screen) = model
                                                    .pane_for(session)
                                                    .and_then(|pane| model.screen_for(pane))
                                                    .map(str::to_owned)
                                                {
                                                    model.mark_screen_dirty(&screen);
                                                }
                                            }
                                        }
                                    }
                                }
                                if json["op"] == "move_pane" {
                                    if let Some(model) = &mut model {
                                        model.reject_move(event.connection_id);
                                    }
                                }
                                if let (Some(model), Some(pane)) =
                                    (&mut model, json["paneId"].as_str())
                                {
                                    delta = model.observe_spawn_failure(event.connection_id, pane);
                                }
                                publish(
                                    &updates,
                                    Update::Status(format!("Operation rejected: {json}")),
                                    &stop,
                                )
                                .await?;
                                if let Some(model) = &mut model {
                                    // A rejected lifecycle or pane operation can change
                                    // catalog membership as well as one screen's layout.
                                    model.mark_catalog_dirty();
                                    if let Some(id) = json["screenId"].as_str() {
                                        model.mark_screen_dirty(id);
                                    }
                                }
                            }
                            _ => {}
                        }
                        let settled = pending_close.as_ref().and_then(|close| {
                            let model = model.as_ref()?;
                            let (screen_exists, pane_exists, matching_snapshot) = match &close.target {
                                CloseTarget::Pane { screen, pane } => (
                                    model.screen(screen).is_some(),
                                    model.screen(screen).and_then(|screen| screen.snapshot())
                                        .is_some_and(|snapshot| snapshot.panes.contains_key(&splitty_core::ids::PaneId::from(pane.as_str()))),
                                    accepted_close_snapshot && json["screen"]["id"] == *screen,
                                ),
                                CloseTarget::Screen { screen } => (model.screen(screen).is_some(), false, false),
                                CloseTarget::Window => (false, false, false),
                            };
                            close.settlement(screen_exists, matching_snapshot, pane_exists)
                        });
                        if let Some(settled) = settled {
                            if let Some(close) = pending_close.take() {
                                let update = match settled {
                                    Ok(()) => Update::CloseSettled { request_id: close.request_id },
                                    Err(reason) => Update::CloseRejected { request_id: close.request_id, reason },
                                };
                                publish(&updates, update, &stop).await?;
                            }
                        }
                    }
                    // Attachment changes are reconciled below. Queue pressure
                    // must not discard a route or close the owned connection.
                    if delta.snapshot_changed
                        || delta.placement.is_some()
                        || !delta.added.is_empty()
                        || !delta.removed.is_empty()
                    {
                        if let Some(model) = &model {
                            publish(&updates, Update::Workspace(Box::new(model.clone())), &stop)
                                .await?;
                        }
                    }
                    if let Some(PlacementOutcome::Rejected(reason)) = delta.placement {
                        publish(&updates, Update::Status(reason), &stop).await?;
                    }
                    // The original native accounting stays alive until UI consumption.
                    publish(&updates, Update::Event(event), &stop).await?;
                }
            }
            if let Some(model) = &mut model {
                let desired_sessions: HashSet<String> = model
                    .routes()
                    .into_iter()
                    .map(|route| route.session_id)
                    .collect();
                sizes.retain(|session, _| desired_sessions.contains(session));
                pending_resize.retain(|session| desired_sessions.contains(session));
                desired_visible.retain(|pane| {
                    model.screen_for(pane) == model.shown_screen_id()
                        && model.session_for(pane).is_some()
                        && model.is_displayed_pane(pane)
                });
                closed_sessions.retain(|session| desired_sessions.contains(session));
                let mut pending_error = None::<String>;
                for session in owned_sessions.clone() {
                    if !desired_sessions.contains(&session) {
                        match client.detach(&session) {
                            Ok(()) => {
                                owned_sessions.remove(&session);
                                attached.remove(&session);
                                pending_hide.remove(&session);
                                pending_resize.remove(&session);
                                sizes.remove(&session);
                            }
                            Err(error) => {
                                pending_error
                                    .get_or_insert_with(|| format!("Detach pending: {error}"));
                            }
                        }
                    }
                }
                visible.retain(|pane| model.session_for(pane).is_some());
                let missing: Vec<_> = desired_sessions
                    .difference(&owned_sessions)
                    .cloned()
                    .collect();
                for session in missing {
                    if !closed_sessions.contains(&session) {
                        match client.attach(&session) {
                            Ok(()) => {
                                owned_sessions.insert(session);
                            }
                            Err(error) => {
                                pending_error
                                    .get_or_insert_with(|| format!("Attach pending: {error}"));
                            }
                        }
                    }
                }
                if activity_pending {
                    match client.activity(true) {
                        Ok(()) => activity_pending = false,
                        Err(error) => {
                            pending_error
                                .get_or_insert_with(|| format!("Activity pending: {error}"));
                        }
                    }
                }
                for pane in visible.clone() {
                    if !desired_visible.contains(&pane) {
                        if let Some(session) = model.session_for(&pane) {
                            match client.hide(session) {
                                Ok(()) => {
                                    visible.remove(&pane);
                                }
                                Err(error) => {
                                    pending_error
                                        .get_or_insert_with(|| format!("Hide pending: {error}"));
                                }
                            }
                        } else {
                            visible.remove(&pane);
                        }
                    }
                }
                for session in pending_hide.clone() {
                    if !attached.contains(&session) || !desired_sessions.contains(&session) {
                        pending_hide.remove(&session);
                    } else if model
                        .pane_for(&session)
                        .is_some_and(|pane| desired_visible.contains(pane))
                    {
                        pending_hide.remove(&session);
                        pending_resize.insert(session);
                    } else {
                        match client.hide(&session) {
                            Ok(()) => {
                                pending_hide.remove(&session);
                            }
                            Err(error) => {
                                pending_error
                                    .get_or_insert_with(|| format!("Hide pending: {error}"));
                            }
                        }
                    }
                }
                for pane in &desired_visible {
                    if let Some(session) = model.session_for(pane) {
                        if attached.contains(session)
                            && (!visible.contains(pane) || pending_resize.contains(session))
                        {
                            let (cols, rows) = sizes.get(session).copied().unwrap_or((80, 24));
                            match client.resize(session, cols, rows, true) {
                                Ok(()) => {
                                    visible.insert(pane.clone());
                                    pending_resize.remove(session);
                                }
                                Err(error) => {
                                    pending_error
                                        .get_or_insert_with(|| format!("Resize pending: {error}"));
                                }
                            }
                        }
                    }
                }
                match pending_error {
                    None => control_error = None,
                    Some(status) if control_error.as_deref() == Some(status.as_str()) => {}
                    Some(status) => {
                        publish(&updates, Update::Status(status.clone()), &stop).await?;
                        control_error = Some(status);
                    }
                }
                match model.flush_catalog(&client) {
                    Ok(()) => refresh_error = None,
                    Err(error) => {
                        // Refreshes are read-only and keep their dirty state on
                        // admission failure. Retry them without ending the owned
                        // service or replaying a mutation/input command.
                        let status = format!("Workspace refresh pending: {error}");
                        if refresh_error.as_deref() != Some(status.as_str()) {
                            publish(&updates, Update::Status(status.clone()), &stop).await?;
                            refresh_error = Some(status);
                        }
                    }
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;
    let settings_cleanup = match settings.take() {
        Some(reader) => reader.finish().await.map_err(anyhow::Error::from),
        None => Ok(()),
    };
    drop(settings);
    let client_cleanup = client.finish().await.map_err(anyhow::Error::from);
    finish_owned_run(
        service,
        result.and(settings_cleanup).and(client_cleanup),
        &cleanup_complete,
        &stop,
    )
    .await
}

#[cfg(all(test, not(target_os = "windows")))]
mod close_tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    async fn next_update(updates: &mut mpsc::Receiver<Update>) -> Update {
        tokio::time::timeout(Duration::from_secs(10), updates.recv())
            .await
            .expect("native close update timeout")
            .expect("native close update channel")
    }

    async fn stable_model(updates: &mut mpsc::Receiver<Update>) -> Box<LocalWorkspace> {
        loop {
            if let Update::Workspace(model) = next_update(updates).await {
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
                if let Update::Event(event) = next_update(updates).await {
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
        reader: &mut RuntimeLocalSettings<'_>,
    ) -> std::io::Result<Arc<AppearanceSnapshot>> {
        loop {
            if let crate::local_settings::LocalSettingsUpdate::Appearance(result) =
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
        let service = crate::embedded::EmbeddedService::from_runtime(runtime);
        let connection = uuid::Uuid::new_v4();
        let mut appearance = RuntimeLocalSettings::new(&service, connection, ColorScheme::Light);
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
        let mut appearance = RuntimeLocalSettings::new(&service, connection, ColorScheme::Light);
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
        let service = crate::embedded::EmbeddedService::from_runtime(runtime);
        let mut appearance =
            RuntimeLocalSettings::new(&service, uuid::Uuid::new_v4(), ColorScheme::Dark);
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
    async fn appearance_worker_failure_keeps_output_and_recovers_from_local_settings() {
        if !splitty_core::pty::testing::pty_available() {
            return;
        }
        let (runtime, path) = crate::appearance::local::tests::fixture();
        let state = runtime.state.clone();
        state.config.write().unwrap().appearance.font_family = "x".repeat(4097);
        let service = crate::embedded::EmbeddedService::from_runtime(runtime);
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
        let pane = model.routes()[0].pane_id.clone();
        let session = model.routes()[0].session_id.clone();
        commands
            .send(Command::Input {
                pane: pane.clone(),
                session: session.clone(),
                bytes: b"printf 'APPEARANCE_%s\\n' 'ALIVE'\n".into(),
            })
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
            .send(Command::SetColorScheme {
                connection: uuid::Uuid::new_v4(),
                scheme: ColorScheme::Light,
            })
            .await
            .unwrap();
        commands
            .send(Command::SetColorScheme {
                connection: model.connection_id(),
                scheme: ColorScheme::Dark,
            })
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
            .send(Command::Input {
                pane,
                session: session.clone(),
                bytes: b"printf 'REFRESH_%s\\n' 'ALIVE'\n".into(),
            })
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
        let screen = first.shown_screen_id().unwrap().to_owned();
        let connection = first.connection_id();
        let pane_a = first.routes()[0].pane_id.clone();
        commands
            .send(Command::Split {
                pane: pane_a.clone(),
                axis: Axis::Horizontal,
            })
            .await
            .unwrap();
        let split = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if let Update::Workspace(model) = next_update(&mut updates).await {
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
            .send(Command::Input {
                pane: pane_a.clone(),
                session: pane_b.session_id.clone(),
                bytes: vec![0, 0x80, 0xff],
            })
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
                .send(Command::ResizePane {
                    pane: pane.clone(),
                    session: session.into(),
                    cols,
                    rows,
                })
                .await
                .unwrap();
        }
        commands
            .send(Command::Visible {
                screen: Some(screen.clone()),
                panes: vec![pane_a.clone(), pane_b.pane_id.clone()],
            })
            .await
            .unwrap();
        commands
            .send(Command::Input {
                pane: pane_b.pane_id.clone(),
                session: pane_b.session_id.clone(),
                bytes: b"printf 'BEFORE_%s\\n' \"$(stty size)\"\n".into(),
            })
            .await
            .unwrap();
        output_contains(&mut updates, &pane_b.session_id, b"BEFORE_19 73").await;

        commands
            .send(Command::SetMaximized {
                screen: screen.clone(),
                connection,
                slot: Some(slot_a.clone()),
                pane: Some(pane_a.clone()),
            })
            .await
            .unwrap();
        loop {
            if let Update::Workspace(model) = next_update(&mut updates).await {
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
            .send(Command::Visible {
                screen: Some(screen.clone()),
                panes: vec![pane_a.clone(), pane_b.pane_id.clone()],
            })
            .await
            .unwrap();
        commands
            .send(Command::ResizePane {
                pane: pane_b.pane_id.clone(),
                session: pane_b.session_id.clone(),
                cols: 15,
                rows: 15,
            })
            .await
            .unwrap();
        commands
            .send(Command::Input {
                pane: pane_b.pane_id.clone(),
                session: pane_b.session_id.clone(),
                bytes: b"printf 'HIDDEN_%s\\n' \"$(stty size)\"\n".into(),
            })
            .await
            .unwrap();
        // With no competing claim the PTY could retain 73x19 even if hide was
        // omitted. 120x40 proves this worker really withdrew its claim while
        // keeping hidden output attached and rejecting stale 15x15 reports.
        output_contains(&mut updates, &pane_b.session_id, b"HIDDEN_40 120").await;
        commands
            .send(Command::SetMaximized {
                screen: screen.clone(),
                connection,
                slot: None,
                pane: None,
            })
            .await
            .unwrap();
        loop {
            if let Update::Workspace(model) = next_update(&mut updates).await {
                if model.maximized_slot_id().is_none() && model.displayed_panes().len() == 2 {
                    break;
                }
            }
        }
        commands
            .send(Command::Visible {
                screen: Some(screen),
                panes: vec![pane_a, pane_b.pane_id.clone()],
            })
            .await
            .unwrap();
        commands
            .send(Command::Input {
                pane: pane_b.pane_id.clone(),
                session: pane_b.session_id.clone(),
                bytes: b"printf 'RESTORED_%s\\n' \"$(stty size)\"\n".into(),
            })
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
        let changed = ClosePlan {
            target: CloseTarget::Screen {
                screen: "other".into(),
            },
            ..plan
        };
        assert!(validate_pre_discovery_close(&changed, true).is_err());
    }

    #[tokio::test]
    async fn early_run_failure_still_awaits_real_owned_service_cleanup() {
        let service = crate::EmbeddedService::start();
        let completed = AtomicBool::new(false);
        let stop = AtomicBool::new(false);
        let error = finish_owned_run(
            service,
            Err(anyhow::anyhow!("connection failed before client admission")),
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
        commands
            .send(Command::RequestClose {
                plan: ClosePlan::startup_window(1),
                confirmed: true,
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
            .send(Command::RequestClose {
                plan,
                confirmed: true,
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
                break model;
            }
        };
        let invalid = |request_id| Command::RequestClose {
            plan: ClosePlan::startup_window(request_id),
            confirmed: true,
        };
        commands.send(invalid(10)).await.unwrap();
        let mut rejections = Vec::new();
        for request in [2, 5, 10] {
            commands.send(invalid(request)).await.unwrap();
        }
        while rejections.len() < 4 {
            if let Update::CloseRejected { request_id, reason } = next_update(&mut updates).await {
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
        let plan = model.assess_close(1, CloseTarget::Window).unwrap();
        commands
            .send(Command::RequestClose {
                plan: plan.clone(),
                confirmed: true,
            })
            .await
            .unwrap();
        commands
            .send(Command::CreateScreen("must not run".into()))
            .await
            .unwrap();
        loop {
            if let Update::WindowCloseApproved {
                request_id,
                connection_id,
            } = next_update(&mut updates).await
            {
                assert_eq!(request_id, 1);
                assert_eq!(connection_id, plan.connection_id);
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

#[cfg(target_os = "windows")]
async fn run(
    _: mpsc::Receiver<Command>,
    _: mpsc::Sender<Update>,
    _: Arc<AtomicBool>,
    cleanup_complete: Arc<AtomicBool>,
    _: ColorScheme,
) -> anyhow::Result<()> {
    cleanup_complete.store(true, Ordering::Release); // no owned service was started
    anyhow::bail!(
        "The native Windows workspace needs its owned WSL launcher; this development launcher is not wired yet"
    )
}

#[cfg(all(test, not(target_os = "windows")))]
mod profile_launch_tests;

#[cfg(all(test, not(target_os = "windows")))]
mod zoom_tests;
