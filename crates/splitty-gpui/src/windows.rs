//! The windows of the application. They are views of one client: one worker
//! serves them all and writes one ordered stream of updates, which the pump
//! here hands to the window each update is for, in order. A window that
//! loses a Screen to another window parks its terminal views here, and the
//! window that gets the Screen takes them, so a Screen changes windows with
//! what its terminals show.
use crate::terminal::Terminal;
use crate::workspace::{UpdateSource, WorkerLink, Workspace};
use gpui_kit::*;
use splitty_client_core::native_workspace::{Update, WindowPresenceCell};
use splitty_client_core::workspace::{PaneKey, ScreenKey, TerminalToken, WindowId};
use std::{
    collections::HashMap,
    mem::Discriminant,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

/// How long a parked terminal view waits for the window that takes its
/// Screen. A view nobody took belonged to a Screen that closed.
const PARKED_FOR: Duration = Duration::from_secs(5);

/// Opens the platform window of window `id` on the worker `link` leads to.
pub(crate) type Opener = fn(WindowId, WorkerLink, &mut App) -> anyhow::Result<()>;

struct Entry {
    id: WindowId,
    handle: AnyWindowHandle,
    workspace: WeakEntity<Workspace>,
    /// Whether the window got the state every window shows, which the
    /// worker only sends again when it changes.
    primed: bool,
}

struct Parked {
    terminal: Entity<Terminal>,
    route: TerminalToken,
    since: Instant,
}

pub(crate) struct Windows {
    entries: Vec<Entry>,
    /// The windows in the order they were active, the latest first.
    active: Vec<WindowId>,
    next: u64,
    /// The first window's link; every other window's is made from it.
    link: WorkerLink,
    open: Opener,
    parked: HashMap<PaneKey, Parked>,
    /// The latest of each update that brings state every window shows.
    state: HashMap<Discriminant<Update>, Update>,
    _pump: Task<()>,
}
impl Global for Windows {}

impl Windows {
    /// Serve the application's windows from `updates`, the worker's one
    /// stream. The first window registers itself afterwards.
    pub(crate) fn start(
        link: WorkerLink,
        updates: mpsc::Receiver<Update>,
        open: Opener,
        cx: &mut App,
    ) {
        let pump = pump(updates, link.stopping.clone(), cx);
        cx.set_global(Self {
            entries: Vec::new(),
            active: Vec::new(),
            next: 1,
            link,
            open,
            parked: HashMap::new(),
            state: HashMap::new(),
            _pump: pump,
        });
    }

    fn get_mut(cx: &mut App) -> Option<&mut Self> {
        cx.has_global::<Self>().then(|| cx.global_mut::<Self>())
    }

    /// Whether the application serves its windows from one stream; a view
    /// built by itself, as in a view test, reads its own.
    pub(crate) fn serving(cx: &App) -> bool {
        cx.has_global::<Self>()
    }

    pub(crate) fn register(
        id: WindowId,
        handle: AnyWindowHandle,
        workspace: &Entity<Workspace>,
        cx: &mut App,
    ) {
        if let Some(windows) = Self::get_mut(cx) {
            windows.entries.retain(|entry| entry.id != id);
            windows.entries.push(Entry {
                id,
                handle,
                workspace: workspace.downgrade(),
                primed: false,
            });
            windows.active.retain(|active| *active != id);
            windows.active.insert(0, id);
        }
    }

    /// The number of open windows; one for a view built by itself.
    pub(crate) fn count(cx: &App) -> usize {
        cx.try_global::<Self>()
            .map_or(1, |windows| windows.entries.len().max(1))
    }

    /// The other windows, the one active last first, each with its title.
    pub(crate) fn others(id: WindowId, cx: &App) -> Vec<(WindowId, String)> {
        let Some(windows) = cx.try_global::<Self>() else {
            return Vec::new();
        };
        let mut others: Vec<&Entry> = windows
            .entries
            .iter()
            .filter(|entry| entry.id != id)
            .collect();
        others.sort_by_key(|entry| {
            windows
                .active
                .iter()
                .position(|active| *active == entry.id)
                .unwrap_or(usize::MAX)
        });
        others
            .into_iter()
            .filter_map(|entry| {
                let workspace = entry.workspace.upgrade()?;
                Some((entry.id, workspace.read(cx).window_title()))
            })
            .collect()
    }

    /// Window `id` became the active one.
    pub(crate) fn activated(id: WindowId, cx: &mut App) {
        if let Some(windows) = Self::get_mut(cx) {
            if windows.entries.iter().any(|entry| entry.id == id) {
                windows.active.retain(|active| *active != id);
                windows.active.insert(0, id);
            }
        }
    }

    /// Open another window. The window that asks tells the worker what the
    /// new one holds, under the ID this returns.
    pub(crate) fn open(cx: &mut App) -> Option<(WindowId, WindowPresenceCell)> {
        let windows = Self::get_mut(cx)?;
        let id = WindowId::new(windows.next);
        windows.next += 1;
        let presence = WindowPresenceCell::default();
        let link = windows.link.for_window(id, presence.clone());
        let open = windows.open;
        match open(id, link, cx) {
            Ok(()) => Some((id, presence)),
            Err(error) => {
                eprintln!("Cannot open another window: {error:#}");
                None
            }
        }
    }

    /// The worker removed window `id`: its platform window goes.
    pub(crate) fn closed(id: WindowId, window: &mut Window, cx: &mut App) {
        if let Some(windows) = Self::get_mut(cx) {
            windows.entries.retain(|entry| entry.id != id);
            windows.active.retain(|active| *active != id);
        }
        window.remove_window();
    }

    /// Whether a window other than `id` shows exactly this terminal view.
    pub(crate) fn shows_elsewhere(
        id: WindowId,
        pane: &PaneKey,
        terminal: EntityId,
        cx: &App,
    ) -> bool {
        cx.try_global::<Self>().is_some_and(|windows| {
            windows
                .entries
                .iter()
                .filter(|entry| entry.id != id)
                .filter_map(|entry| entry.workspace.upgrade())
                .any(|workspace| workspace.read(cx).shows_terminal(pane, terminal))
        })
    }

    /// Close a window the worker never heard of.
    pub(crate) fn abandon(id: WindowId, cx: &mut App) {
        if let Some((handle, _)) = Self::entry(id, cx) {
            let _ = handle.update(cx, |_, window, cx| Self::closed(id, window, cx));
        }
    }

    /// Activate window `id` and show `screen` there, which it holds.
    pub(crate) fn show(id: WindowId, screen: ScreenKey, cx: &mut App) {
        let Some((handle, workspace)) = Self::entry(id, cx) else {
            return;
        };
        let _ = handle.update(cx, |_, window, cx| {
            window.activate_window();
            workspace.update(cx, |workspace, cx| {
                workspace.show_held_screen(screen, window, cx);
            });
        });
    }

    /// Hand one update to its window as the pump does, without the stream.
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(crate) fn ui_test_deliver(update: Update, cx: &mut App) {
        let (_, updates) = mpsc::channel(1);
        let mut source = Stream {
            updates,
            held: None,
        };
        Self::deliver(Some(addressed(update)), &mut source, cx);
    }
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(crate) fn ui_test_window(
        id: WindowId,
        cx: &App,
    ) -> Option<(AnyWindowHandle, Entity<Workspace>)> {
        Self::entry(id, cx)
    }

    fn entry(id: WindowId, cx: &App) -> Option<(AnyWindowHandle, Entity<Workspace>)> {
        let windows = cx.try_global::<Self>()?;
        let entry = windows.entries.iter().find(|entry| entry.id == id)?;
        Some((entry.handle, entry.workspace.upgrade()?))
    }

    /// Keep a terminal view of a Screen this window no longer holds, for
    /// the window that holds the Screen now.
    pub(crate) fn park(
        pane: PaneKey,
        route: TerminalToken,
        terminal: Entity<Terminal>,
        cx: &mut App,
    ) {
        if let Some(windows) = Self::get_mut(cx) {
            windows.parked.insert(
                pane,
                Parked {
                    terminal,
                    route,
                    since: Instant::now(),
                },
            );
        }
    }

    /// The terminal view another window parked for exactly this terminal.
    pub(crate) fn take_parked(
        pane: &PaneKey,
        route: &TerminalToken,
        cx: &mut App,
    ) -> Option<Entity<Terminal>> {
        let windows = Self::get_mut(cx)?;
        if windows
            .parked
            .get(pane)
            .is_some_and(|parked| parked.route == *route)
        {
            windows.parked.remove(pane).map(|parked| parked.terminal)
        } else {
            None
        }
    }

    /// Hand an update, or the tick between updates, to the windows.
    fn deliver(
        first: Option<(WindowId, Update)>,
        source: &mut Stream,
        cx: &mut App,
    ) -> (bool, usize) {
        match first {
            Some((id, update)) => Self::poll(id, Some(update), source, cx),
            None => {
                let ids: Vec<WindowId> = cx
                    .try_global::<Self>()
                    .map(|windows| windows.entries.iter().map(|entry| entry.id).collect())
                    .unwrap_or_default();
                let mut result = (false, 0);
                for id in ids {
                    let (paced, processed) = Self::poll(id, None, source, cx);
                    result = (result.0 || paced, result.1 + processed);
                }
                if let Some(windows) = Self::get_mut(cx) {
                    windows
                        .parked
                        .retain(|_, parked| parked.since.elapsed() < PARKED_FOR);
                }
                result
            }
        }
    }

    fn poll(
        id: WindowId,
        first: Option<Update>,
        source: &mut Stream,
        cx: &mut App,
    ) -> (bool, usize) {
        if let Some(update) = &first {
            if let (Some(copy), Some(windows)) = (update.for_every_window(), Self::get_mut(cx)) {
                windows.state.insert(std::mem::discriminant(update), copy);
            }
        }
        let entry = Self::entry(id, cx).or_else(|| {
            // The worker's failure is written before it knows its windows;
            // with the first window closed, another one shows it.
            let any = cx.try_global::<Self>()?.entries.first()?.id;
            matches!(first, Some(Update::LocalServiceFailed(_)))
                .then(|| Self::entry(any, cx))
                .flatten()
        });
        let Some((handle, workspace)) = entry else {
            // A window that is gone reads nothing; what it was sent is dropped.
            return (false, usize::from(first.is_some()));
        };
        let brings_model = matches!(first, Some(Update::Workspace(_)));
        let mut own = Addressed { source, window: id };
        let result = handle
            .update(cx, |_, window, cx| {
                workspace.update(cx, |workspace, cx| {
                    workspace.poll(first, &mut own, window, cx)
                })
            })
            .unwrap_or((false, 0));
        if brings_model {
            Self::prime(id, cx);
        }
        result
    }

    /// Give a window that just read its first model the state every window
    /// shows as the others have it.
    fn prime(id: WindowId, cx: &mut App) {
        let Some(windows) = Self::get_mut(cx) else {
            return;
        };
        let Some(entry) = windows.entries.iter_mut().find(|entry| entry.id == id) else {
            return;
        };
        if std::mem::replace(&mut entry.primed, true) {
            return;
        }
        let state: Vec<Update> = windows
            .state
            .values()
            .filter_map(Update::for_every_window)
            .collect();
        let Some((handle, workspace)) = Self::entry(id, cx) else {
            return;
        };
        for update in state {
            let _ = handle.update(cx, |_, window, cx| {
                workspace.update(cx, |workspace, cx| {
                    workspace.poll(Some(update), &mut Nothing, window, cx)
                })
            });
        }
    }
}

/// The worker's stream and the one update read ahead of its window.
struct Stream {
    updates: mpsc::Receiver<Update>,
    held: Option<(WindowId, Update)>,
}

fn addressed(update: Update) -> (WindowId, Update) {
    match update {
        Update::ForWindow { window, update } => (window, *update),
        update => (WindowId::FIRST, update),
    }
}

/// One window's part of the stream: the updates at its head that are for
/// this window. An update for another window stays where it is, so no window
/// reads past what an earlier update still has to do elsewhere.
struct Addressed<'a> {
    source: &'a mut Stream,
    window: WindowId,
}
impl UpdateSource for Addressed<'_> {
    fn next_update(&mut self) -> Option<Update> {
        if self.source.held.is_none() {
            self.source.held = self.source.updates.try_recv().ok().map(addressed);
        }
        if self
            .source
            .held
            .as_ref()
            .is_some_and(|(window, _)| *window == self.window)
        {
            self.source.held.take().map(|(_, update)| update)
        } else {
            None
        }
    }
    fn ended(&self) -> bool {
        self.source.held.is_none() && self.source.updates.is_closed()
    }
}

