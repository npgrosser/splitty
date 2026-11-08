//! The window owns source membership, global selection and aggregate admission.
//! Source collections retain their canonical Screens; there is no window mux tree.
use super::*;
use splitty_remote::ssh::SshTarget;
use splitty_transport::{
    budget::{Budget, ConnectionBudget},
    wire::Purpose,
};

mod diagnostic;
mod host;
mod host_update;
pub use host::{CatalogState, HostActionTarget, HostDiagnostic, HostPresentation};
pub use host_update::{HostUpdateOffer, HostUpdatePlan, HostUpdateState};

pub const MAX_REMOTE_SOURCES: usize = 8;
pub const MAX_PENDING_REMOTE_OPENS: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceStatus {
    Connecting,
    Reconnecting,
    Ready,
    Offline,
    Closing,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenedScreenState {
    Loading,
    Ready,
    Missing,
    Offline,
}

/// A reference in grouped window order; its identity is never its list position.
pub struct OpenedScreen<'a> {
    key: ScreenKey,
    name: Option<&'a str>,
    state: OpenedScreenState,
}
impl OpenedScreen<'_> {
    pub fn key(&self) -> &ScreenKey {
        &self.key
    }
    pub fn name(&self) -> Option<&str> {
        self.name
    }
    pub fn state(&self) -> OpenedScreenState {
        self.state
    }
}

