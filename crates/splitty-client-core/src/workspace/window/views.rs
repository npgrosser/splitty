//! The windows of one client. Every opened Screen is held by exactly one
//! window, and every window has its own selection. The model holds the
//! selection of one window, the one in view; `enter` brings another into
//! view. A client with a single window holds every Screen there, and nothing
//! in this module changes what it does.
use super::*;

/// One window of the application. The application issues the IDs and never
/// reuses one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(u64);
impl WindowId {
    /// The window an application starts with.
    pub const FIRST: Self = Self(0);
    pub const fn new(ordinal: u64) -> Self {
        Self(ordinal)
    }
    pub const fn ordinal(self) -> u64 {
        self.0
    }
}
impl Default for WindowId {
    fn default() -> Self {
        Self::FIRST
    }
}
impl std::fmt::Display for WindowId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// What a window that is not in view keeps of itself.
#[derive(Clone)]
struct SavedView {
    id: WindowId,
    selected: Option<ScreenKey>,
    sources: Vec<(SourceId, catalog::SourceView)>,
    order: Vec<ScreenKey>,
}

#[derive(Clone, Default)]
pub(super) struct Windows {
    /// The window whose selection the model holds.
    view: WindowId,
    /// The other windows, in no order.
    others: Vec<SavedView>,
    /// The window that holds each opened Screen; empty with a single window.
    holders: HashMap<ScreenKey, WindowId>,
    /// Screens this client is bringing into being, by the window that asked.
    claims: HashMap<ScreenKey, WindowId>,
    /// The window that takes a Screen no window here asked for.
    intake: WindowId,
    /// The Screens the window in view listed when it was last settled, in
    /// order: where its selection goes when the shown Screen leaves it.
    order: Vec<ScreenKey>,
    /// The window that waits for an outcome on a source which changes what
    /// it shows (a creation, a move, a template).
    awaited: HashMap<SourceId, WindowId>,
    /// The windows a Screen left for another window since the models were
    /// last published; they read their model first.
    losers: Vec<WindowId>,
}

impl WindowWorkspace {
    fn each_source(&self) -> impl Iterator<Item = &SourceWorkspace> {
        std::iter::once(&self.local).chain(
            self.remotes
                .iter()
                .filter_map(|entry| entry.workspace.as_ref()),
        )
    }
    fn each_source_mut(&mut self) -> impl Iterator<Item = &mut SourceWorkspace> {
        std::iter::once(&mut self.local).chain(
            self.remotes
                .iter_mut()
                .filter_map(|entry| entry.workspace.as_mut()),
        )
    }

    /// The window in view.
    pub fn window(&self) -> WindowId {
        self.windows.view
    }
    /// Every window of the client, in the order of their IDs.
    pub fn window_ids(&self) -> Vec<WindowId> {
        let mut ids: Vec<WindowId> = std::iter::once(self.windows.view)
            .chain(self.windows.others.iter().map(|view| view.id))
            .collect();
        ids.sort();
        ids
    }
    pub fn has_window(&self, id: WindowId) -> bool {
        id == self.windows.view || self.windows.others.iter().any(|view| view.id == id)
    }
    pub fn single_window(&self) -> bool {
        self.windows.others.is_empty()
    }