struct Nothing;
impl UpdateSource for Nothing {
    fn next_update(&mut self) -> Option<Update> {
        None
    }
    fn ended(&self) -> bool {
        false
    }
}

fn pump(updates: mpsc::Receiver<Update>, stopping: Arc<AtomicBool>, cx: &mut App) -> Task<()> {
    cx.spawn(async move |cx| {
        let maintenance = Duration::from_millis(16);
        let mut next_tick = Instant::now() + maintenance;
        let mut source = Stream {
            updates,
            held: None,
        };
        let mut open = true;
        let mut work_batches = 0_usize;
        let mut work_updates = 0_usize;
        loop {
            if stopping.load(Ordering::Acquire) {
                break;
            }
            let now = Instant::now();
            let first = if source.held.is_some() {
                source.held.take()
            } else if now >= next_tick {
                next_tick = now + maintenance;
                None
            } else {
                let timer = cx.background_executor().timer(next_tick - now);
                if open {
                    tokio::select! {
                        update = source.updates.recv() => {
                            if update.is_none() {
                                open = false;
                            }
                            update.map(addressed)
                        }
                        _ = timer => {
                            next_tick = Instant::now() + maintenance;
                            None
                        }
                    }
                } else {
                    timer.await;
                    next_tick = Instant::now() + maintenance;
                    None
                }
            };
            let (paced, processed) = cx.update(|cx| Windows::deliver(first, &mut source, cx));
            if stopping.load(Ordering::Acquire) {
                break;
            }
            if paced {
                // Keep each window's 64-update and 256 KiB pacing under
                // sustained traffic while small batches wake immediately.
                cx.background_executor().timer(maintenance).await;
                work_batches = 0;
                work_updates = 0;
            } else if processed != 0 {
                work_batches += 1;
                work_updates += processed;
                if work_batches >= 4 || work_updates >= 64 {
                    // Many small ready batches must still yield to the UI.
                    cx.background_executor()
                        .timer(Duration::from_millis(1))
                        .await;
                    work_batches = 0;
                    work_updates = 0;
                }
            }
        }
    })
}
