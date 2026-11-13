//! Explicit local-owner outbound forwarding, independently of Screen lifetime.
mod api;
mod runtime;
#[cfg(feature = "ui-test-support")]
mod test_support;
use crate::{
    external_links::WebUrl,
    workspace::{SourceToken, TerminalToken, WindowWorkspace},
};
pub(crate) use runtime::{PortsClient, PortsEvent};
// The local hub's replies (`native_api::forwards`), as this module reads them.
use splitty_core::native_api::forwards::{
    ForwardInfo as ActiveForward, ForwardPreferences as Preferences, ForwardState as ActiveState,
    SavedForward,
};
use splitty_remote::ssh::SshTarget;
use splitty_transport::budget::{Charged, Lease};
use std::{
    collections::BTreeMap,
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MappingKey {
    target: String,
    remote: u16,
}
impl MappingKey {
    pub fn new(target: &str, remote: u16) -> io::Result<Self> {
        let target = SshTarget::parse(target).map_err(|_| invalid("Invalid SSH target"))?;
        if remote == 0 {
            return Err(invalid("Remote port must be between 1 and 65535"));
        }
        Ok(Self {
            target: target.as_str().into(),
            remote,
        })
    }
    pub fn target(&self) -> &str {
        &self.target
    }
    pub fn remote(&self) -> u16 {
        self.remote
    }
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn validate_local(local: Option<u16>) -> io::Result<()> {
    if local == Some(0) {
        Err(invalid("Local port must be between 1 and 65535"))
    } else {
        Ok(())
    }
}
fn validate_label(label: Option<&str>) -> io::Result<()> {
    if label.is_some_and(|value| value.chars().count() > 200 || value.chars().any(char::is_control))
    {
        Err(invalid(
            "Label must be at most 200 characters without control characters",
        ))
    } else {
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForwardAction {
    Start,
    Open,
    Stop,
    Save,
    Forget,
}
#[derive(Clone, Debug)]
pub struct ForwardRequest {
    key: MappingKey,
    action: ForwardAction,
    local: Option<u16>,
    remember: bool,
    restore: Option<bool>,
    label: Option<String>,
    link: Option<(TerminalToken, WebUrl)>,
}
impl ForwardRequest {
    pub fn start(
        key: MappingKey,
        local: Option<u16>,
        remember: bool,
        label: Option<String>,
    ) -> io::Result<Self> {
        validate_local(local)?;
        validate_label(label.as_deref())?;
        Ok(Self {
            key,
            action: ForwardAction::Start,
            local,
            remember,
            restore: None,
            label,
            link: None,
        })
    }
    pub fn open(
        key: MappingKey,
        local: Option<u16>,
        remember: bool,
        label: Option<String>,
    ) -> io::Result<Self> {
        let mut request = Self::start(key, local, remember, label)?;
        request.action = ForwardAction::Open;
        Ok(request)
    }
    pub fn open_link(key: MappingKey, terminal: TerminalToken, url: WebUrl) -> io::Result<Self> {
        let parsed =
            url::Url::parse(url.as_str()).map_err(|_| invalid("Invalid development URL"))?;
        if !url.is_loopback_or_unspecified() || parsed.port_or_known_default() != Some(key.remote) {
            return Err(invalid("Development URL does not match the remote port"));
        }
        let mut request = Self::open(key, None, false, None)?;
        request.link = Some((terminal, url));
        Ok(request)
    }
    pub fn save(key: MappingKey, local: Option<u16>, label: Option<String>) -> io::Result<Self> {
        let mut request = Self::start(key, local, false, label)?;
        request.action = ForwardAction::Save;
        Ok(request)
    }
    /// Saving restoration is independent of starting or stopping a tunnel.
    pub fn with_restore(mut self, enabled: bool) -> Self {
        self.restore = Some(enabled);
        self
    }
    pub fn stop(key: MappingKey) -> Self {
        Self {
            key,
            action: ForwardAction::Stop,
            local: None,
            remember: false,
            restore: None,
            label: None,
            link: None,
        }
    }
    pub fn forget(key: MappingKey) -> Self {
        let mut request = Self::stop(key);
        request.action = ForwardAction::Forget;
        request
    }
    pub fn key(&self) -> &MappingKey {
        &self.key
    }
    pub fn action(&self) -> ForwardAction {
        self.action
    }
    pub fn local(&self) -> Option<u16> {
        self.local
    }
    pub fn remember(&self) -> bool {
        self.remember
    }
    pub(crate) fn valid_origin(&self, window: &WindowWorkspace) -> bool {
        self.link.as_ref().is_none_or(|(terminal, _)| {
            window.accepts_terminal(terminal)
                && window.source_target(terminal.source_token().source_id())
                    == Some(self.key.target())
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForwardOutcome {
    Pending,
    Succeeded,
    Partial,
    Failed,
    Uncertain,
    Rejected,
}
/// A single deliberate browser continuation. Copies share revocation and consumption.
#[derive(Clone, Debug)]
pub struct ForwardOpen {
    _allocation: Option<Arc<Lease>>,
    url: WebUrl,
    terminal: Option<TerminalToken>,
    active: Arc<AtomicBool>,
}
impl ForwardOpen {
    pub fn url(&self) -> &WebUrl {
        &self.url
    }
    pub fn terminal(&self) -> Option<&TerminalToken> {
        self.terminal.as_ref()
    }
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
    pub fn revoke(&self) {
        self.active.store(false, Ordering::Release);
    }
    pub fn consume(&self) -> bool {
        self.active.swap(false, Ordering::AcqRel)
    }
}
#[derive(Clone, Debug)]
pub struct ForwardResult {
    _allocation: Option<Arc<Lease>>,
    owner: SourceToken,
    request_id: Uuid,
    key: MappingKey,
    action: ForwardAction,
    outcome: ForwardOutcome,
    reason: Option<String>,
    open: Option<ForwardOpen>,
}
impl ForwardResult {
    pub(crate) fn new(
        owner: SourceToken,
        request_id: Uuid,
        request: &ForwardRequest,
        outcome: ForwardOutcome,
        reason: Option<String>,
    ) -> Self {
        Self {
            _allocation: None,
            owner,
            request_id,
            key: request.key.clone(),
            action: request.action,
            outcome,
            reason,
            open: None,
        }
    }
    pub fn owner(&self) -> SourceToken {
        self.owner
    }
    pub fn request_id(&self) -> Uuid {
        self.request_id
    }
    pub fn key(&self) -> &MappingKey {
        &self.key
    }
    pub fn action(&self) -> ForwardAction {
        self.action
    }
    pub fn outcome(&self) -> ForwardOutcome {
        self.outcome
    }
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
    pub fn eligible_open(&self) -> Option<&ForwardOpen> {
        self.open.as_ref().filter(|open| open.is_active())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForwardState {
    Detected,
    Saved,
    Paused,
    Failed,
    Up,
    Rebuilding,
}
/// Independent last-good service resources. Discovery is joined only at presentation time.
#[derive(Clone, Debug)]
pub struct ForwardSnapshot {
    owner: SourceToken,
    generation: u64,
    available: bool,
    loading: bool,
    active: Option<Arc<Charged<Vec<ActiveForward>>>>,
    saved: Option<Arc<Charged<Preferences>>>,
    active_error: Option<String>,
    saved_error: Option<String>,
}
impl ForwardSnapshot {
    pub(crate) fn new(owner: SourceToken, available: bool) -> Self {
        Self {
            owner,
            generation: 0,
            available,
            loading: false,
            active: None,
            saved: None,
            active_error: None,
            saved_error: None,
        }
    }
    pub fn owner(&self) -> SourceToken {
        self.owner
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn is_available(&self) -> bool {
        self.available
    }
    pub fn is_loading(&self) -> bool {
        self.loading
    }
    pub fn active_error(&self) -> Option<&str> {
        self.active_error.as_deref()
    }
    pub fn saved_error(&self) -> Option<&str> {
        self.saved_error.as_deref()
    }
    pub fn rows(&self, window: &WindowWorkspace) -> Vec<ForwardRow> {
        let mut rows = BTreeMap::<MappingKey, ForwardRow>::new();
        for host in window.hosts() {
            let Some(source) = window.source(host.action_target().source_id()) else {
                continue;
            };
            let Some(observation) = source.port_observation() else {
                continue;
            };
            let Some(target) = source.target() else {
                continue;
            };
            for observed in observation.ports() {
                let Ok(key) = MappingKey::new(target, observed.port()) else {
                    continue;
                };
                let row = rows
                    .entry(key.clone())
                    .or_insert_with(|| ForwardRow::new(key));
                row.process = Some(observed.process().into());
            }
        }
        if let Some(saved) = &self.saved {
            for saved in &saved.forwards {
                let key = MappingKey {
                    target: saved.target.clone(),
                    remote: saved.remote,
                };
                let row = rows
                    .entry(key.clone())
                    .or_insert_with(|| ForwardRow::new(key));
                row.local = Some(saved.local);
                row.label = Some(saved.label.clone());
                row.saved = true;
                row.restore = saved.enabled;
                row.state = if !saved.enabled {
                    ForwardState::Paused
                } else if saved.last_error.is_some() {
                    ForwardState::Failed
                } else {
                    ForwardState::Saved
                };
                row.error = saved.last_error.as_ref().map(|error| error.message.clone());
            }
        }
        if let Some(active) = &self.active {
            for active in active.iter() {
                let key = MappingKey {
                    target: active.target.clone(),
                    remote: active.remote,
                };
                let row = rows
                    .entry(key.clone())
                    .or_insert_with(|| ForwardRow::new(key));
                row.local = Some(active.local);
                row.state = if active.state == ActiveState::Up {
                    ForwardState::Up
                } else {
                    ForwardState::Rebuilding
                };
                row.error = None;
            }
        }
        rows.into_values().collect()
    }
}
#[derive(Clone, Debug)]
pub struct ForwardRow {
    key: MappingKey,
    local: Option<u16>,
    label: Option<String>,
    process: Option<String>,
    saved: bool,
    restore: bool,
    state: ForwardState,
    error: Option<String>,
}
impl ForwardRow {
    fn new(key: MappingKey) -> Self {
        Self {
            key,
            local: None,
            label: None,
            process: None,
            saved: false,
            restore: false,
            state: ForwardState::Detected,
            error: None,
        }
    }
    pub fn key(&self) -> &MappingKey {
        &self.key
    }
    pub fn local(&self) -> Option<u16> {
        self.local
    }
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }
    pub fn process(&self) -> Option<&str> {
        self.process.as_deref()
    }
    pub fn is_saved(&self) -> bool {
        self.saved
    }
    pub fn restores_on_connect(&self) -> bool {
        self.restore
    }
    pub fn state(&self) -> ForwardState {
        self.state
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}
fn rewritten_url(request: &ForwardRequest, local: u16) -> Option<WebUrl> {
    let mut parsed = match &request.link {
        Some((_, url)) => url::Url::parse(url.as_str()).ok()?,
        None => url::Url::parse("http://localhost").ok()?,
    };
    parsed.set_username("").ok()?;
    parsed.set_password(None).ok()?;
    parsed.set_host(Some("localhost")).ok()?;
    parsed.set_port(Some(local)).ok()?;
    WebUrl::parse(parsed.as_str())
}
#[cfg(test)]
mod tests;