    /// Bring window `id` into view: the selection the model holds becomes
    /// that window's. Returns whether the window exists.
    pub(crate) fn enter(&mut self, id: WindowId) -> bool {
        if id == self.windows.view {
            return true;
        }
        let Some(index) = self.windows.others.iter().position(|view| view.id == id) else {
            return false;
        };
        let mut next = self.windows.others.swap_remove(index);
        let leaving = SavedView {
            id: self.windows.view,
            selected: self.selected.take(),
            sources: self
                .each_source_mut()
                .map(|source| (source.source_id(), source.take_view()))
                .collect(),
            order: std::mem::take(&mut self.windows.order),
        };
        self.windows.others.push(leaving);
        self.selected = next.selected.take();
        for source in self.each_source_mut() {
            let id = source.source_id();
            let view = next
                .sources
                .iter()
                .position(|(source, _)| *source == id)
                .map(|index| next.sources.swap_remove(index).1)
                .unwrap_or_default();
            source.put_view(view);
        }
        self.windows.order = next.order;
        self.windows.view = id;
        true
    }
    /// Bring the window into view that waits for an outcome on `source`, so
    /// that the outcome changes what that window shows and no other.
    pub(crate) fn enter_awaited(&mut self, source: SourceId) {
        if let Some(window) = self.windows.awaited.get(&source).copied() {
            self.enter(window);
        }
    }
    /// The window that takes Screens no window here asked for: the one that
    /// was active last.
    pub(crate) fn set_intake(&mut self, id: WindowId) {
        if self.has_window(id) {
            self.windows.intake = id;
        }
    }
    fn intake(&self) -> WindowId {
        if self.has_window(self.windows.intake) {
            self.windows.intake
        } else {
            self.window_ids()[0]
        }
    }
    fn holder_or_intake(&self, key: &ScreenKey) -> WindowId {
        self.windows
            .holders
            .get(key)
            .or_else(|| self.windows.claims.get(key))
            .copied()
            .filter(|window| self.has_window(*window))
            .unwrap_or_else(|| self.intake())
    }
    fn knows(&self, key: &ScreenKey) -> bool {
        self.source(key.source_id()).is_some_and(|source| {
            source
                .screens()
                .iter()
                .any(|info| info.id.as_str() == key.screen_id())
                || source
                    .claimed_screens()
                    .iter()
                    .any(|id| *id == key.screen_id())
        })
    }
    /// The window that holds an opened Screen, or is bringing it into being.
    pub fn holder(&self, key: &ScreenKey) -> Option<WindowId> {
        if !self.knows(key) {
            return None;
        }
        Some(if self.single_window() {
            self.windows.view
        } else {
            self.holder_or_intake(key)
        })
    }
    /// Whether the window in view holds the Screen.
    pub fn holds(&self, key: &ScreenKey) -> bool {
        self.single_window() || self.holder_or_intake(key) == self.windows.view
    }
    /// The other window that holds the Screen, if one does.
    pub fn held_elsewhere(&self, key: &ScreenKey) -> Option<WindowId> {
        self.holder(key)
            .filter(|window| *window != self.windows.view)
    }
    /// The windows that show a Screen of `source`.
    pub fn windows_showing(&self, source: SourceId) -> Vec<WindowId> {
        let mut windows: Vec<WindowId> = self
            .selected
            .as_ref()
            .filter(|key| key.source_id() == source)
            .map(|_| self.windows.view)
            .into_iter()
            .chain(
                self.windows
                    .others
                    .iter()
                    .filter(|view| {
                        view.selected
                            .as_ref()
                            .is_some_and(|key| key.source_id() == source)
                    })
                    .map(|view| view.id),
            )
            .collect();
        windows.sort();
        windows
    }
    /// The Screen window `id` shows.
    pub fn shown_in(&self, id: WindowId) -> Option<ScreenKey> {
        if id == self.windows.view {
            return self.selected.clone();
        }
        self.windows
            .others
            .iter()
            .find(|view| view.id == id)
            .and_then(|view| view.selected.clone())
    }

