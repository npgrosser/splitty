//! Own the local service and route workspace intents on one ordered connection.
use crate::{
    workspace::{Axis, LocalWorkspace, MovePlacement, PaneDrag},
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
    CloseScreen {
        screen: String,
        connection: String,
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
    Close(String),
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
}
pub struct Worker {
    pub stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<anyhow::Result<()>>>,
}
impl Worker {
    pub fn start(commands: mpsc::Receiver<Command>, updates: mpsc::Sender<Update>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()?;
            runtime.block_on(async {
                let result = run(commands, updates.clone(), stopping).await;
                if let Err(error) = &result {
                    let _ = updates.try_send(Update::Status(format!("Local service: {error:#}")));
                }
                result
            })
        });
        Self {
            stop,
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
            match next {
                Next::Tick => {}
                Next::Command(None) => break,
                Next::Command(Some(command)) => {
                    let Some(model) = &mut model else {
                        continue;
                    };
                    let result: std::io::Result<()> = match command {
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
                        Command::CloseScreen { screen, connection } => {
                            if model.connection_id().to_string() == connection {
                                model.close_screen(&client, &screen)
                            } else {
                                Err(std::io::Error::new(
                                    std::io::ErrorKind::InvalidInput,
                                    "Close belongs to a previous connection",
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
                        Command::Close(pane) => model.close_pane(&client, &pane),
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
                                    let first = !model.catalog_initialized();
                                    match model.apply_catalog(event.connection_id, json) {
                                        Ok(applied) => delta = applied,
                                        Err(error) => {
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
                                        Ok(applied) => delta = applied,
                                        Err(error) => {
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
    let cleanup = service.close().await;
    cleanup?;
    if stop.load(Ordering::Acquire) {
        Ok(())
    } else {
        result
    }
}

#[cfg(target_os = "windows")]
async fn run(
    _: mpsc::Receiver<Command>,
    _: mpsc::Sender<Update>,
    _: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    anyhow::bail!(
        "The native Windows workspace needs its owned WSL launcher; this development launcher is not wired yet"
    )
}
