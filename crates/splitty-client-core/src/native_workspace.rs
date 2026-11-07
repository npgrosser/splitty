//! Own the local service and route workspace intents on one ordered connection.
use crate::{
    workspace::{Axis, ClosePlan, CloseTarget, LocalWorkspace, MovePlacement, PaneDrag},
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
    NewTerminal {
        screen: String,
        connection: String,
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
        text: String,
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
                let result = run(commands, updates.clone(), stopping, completed).await;
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
    mut commands: mpsc::Receiver<Command>,
    updates: mpsc::Sender<Update>,
    stop: Arc<AtomicBool>,
    cleanup_complete: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    use crate::{workspace::WorkspaceDelta, EmbeddedService};
    let service = EmbeddedService::start();
    let result = async {
        let mut client = service.connect().await?;
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
            Event(std::io::Result<Option<Box<Event>>>),
            Command(Option<Command>),
            Tick,
        }
        loop {
            let next = tokio::select! {
                event = client.next_event() => Next::Event(event.map(|event| event.map(Box::new))),
                command = commands.recv() => Next::Command(command),
                _ = tick.tick() => Next::Tick,
            };
            if stop.load(Ordering::Acquire) {
                break;
            }
            let mut close_window = false;
            match next {
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
                    let Some(model) = &mut model else {
                        continue;
                    };
                    let result: std::io::Result<()> = match command {
                        Command::RequestClose { .. } => {
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
                            model.split(&client, &pane, axis).map(|_| ())
                        }
                        Command::Tab(pane) => model.new_tab(&client, &pane).map(|_| ()),
                        Command::MovePane {
                            drag,
                            target,
                            placement,
                        } => model
                            .move_pane(&client, drag, target, placement)
                            .map(|_| ()),
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
                            text,
                        } => {
                            if model.session_for(&pane) == Some(session.as_str())
                                && attached.contains(&session)
                            {
                                client.input(&session, &text)
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
                                && model.screen_for(&pane) == model.shown_screen_id()
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
                                    .filter(|pane| model.screen_for(pane) == screen.as_deref())
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
                            Some("mux.welcome") if model.is_none() => {
                                let mut workspace = LocalWorkspace::new(client.connection_id());
                                let supports_tab_selection =
                                    json["capabilities"].as_array().is_some_and(|capabilities| {
                                        capabilities.iter().any(|capability| {
                                            capability.as_str() == Some("tab-select-v1")
                                        })
                                    });
                                workspace.set_tab_selection_supported(supports_tab_selection);
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
                                            accepted_close_snapshot = true;
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
                        || !delta.added.is_empty()
                        || !delta.removed.is_empty()
                    {
                        if let Some(model) = &model {
                            publish(&updates, Update::Workspace(Box::new(model.clone())), &stop)
                                .await?;
                        }
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
        client.finish().await?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    finish_owned_run(service, result, &cleanup_complete, &stop).await
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
) -> anyhow::Result<()> {
    cleanup_complete.store(true, Ordering::Release); // no owned service was started
    anyhow::bail!(
        "The native Windows workspace needs its owned WSL launcher; this development launcher is not wired yet"
    )
}