    /// The Screens the window in view holds, in display order.
    pub fn opened(&self) -> Vec<OpenedScreen<'_>> {
        let mut opened = self.all_opened();
        if !self.single_window() {
            opened.retain(|entry| self.holds(&entry.key));
        }
        opened
    }

    /// Add a window that holds nothing yet.
    pub(crate) fn open_window(&mut self, id: WindowId) -> io::Result<()> {
        if self.has_window(id) {
            return Err(invalid("window is already open"));
        }
        if self.single_window() {
            let view = self.windows.view;
            let keys: Vec<ScreenKey> = self
                .all_opened()
                .into_iter()
                .map(|entry| entry.key)
                .collect();
            self.windows.order.clone_from(&keys);
            self.windows.holders = keys.into_iter().map(|key| (key, view)).collect();
            self.windows.claims = self
                .each_source()
                .flat_map(|source| {
                    source
                        .claimed_screens()
                        .into_iter()
                        .map(|id| (source.screen_key(id), view))
                        .collect::<Vec<_>>()
                })
                .collect();
            self.windows.awaited = self
                .each_source()
                .filter(|source| source.awaits_view_outcome())
                .map(|source| (source.source_id(), view))
                .collect();
            self.windows.intake = view;
        }
        self.windows.others.push(SavedView {
            id,
            selected: None,
            sources: Vec::new(),
            order: Vec::new(),
        });
        Ok(())
    }
    /// Remove window `id` and hand what it holds to `heir`, which keeps the
    /// Screen it shows.
    pub(crate) fn close_window(&mut self, id: WindowId, heir: WindowId) -> io::Result<()> {
        if id == heir || !self.has_window(id) || !self.has_window(heir) {
            return Err(invalid("window or its heir is not open"));
        }
        self.settle();
        for holder in self
            .windows
            .holders
            .values_mut()
            .chain(self.windows.claims.values_mut())
            .chain(self.windows.awaited.values_mut())
        {
            if *holder == id {
                *holder = heir;
            }
        }
        if self.windows.intake == id {
            self.windows.intake = heir;
        }
        if self.windows.view == id {
            self.enter(heir);
        }
        self.windows.others.retain(|view| view.id != id);
        if self.single_window() {
            self.windows.holders.clear();
            self.windows.claims.clear();
            self.windows.awaited.clear();
            self.windows.intake = self.windows.view;
            let shown = self.selected.clone();
            self.restore_selection(&[], shown);
        } else {
            self.settle();
        }
        Ok(())
    }
    /// Hand an opened Screen to window `target` and show it there. The window
    /// it leaves shows a neighboring Screen. Nothing else changes: no source
    /// hears of it.
    pub(crate) fn move_screen_to_window(
        &mut self,
        key: &ScreenKey,
        target: WindowId,
    ) -> io::Result<()> {
        if !self.has_window(target) {
            return Err(invalid("target window is not open"));
        }
        self.settle();
        if !self.all_opened().iter().any(|entry| &entry.key == key) {
            return Err(invalid("Screen reference is no longer open"));
        }
        let from = self.holder_or_intake(key);
        if from == target {
            return Ok(());
        }
        self.windows.losers.push(from);
        self.windows.holders.insert(key.clone(), target);
        let back = self.windows.view;
        self.enter(target);
        let shown = self.show_held(key);
        self.enter(back);
        self.settle();
        shown
    }
    /// Show a Screen the window in view holds.
    pub(super) fn show_held(&mut self, key: &ScreenKey) -> io::Result<()> {
        if !self.source(key.source_id()).is_some_and(|source| {
            source
                .screens()
                .iter()
                .any(|entry| entry.id.as_str() == key.screen_id())
        }) {
            return Err(invalid("Screen reference is no longer open"));
        }
        if !self.holds(key) {
            return Err(invalid("Screen is shown in another window"));
        }
        self.cancel_remote_creation_follow();
        if let Some(previous) = self.selected.as_ref().map(ScreenKey::source_id) {
            if let Some(source) = self.source_mut_raw(previous) {
                source.cancel_navigation_follow();
            }
        }
        self.source_mut_raw(key.source_id())
            .ok_or_else(|| invalid("Screen source is gone"))?
            .select_opened(key.screen_id())?;
        self.selected = Some(key.clone());
        Ok(())
    }
    /// Note what an edit of `source` left behind for the window in view: the
    /// Screens it is bringing into being, and that it waits for an outcome.
    pub(super) fn note_source_edit(&mut self, id: SourceId) {
        if self.single_window() {
            return;
        }
        let view = self.windows.view;
        let Some(source) = self.source(id) else {
            return;
        };
        let claimed: Vec<ScreenKey> = source
            .claimed_screens()
            .into_iter()
            .map(|screen| source.screen_key(screen))
            .collect();
        let awaits = source.awaits_view_outcome();
        for key in claimed {
            self.windows.claims.entry(key).or_insert(view);
        }
        if awaits {
            self.windows.awaited.entry(id).or_insert(view);
        } else {
            self.windows.awaited.remove(&id);
        }
    }
    /// The window in view takes a Screen it just opened.
    pub(super) fn hold(&mut self, key: &ScreenKey) {
        if !self.single_window() {
            self.windows.holders.insert(key.clone(), self.windows.view);
        }
    }

    /// Restore the invariants of several windows after the model changed:
    /// every opened Screen has one holder, and every window shows a Screen it
    /// holds, the neighbor of the one that left it or a Screen it created.
    pub(crate) fn settle(&mut self) {
        if self.single_window() {
            return;
        }
        let keys: Vec<ScreenKey> = self
            .all_opened()
            .into_iter()
            .map(|entry| entry.key)
            .collect();
        let intake = self.intake();
        for key in &keys {
            if !self.windows.holders.contains_key(key) {
                let holder = self
                    .windows
                    .claims
                    .remove(key)
                    .filter(|window| self.has_window(*window))
                    .unwrap_or(intake);
                self.windows.holders.insert(key.clone(), holder);
            }
        }
        let listed: HashSet<&ScreenKey> = keys.iter().collect();
        self.windows.holders.retain(|key, _| listed.contains(key));
        let claimed: HashSet<ScreenKey> = self
            .each_source()
            .flat_map(|source| {
                source
                    .claimed_screens()
                    .into_iter()
                    .map(|id| source.screen_key(id))
                    .collect::<Vec<_>>()
            })
            .collect();
        self.windows.claims.retain(|key, _| claimed.contains(key));
        let awaiting: HashSet<SourceId> = self
            .each_source()
            .filter(|source| source.awaits_view_outcome())
            .map(SourceWorkspace::source_id)
            .collect();
        self.windows
            .awaited
            .retain(|source, _| awaiting.contains(source));
        let back = self.windows.view;
        for id in self.window_ids() {
            self.enter(id);
            let previous = std::mem::take(&mut self.windows.order);
            let created = self.each_source_mut().find_map(|source| {
                let id = source.take_listed_creation()?;
                Some(source.screen_key(&id))
            });
            let created = created.filter(|key| self.holds(key));
            let shown = created.or_else(|| self.selected.clone());
            self.restore_selection(&previous, shown);
            self.windows.order = self.opened().into_iter().map(|entry| entry.key).collect();
        }
        self.enter(back);
    }

    /// The windows a Screen left for another window since the last call.
    pub(crate) fn take_losers(&mut self) -> Vec<WindowId> {
        std::mem::take(&mut self.windows.losers)
    }

    /// The model as window `id` sees it: its selection and the Screens it
    /// holds. The projection is a model of a single window.
    pub fn projection(&self, id: WindowId) -> Option<WindowWorkspace> {
        if !self.has_window(id) {
            return None;
        }
        let mut projection = self.clone();
        if projection.single_window() {
            return Some(projection);
        }
        projection.enter(id);
        let kept: Vec<(SourceId, HashSet<String>)> = projection
            .each_source()
            .map(|source| {
                let ids = source
                    .screens()
                    .iter()
                    .map(|info| info.id.as_str())
                    .chain(source.claimed_screens())
                    .filter(|screen| projection.holds(&source.screen_key(screen)))
                    .map(str::to_owned)
                    .collect();
                (source.source_id(), ids)
            })
            .collect();
        for source in projection.each_source_mut() {
            let id = source.source_id();
            let kept = kept
                .iter()
                .find(|(source, _)| *source == id)
                .map(|(_, kept)| kept);
            source.prune(|screen| kept.is_some_and(|kept| kept.contains(screen)));
        }
        projection.windows = Windows {
            view: id,
            intake: id,
            ..Windows::default()
        };
        Some(projection)
    }
}