/// One source's exact retained route, including hidden, attaching and offline
/// terminals. is_ready is execution readiness, not permission to retain a view.
pub struct WindowRoute {
    screen: ScreenKey,
    terminal: TerminalToken,
    generation: u64,
    ready: bool,
}
impl WindowRoute {
    pub fn screen(&self) -> &ScreenKey {
        &self.screen
    }
    pub fn terminal(&self) -> &TerminalToken {
        &self.terminal
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn is_ready(&self) -> bool {
        self.ready
    }
}

#[derive(Clone)]
struct RemoteSource {
    id: SourceId,
    presentation: HostPresentation,
    workspace: Option<SourceWorkspace>,
    opening: bool,
    open_order: Option<u64>,
}

/// The actual local collection plus explicitly admitted host sources. Setup may
/// reserve a source before it has a Client; it never invents a connection UUID.
#[derive(Clone)]
pub struct WindowWorkspace {
    local: SourceWorkspace,
    remotes: Vec<RemoteSource>,
    selected: Option<ScreenKey>,
    host_order: Vec<String>,
    next_open_order: u64,
    backend_version: Option<Arc<splitty_transport::budget::Charged<String>>>,
    backend_version_failed: bool,
    staged: Arc<Budget>,
}
impl WindowWorkspace {
    pub fn new(connection: Uuid) -> Self {
        let local = SourceWorkspace::new(connection);
        Self {
            selected: None,
            staged: local.staged.clone(),
            local,
            remotes: Vec::new(),
            host_order: Vec::new(),
            next_open_order: 0,
            backend_version: None,
            backend_version_failed: false,
        }
    }
    pub fn accepts_host_update_plan(&self, plan: &HostUpdatePlan) -> bool {
        plan.owner() == self.lifecycle_token()
            && self.host(plan.target().source_id()).is_some_and(|host| {
                host.is_update_available()
                    && host.update_offer().is_some_and(|offer| {
                        plan.matches(host.action_target(), offer, self.backend_version())
                    })
            })
    }
    pub fn backend_version(&self) -> Option<&str> {
        self.backend_version.as_deref().map(|value| value.as_str())
    }
    pub fn backend_version_failed(&self) -> bool {
        self.backend_version_failed
    }
    pub(crate) fn set_backend_version(
        &mut self,
        owner: Uuid,
        result: io::Result<Arc<splitty_transport::budget::Charged<String>>>,
    ) -> bool {
        if owner != self.lifecycle_token().connection_id() {
            return false;
        }
        match result {
            Ok(value) => {
                self.backend_version = Some(value);
                self.backend_version_failed = false;
            }
            Err(_) => {
                self.backend_version = None;
                self.backend_version_failed = true;
            }
        }
        true
    }
    pub fn local_source(&self) -> &SourceWorkspace {
        &self.local
    }
    pub(crate) fn validate_remote_creation_owner(
        &self,
        target: &HostActionTarget,
        source: SourceToken,
    ) -> io::Result<()> {
        self.check_owner(target.local_owner())?;
        if self
            .host(target.source_id())
            .is_none_or(|host| host.action_target() != target)
            || source.source_id() != target.source_id()
            || self.execution_token(target.source_id()) != Some(source)
        {
            return Err(invalid(
                "Screen creation belongs to an unavailable host connection",
            ));
        }
        Ok(())
    }
    /// Validate a ready opened host independently of the window's selected source.
    pub fn validate_remote_screen_creation(
        &self,
        target: &HostActionTarget,
        source: SourceToken,
    ) -> io::Result<()> {
        self.validate_remote_creation_owner(target, source)?;
        if self
            .host(target.source_id())
            .is_none_or(|host| host.catalog_state() != CatalogState::Ready)
        {
            return Err(invalid("Host catalog is not ready for Screen creation"));
        }
        let model = self
            .source(target.source_id())
            .ok_or_else(|| invalid("Remote source is gone"))?;
        model.remote_creation_available()?;
        let screens: usize = std::iter::once(&self.local)
            .chain(
                self.remotes
                    .iter()
                    .filter_map(|entry| entry.workspace.as_ref()),
            )
            .map(SourceWorkspace::screen_count)
            .sum();
        let panes: usize = std::iter::once(&self.local)
            .chain(
                self.remotes
                    .iter()
                    .filter_map(|entry| entry.workspace.as_ref()),
            )
            .map(SourceWorkspace::admission_count)
            .sum();
        if screens >= catalog::MAX_LOCAL_SCREENS || panes >= MAX_ATTACHMENTS {
            return Err(busy("Window Screen or pane capacity exhausted"));
        }
        Ok(())
    }
    /// Capture one deliberate action; availability checks alone do not allocate intent.
    pub fn prepare_remote_screen_creation(
        &self,
        target: &HostActionTarget,
        source: SourceToken,
    ) -> io::Result<RemoteScreenCreateRequest> {
        self.validate_remote_screen_creation(target, source)?;
        Ok(RemoteScreenCreateRequest::new(target.clone(), source))
    }
    /// Consume revocable navigation only at the final selection boundary, with no await.
    pub(crate) fn follow_remote_creation(
        &mut self,
        request: &RemoteScreenCreateRequest,
    ) -> io::Result<bool> {
        if self.check_owner(request.target().local_owner()).is_err()
            || self.execution_token(request.target().source_id()) != Some(request.source())
            || self
                .host(request.target().source_id())
                .is_none_or(|host| host.action_target() != request.target())
        {
            return Ok(false);
        }
        let Some(key) = self
            .source(request.target().source_id())
            .and_then(|source| source.consume_remote_creation_follow(request))
        else {
            return Ok(false);
        };
        self.select(self.lifecycle_token(), &key)?;
        Ok(true)
    }
    pub(crate) fn cancel_remote_creation_follow(&self) {
        for source in std::iter::once(&self.local).chain(
            self.remotes
                .iter()
                .filter_map(|entry| entry.workspace.as_ref()),
        ) {
            source.cancel_remote_creation_follow();
        }
    }
    pub fn source(&self, id: SourceId) -> Option<&SourceWorkspace> {
        if id == self.local.source_id() {
            Some(&self.local)
        } else {
            self.remotes
                .iter()
                .find(|source| source.id == id)?
                .workspace
                .as_ref()
        }
    }
    pub fn source_status(&self, id: SourceId) -> Option<SourceStatus> {
        if id == self.local.source_id() {
            Some(if self.local.ready {
                SourceStatus::Ready
            } else {
                SourceStatus::Offline
            })
        } else {
            self.remotes
                .iter()
                .find(|source| source.id == id)
                .map(|source| source.presentation.status)
        }
    }
    pub fn source_target(&self, id: SourceId) -> Option<&str> {
        self.remotes
            .iter()
            .find(|source| source.id == id)
            .map(|source| source.presentation.action.target.as_str())
    }
    pub fn hosts(&self) -> impl Iterator<Item = &HostPresentation> {
        self.remotes.iter().map(|source| &source.presentation)
    }
    pub fn host(&self, id: SourceId) -> Option<&HostPresentation> {
        self.remotes
            .iter()
            .find(|source| source.id == id)
            .map(|source| &source.presentation)
    }
    pub(crate) fn host_mut(&mut self, action: &HostActionTarget) -> Option<&mut HostPresentation> {
        if action.local_owner() != self.lifecycle_token() {
            return None;
        }
        let entry = self
            .remotes
            .iter_mut()
            .find(|entry| entry.id == action.source_id())?;
        (entry.presentation.action_target() == action).then_some(&mut entry.presentation)
    }
    /// Resolve an exact directory target, including setup before a Client exists.
    pub fn source_for_target(&self, target: &str) -> Option<SourceId> {
        self.remotes
            .iter()
            .find(|source| source.presentation.action.target == target)
            .map(|source| source.id)
    }
    /// Window lifecycle/navigation authority stays local while a remote is shown.
    pub fn lifecycle_token(&self) -> SourceToken {
        self.local.source_token()
    }
    pub fn execution_token(&self, id: SourceId) -> Option<SourceToken> {
        (self.source_status(id) == Some(SourceStatus::Ready))
            .then(|| {
                self.source(id)
                    .filter(|source| source.is_ready())
                    .map(SourceWorkspace::source_token)
            })
            .flatten()
    }
    pub fn shown_key(&self) -> Option<ScreenKey> {
        self.selected.clone()
    }
    pub fn shown(&self) -> Option<&ScreenModel> {
        let key = self.selected.as_ref()?;
        self.source(key.source_id())?.screen(key.screen_id())
    }
    pub fn opened(&self) -> Vec<OpenedScreen<'_>> {
        let mut sources = vec![&self.local];
        let mut remote: Vec<_> = self.remotes.iter().enumerate().collect();
        remote.sort_by_key(|(position, entry)| {
            (
                self.host_order
                    .iter()
                    .position(|target| target == &entry.presentation.action.target)
                    .unwrap_or(usize::MAX),
                entry.open_order.unwrap_or(u64::MAX),
                *position,
            )
        });
        sources.extend(
            remote
                .into_iter()
                .filter_map(|(_, source)| source.workspace.as_ref()),
        );
        sources
            .into_iter()
            .flat_map(|source| {
                source.screens().iter().map(move |info| {
                    let state = if !source.is_ready() {
                        OpenedScreenState::Offline
                    } else if source.is_missing(info.id.as_str()) {
                        OpenedScreenState::Missing
                    } else if source
                        .screen(info.id.as_str())
                        .is_some_and(|screen| screen.snapshot().is_some())
                    {
                        OpenedScreenState::Ready
                    } else {
                        OpenedScreenState::Loading
                    };
                    OpenedScreen {
                        key: source.screen_key(info.id.as_str()),
                        name: info.name.as_deref(),
                        state,
                    }
                })
            })
            .collect()
    }
    pub fn routes(&self) -> Vec<WindowRoute> {
        std::iter::once(&self.local)
            .chain(
                self.remotes
                    .iter()
                    .filter_map(|entry| entry.workspace.as_ref()),
            )
            .flat_map(|source| {
                source.routes().into_iter().filter_map(|route| {
                    Some(WindowRoute {
                        screen: source.screen_key(source.screen_for(&route.pane_id)?),
                        terminal: source.terminal_token(&route.pane_id)?,
                        generation: source.route_generation(),
                        ready: self.execution_token(source.source_id()).is_some()
                            && source.session_ready(&route.session_id),
                    })
                })
            })
            .collect()
    }
    /// Current ready connection delivery, including replay before Attached.
    /// Hidden routes resolve by source and session, never by global selection.
    pub fn route_for_session(&self, owner: SourceToken, session: &str) -> Option<WindowRoute> {
        if self.execution_token(owner.source_id()) != Some(owner) {
            return None;
        }
        let source = self.source(owner.source_id())?;
        let pane = source.pane_for(session)?;
        Some(WindowRoute {
            screen: source.screen_key(source.screen_for(pane)?),
            terminal: source.terminal_token(pane)?,
            generation: source.route_generation(),
            ready: source.session_ready(session),
        })
    }
    /// Exact retained binding, including early replay and offline presentation.
    /// This does not authorize input or an event from an inactive connection.
    pub fn contains_terminal_binding(&self, route: &TerminalToken) -> bool {
        self.source(route.source_token().source_id())
            .is_some_and(|source| source.contains_terminal_binding(route))
    }
    /// Validate a displayed retained terminal for local presentation intent. This
    /// deliberately grants no input, transport, or structural command authority.
    pub fn validate_terminal_focus(
        &self,
        screen: &ScreenKey,
        route: &TerminalToken,
    ) -> io::Result<()> {
        let source = self
            .source(screen.source_id())
            .ok_or_else(|| invalid("Terminal source is gone"))?;
        if !matches!(
            self.source_status(screen.source_id()),
            Some(SourceStatus::Ready | SourceStatus::Offline)
        ) || self.shown_key().as_ref() != Some(screen)
            || source.source_token() != route.source_token()
            || !source.contains_terminal_binding(route)
            || source.screen_for(route.pane_id()) != Some(screen.screen_id())
            || !source
                .displayed_panes()
                .iter()
                .any(|pane| pane == route.pane_id())
        {
            return Err(invalid(
                "Terminal no longer belongs to the displayed source and Screen",
            ));
        }
        Ok(())
    }
    /// Pure window presentation state, including retained offline terminals.
    pub fn focus_terminal(
        &mut self,
        screen: &ScreenKey,
        route: &TerminalToken,
    ) -> io::Result<bool> {
        self.validate_terminal_focus(screen, route)?;
        Ok(self
            .source_mut_raw(screen.source_id())
            .expect("validated source")
            .focus_pane(route.pane_id()))
    }
    /// Execution additionally requires a ready source and completed attachment.
    pub fn accepts_terminal(&self, route: &TerminalToken) -> bool {
        self.execution_token(route.source_token().source_id()) == Some(route.source_token())
            && self
                .source(route.source_token().source_id())
                .is_some_and(|source| {
                    source.accepts_terminal(route) && source.session_ready(route.session_id())
                })
    }
    fn check_owner(&self, owner: SourceToken) -> io::Result<()> {
        if owner != self.lifecycle_token() || !self.local.is_ready() {
            return Err(invalid("window owner changed"));
        }
        Ok(())
    }
    pub fn set_host_order(&mut self, owner: SourceToken, order: &[String]) -> io::Result<()> {
        self.check_owner(owner)?;
        if order.len() > 256 {
            return Err(invalid("host order exceeds limit"));
        }
        for target in order {
            validate_target(target)?;
        }
        let mut seen = HashSet::new();
        self.host_order = order
            .iter()
            .filter(|target| seen.insert(target.as_str()))
            .cloned()
            .collect();
        Ok(())
    }
    /// Reserve before starting setup. The runtime additionally validates current
    /// known-directory membership and owns the attempt's cancellation context.
    pub fn admit_remote(&mut self, owner: SourceToken, target: &str) -> io::Result<SourceId> {
        self.check_owner(owner)?;
        validate_target(target)?;
        if let Some(source) = self
            .remotes
            .iter()
            .find(|source| source.presentation.action.target == target)
        {
            return if source.presentation.status == SourceStatus::Closing {
                Err(busy("host cleanup is pending"))
            } else {
                Ok(source.id)
            };
        }
        if self.remotes.len() >= MAX_REMOTE_SOURCES
            || self.remotes.iter().filter(|source| source.opening).count()
                >= MAX_PENDING_REMOTE_OPENS
        {
            return Err(busy("window host capacity exhausted"));
        }
        let id = SourceId::initial(Uuid::new_v4());
        self.remotes.push(RemoteSource {
            id,
            presentation: HostPresentation {
                action: HostActionTarget {
                    local_owner: owner,
                    target: target.to_owned(),
                    source: id,
                    attempt: 1,
                },
                status: SourceStatus::Connecting,
                progress: None,
                diagnostic: None,
                catalog: CatalogState::Unrequested,
                catalog_error: None,
                browsing: true,
                retry_available: false,
                update_offer: None,
                update_state: HostUpdateState::Idle,
            },
            workspace: None,
            opening: true,
            open_order: None,
        });
        Ok(id)
    }
    /// Bind only the Client that the fixed-target adapter actually negotiated.
    pub fn bind_remote(&mut self, id: SourceId, client: &Client) -> io::Result<()> {
        let target = self
            .source_target(id)
            .ok_or_else(|| invalid("host source is gone"))?;
        if !matches!(&client.ready().purpose, Purpose::Host { target: assigned } if assigned == target)
        {
            return Err(invalid("host transport assignment changed"));
        }
        self.bind_remote_parts(id, client.connection_id(), client.sender().context().budget)
    }
    pub(super) fn bind_remote_parts(
        &mut self,
        id: SourceId,
        connection: Uuid,
        budget: ConnectionBudget,
    ) -> io::Result<()> {
        let source = self
            .remotes
            .iter_mut()
            .find(|source| source.id == id)
            .ok_or_else(|| invalid("host source is gone"))?;
        if !matches!(
            source.presentation.status,
            SourceStatus::Connecting | SourceStatus::Reconnecting
        ) {
            return Err(invalid("host has no admitted opening"));
        }
        let mut workspace = match source.workspace.take() {
            Some(mut workspace) => {
                workspace.reset_connection(connection);
                workspace.replace_catalog_budget(budget);
                workspace
            }
            None => SourceWorkspace::remote(
                id,
                connection,
                source.presentation.action.target.clone(),
                budget,
            ),
        };
        workspace.staged = self.staged.clone();
        workspace.ready = false;
        workspace.clear_port_observation();
        source.workspace = Some(workspace);
        Ok(())
    }
    /// Called after the runtime validates the remote readiness/welcome prelude.
    /// Transport negotiation alone never grants command authority.
    pub fn confirm_ready(&mut self, owner: SourceToken) -> io::Result<()> {
        let source = self
            .remotes
            .iter_mut()
            .find(|source| source.id == owner.source_id())
            .ok_or_else(|| invalid("host source is gone"))?;
        let workspace = source
            .workspace
            .as_mut()
            .ok_or_else(|| invalid("host transport is not bound"))?;
        if !matches!(
            source.presentation.status,
            SourceStatus::Connecting | SourceStatus::Reconnecting
        ) || workspace.source_token() != owner
        {
            return Err(invalid("host readiness is stale"));
        }
        workspace.ready = true;
        source.presentation.status = SourceStatus::Ready;
        source.presentation.progress = None;
        source.presentation.diagnostic = None;
        source.presentation.catalog = CatalogState::Unrequested;
        source.presentation.catalog_error = None;
        source.presentation.retry_available = false;
        source.opening = false;
        Ok(())
    }
    /// Release failed setup admission only after its retained future and owned
    /// cleanup have finished. A Closing entry remains reserved until finish_close.
    pub fn finish_attempt(&mut self, id: SourceId) -> io::Result<()> {
        let source = self
            .remotes
            .iter_mut()
            .find(|source| source.id == id)
            .ok_or_else(|| invalid("host source is gone"))?;
        if source.presentation.status != SourceStatus::Offline {
            return Err(invalid("host setup has not failed"));
        }
        source.opening = false;
        source.presentation.retry_available = true;
        Ok(())
    }
    /// Revoke execution while retaining the last observed presentation.
    pub fn disconnect(&mut self, id: SourceId) -> io::Result<()> {
        let source = self
            .remotes
            .iter_mut()
            .find(|source| source.id == id)
            .ok_or_else(|| invalid("host source is gone"))?;
        if source.presentation.status == SourceStatus::Closing {
            return Err(invalid("host cleanup has already started"));
        }
        source.presentation.status = SourceStatus::Offline;
        source.presentation.retry_available = false;
        if let Some(workspace) = &mut source.workspace {
            workspace.ready = false;
            workspace.remote_creation_lost();
            workspace.clear_port_observation();
        }
        Ok(())
    }
    /// Cleanup must have settled before the runtime admits a replacement open.
    pub fn retry(&mut self, id: SourceId) -> io::Result<()> {
        self.begin_attempt(id, false)
    }
    /// A recovery wait owns demand, but no preparation slot. Only the runtime
    /// calls this after observing cleanup of the preceding attempt.
    pub(crate) fn wait_recovery(&mut self, id: SourceId) -> io::Result<()> {
        self.finish_attempt(id)?;
        let source = self
            .remotes
            .iter_mut()
            .find(|source| source.id == id)
            .unwrap();
        source.presentation.status = SourceStatus::Reconnecting;
        source.presentation.retry_available = false;
        Ok(())
    }
    pub(crate) fn recover(&mut self, id: SourceId) -> io::Result<()> {
        self.begin_attempt(id, true)
    }
    fn begin_attempt(&mut self, id: SourceId, recovery: bool) -> io::Result<()> {
        if self.remotes.iter().filter(|source| source.opening).count() >= MAX_PENDING_REMOTE_OPENS {
            return Err(busy("window opening capacity exhausted"));
        }
        let source = self
            .remotes
            .iter_mut()
            .find(|source| source.id == id)
            .ok_or_else(|| invalid("host source is gone"))?;
        let eligible = if recovery {
            source.presentation.status == SourceStatus::Reconnecting
        } else {
            source.presentation.can_retry()
        };
        if !eligible || source.opening {
            return Err(invalid("host is not awaiting retry or cleanup is pending"));
        }
        let attempt = source
            .presentation
            .action
            .attempt
            .checked_add(1)
            .ok_or_else(|| invalid("host attempt sequence exhausted"))?;
        source.presentation.action.attempt = attempt;
        source.presentation.status = if recovery {
            SourceStatus::Reconnecting
        } else {
            SourceStatus::Connecting
        };
        source.presentation.retry_available = false;
        source.presentation.update_offer = None;
        source.presentation.update_state = HostUpdateState::Idle;
        source.opening = true;
        Ok(())
    }
    pub fn begin_close(&mut self, id: SourceId) -> io::Result<()> {
        let source = self
            .remotes
            .iter_mut()
            .find(|source| source.id == id)
            .ok_or_else(|| invalid("host source is gone"))?;
        source.presentation.status = SourceStatus::Closing;
        if let Some(workspace) = &mut source.workspace {
            workspace.ready = false;
            workspace.remote_creation_lost();
            workspace.clear_port_observation();
        }
        Ok(())
    }
    /// The runtime calls this only after it has observed owned cleanup. Closing
    /// entries retain their model and source reservations until this boundary.
    pub fn finish_close(&mut self, id: SourceId) -> io::Result<()> {
        let index = self
            .remotes
            .iter()
            .position(|source| source.id == id)
            .ok_or_else(|| invalid("host source is gone"))?;
        if self.remotes[index].presentation.status != SourceStatus::Closing {
            return Err(invalid("host cleanup was not started"));
        }
        let previous = self
            .opened()
            .into_iter()
            .map(|entry| entry.key)
            .collect::<Vec<_>>();
        let shown = self.shown_key();
        self.remotes.remove(index);
        self.restore_selection(&previous, shown);
        Ok(())
    }
    pub fn select(&mut self, owner: SourceToken, key: &ScreenKey) -> io::Result<()> {
        self.check_owner(owner)?;
        if !self.source(key.source_id()).is_some_and(|source| {
            source
                .screens()
                .iter()
                .any(|entry| entry.id.as_str() == key.screen_id())
        }) {
            return Err(invalid("Screen reference is no longer open"));
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

    pub fn can_move_screen(&self, screen: &ScreenKey) -> bool {
        self.source_status(screen.source_id()) != Some(SourceStatus::Closing)
            && self
                .source(screen.source_id())
                .is_some_and(|source| source.can_move_screen(screen.screen_id()))
    }

    /// A captured relative placement may survive a remote reconnect, but never
    /// a replaced local owner, disposed source, or removed reference/anchor.
    pub fn validate_screen_move(
        &self,
        owner: SourceToken,
        screen: &ScreenKey,
        before: Option<&ScreenKey>,
    ) -> io::Result<bool> {
        self.check_owner(owner)?;
        if !self.can_move_screen(screen)
            || before.is_some_and(|key| key.source_id() != screen.source_id())
        {
            return Err(invalid(
                "Screen placement is unavailable or crosses source groups",
            ));
        }
        let source = self
            .source(screen.source_id())
            .ok_or_else(|| invalid("Screen source is gone"))?;
        let (from, to) =
            source.screen_move_positions(screen.screen_id(), before.map(ScreenKey::screen_id))?;
        Ok(from != to)
    }

    pub(crate) fn move_opened_screen(
        &mut self,
        owner: SourceToken,
        screen: &ScreenKey,
        before: Option<&ScreenKey>,
    ) -> io::Result<bool> {
        if !self.validate_screen_move(owner, screen, before)? {
            return Ok(false);
        }
        self.source_mut_raw(screen.source_id())
            .ok_or_else(|| invalid("Screen source is gone"))?
            .move_opened_screen(screen.screen_id(), before.map(ScreenKey::screen_id))
    }
    pub fn cycle(&mut self, owner: SourceToken, step: TabStep) -> io::Result<bool> {
        self.check_owner(owner)?;
        let Some(current) = self.shown_key() else {
            return Ok(false);
        };
        let opened = self.opened();
        let Some(index) = opened.iter().position(|entry| entry.key == current) else {
            return Ok(false);
        };
        let next = match step {
            TabStep::Previous => index.saturating_sub(1),
            TabStep::Next => (index + 1).min(opened.len() - 1),
        };
        if index == next {
            return Ok(false);
        }
        let key = opened[next].key.clone();
        self.select(owner, &key)?;
        Ok(true)
    }
    pub fn open_existing(&mut self, owner: SourceToken, key: &ScreenKey) -> io::Result<()> {
        if self.execution_token(key.source_id()) != Some(owner) {
            return Err(invalid("remote connection changed"));
        }
        let already_open = self.source(key.source_id()).is_some_and(|source| {
            source
                .screens()
                .iter()
                .any(|entry| entry.id.as_str() == key.screen_id())
        });
        if !already_open
            && self
                .host(key.source_id())
                .is_none_or(|host| host.catalog_state() != CatalogState::Ready)
        {
            return Err(invalid(
                "remote catalog is not ready for opening another Screen",
            ));
        }
        let first = self
            .remotes
            .iter()
            .find(|entry| entry.id == key.source_id())
            .is_some_and(|entry| entry.open_order.is_none());
        let next_order = if first {
            self.next_open_order
                .checked_add(1)
                .ok_or_else(|| invalid("host open order exhausted"))?
        } else {
            self.next_open_order
        };
        self.source_mut_raw(key.source_id())
            .ok_or_else(|| invalid("remote source is gone"))?
            .open_existing(key.screen_id())?;
        if let Some(previous) = self.selected.as_ref().map(ScreenKey::source_id) {
            if let Some(source) = self.source_mut_raw(previous) {
                source.cancel_navigation_follow();
            }
        }
        if first {
            self.remotes
                .iter_mut()
                .find(|entry| entry.id == key.source_id())
                .expect("opened remote")
                .open_order = Some(self.next_open_order);
            self.next_open_order = next_order;
        }
        self.selected = Some(key.clone());
        Ok(())
    }
    pub fn detach(&mut self, owner: SourceToken, key: &ScreenKey) -> io::Result<WorkspaceDelta> {
        self.check_owner(owner)?;
        let previous = self
            .opened()
            .into_iter()
            .map(|entry| entry.key)
            .collect::<Vec<_>>();
        let shown = self.shown_key();
        let delta = self
            .source_mut_raw(key.source_id())
            .ok_or_else(|| invalid("remote source is gone"))?
            .detach_reference(key.screen_id())?;
        self.restore_selection(&previous, shown);
        Ok(delta)
    }
    fn restore_selection(&mut self, previous: &[ScreenKey], shown: Option<ScreenKey>) {
        let current: Vec<_> = self.opened().into_iter().map(|entry| entry.key).collect();
        let next = shown
            .as_ref()
            .filter(|key| current.contains(key))
            .cloned()
            .or_else(|| {
                let index = previous
                    .iter()
                    .position(|key| Some(key) == shown.as_ref())?;
                previous[index..]
                    .iter()
                    .chain(previous[..index].iter().rev())
                    .find(|key| current.contains(key))
                    .cloned()
            })
            .or_else(|| current.first().cloned());
        if let Some(key) = next {
            let source = self.source_mut_raw(key.source_id()).expect("opened source");
            if source.shown_screen_id() != Some(key.screen_id()) {
                source
                    .select_opened(key.screen_id())
                    .expect("opened Screen");
            }
            self.selected = Some(key);
        } else {
            self.selected = None;
        }
    }
    /// Borrowing one source recalculates its allowance from every other source,
    /// including offline/closing retained models. Mutation cannot overlap another
    /// mutable source borrow; common operations reserve before their send closure.
    pub fn source_mut(&mut self, id: SourceId) -> Option<SourceEdit<'_>> {
        let source_shown = self.source(id)?.shown_screen_id().map(str::to_owned);
        let followed = self
            .source(id)?
            .selection_follow_target()
            .map(str::to_owned);
        let previous = self.opened().into_iter().map(|entry| entry.key).collect();
        let shown = self.shown_key();
        self.source_mut_raw(id)?; // Recalculate admission before exposing mutation.
        Some(SourceEdit {
            window: self,
            id,
            previous,
            shown,
            source_shown,
            followed,
        })
    }
    fn source_mut_raw(&mut self, id: SourceId) -> Option<&mut SourceWorkspace> {
        let (mut screens, mut panes) = (0, 0);
        for source in std::iter::once(&self.local).chain(
            self.remotes
                .iter()
                .filter_map(|entry| entry.workspace.as_ref()),
        ) {
            if source.source_id() != id {
                screens += source.screen_count();
                panes += source.admission_count();
            }
        }
        let source = if id == self.local.source_id() {
            Some(&mut self.local)
        } else {
            self.remotes
                .iter_mut()
                .find(|source| source.id == id)?
                .workspace
                .as_mut()
        }?;
        source.limits = catalog::SourceLimits {
            screens: catalog::MAX_LOCAL_SCREENS.saturating_sub(screens),
            panes: MAX_ATTACHMENTS.saturating_sub(panes),
        };
        Some(source)
    }
}
/// One mutable borrow of the common collection. On release, the window
/// reconciles its single global selection and the source's remembered choice.
/// No Screen or layout data is copied into this guard.
pub struct SourceEdit<'a> {
    window: &'a mut WindowWorkspace,
    id: SourceId,
    previous: Vec<ScreenKey>,
    shown: Option<ScreenKey>,
    source_shown: Option<String>,
    followed: Option<String>,
}
impl std::ops::Deref for SourceEdit<'_> {
    type Target = SourceWorkspace;
    fn deref(&self) -> &Self::Target {
        self.window.source(self.id).expect("borrowed source")
    }
}
impl std::ops::DerefMut for SourceEdit<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.window
            .source_mut_raw(self.id)
            .expect("borrowed source")
    }
}
impl Drop for SourceEdit<'_> {
    fn drop(&mut self) {
        let source = self.window.source(self.id).expect("borrowed source");
        let changed = source.shown_screen_id() != self.source_shown.as_deref();
        let still_open = self.shown.as_ref().is_some_and(|key| {
            key.source_id() == self.id
                && source
                    .screens()
                    .iter()
                    .any(|entry| entry.id.as_str() == key.screen_id())
        });
        let followed = changed
            && self
                .shown
                .as_ref()
                .is_some_and(|key| key.source_id() == self.id)
            && self
                .followed
                .as_deref()
                .is_some_and(|target| Some(target) == source.shown_screen_id());
        let next = if (still_open && changed) || followed || self.shown.is_none() {
            source.shown_screen_id().map(|id| source.screen_key(id))
        } else {
            None
        };
        self.window
            .restore_selection(&self.previous, next.or_else(|| self.shown.clone()));
    }
}
fn validate_target(target: &str) -> io::Result<()> {
    let parsed = SshTarget::parse(target).map_err(io::Error::other)?;
    if parsed.as_str() != target {
        return Err(invalid("host target must retain its exact server identity"));
    }
    Ok(())
}
fn busy(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::WouldBlock, message)
}

#[cfg(any(test, feature = "ui-test-support"))]
mod test_support;
#[cfg(test)]
pub(crate) mod tests;
