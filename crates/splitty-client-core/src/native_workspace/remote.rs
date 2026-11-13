//! Deliberate host demand and retained ownership over the existing host adapter.
//! The same SourceRuntime drives each ready host and the local connection.
use super::{
    source::{SourceEffects, SourceRuntime},
    *,
};
use crate::{
    owned_service::OwnedService,
    workspace::{
        CatalogState, HostActionTarget, HostDiagnostic, HostUpdateOffer, HostUpdatePlan,
        HostUpdateState, SourceId, SourceStatus,
    },
    Client,
};
use splitty_remote::ssh::SshTarget;
use splitty_transport::{
    budget::{Charged, ConnectionBudget},
    queue, wire,
};
use std::{
    future::{poll_fn, Future},
    io,
    pin::Pin,
    sync::Mutex,
    task::{Context, Poll},
};
use tokio::time::Instant;

type Setup<'a> = Pin<Box<dyn Future<Output = io::Result<Client>> + Send + 'a>>;
type Closing = Pin<Box<dyn Future<Output = io::Result<()>> + Send>>;

#[cfg(all(test, not(target_os = "windows")))]
mod tests;
mod update;

pub(super) enum Step {
    Prepared(HostActionTarget),
    Event(HostActionTarget, io::Result<Option<Box<Event>>>),
    Closed(HostActionTarget),
    Progress(HostActionTarget),
}
struct Host<'a> {
    action: HostActionTarget,
    offer: Option<Arc<Charged<HostUpdateOffer>>>,
    daemon_version: Option<Arc<Charged<String>>>,
    update_pending: bool,
    update_dispatched: bool,
    context: queue::Context,
    setup: Option<Setup<'a>>,
    // A completed setup still owns its returned Client until the worker handles
    // it. Cancelling the enclosing select can never silently drop that Client.
    prepared: Option<io::Result<Client>>,
    source: Option<SourceRuntime>,
    closing: Option<Closing>,
    closed_due: bool,
    progress: Arc<Mutex<Option<Charged<wire::Progress>>>>,
    remote_ready: bool,
    was_ready: bool,
    recovery_deadline: Option<Instant>,
    retry_at: Option<Instant>,
    retry_delay: Duration,
    prelude_deadline: Option<Instant>,
    observation_deadline: Option<Instant>,
    cancelled: bool,
    dispose: bool,
    reason: Option<Arc<Charged<wire::Closed>>>,
    failure: Option<io::Error>,
    cleanup_error: Option<io::Error>,
    close_rejected: Option<Update>,
}
impl<'a> Host<'a> {
    fn new(service: &'a OwnedService, action: HostActionTarget) -> io::Result<Self> {
        Self::with_update(service, action, false)
    }
    fn with_update(
        service: &'a OwnedService,
        action: HostActionTarget,
        update: bool,
    ) -> io::Result<Self> {
        let context = queue::Context::default();
        let progress = Arc::new(Mutex::new(None));
        let target = SshTarget::parse(action.target()).map_err(io::Error::other)?;
        let callbacks = progress.clone();
        let setup_context = context.clone();
        let setup = Box::pin(async move {
            service
                .host_stream(target, update, setup_context, move |value| {
                    // One latest charged value, no queue or publication from callback.
                    *callbacks.lock().unwrap_or_else(|error| error.into_inner()) = Some(value);
                })
                .await
        });
        Ok(Self {
            action,
            offer: None,
            daemon_version: None,
            update_pending: false,
            update_dispatched: update,
            context,
            setup: Some(setup),
            prepared: None,
            source: None,
            closing: None,
            closed_due: false,
            progress,
            remote_ready: false,
            was_ready: false,
            recovery_deadline: None,
            retry_at: None,
            retry_delay: Duration::from_secs(1),
            prelude_deadline: None,
            observation_deadline: None,
            cancelled: false,
            dispose: false,
            reason: None,
            failure: None,
            cleanup_error: None,
            close_rejected: None,
        })
    }
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Step> {
        if self.prepared.is_some() {
            return Poll::Ready(Step::Prepared(self.action.clone()));
        }
        if let Some(setup) = &mut self.setup {
            if let Poll::Ready(result) = setup.as_mut().poll(cx) {
                self.setup = None;
                self.prepared = Some(result);
                return Poll::Ready(Step::Prepared(self.action.clone()));
            }
        }
        if self.closed_due {
            return Poll::Ready(Step::Closed(self.action.clone()));
        }
        if let Some(closing) = &mut self.closing {
            if let Poll::Ready(result) = closing.as_mut().poll(cx) {
                self.closing = None;
                if let Err(error) = result {
                    if self.cleanup_error.is_none() {
                        self.cleanup_error = Some(error);
                    }
                }
                self.closed_due = true;
                return Poll::Ready(Step::Closed(self.action.clone()));
            }
        }
        if self
            .progress
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_some()
        {
            return Poll::Ready(Step::Progress(self.action.clone()));
        }
        if let Some(source) = &mut self.source {
            // Client owns the persistent peer reader. Dropping this pending wait
            // does not restart framing or abandon an owned setup/close future.
            let read = source.client.next_event();
            let mut read = std::pin::pin!(read);
            if let Poll::Ready(result) = read.as_mut().poll(cx) {
                return Poll::Ready(Step::Event(
                    self.action.clone(),
                    result.map(|event| event.map(Box::new)),
                ));
            }
        }
        Poll::Pending
    }
    fn close_source(&mut self) {
        if !self.cancelled
            && !self.update_pending
            && !self.update_dispatched
            && self.was_ready
            && self.recovery_deadline.is_none()
        {
            self.recovery_deadline = Some(Instant::now() + Duration::from_secs(120));
        }
        if self
            .failure
            .as_ref()
            .is_some_and(|error| error.kind() == io::ErrorKind::InvalidData)
        {
            self.offer = None;
        }
        self.context.close();
        self.prelude_deadline = None;
        self.observation_deadline = None;
        if let Some(mut source) = self.source.take() {
            if source.welcomed {
                self.offer = None;
            }
            self.close_rejected = source.reject_pending_close_on_loss();
            self.reason = source.client.closed_reason();
            self.closing = Some(Box::pin(source.client.finish()));
        }
    }
    fn retryable(&self, diagnostic: Option<&HostDiagnostic>) -> bool {
        if self
            .cleanup_error
            .as_ref()
            .is_some_and(|error| error.kind() == io::ErrorKind::InvalidData)
        {
            return false;
        }
        if self.reason.is_some()
            || self.failure.as_ref().is_some_and(|error| {
                splitty_transport::negotiation::preparation_failure(error).is_some()
            })
        {
            return diagnostic.is_some_and(HostDiagnostic::retryable);
        }
        self.failure.as_ref().is_some_and(|error| {
            matches!(
                error.kind(),
                io::ErrorKind::UnexpectedEof
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::BrokenPipe
                    | io::ErrorKind::NotConnected
                    | io::ErrorKind::TimedOut
            )
        })
    }
    /// Validate application readiness before any catalog or session authority.
    /// A transport assignment alone is insufficient; the welcome must follow
    /// this attempt's exact target and advertise the coherent catalog protocol.
    fn observe_prelude(&mut self, json: &serde_json::Value) -> io::Result<bool> {
        match json["type"].as_str() {
            // Progress arrives as transport reports during preparation. A hub
            // of the previous release repeats it here as text; ignore it.
            Some("remote.progress") => Ok(false),
            Some("remote.ready")
                if !self.remote_ready && json["target"].as_str() == Some(self.action.target()) =>
            {
                self.offer = HostUpdateOffer::from_ready(json, &self.context.budget)?;
                self.daemon_version =
                    crate::workspace::daemon_version_from_ready(json, &self.context.budget);
                self.remote_ready = true;
                self.was_ready = true;
                Ok(false)
            }
            Some("mux.welcome") if self.remote_ready => {
                if !json["capabilities"]
                    .as_array()
                    .is_some_and(|caps| caps.iter().any(|cap| cap == "workspace-catalog-v1"))
                {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "Host does not support workspace-catalog-v1",
                    ));
                }
                Ok(true)
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Host readiness prelude does not match its assigned target",
            )),
        }
    }
}

