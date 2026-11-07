//! Own the local service and route workspace intents on one ordered connection.
use splitty_client_core::{
    workspace::{Axis, LocalWorkspace},
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
    Split {
        pane: String,
        axis: Axis,
    },
    Tab(String),
    NewTerminal,
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
    Visible(Vec<String>),
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
    use splitty_client_core::{workspace::WorkspaceDelta, EmbeddedService};
    let service = EmbeddedService::start();
    let result = async {
        let mut client = service.connect().await?;
        let mut model = None::<LocalWorkspace>;
        let mut attached = HashSet::<String>::new();
        let mut visible = HashSet::<String>::new();
        let mut sizes = HashMap::<String, (u16, u16)>::new();
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
                        Command::Split { pane, axis } => {
                            model.split(&client, &pane, axis).map(|_| ())
                        }
                        Command::Tab(pane) => model.new_tab(&client, &pane).map(|_| ()),
                        Command::NewTerminal => model.new_terminal(&client).map(|_| ()),
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
                            if model.session_for(&pane) == Some(session.as_str()) {
                                sizes.insert(session.clone(), (cols, rows));
                                if attached.contains(&session) && visible.contains(&pane) {
                                    client.resize(&session, cols, rows, true)
                                } else {
                                    Ok(())
                                }
                            } else {
                                Ok(())
                            }
                        }
                        Command::Visible(panes) => {
                            let next: HashSet<_> = panes.into_iter().collect();
                            for pane in visible.difference(&next) {
                                if let Some(session) = model.session_for(pane) {
                                    client.hide(session)?;
                                }
                            }
                            for pane in next.difference(&visible) {
                                if let Some(session) = model.session_for(pane) {
                                    if attached.contains(session) {
                                        let (cols, rows) =
                                            sizes.get(session).copied().unwrap_or((80, 24));
                                        client.resize(session, cols, rows, true)?;
                                    }
                                }
                            }
                            visible = next;
                            Ok(())
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
                                let ids = client.create_screen("Screen 1")?;
                                let mut workspace =
                                    LocalWorkspace::new(ids, client.connection_id());
                                let supports_tab_selection =
                                    json["capabilities"].as_array().is_some_and(|capabilities| {
                                        capabilities.iter().any(|capability| {
                                            capability.as_str() == Some("tab-select-v1")
                                        })
                                    });
                                workspace.set_tab_selection_supported(supports_tab_selection);
                                model = Some(workspace);
                            }
                            Some("mux.screen_snapshot") => {
                                if let Some(model) = &mut model {
                                    delta = model.apply_snapshot(event.connection_id, json)?;
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
                                if let Some(model) = &model {
                                    client.resync(model.screen_id())?;
                                }
                            }
                            Some("attached") => {
                                if let Some(session) = json["sessionId"].as_str() {
                                    if model.as_mut().is_some_and(|model| {
                                        model.observe_attached(event.connection_id, session)
                                    }) {
                                        attached.insert(session.to_owned());
                                        client.activity(true)?;
                                        let (cols, rows) =
                                            sizes.get(session).copied().unwrap_or((80, 24));
                                        if model
                                            .as_ref()
                                            .and_then(|model| model.pane_for(session))
                                            .is_some_and(|pane| visible.contains(pane))
                                        {
                                            client.resize(session, cols, rows, true)?;
                                        } else {
                                            client.hide(session)?;
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
                                }
                            }
                            Some("mux.op_failed") | Some("error") => {
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
                                if let Some(model) = &model {
                                    client.resync(model.screen_id())?;
                                }
                            }
                            _ => {}
                        }
                    }
                    for route in &delta.removed {
                        attached.remove(&route.session_id);
                        sizes.remove(&route.session_id);
                        client.detach(&route.session_id)?;
                    }
                    for route in &delta.added {
                        client.attach(&route.session_id)?;
                    }
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
    anyhow::bail!("The native Windows workspace needs its owned WSL launcher; this development launcher is not wired yet")
}