pub(super) struct Remotes<'a> {
    service: &'a OwnedService,
    hosts: Vec<Host<'a>>,
    update_plan: Option<Arc<HostUpdatePlan>>,
    cursor: usize,
    cleanup_error: Option<io::Error>,
    /// Hosts connected at start, each with the Screens to open once its
    /// catalog is known. This demand is not a view's to release.
    restoring: Vec<(HostActionTarget, Vec<String>)>,
    /// The window's presence, which each host connection greets its welcome with.
    presence: super::WindowPresenceCell,
}
impl<'a> Remotes<'a> {
    pub fn new(service: &'a OwnedService) -> Self {
        Self {
            service,
            hosts: Vec::new(),
            update_plan: None,
            cursor: 0,
            cleanup_error: None,
            restoring: Vec::new(),
            presence: super::WindowPresenceCell::default(),
        }
    }
    pub fn with_window_presence(mut self, presence: super::WindowPresenceCell) -> Self {
        self.presence = presence;
        self
    }
    /// The user acted while the window showed a Screen of the source `id`.
    pub fn acted(&mut self, id: SourceId) {
        if let Some(source) = self
            .hosts
            .iter_mut()
            .find(|host| host.action.source_id() == id)
            .and_then(|host| host.source.as_mut())
        {
            source.acted();
        }
    }
    /// The window was hidden: no host connection is in use.
    pub fn hidden(&mut self) {
        for source in self
            .hosts
            .iter_mut()
            .filter_map(|host| host.source.as_mut())
        {
            source.hidden();
        }
    }
    pub async fn next(&mut self) -> Step {
        poll_fn(|cx| {
            for offset in 0..self.hosts.len() {
                let index = (self.cursor + offset) % self.hosts.len();
                if let Poll::Ready(step) = self.hosts[index].poll(cx) {
                    self.cursor = (index + 1) % self.hosts.len();
                    return Poll::Ready(step);
                }
            }
            Poll::Pending
        })
        .await
    }
    pub fn connect(
        &mut self,
        window: &mut WindowWorkspace,
        owner: SourceToken,
        target: &str,
        known: &HostDirectorySnapshot,
    ) -> io::Result<()> {
        if owner != window.lifecycle_token()
            || known.connection_id() != owner.connection_id()
            || !known
                .directory()
                .hosts
                .iter()
                .any(|host| host.target == target)
        {
            return Err(crate::connection::invalid(
                "Host is not in this window's accepted directory",
            ));
        }
        self.connect_explicit(window, owner, target)
    }
    /// Connect a known host and, once its catalog is known, open the listed
    /// Screens it still has. A host already in the window is left as it is.
    pub fn restore(
        &mut self,
        window: &mut WindowWorkspace,
        owner: SourceToken,
        target: &str,
        screens: Vec<String>,
        known: &HostDirectorySnapshot,
    ) -> io::Result<()> {
        if window.source_for_target(target).is_some() {
            return Ok(());
        }
        self.connect(window, owner, target, known)?;
        let action = window
            .source_for_target(target)
            .and_then(|id| window.host(id))
            .expect("admitted host")
            .action_target()
            .clone();
        self.restoring.push((action, screens));
        Ok(())
    }
    /// Open what a restored host still has and give up its browse demand; a
    /// host that did not come up leaves the window with a notice.
    fn settle_restores(&mut self, window: &mut WindowWorkspace, out: &mut SourceEffects) {
        let mut index = 0;
        while index < self.restoring.len() {
            let (action, _) = &self.restoring[index];
            let id = action.source_id();
            let state = window
                .host(id)
                .filter(|host| host.action_target() == action)
                .map(|host| (host.status(), host.catalog_state()));
            let ready = match state {
                // Another attempt replaced this one, or the host is gone.
                None => {
                    self.restoring.swap_remove(index);
                    continue;
                }
                Some((SourceStatus::Ready, CatalogState::Ready)) => true,
                Some((SourceStatus::Ready, CatalogState::Failed))
                | Some((SourceStatus::Offline, _)) => false,
                Some(_) => {
                    index += 1;
                    continue;
                }
            };
            let (action, screens) = self.restoring.swap_remove(index);
            if ready {
                // A start does not take the view from what is already used.
                let shown = window.shown_key();
                let keys: Vec<ScreenKey> = window
                    .source(id)
                    .map(|model| {
                        let live = model.live_catalog().unwrap_or_default();
                        screens
                            .iter()
                            .filter(|id| {
                                live.iter().any(|screen| screen.id.as_str() == id.as_str())
                            })
                            .map(|id| model.screen_key(id))
                            .collect()
                    })
                    .unwrap_or_default();
                if let Some(token) = window.execution_token(id).filter(|_| !keys.is_empty()) {
                    let _ = self.open_many(window, token, &keys);
                }
                if let Some(shown) = shown {
                    let _ = window.select(window.lifecycle_token(), &shown);
                }
            } else {
                out.updates.push(Update::HostReconnectFailed {
                    target: action.target().to_owned(),
                });
            }
            let _ = self.release_browse(window, &action);
            out.changed = true;
        }
    }
    /// Called only after confirmed local metadata preparation for an entered target.
    pub(super) fn connect_explicit(
        &mut self,
        window: &mut WindowWorkspace,
        owner: SourceToken,
        target: &str,
    ) -> io::Result<()> {
        let id = window.admit_remote(owner, target)?;
        let action = window
            .host(id)
            .expect("admitted host")
            .action_target()
            .clone();
        if let Some(presentation) = window.host_mut(&action) {
            presentation.browsing = true;
        }
        if !self.hosts.iter().any(|host| host.action.source_id() == id) {
            self.hosts.push(Host::new(self.service, action)?);
        }
        Ok(())
    }
    fn index(&self, window: &WindowWorkspace, action: &HostActionTarget) -> io::Result<usize> {
        if action.local_owner() != window.lifecycle_token()
            || window
                .host(action.source_id())
                .is_none_or(|host| host.action_target() != action)
        {
            return Err(crate::connection::invalid(
                "Host action belongs to an obsolete attempt",
            ));
        }
        self.hosts
            .iter()
            .position(|host| &host.action == action)
            .ok_or_else(|| crate::connection::invalid("Host runtime is gone"))
    }
    pub fn reject_action(
        &self,
        window: &mut WindowWorkspace,
        action: &HostActionTarget,
        error: &io::Error,
    ) -> bool {
        let Ok(index) = self.index(window, action) else {
            return false;
        };
        let host = &self.hosts[index];
        let Some(state) = window.host_mut(action) else {
            return false;
        };
        state.diagnostic = plain_diagnostic(&host.context.budget, error).ok();
        true
    }
    pub fn release_all_browse(&mut self, window: &mut WindowWorkspace) {
        let actions: Vec<_> = window
            .hosts()
            .filter(|host| host.is_browsing())
            .map(|host| host.action_target().clone())
            .filter(|action| !self.restoring.iter().any(|(held, _)| held == action))
            .collect();
        for action in actions {
            let _ = self.release_browse(window, &action);
        }
    }
    pub fn release_browse(
        &mut self,
        window: &mut WindowWorkspace,
        action: &HostActionTarget,
    ) -> io::Result<()> {
        self.index(window, action)?;
        self.restoring.retain(|(held, _)| held != action);
        window.host_mut(action).expect("validated host").browsing = false;
        self.remove_unused(window);
        Ok(())
    }
    pub fn cancel(
        &mut self,
        window: &mut WindowWorkspace,
        action: &HostActionTarget,
    ) -> io::Result<()> {
        let index = self.index(window, action)?;
        self.restoring.retain(|(held, _)| held != action);
        if !window
            .host(action.source_id())
            .is_some_and(|host| host.can_cancel())
        {
            return Err(crate::connection::invalid(
                "Host attempt is no longer preparing",
            ));
        }
        let host = &mut self.hosts[index];
        let updating = host.update_pending || host.update_dispatched;
        host.cancelled = true;
        host.update_pending = false;
        host.offer = None;
        host.close_source();
        if !has_references(window, action.source_id()) && !updating {
            window.host_mut(action).expect("validated host").browsing = false;
            host.dispose = true;
            window.begin_close(action.source_id())?;
        } else {
            window.disconnect(action.source_id())?;
        }
        if host.setup.is_none() && host.prepared.is_none() && host.closing.is_none() {
            host.closed_due = true;
        }
        Ok(())
    }
    pub fn retry(
        &mut self,
        window: &mut WindowWorkspace,
        action: &HostActionTarget,
    ) -> io::Result<()> {
        let index = self.index(window, action)?;
        if window
            .host(action.source_id())
            .is_some_and(|host| host.can_retry_catalog())
        {
            let host = &mut self.hosts[index];
            host.source
                .as_mut()
                .ok_or_else(|| crate::connection::invalid("Host connection is unavailable"))?
                .begin_observation(window)?;
            host.observation_deadline = Some(Instant::now() + Duration::from_secs(15));
            let presentation = window.host_mut(action).expect("validated host");
            presentation.catalog = CatalogState::Loading;
            presentation.catalog_error = None;
            return Ok(());
        }
        window.retry(action.source_id())?;
        let next = window
            .host(action.source_id())
            .expect("retried host")
            .action_target()
            .clone();
        let previous = std::mem::replace(&mut self.hosts[index], Host::new(self.service, next)?);
        if self.cleanup_error.is_none() {
            self.cleanup_error = previous.cleanup_error;
        }
        Ok(())
    }
    pub fn open(
        &mut self,
        window: &mut WindowWorkspace,
        owner: SourceToken,
        key: &ScreenKey,
    ) -> io::Result<()> {
        let index = self
            .hosts
            .iter()
            .position(|host| host.action.source_id() == key.source_id())
            .ok_or_else(|| crate::connection::invalid("Host source is gone"))?;
        let already_open = window.source(key.source_id()).is_some_and(|source| {
            source
                .screens()
                .iter()
                .any(|entry| entry.id.as_str() == key.screen_id())
        });
        window.open_existing(owner, key)?;
        if already_open {
            return Ok(());
        }
        let host = &mut self.hosts[index];
        host.source
            .as_mut()
            .ok_or_else(|| crate::connection::invalid("Host connection is unavailable"))?
            .begin_observation(window)?;
        host.observation_deadline
            .get_or_insert_with(|| Instant::now() + Duration::from_secs(15));
        window.host_mut(&host.action).expect("opened host").catalog = CatalogState::Loading;
        Ok(())
    }
    /// Open the listed Screens the host still has, in their order, and show
    /// the first. Screens that have ended are left out.
    pub fn open_many(
        &mut self,
        window: &mut WindowWorkspace,
        owner: SourceToken,
        keys: &[ScreenKey],
    ) -> io::Result<()> {
        let index = self
            .hosts
            .iter()
            .position(|host| {
                Some(host.action.source_id()) == keys.first().map(ScreenKey::source_id)
            })
            .ok_or_else(|| crate::connection::invalid("Host source is gone"))?;
        if !window.open_listed(owner, keys)? {
            return Ok(());
        }
        let host = &mut self.hosts[index];
        host.source
            .as_mut()
            .ok_or_else(|| crate::connection::invalid("Host connection is unavailable"))?
            .begin_observation(window)?;
        host.observation_deadline
            .get_or_insert_with(|| Instant::now() + Duration::from_secs(15));
        window.host_mut(&host.action).expect("opened host").catalog = CatalogState::Loading;
        Ok(())
    }
    pub fn detach(
        &mut self,
        window: &mut WindowWorkspace,
        owner: SourceToken,
        key: &ScreenKey,
    ) -> io::Result<()> {
        window.detach(owner, key)?;
        self.remove_unused(window);
        Ok(())
    }
    /// Take every Screen of the host out of the window and release the host.
    /// Nothing is ended on the host.
    pub fn close(
        &mut self,
        window: &mut WindowWorkspace,
        action: &HostActionTarget,
    ) -> io::Result<()> {
        self.index(window, action)?;
        self.restoring.retain(|(held, _)| held != action);
        window.detach_all(action.local_owner(), action.source_id())?;
        window.host_mut(action).expect("validated host").browsing = false;
        self.remove_unused(window);
        Ok(())
    }
    pub fn command(
        &mut self,
        window: &mut WindowWorkspace,
        command: Command,
    ) -> anyhow::Result<SourceEffects> {
        let owner = command
            .source
            .ok_or_else(|| crate::connection::invalid("Remote command has no source"))?;
        let host = self
            .hosts
            .iter_mut()
            .find(|host| host.action.source_id() == owner.source_id())
            .ok_or_else(|| crate::connection::invalid("Remote source is gone"))?;
        let allowed = matches!(
            &command.kind,
            CommandKind::CreateRemoteScreen { .. }
                | CommandKind::RenameScreen { .. }
                // A refused save would leave the text in the window alone,
                // where the Screen's next snapshot replaces it.
                | CommandKind::SetScratchpad { .. }
                | CommandKind::Split { .. }
                | CommandKind::Tab { .. }
                | CommandKind::Select { .. }
                | CommandKind::Focus { .. }
                | CommandKind::Input { .. }
                | CommandKind::StoreFiles { .. }
                | CommandKind::CancelFileStore { .. }
                | CommandKind::AnswerFileOffer { .. }
                | CommandKind::AnswerEditorRequest { .. }
                | CommandKind::ResizePane { .. }
                | CommandKind::TakeOverSize { .. }
                | CommandKind::Visible { .. }
                | CommandKind::ResizeSplit { .. }
                | CommandKind::CycleTab { .. }
                | CommandKind::CycleTabInSlot { .. }
                | CommandKind::MovePane { .. }
                | CommandKind::PlaceTab { .. }
                | CommandKind::SetMaximized { .. }
                | CommandKind::ReadViewer { .. }
                | CommandKind::AnchorViewer { .. }
                | CommandKind::ReviewViewer { .. }
                | CommandKind::OpenTextViewer { .. }
                | CommandKind::Annotate { .. }
                | CommandKind::AnswerReview { .. }
                | CommandKind::NewTerminal { .. }
                | CommandKind::FloatPane { .. }
                | CommandKind::DockFloat { .. }
                | CommandKind::MoveFloat { .. }
                | CommandKind::TransferPane { .. }
                | CommandKind::ApplyTemplate { .. }
                | CommandKind::ExtractTemplate { .. }
                | CommandKind::AssessClose { .. }
        ) || matches!(&command.kind,CommandKind::RequestClose {plan,..} if plan.target!=CloseTarget::Window);
        let effects = if allowed {
            host.source
                .as_mut()
                .ok_or_else(|| crate::connection::invalid("Remote connection is not ready"))?
                .command(window, command)?
        } else {
            let mut effects = SourceEffects::default();
            effects.updates.push(rejected_command(
                Some(owner),
                &command.kind,
                "Action is not available on remote Screens".into(),
            ));
            effects
        };
        let mut out = SourceEffects::default();
        merge_scoped(host, window, &mut out, effects);
        Ok(out)
    }
    pub fn cancel_created_follow(&mut self) {
        for host in &mut self.hosts {
            if let Some(source) = &mut host.source {
                source.cancel_created_follow();
            }
        }
    }
    /// A Screen that its connected host no longer has leaves the window, the
    /// same as if it had been detached by hand.
    fn detach_missing(&mut self, window: &mut WindowWorkspace) -> bool {
        let mut gone = Vec::new();
        for host in &self.hosts {
            if host.cancelled || host.dispose {
                continue;
            }
            let Some(source) = window.source(host.action.source_id()) else {
                continue;
            };
            if !source.is_ready() {
                continue;
            }
            gone.extend(
                source
                    .screens()
                    .iter()
                    .filter(|info| source.is_missing(info.id.as_str()))
                    .map(|info| {
                        (
                            host.action.local_owner(),
                            source.screen_key(info.id.as_str()),
                        )
                    }),
            );
        }
        let mut changed = false;
        for (owner, key) in gone {
            changed |= window.detach(owner, &key).is_ok();
        }
        changed
    }
    fn remove_unused(&mut self, window: &mut WindowWorkspace) {
        for host in &mut self.hosts {
            if !host.dispose
                && window
                    .host(host.action.source_id())
                    .is_some_and(|state| !state.is_browsing())
                && !has_references(window, host.action.source_id())
            {
                host.dispose = true;
                host.cancelled = true;
                host.close_source();
                let _ = window.begin_close(host.action.source_id());
                if host.setup.is_none() && host.prepared.is_none() && host.closing.is_none() {
                    host.closed_due = true;
                }
            }
        }
    }
    pub fn handle(&mut self, window: &mut WindowWorkspace, step: Step) -> SourceEffects {
        let action = match &step {
            Step::Prepared(action)
            | Step::Event(action, _)
            | Step::Closed(action)
            | Step::Progress(action) => action.clone(),
        };
        let mut out = match self.handle_inner(window, step) {
            Ok(out) => out,
            Err(error) => {
                // An offending host cannot fail the local worker or abandon
                // another host's retained setup/close. Keep cleanup owned here.
                if let Some(host) = self.hosts.iter_mut().find(|host| host.action == action) {
                    host.failure = Some(io::Error::other(error));
                    host.close_source();
                    let _ = window.disconnect(action.source_id());
                    if host.setup.is_none() && host.prepared.is_none() && host.closing.is_none() {
                        if host.dispose {
                            host.closed_due = true;
                        } else {
                            let _ = window.finish_attempt(action.source_id());
                        }
                    }
                }
                SourceEffects {
                    changed: true,
                    ..SourceEffects::default()
                }
            }
        };
        self.drain_close_outcomes(&mut out);
        out
    }
    fn drain_close_outcomes(&mut self, out: &mut SourceEffects) {
        for host in &mut self.hosts {
            if let Some(update) = host.close_rejected.take() {
                out.updates.push(update);
            }
        }
    }
    fn handle_inner(
        &mut self,
        window: &mut WindowWorkspace,
        step: Step,
    ) -> anyhow::Result<SourceEffects> {
        let action = match &step {
            Step::Prepared(action)
            | Step::Event(action, _)
            | Step::Closed(action)
            | Step::Progress(action) => action,
        };
        let Some(index) = self.hosts.iter().position(|host| &host.action == action) else {
            return Ok(SourceEffects::default());
        };
        let host = &mut self.hosts[index];
        let action = host.action.clone();
        let id = action.source_id();
        let mut out = SourceEffects::default();
        match step {
            Step::Progress(_) => {
                let progress = host
                    .progress
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .take();
                if !host.cancelled {
                    if let (Some(progress), Some(state)) = (progress, window.host_mut(&action)) {
                        if state.can_cancel() {
                            state.progress = Some(Arc::new(progress));
                            out.changed = true;
                        }
                    }
                }
            }
            Step::Prepared(_) => {
                if host
                    .recovery_deadline
                    .is_some_and(|deadline| Instant::now() >= deadline)
                {
                    host.cancelled = true;
                    host.context.close();
                    let _ = window.disconnect(id);
                }
                let result = host.prepared.take().expect("completed setup retained");
                match result {
                    Ok(client) if host.cancelled => {
                        host.closing = Some(Box::pin(client.finish()));
                    }
                    Ok(client) => {
                        if let Err(error) = window.bind_remote(id, &client) {
                            host.context.close();
                            host.failure = Some(error);
                            host.closing = Some(Box::pin(client.finish()));
                            let _ = window.disconnect(id);
                        } else {
                            let owner = window.source(id).expect("bound source").source_token();
                            host.source = Some(
                                SourceRuntime::new(client, owner)
                                    .with_window_presence(self.presence.clone()),
                            );
                            host.prelude_deadline = Some(Instant::now() + Duration::from_secs(15));
                        }
                    }
                    Err(error) => {
                        host.context.close();
                        host.failure = Some(error);
                        host.closed_due = true;
                        if !host.dispose {
                            let _ = window.disconnect(id);
                        }
                    }
                }
                out.changed = true;
            }
            Step::Event(_, result) => match result {
                Ok(Some(event)) => {
                    if host
                        .recovery_deadline
                        .is_some_and(|deadline| Instant::now() >= deadline)
                    {
                        host.cancelled = true;
                        host.failure = Some(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "Host recovery timed out; retry to reconnect",
                        ));
                        host.close_source();
                        let _ = window.disconnect(id);
                        out.changed = true;
                        return Ok(out);
                    }
                    let outcome = host_event(host, window, event);
                    match outcome {
                        Ok(effects) => out = effects,
                        Err(error) => {
                            host.failure = Some(error);
                            host.close_source();
                            let _ = window.disconnect(id);
                            out.changed = true;
                        }
                    }
                }
                result => {
                    host.failure = Some(result.err().unwrap_or_else(|| {
                        io::Error::new(io::ErrorKind::UnexpectedEof, "Host connection closed")
                    }));
                    host.close_source();
                    let _ = window.disconnect(id);
                    out.changed = true;
                }
            },
            Step::Closed(_) => {
                host.closed_due = false;
                if host.dispose {
                    let mut removed = self.hosts.remove(index);
                    if let Some(update) = removed.close_rejected.take() {
                        out.updates.push(update);
                    }
                    if self.cleanup_error.is_none() {
                        self.cleanup_error = removed.cleanup_error.take();
                    }
                    window.finish_close(id)?;
                } else {
                    let diagnostic = if host
                        .cleanup_error
                        .as_ref()
                        .is_some_and(|error| error.kind() == io::ErrorKind::InvalidData)
                    {
                        plain_diagnostic(
                            &host.context.budget,
                            host.cleanup_error
                                .as_ref()
                                .expect("invalid final protocol state"),
                        )
                    } else if let Some(reason) = &host.reason {
                        HostDiagnostic::from_closed(reason, &host.context.budget).map(Arc::new)
                    } else if let Some(error) = host.failure.as_ref() {
                        match splitty_transport::negotiation::preparation_failure(error) {
                            Some(reason) => {
                                HostDiagnostic::from_closed(reason, &host.context.budget)
                                    .map(Arc::new)
                            }
                            None => plain_diagnostic(&host.context.budget, error),
                        }
                    } else {
                        plain_diagnostic(&host.context.budget, &"Host connection cancelled")
                    };
                    // A decoder failure cannot inherit retryability from a
                    // purported structured payload or a formatted error.
                    let retryable = diagnostic
                        .as_ref()
                        .is_ok_and(|value| host.retryable(Some(value)));
                    if diagnostic.is_err() {
                        host.offer = None;
                    }
                    let diagnostic = diagnostic
                        .or_else(|error| plain_diagnostic(&host.context.budget, &error))
                        .ok();
                    if let Some(offer) = diagnostic
                        .as_ref()
                        .and_then(|value| value.update_offer.as_ref())
                    {
                        host.offer = host
                            .context
                            .budget
                            .reserve(offer.retained_bytes(), false)
                            .ok()
                            .map(|lease| Arc::new(lease.charge(offer.clone())));
                    }
                    if host.cancelled
                        || host.update_dispatched
                        || host
                            .cleanup_error
                            .as_ref()
                            .is_some_and(|error| error.kind() == io::ErrorKind::InvalidData)
                    {
                        host.offer = None;
                    }
                    if let Some(state) = window.host_mut(&action) {
                        state.update_offer = host.offer.clone();
                        state.update_state = if host.update_dispatched {
                            HostUpdateState::Unconfirmed
                        } else if host.update_pending {
                            HostUpdateState::Closing
                        } else {
                            HostUpdateState::Idle
                        };
                        state.diagnostic = diagnostic;
                        state.progress = None;
                    }
                    let now = Instant::now();
                    if !host.cancelled
                        && !host.update_pending
                        && !host.update_dispatched
                        && host.was_ready
                        && retryable
                        && host
                            .recovery_deadline
                            .is_some_and(|deadline| now < deadline)
                    {
                        window.wait_recovery(id)?;
                        host.retry_at = Some(now + host.retry_delay);
                        host.retry_delay = (host.retry_delay * 2).min(Duration::from_secs(10));
                    } else {
                        host.recovery_deadline = None;
                        host.retry_at = None;
                        window.finish_attempt(id)?;
                    }
                }
                out.changed = true;
            }
        }
        self.drain_close_outcomes(&mut out);
        Ok(out)
    }
    pub fn reconcile(&mut self, window: &mut WindowWorkspace) -> anyhow::Result<SourceEffects> {
        let mut out = SourceEffects::default();
        out.changed |= self.detach_missing(window);
        self.remove_unused(window);
        if self
            .update_plan
            .as_ref()
            .is_some_and(|plan| !window.accepts_host_update_plan(plan))
        {
            let plan = self.update_plan.take().expect("invalidated update plan");
            out.updates.push(Update::HostUpdateRejected {
                owner: plan.owner(),
                request_id: plan.request_id(),
                reason: "Host update confirmation is no longer current".into(),
            });
        }
        for host in &mut self.hosts {
            if host.cancelled {
                continue;
            }
            let id = host.action.source_id();
            if host.update_pending
                && host.setup.is_none()
                && host.prepared.is_none()
                && host.closing.is_none()
                && !host.closed_due
            {
                if host.cleanup_error.is_some()
                    || host.action.local_owner() != window.lifecycle_token()
                {
                    host.update_pending = false;
                    if let Some(state) = window.host_mut(&host.action) {
                        state.update_state = HostUpdateState::Idle;
                    }
                    out.changed = true;
                    continue;
                }
                if let Some(state) = window.host_mut(&host.action) {
                    state.update_state = HostUpdateState::Idle;
                }
                match window.retry(id) {
                    Ok(()) => {
                        let action = window
                            .host(id)
                            .expect("admitted update")
                            .action_target()
                            .clone();
                        *host = Host::with_update(self.service, action, true)?;
                        window
                            .host_mut(&host.action)
                            .expect("update host")
                            .update_state = HostUpdateState::Updating;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        if let Some(state) = window.host_mut(&host.action) {
                            state.update_state = HostUpdateState::Closing;
                        }
                    }
                    Err(error) => {
                        host.update_pending = false;
                        host.failure = Some(error);
                        host.closed_due = true;
                    }
                }
                out.changed = true;
                continue;
            }
            let now = Instant::now();
            if host
                .recovery_deadline
                .is_some_and(|deadline| now >= deadline)
            {
                host.cancelled = true;
                host.retry_at = None;
                host.failure = Some(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Host recovery timed out; retry to reconnect",
                ));
                host.close_source();
                let _ = window.disconnect(id);
                if host.setup.is_none() && host.prepared.is_none() && host.closing.is_none() {
                    host.closed_due = true;
                }
                out.changed = true;
                continue;
            }
            if host.retry_at.is_some_and(|deadline| now >= deadline) {
                // Capacity may be occupied by other sources. Keep this bounded
                // wait and its original overall deadline, without dialing.
                if let Err(error) = window.recover(id) {
                    if error.kind() == io::ErrorKind::WouldBlock {
                        continue;
                    }
                    host.failure = Some(error);
                    host.cancelled = true;
                    host.retry_at = None;
                    let _ = window.disconnect(id);
                    host.closed_due = true;
                } else {
                    let action = window
                        .host(id)
                        .expect("admitted recovery")
                        .action_target()
                        .clone();
                    let mut replacement = Host::new(self.service, action)?;
                    replacement.was_ready = host.was_ready;
                    replacement.recovery_deadline = host.recovery_deadline;
                    replacement.retry_delay = host.retry_delay;
                    if self.cleanup_error.is_none() {
                        self.cleanup_error = host.cleanup_error.take();
                    }
                    *host = replacement;
                }
                out.changed = true;
                continue;
            }
            if host
                .prelude_deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
            {
                host.failure = Some(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Host readiness timed out",
                ));
                host.close_source();
                let _ = window.disconnect(id);
                out.changed = true;
                continue;
            }
            if window.execution_token(id).is_some() {
                if let Some(source) = &mut host.source {
                    let failed = window
                        .host(id)
                        .is_some_and(|state| state.catalog_state() == CatalogState::Failed);
                    if !failed && source.observation_pending(window) {
                        host.observation_deadline
                            .get_or_insert_with(|| Instant::now() + Duration::from_secs(15));
                    }
                    if host
                        .observation_deadline
                        .is_some_and(|deadline| Instant::now() >= deadline)
                    {
                        let effects = source.abort_observation(
                            window,
                            "Host Screen observation timed out; retry to refresh",
                        );
                        host.observation_deadline = None;
                        let diagnostic = plain_diagnostic(
                            &host.context.budget,
                            &"Host Screen observation timed out; retry to refresh",
                        )
                        .ok();
                        if let Some(state) = window.host_mut(&host.action) {
                            state.catalog = CatalogState::Failed;
                            state.catalog_error = diagnostic;
                        }
                        merge_scoped(host, window, &mut out, effects);
                    } else {
                        match source.reconcile(window) {
                            Ok(effects) => {
                                if source.observation_active() {
                                    host.observation_deadline.get_or_insert_with(|| {
                                        Instant::now() + Duration::from_secs(15)
                                    });
                                    if let Some(state) = window.host_mut(&host.action) {
                                        if state.catalog == CatalogState::Failed {
                                            state.catalog = CatalogState::Loading;
                                            state.catalog_error = None;
                                            out.changed = true;
                                        }
                                    }
                                }
                                merge_scoped(host, window, &mut out, effects);
                            }
                            Err(error) => {
                                host.failure = Some(io::Error::other(error));
                                host.close_source();
                                let _ = window.disconnect(id);
                                out.changed = true;
                            }
                        }
                    }
                }
            }
        }
        self.settle_restores(window, &mut out);
        self.drain_close_outcomes(&mut out);
        Ok(out)
    }
    pub async fn finish(mut self, window: &mut WindowWorkspace) -> io::Result<()> {
        for host in &mut self.hosts {
            host.cancelled = true;
            host.dispose = true;
            host.close_source();
            let _ = window.begin_close(host.action.source_id());
            if host.setup.is_none() && host.prepared.is_none() && host.closing.is_none() {
                host.closed_due = true;
            }
        }
        while !self.hosts.is_empty() {
            let step = self.next().await;
            self.handle(window, step);
        }
        self.cleanup_error.map_or(Ok(()), Err)
    }
}
fn has_references(window: &WindowWorkspace, id: SourceId) -> bool {
    window
        .source(id)
        .is_some_and(|source| !source.screens().is_empty())
}
fn host_event(
    host: &mut Host<'_>,
    window: &mut WindowWorkspace,
    event: Box<Event>,
) -> io::Result<SourceEffects> {
    let id = host.action.source_id();
    let mut out = SourceEffects::default();
    if !host
        .source
        .as_ref()
        .expect("event has a source driver")
        .welcomed
    {
        let json = event.json().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Host sent output before readiness",
            )
        })?;
        if !host.observe_prelude(json)? {
            if host.remote_ready {
                if let Some(state) = window.host_mut(&host.action) {
                    state.update_offer = host.offer.clone();
                    state.daemon_version = host.daemon_version.clone();
                }
                out.changed = true;
            }
            return Ok(out);
        }
        window.confirm_ready(host.source.as_ref().expect("validated source").owner())?;
        host.update_dispatched = false;
        window
            .host_mut(&host.action)
            .expect("ready host")
            .update_state = HostUpdateState::Idle;
        host.recovery_deadline = None;
        host.retry_at = None;
        host.retry_delay = Duration::from_secs(1);
        host.prelude_deadline = None;
        host.observation_deadline = Some(Instant::now() + Duration::from_secs(15));
        window.host_mut(&host.action).expect("ready host").catalog = CatalogState::Loading;
    }
    let source = host.source.as_mut().expect("event has a source driver");
    if event.json().is_none() {
        out.updates.push(Update::Event {
            source: source.owner(),
            event,
        });
    } else {
        let first_welcome = !source.welcomed;
        out = source.event(window, event).map_err(io::Error::other)?;
        if first_welcome {
            source.begin_observation(window)?;
        }
    }
    if !source.observation_pending(window)
        && window.source(id).is_some_and(|source| {
            source.live_catalog().is_some()
                && !source.routes_reconciling()
                && source.screens().iter().all(|info| {
                    source.is_missing(info.id.as_str())
                        || source
                            .screen(info.id.as_str())
                            .is_some_and(|screen| screen.snapshot().is_some())
                })
        })
    {
        host.observation_deadline = None;
        if let Some(state) = window.host_mut(&host.action) {
            state.catalog = CatalogState::Ready;
            state.catalog_error = None;
        }
    }
    let mut scoped = SourceEffects::default();
    merge_scoped(host, window, &mut scoped, out);
    Ok(scoped)
}
fn merge_scoped(
    host: &mut Host<'_>,
    window: &mut WindowWorkspace,
    out: &mut SourceEffects,
    effects: SourceEffects,
) {
    out.changed |= effects.changed;
    out.select_created = effects.select_created.or(out.select_created.take());
    out.follow_remote_creation = effects
        .follow_remote_creation
        .or(out.follow_remote_creation.take());
    if let Some(reason) = effects.observation_error {
        host.observation_deadline = None;
        if let Some(state) = window.host_mut(&host.action) {
            state.catalog = CatalogState::Failed;
            state.catalog_error = plain_diagnostic(&host.context.budget, &reason).ok();
            out.changed = true;
        }
    }
    for update in effects.updates {
        match update {
            Update::Status(message) => {
                if let Some(state) = window.host_mut(&host.action) {
                    // Presentation pressure must not tear down the window or
                    // another source. Keep the previous charged diagnostic if
                    // this connection cannot admit a replacement.
                    if let Ok(diagnostic) = plain_diagnostic(&host.context.budget, &message) {
                        state.diagnostic = Some(diagnostic);
                        out.changed = true;
                    }
                }
            }
            other => out.updates.push(other),
        }
    }
}
fn plain_diagnostic(
    budget: &ConnectionBudget,
    message: &dyn std::fmt::Display,
) -> io::Result<Arc<Charged<HostDiagnostic>>> {
    use std::fmt::Write;
    const LIMIT: usize = 4096;
    struct Bounded(String);
    impl std::fmt::Write for Bounded {
        fn write_str(&mut self, value: &str) -> std::fmt::Result {
            let mut end = value.len().min(LIMIT - self.0.len());
            while !value.is_char_boundary(end) {
                end -= 1;
            }
            self.0.push_str(&value[..end]);
            Ok(())
        }
    }
    let lease = budget
        .reserve(std::mem::size_of::<HostDiagnostic>() + LIMIT, false)
        .map_err(|error| io::Error::new(io::ErrorKind::OutOfMemory, error))?;
    let mut text = Bounded(String::with_capacity(LIMIT));
    let _ = write!(&mut text, "{message}");
    Ok(Arc::new(lease.charge(HostDiagnostic {
        code: None,
        message: text.0,
        hint: None,
        retryable: false,
        update_offer: None,
    })))
}
