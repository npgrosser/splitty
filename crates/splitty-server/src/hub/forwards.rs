//! Live forwards and durable opt-in mappings, ordered independently per key.
//! Only explicit POST and successful host readiness may open a tunnel. Short
//! map/storage locks never span SSH waits; a lane includes write-failure cleanup.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use serde::Serialize;
use splitty_remote::ssh::{SshTarget, TargetError};
use splitty_remote::tunnel::{self, backoff, PortForward, PortReservation};
use tokio::sync::{oneshot, Mutex as AsyncMutex};
use tokio::task::JoinHandle;

use super::preferences::{Preferences, SavedError, SavedForward, Store, MAX_LABEL};
use super::{Connector, Forwarding, HubError};

pub const MAX_PER_HOST: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ForwardInfo {
    pub target: String,
    pub remote: u16,
    pub local: u16,
    pub url: String,
    pub state: ForwardState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ForwardState {
    Up,
    Down,
}

#[derive(Debug, thiserror::Error)]
pub enum ForwardError {
    #[error(transparent)]
    Target(#[from] TargetError),
    #[error("{0} is not a port from 1 to 65535.")]
    Port(i64),
    #[error("{0}")]
    Invalid(String),
    #[error("{host} has {MAX_PER_HOST} forwards already, the most one host can have.")]
    Limit { host: String },
    #[error("At most 32 saved forwards per host and 256 total are allowed.")]
    PreferenceLimit,
    #[error("There is no forward of port {port} on {host}.")]
    NotFound { host: String, port: u16 },
    #[error("Local port {port} is unavailable: {source}")]
    LocalConflict { port: u16, source: std::io::Error },
    #[error("This saved forward is paused. Resume it explicitly to enable it.")]
    Paused,
    #[error("The active forward uses local port {local}; stop it before selecting another port.")]
    ActiveConflict { local: u16 },
    #[error("Cannot write remote-forwards.json: {0}")]
    Storage(#[from] std::io::Error),
    #[error("Forward operation ended before completion.")]
    OperationEnded,
    /// A reverse forward's port is in use on the host.
    #[error("Port {port} is already in use on {host}; choose another port on the host.")]
    HostPortConflict { host: String, port: u16 },
    /// A reverse forward would share the local server's own port.
    #[error("Port {port} is this Splitty's own port; it is never shared.")]
    LocalPortReserved { port: u16 },
    #[error(transparent)]
    Failed(HubError),
}

impl ForwardError {
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Limit { .. } => Some("forward_limit"),
            Self::PreferenceLimit => Some("preference_limit"),
            Self::LocalConflict { .. } => Some("local_port_conflict"),
            Self::Paused => Some("forward_paused"),
            Self::ActiveConflict { .. } => Some("active_mapping_conflict"),
            Self::Storage(_) => Some("storage_failure"),
            Self::OperationEnded => Some("internal"),
            Self::HostPortConflict { .. } => Some("host_port_conflict"),
            Self::LocalPortReserved { .. } => Some("local_port_reserved"),
            _ => None,
        }
    }

    pub(super) fn saved_error(&self, target: &str) -> SavedError {
        if let Self::Failed(error) = self {
            let failure = error.failure(target);
            return SavedError {
                code: failure.code.as_str().into(),
                message: failure.message,
                hint: failure.hint,
            };
        }
        SavedError {
            code: self.code().unwrap_or("internal").into(),
            message: self.to_string(),
            hint: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct Key {
    target: String,
    remote: u16,
}

impl Key {
    pub(super) fn matches(&self, row: &SavedForward) -> bool {
        self.target == row.target && self.remote == row.remote
    }
}

enum Slot {
    Opening,
    Open(Supervised),
}

struct State {
    slots: HashMap<Key, Slot>,
    preferences: Store,
    errors: HashMap<Key, SavedError>,
    restoring: HashSet<Key>,
}

struct Shared {
    connector: Arc<dyn Connector>,
    state: Mutex<State>,
    lanes: Mutex<HashMap<Key, Weak<AsyncMutex<()>>>>,
}

pub struct Forwards {
    shared: Arc<Shared>,
    // Tasks own Shared, not Forwards. Request cancellation does not release a
    // lane during cleanup; hub teardown cancels every task without an owner cycle.
    operations: Mutex<Vec<JoinHandle<()>>>,
}

impl Forwards {
    pub fn new(connector: Arc<dyn Connector>, config_dir: PathBuf) -> Self {
        Self {
            shared: Arc::new(Shared {
                connector,
                state: Mutex::new(State {
                    slots: HashMap::new(),
                    preferences: Store::new(config_dir),
                    errors: HashMap::new(),
                    restoring: HashSet::new(),
                }),
                lanes: Mutex::new(HashMap::new()),
            }),
            operations: Mutex::new(Vec::new()),
        }
    }

    pub async fn open(
        &self,
        target: &str,
        port: i64,
        local: Option<i64>,
        automatic: bool,
    ) -> Result<ForwardInfo, ForwardError> {
        let (target, key) = key(target, port)?;
        let local = local.map(check_port).transpose()?;
        if automatic && local.is_some() {
            return Err(ForwardError::Invalid(
                "automatic forwarding cannot select a local port".into(),
            ));
        }
        self.ordered(key, move |shared, key| async move {
            shared.open_ordered(&target, &key, local, !automatic).await
        })
        .await
    }

    pub fn list(&self) -> Vec<ForwardInfo> {
        let mut list: Vec<_> = lock(&self.shared.state)
            .slots
            .iter()
            .filter_map(|(key, slot)| match slot {
                Slot::Open(forward) => Some(forward.info(key)),
                Slot::Opening => None,
            })
            .collect();
        list.sort_by(|a, b| (&a.target, a.remote).cmp(&(&b.target, b.remote)));
        list
    }

    pub fn preferences(&self) -> Preferences {
        let state = lock(&self.shared.state);
        let mut document = state.preferences.list();
        for row in &mut document.forwards {
            row.last_error = state
                .errors
                .get(&Key {
                    target: row.target.clone(),
                    remote: row.remote,
                })
                .cloned();
        }
        document
    }

    pub async fn save(
        &self,
        target: &str,
        port: i64,
        local: Option<i64>,
        label: Option<String>,
    ) -> Result<SavedForward, ForwardError> {
        let (_, key) = key(target, port)?;
        let local = local.map(check_port).transpose()?;
        if label
            .as_ref()
            .is_some_and(|label| label.chars().count() > MAX_LABEL)
        {
            return Err(ForwardError::Invalid(format!(
                "label must have at most {MAX_LABEL} characters"
            )));
        }
        let lane = self.shared.lane(&key);
        let _ordered = lane.lock().await;
        let mut state = lock(&self.shared.state);
        let active = state.slots.get(&key).and_then(|slot| match slot {
            Slot::Open(forward) => Some(forward.local),
            _ => None,
        });
        check_active(local, active)?;
        let prior = state.preferences.get(&key);
        let mut row = prior.clone().unwrap_or_else(|| SavedForward {
            target: key.target.clone(),
            remote: key.remote,
            local: active.unwrap_or(key.remote),
            label: String::new(),
            enabled: true,
            last_error: None,
        });
        if let Some(local) = local {
            row.local = local;
        }
        if let Some(label) = label {
            row.label = label;
        }
        state.preferences.put(row.clone())?;
        if prior.is_some_and(|prior| prior.local != row.local) {
            state.errors.remove(&key);
        }
        row.last_error = state.errors.get(&key).cloned();
        Ok(row)
    }

    pub async fn forget(&self, target: &str, port: i64) -> Result<(), ForwardError> {
        let (_, key) = key(target, port)?;
        let lane = self.shared.lane(&key);
        let _ordered = lane.lock().await;
        let mut state = lock(&self.shared.state);
        state.preferences.remove(&key)?;
        state.errors.remove(&key);
        Ok(())
    }

    pub async fn stop(&self, target: &str, port: i64) -> Result<(), ForwardError> {
        let (_, key) = key(target, port)?;
        self.ordered(
            key,
            |shared, key| async move { shared.stop_ordered(&key).await },
        )
        .await
    }

    /// Once admitted to a lane, an operation owns that lane through cleanup,
    /// even if the HTTP future is dropped. The hub still owns/cancels the task.
    async fn ordered<T, F, Fut>(&self, key: Key, action: F) -> Result<T, ForwardError>
    where
        T: Send + 'static,
        F: FnOnce(Arc<Shared>, Key) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T, ForwardError>> + Send + 'static,
    {
        let ordered = self.shared.lane(&key).lock_owned().await;
        let shared = self.shared.clone();
        let (answer, response) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _ordered = ordered;
            let result = action(shared, key).await;
            let _ = answer.send(result);
        });
        {
            let mut tasks = lock(&self.operations);
            tasks.retain(|task| !task.is_finished());
            tasks.push(task);
        }
        response.await.map_err(|_| ForwardError::OperationEnded)?
    }

    /// Called only after successfully sending remote.ready. Claim scheduled
    /// work now; each task rechecks current consent inside its key's lane.
    pub fn restore(&self, target: &str) {
        let keys: Vec<_> = {
            let mut state = lock(&self.shared.state);
            state
                .preferences
                .list()
                .forwards
                .into_iter()
                .filter_map(|row| {
                    let key = Key {
                        target: row.target,
                        remote: row.remote,
                    };
                    if key.target == target
                        && row.enabled
                        && !state.slots.contains_key(&key)
                        && state.restoring.insert(key.clone())
                    {
                        Some(key)
                    } else {
                        None
                    }
                })
                .collect()
        };
        let mut tasks = lock(&self.operations);
        tasks.retain(|task| !task.is_finished());
        for key in keys {
            let shared = self.shared.clone();
            let pending = Pending {
                shared: shared.clone(),
                key: key.clone(),
            };
            tasks.push(tokio::spawn(async move {
                let _pending = pending;
                let lane = shared.lane(&key);
                let _ordered = lane.lock().await;
                {
                    let state = lock(&shared.state);
                    if !state.preferences.get(&key).is_some_and(|row| row.enabled)
                        || state.slots.contains_key(&key)
                    {
                        return;
                    }
                }
                if let Ok(target) = SshTarget::parse(&key.target) {
                    let _ = shared.open_ordered(&target, &key, None, false).await;
                }
            }));
        }
    }
}

impl Drop for Forwards {
    fn drop(&mut self) {
        for task in lock(&self.operations).drain(..) {
            task.abort();
        }
    }
}

impl Shared {
    async fn stop_ordered(&self, key: &Key) -> Result<(), ForwardError> {
        let slot = {
            let mut state = lock(&self.state);
            if let Some(mut row) = state.preferences.get(key) {
                row.enabled = false;
                state.preferences.put(row)?;
            } else if !state.slots.contains_key(key) {
                return Err(ForwardError::NotFound {
                    host: key.target.clone(),
                    port: key.remote,
                });
            }
            state.errors.remove(key);
            state.slots.remove(key)
        };
        if let Some(Slot::Open(forward)) = slot {
            forward.stop().await;
        }
        Ok(())
    }

    fn lane(&self, key: &Key) -> Arc<AsyncMutex<()>> {
        let mut lanes = lock(&self.lanes);
        lanes.retain(|_, lane| lane.strong_count() > 0);
        if let Some(lane) = lanes.get(key).and_then(Weak::upgrade) {
            return lane;
        }
        let lane = Arc::new(AsyncMutex::new(()));
        lanes.insert(key.clone(), Arc::downgrade(&lane));
        lane
    }

    async fn open_ordered(
        self: &Arc<Self>,
        target: &SshTarget,
        key: &Key,
        local: Option<u16>,
        explicit: bool,
    ) -> Result<ForwardInfo, ForwardError> {
        let result = self.open_inner(target, key, local, explicit).await;
        let mut state = lock(&self.state);
        match &result {
            Ok(_) => {
                state.errors.remove(key);
            }
            Err(error)
                if state.preferences.get(key).is_some()
                    && !matches!(
                        error,
                        ForwardError::Paused | ForwardError::ActiveConflict { .. }
                    ) =>
            {
                state
                    .errors
                    .insert(key.clone(), error.saved_error(&key.target));
            }
            _ => {}
        }
        result
    }

    async fn open_inner(
        self: &Arc<Self>,
        target: &SshTarget,
        key: &Key,
        local: Option<u16>,
        explicit: bool,
    ) -> Result<ForwardInfo, ForwardError> {
        let saved = {
            let mut state = lock(&self.state);
            let saved = state.preferences.get(key);
            if !explicit && saved.as_ref().is_some_and(|row| !row.enabled) {
                return Err(ForwardError::Paused);
            }
            if let Some(Slot::Open(forward)) = state.slots.get(key) {
                check_active(local, Some(forward.local))?;
                let info = forward.info(key);
                if explicit {
                    if let Some(mut row) = saved {
                        row.enabled = true;
                        row.local = info.local;
                        state.preferences.put(row)?;
                    }
                }
                return Ok(info);
            }
            if state
                .slots
                .keys()
                .filter(|other| other.target == key.target)
                .count()
                >= MAX_PER_HOST
            {
                return Err(ForwardError::Limit {
                    host: key.target.clone(),
                });
            }
            state.slots.insert(key.clone(), Slot::Opening);
            saved
        };
        let _opening = Opening {
            shared: self.clone(),
            key: key.clone(),
        };
        let exact = local.or_else(|| saved.as_ref().map(|row| row.local));
        let forward = self.start(target, key.remote, exact).await?;
        let info = forward.info(key);
        let commit = {
            let mut state = lock(&self.state);
            if explicit {
                if let Some(mut row) = saved {
                    row.enabled = true;
                    row.local = info.local;
                    state.preferences.put(row)
                } else {
                    Ok(())
                }
            } else {
                Ok(())
            }
        };
        if let Err(error) = commit {
            forward.stop().await;
            return Err(error);
        }
        lock(&self.state)
            .slots
            .insert(key.clone(), Slot::Open(forward));
        Ok(info)
    }

    async fn start(
        &self,
        target: &SshTarget,
        remote: u16,
        exact: Option<u16>,
    ) -> Result<Supervised, ForwardError> {
        let reservation = match exact {
            Some(port) => tunnel::reserve_exact(port)
                .map_err(|source| ForwardError::LocalConflict { port, source })?,
            None => {
                tunnel::reserve(PortForward {
                    local: remote,
                    remote,
                })
                .map_err(|error| ForwardError::Failed(HubError::ForwardPort(error)))?
                .1
            }
        };
        let local = reservation.port();
        let mut forwarding = self
            .connector
            .forward(target, reservation, remote)
            .await
            .map_err(|error| match error {
                HubError::Forward(tunnel::PortTunnelError::LocalPortUnavailable {
                    local: port,
                    source,
                }) => ForwardError::LocalConflict { port, source },
                other => ForwardError::Failed(other),
            })?;
        // Connector setup may await after the reservation check. Never expose
        // a tunnel whose localhost address was taken by IPv6 during that wait.
        if let Err(source) = tunnel::check_forward_address(local) {
            forwarding.stop().await;
            return Err(ForwardError::LocalConflict {
                port: local,
                source,
            });
        }
        let up = Arc::new(AtomicBool::new(true));
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(supervise(
            self.connector.clone(),
            target.clone(),
            remote,
            local,
            forwarding,
            up.clone(),
            stopped,
        ));
        Ok(Supervised {
            local,
            up,
            stop: Some(stop),
            task,
        })
    }
}

fn check_port(port: i64) -> Result<u16, ForwardError> {
    u16::try_from(port)
        .ok()
        .filter(|port| *port != 0)
        .ok_or(ForwardError::Port(port))
}

fn key(target: &str, port: i64) -> Result<(SshTarget, Key), ForwardError> {
    let target = SshTarget::parse(target)?;
    let key = Key {
        target: target.as_str().to_owned(),
        remote: check_port(port)?,
    };
    Ok((target, key))
}

fn check_active(local: Option<u16>, active: Option<u16>) -> Result<(), ForwardError> {
    if let (Some(asked), Some(local)) = (local, active) {
        if asked != local {
            return Err(ForwardError::ActiveConflict { local });
        }
    }
    Ok(())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct Opening {
    shared: Arc<Shared>,
    key: Key,
}
impl Drop for Opening {
    fn drop(&mut self) {
        let mut state = lock(&self.shared.state);
        if matches!(state.slots.get(&self.key), Some(Slot::Opening)) {
            state.slots.remove(&self.key);
        }
    }
}

struct Pending {
    shared: Arc<Shared>,
    key: Key,
}
impl Drop for Pending {
    fn drop(&mut self) {
        lock(&self.shared.state).restoring.remove(&self.key);
    }
}

/// An open forward and the task that keeps it. Dropping it ends both.
struct Supervised {
    local: u16,
    up: Arc<AtomicBool>,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl Supervised {
    fn info(&self, key: &Key) -> ForwardInfo {
        ForwardInfo {
            target: key.target.clone(),
            remote: key.remote,
            local: self.local,
            url: format!("http://localhost:{}", self.local),
            state: if self.up.load(Ordering::SeqCst) {
                ForwardState::Up
            } else {
                ForwardState::Down
            },
        }
    }

    /// End the forward and wait until its `ssh` has ended.
    async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        // Aborting a task only schedules child-process cleanup; it does not
        // prove the port has been released. Keep the operation's lane until
        // the supervisor has finished closing (including an in-flight rebuild).
        let _ = (&mut self.task).await;
    }
}

impl Drop for Supervised {
    fn drop(&mut self) {
        // Drops the forward's `ssh`, which kills it.
        self.task.abort();
    }
}

/// Keep a forward of `remote` on `target` at local port `local` until
/// `stop` resolves, rebuilding it on that port whenever it ends.
async fn supervise(
    connector: Arc<dyn Connector>,
    target: SshTarget,
    remote: u16,
    local: u16,
    mut forwarding: Box<dyn Forwarding>,
    up: Arc<AtomicBool>,
    mut stop: oneshot::Receiver<()>,
) {
    let host = target.as_str();
    loop {
        let reason = tokio::select! {
            _ = &mut stop => {
                forwarding.stop().await;
                return;
            }
            reason = forwarding.ended() => reason,
        };
        // Take the port back before another process can (SR-13).
        let mut held = PortReservation::acquire_port(local).ok();
        up.store(false, Ordering::SeqCst);
        tracing::info!(
            "remote screen hub: the forward of port {remote} of {host} at local port {local} \
             ended ({reason}); rebuilding it"
        );

        let mut failures = 0;
        forwarding = loop {
            let result = {
                let attempt = async {
                    let reservation = match held.take() {
                        Some(reservation) => reservation,
                        None => {
                            PortReservation::acquire_port(local).map_err(HubError::ForwardPort)?
                        }
                    };
                    reservation
                        .check_forward_address()
                        .map_err(HubError::ForwardPort)?;
                    let mut forwarding = connector.forward(&target, reservation, remote).await?;
                    if let Err(error) = tunnel::check_forward_address(local) {
                        forwarding.stop().await;
                        return Err(HubError::ForwardPort(error));
                    }
                    Ok(forwarding)
                };
                tokio::pin!(attempt);
                tokio::select! {
                    _ = &mut stop => {
                        // Do not cancel SSH setup and merely drop its child: the
                        // bounded attempt must finish, then close and reap it,
                        // before an acknowledged Stop releases this key's lane.
                        if let Ok(mut forwarding) = attempt.await {
                            forwarding.stop().await;
                        }
                        return;
                    }
                    result = &mut attempt => result,
                }
            };
            match result {
                Ok(forwarding) => break forwarding,
                Err(error) => {
                    held = PortReservation::acquire_port(local).ok();
                    let wait = backoff(failures);
                    failures += 1;
                    tracing::info!(
                        "remote screen hub: the forward of port {remote} of {host} could not be \
                         rebuilt ({}); retrying in {} s",
                        error.failure(host).message,
                        wait.as_secs()
                    );
                    tokio::select! {
                        _ = &mut stop => return,
                        () = tokio::time::sleep(wait) => {}
                    }
                }
            }
        };
        up.store(true, Ordering::SeqCst);
        tracing::info!(
            "remote screen hub: the forward of port {remote} of {host} is back at local port \
             {local}"
        );
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::hub::testing::ScriptedHost;
    use futures::poll;
    use std::net::TcpListener;
    use std::time::Duration;

    fn ipv6_listener(port: u16) -> Option<TcpListener> {
        match TcpListener::bind(("::1", port)) {
            Ok(listener) => Some(listener),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::AddrNotAvailable | std::io::ErrorKind::Unsupported
                ) || cfg!(target_os = "linux") && error.raw_os_error() == Some(97)
                    || cfg!(target_os = "macos") && error.raw_os_error() == Some(47) =>
            {
                eprintln!("skipping unavailable IPv6 loopback: {error}");
                None
            }
            Err(error) => panic!("IPv6 probe failed for a reason other than absence: {error}"),
        }
    }

    fn free_port() -> u16 {
        tunnel::reserve(PortForward {
            local: 0,
            remote: 5173,
        })
        .expect("free localhost")
        .0
        .local
    }

    async fn settle(manager: &Forwards) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !lock(&manager.shared.state).restoring.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("restores settle");
    }

    fn forwards_manager(host: &ScriptedHost, dir: &std::path::Path) -> Forwards {
        Forwards::new(Arc::new(host.clone()), dir.join("config"))
    }

    /// A slow reap must not turn the former five-second deadline into a
    /// successful Stop. Virtual time makes cancellation/ordering deterministic.
    #[tokio::test(start_paused = true)]
    async fn cancelled_stop_waits_past_old_deadline_for_supervisor_completion() {
        let dir = tempfile::tempdir().expect("dir");
        let host = ScriptedHost::new(dir.path());
        let manager = forwards_manager(&host, dir.path());
        manager.save("vm1", 5173, None, None).await.expect("save");
        let (_, key) = key("vm1", 5173).expect("key");
        let (stop, stopped) = oneshot::channel();
        let (entered, cleanup_started) = oneshot::channel();
        let (release, cleanup) = oneshot::channel();
        let released = Arc::new(AtomicBool::new(false));
        let observed = released.clone();
        let task = tokio::spawn(async move {
            let _ = stopped.await;
            let _ = entered.send(());
            let _ = cleanup.await;
            released.store(true, Ordering::SeqCst);
        });
        lock(&manager.shared.state).slots.insert(
            key,
            Slot::Open(Supervised {
                local: 5173,
                up: Arc::new(AtomicBool::new(true)),
                stop: Some(stop),
                task,
            }),
        );
        let mut first = Box::pin(manager.stop("vm1", 5173));
        assert!(poll!(first.as_mut()).is_pending());
        cleanup_started.await.expect("supervisor closing");
        drop(first);
        let mut second = Box::pin(manager.stop("vm1", 5173));
        assert!(poll!(second.as_mut()).is_pending());
        tokio::time::advance(Duration::from_secs(6)).await;
        assert!(
            poll!(second.as_mut()).is_pending(),
            "no deadline releases lane early"
        );
        assert!(!observed.load(Ordering::SeqCst));
        release.send(()).expect("supervisor still owned");
        second.await.expect("ordered Stop");
        assert!(observed.load(Ordering::SeqCst));
        assert!(!manager.preferences().forwards[0].enabled);
    }

    #[tokio::test]
    async fn queued_restore_rereads_stop_and_forget_consent() {
        for forget in [false, true] {
            let dir = tempfile::tempdir().expect("dir");
            let host = ScriptedHost::new(dir.path());
            let manager = forwards_manager(&host, dir.path());
            manager
                .save("vm1", 5173, Some(free_port().into()), None)
                .await
                .expect("save");
            let (_, key) = key("vm1", 5173).expect("key");
            let lane = manager.shared.lane(&key);
            let holding = lane.lock().await;
            let action = async {
                if forget {
                    manager.forget("vm1", 5173).await
                } else {
                    manager.stop("vm1", 5173).await
                }
            };
            tokio::pin!(action);
            assert!(poll!(&mut action).is_pending(), "mutation queued first");
            manager.restore("vm1");
            tokio::task::yield_now().await;
            drop(holding);
            action.await.expect("mutation");
            settle(&manager).await;
            assert!(host.asked().is_empty());
            assert!(manager.list().is_empty());
            if forget {
                assert!(manager.preferences().forwards.is_empty());
            } else {
                assert!(!manager.preferences().forwards[0].enabled);
            }
        }
    }

    #[tokio::test]
    async fn mutation_during_restore_is_ordered_and_other_keys_progress() {
        for forget in [false, true] {
            let dir = tempfile::tempdir().expect("dir");
            let host = ScriptedHost::new(dir.path());
            let manager = forwards_manager(&host, dir.path());
            let local = free_port();
            manager
                .save("vm1", 5173, Some(local.into()), None)
                .await
                .expect("save");
            let gate = host.pause_forward(5173);
            manager.restore("vm1");
            gate.entered().await;
            manager.restore("vm1");
            let action = async {
                if forget {
                    manager.forget("vm1", 5173).await
                } else {
                    manager.stop("vm1", 5173).await
                }
            };
            tokio::pin!(action);
            assert!(poll!(&mut action).is_pending());
            tokio::time::timeout(
                Duration::from_secs(2),
                manager.open("vm2", 8000, None, false),
            )
            .await
            .expect("independent key proceeds")
            .expect("open");
            gate.release();
            action.await.expect("mutation");
            settle(&manager).await;
            assert_eq!(
                host.asked()
                    .iter()
                    .filter(|call| call.starts_with("forward vm1 5173 "))
                    .count(),
                1,
                "two readiness events share one attempt"
            );
            if forget {
                assert!(manager.preferences().forwards.is_empty());
                assert!(manager.list().iter().any(|row| row.remote == 5173));
                manager
                    .stop("vm1", 5173)
                    .await
                    .expect("stop forgotten live");
            } else {
                assert!(!manager.preferences().forwards[0].enabled);
                assert!(!manager.list().iter().any(|row| row.remote == 5173));
            }
            assert!(TcpListener::bind(("127.0.0.1", local)).is_ok());
        }
    }

    #[tokio::test]
    async fn failed_open_commit_finishes_cleanup_before_later_forget() {
        let dir = tempfile::tempdir().expect("dir");
        let host = ScriptedHost::new(dir.path());
        let manager = forwards_manager(&host, dir.path());
        let local = free_port();
        manager
            .save("vm1", 5173, Some(local.into()), None)
            .await
            .expect("save");
        manager.stop("vm1", 5173).await.expect("pause");
        let gate = host.pause_forward(5173);
        let opening = manager.open("vm1", 5173, None, false);
        tokio::pin!(opening);
        assert!(poll!(&mut opening).is_pending());
        gate.entered().await;
        let config = dir.path().join("config");
        let backup = dir.path().join("backup");
        std::fs::rename(&config, &backup).expect("move");
        std::fs::write(&config, b"blocked").expect("block");
        let forgetting = manager.forget("vm1", 5173);
        tokio::pin!(forgetting);
        assert!(
            poll!(&mut forgetting).is_pending(),
            "queued behind opening and rollback"
        );
        gate.release();
        assert!(matches!(opening.await, Err(ForwardError::Storage(_))));
        assert!(manager.list().is_empty());
        assert!(
            TcpListener::bind(("127.0.0.1", local)).is_ok(),
            "cleanup precedes lane release"
        );
        std::fs::remove_file(&config).expect("unblock");
        std::fs::rename(&backup, &config).expect("restore");
        forgetting.await.expect("forget");
        settle(&manager).await;
        assert!(manager.preferences().forwards.is_empty());
        assert!(Store::new(config).list().forwards.is_empty());
    }

    #[tokio::test]
    async fn duplicate_opens_wait_for_one_verified_attempt() {
        let dir = tempfile::tempdir().expect("dir");
        let host = ScriptedHost::new(dir.path());
        let manager = forwards_manager(&host, dir.path());
        let gate = host.pause_forward(5173);
        let first = manager.open("vm1", 5173, None, false);
        tokio::pin!(first);
        assert!(poll!(&mut first).is_pending());
        gate.entered().await;
        let second = manager.open("vm1", 5173, None, false);
        tokio::pin!(second);
        assert!(poll!(&mut second).is_pending());
        gate.release();
        let (first, second) = tokio::join!(first, second);
        assert_eq!(first.expect("first"), second.expect("second"));
        assert_eq!(host.asked().len(), 1);
    }

    #[tokio::test]
    async fn opening_slots_enforce_capacity_across_independent_lanes() {
        let dir = tempfile::tempdir().expect("dir");
        let host = ScriptedHost::new(dir.path());
        let manager = forwards_manager(&host, dir.path());
        let mut pending = Vec::new();
        let mut gates = Vec::new();
        for remote in 10000..10032 {
            let gate = host.pause_forward(remote);
            let mut opening = Box::pin(manager.open("vm1", remote.into(), None, false));
            assert!(poll!(opening.as_mut()).is_pending());
            gate.entered().await;
            gates.push(gate);
            pending.push(opening);
        }
        assert!(matches!(
            manager.open("vm1", 11000, None, false).await,
            Err(ForwardError::Limit { .. })
        ));
        manager
            .open("vm2", 11000, None, false)
            .await
            .expect("independent host");
        assert!(manager.list().iter().all(|row| row.target == "vm2"));
        drop(pending);
        for gate in gates {
            gate.release();
        }
        for remote in 10000..10032 {
            manager
                .stop("vm1", remote)
                .await
                .expect("cleanup abandoned caller");
        }
        assert_eq!(
            lock(&manager.shared.state).slots.len(),
            1,
            "ordered cleanup releases capacity"
        );
    }

    #[tokio::test]
    async fn dropping_forward_owner_cancels_initial_restore_and_frees_reservation() {
        let dir = tempfile::tempdir().expect("dir");
        let host = ScriptedHost::new(dir.path());
        let manager = forwards_manager(&host, dir.path());
        let local = free_port();
        manager
            .save("vm1", 5173, Some(local.into()), None)
            .await
            .expect("save");
        let gate = host.pause_forward(5173);
        manager.restore("vm1");
        gate.entered().await;
        let weak = Arc::downgrade(&manager.shared);
        assert!(TcpListener::bind(("127.0.0.1", local)).is_err());
        drop(manager);
        tokio::time::timeout(Duration::from_secs(2), async {
            while weak.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all restore ownership released");
        assert!(TcpListener::bind(("127.0.0.1", local)).is_ok());
        gate.release();
        tokio::task::yield_now().await;
        assert_eq!(host.asked().len(), 1, "cancelled task cannot resume SSH");
        let fresh = forwards_manager(&host, dir.path());
        assert_eq!(fresh.preferences().forwards.len(), 1);
        assert!(fresh.list().is_empty());
        assert_eq!(host.asked().len(), 1, "reconstruction does not dial");
    }

    #[tokio::test]
    async fn restore_failures_are_runtime_only_and_retry_only_on_readiness() {
        let dir = tempfile::tempdir().expect("dir");
        let host = ScriptedHost::new(dir.path()).forwards_failing("Host key verification failed.");
        let manager = forwards_manager(&host, dir.path());
        manager
            .save("vm1", 5173, Some(free_port().into()), None)
            .await
            .expect("save");
        manager.restore("vm1");
        settle(&manager).await;
        assert_eq!(
            manager.preferences().forwards[0]
                .last_error
                .as_ref()
                .expect("error")
                .code,
            "host_key"
        );
        assert!(manager.list().is_empty());
        for _ in 0..5 {
            manager.preferences();
            manager.list();
        }
        assert_eq!(host.asked().len(), 1);
        manager.restore("vm1");
        settle(&manager).await;
        assert_eq!(host.asked().len(), 2);
        let disk = Store::new(dir.path().join("config")).list();
        assert!(disk.forwards[0].last_error.is_none());
        manager.stop("vm1", 5173).await.expect("stop clears error");
        assert!(manager.preferences().forwards[0].last_error.is_none());
    }

    #[tokio::test]
    async fn ipv6_occupation_blocks_saved_open_and_supervisor_rebuild_without_moving() {
        let dir = tempfile::tempdir().expect("dir");
        let host = ScriptedHost::new(dir.path());
        let manager = forwards_manager(&host, dir.path());
        let Some(busy6) = ipv6_listener(0) else {
            return;
        };
        let local = busy6.local_addr().expect("addr").port();
        manager
            .save("vm1", 5173, Some(local.into()), None)
            .await
            .expect("save");
        manager.restore("vm1");
        settle(&manager).await;
        assert_eq!(
            manager.preferences().forwards[0]
                .last_error
                .as_ref()
                .expect("error")
                .code,
            "local_port_conflict"
        );
        assert!(host.asked().is_empty());
        drop(busy6);
        manager
            .open("vm1", 5173, None, false)
            .await
            .expect("retry exact");
        let busy6 = TcpListener::bind(("::1", local)).expect("occupy after establishment");
        host.end_forward(5173);
        tokio::time::timeout(Duration::from_secs(2), async {
            while manager.list()[0].state != ForwardState::Down {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("down");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(host.asked().len(), 1, "IPv6 prevents another SSH attempt");
        assert_eq!(manager.list()[0].local, local);
        drop(busy6);
        tokio::time::timeout(Duration::from_secs(3), async {
            while manager.list()[0].state != ForwardState::Up {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("same-port recovery");
        assert_eq!(manager.list()[0].local, local);
    }
    #[tokio::test]
    async fn cancelled_stop_keeps_cleanup_ordered_before_an_acknowledged_second_stop() {
        let dir = tempfile::tempdir().expect("dir");
        let host = ScriptedHost::new(dir.path());
        let manager = forwards_manager(&host, dir.path());
        let local = free_port();
        manager
            .save("vm1", 5173, Some(local.into()), None)
            .await
            .expect("save");
        manager.open("vm1", 5173, None, false).await.expect("open");
        let gate = host.pause_stop(5173);
        let mut stopping = Box::pin(manager.stop("vm1", 5173));
        assert!(poll!(stopping.as_mut()).is_pending());
        gate.entered().await;
        drop(stopping);
        let mut second = Box::pin(manager.stop("vm1", 5173));
        assert!(poll!(second.as_mut()).is_pending());
        assert!(TcpListener::bind(("127.0.0.1", local)).is_err());
        gate.release();
        second.await.expect("second stop");
        assert!(
            TcpListener::bind(("127.0.0.1", local)).is_ok(),
            "acknowledged Stop waits for cancelled caller cleanup"
        );
        assert!(!manager.preferences().forwards[0].enabled);
        manager
            .open("vm1", 5173, None, false)
            .await
            .expect("resume after cancelled stop");
        assert_eq!(manager.list()[0].local, local);
    }
    #[tokio::test]
    async fn ipv6_race_during_setup_never_announces_up_or_commits_alternate() {
        for rebuilding in [false, true] {
            let dir = tempfile::tempdir().expect("dir");
            let host = ScriptedHost::new(dir.path());
            let manager = forwards_manager(&host, dir.path());
            let original = free_port();
            let alternate = free_port();
            manager
                .save("vm1", 5173, Some(original.into()), None)
                .await
                .expect("save");
            if rebuilding {
                manager.open("vm1", 5173, None, false).await.expect("open");
            }
            let gate = host.pause_forward(5173);
            let local = if rebuilding { original } else { alternate };
            let mut opening = Box::pin(manager.open("vm1", 5173, Some(alternate.into()), false));
            if rebuilding {
                host.end_forward(5173);
            } else {
                assert!(poll!(opening.as_mut()).is_pending());
            }
            gate.entered().await;
            let Some(busy6) = ipv6_listener(local) else {
                gate.release();
                return;
            };
            let cleanup = rebuilding.then(|| host.pause_stop(5173));
            gate.release();
            if let Some(cleanup) = cleanup {
                cleanup.entered().await;
                assert_eq!(manager.list()[0].state, ForwardState::Down);
                assert_eq!(manager.list()[0].local, original);
                cleanup.release();
                manager.stop("vm1", 5173).await.expect("finish cleanup");
                assert!(TcpListener::bind(("127.0.0.1", original)).is_ok());
            } else {
                assert!(
                    matches!(opening.await, Err(ForwardError::LocalConflict { port, .. }) if port == alternate)
                );
                assert!(manager.list().is_empty());
                assert!(
                    TcpListener::bind(("127.0.0.1", alternate)).is_ok(),
                    "failed tunnel closed before response"
                );
            }
            assert_eq!(manager.preferences().forwards[0].local, original);
            drop(busy6);
        }
    }

    #[tokio::test]
    async fn stop_during_rebuild_waits_for_setup_and_port_release() {
        let dir = tempfile::tempdir().expect("dir");
        let host = ScriptedHost::new(dir.path());
        let manager = forwards_manager(&host, dir.path());
        let local = free_port();
        manager
            .save("vm1", 5173, Some(local.into()), None)
            .await
            .expect("save");
        manager.open("vm1", 5173, None, false).await.expect("open");
        let opening = host.pause_forward(5173);
        host.end_forward(5173);
        opening.entered().await;
        let cleanup = host.pause_stop(5173);
        let mut stopping = Box::pin(manager.stop("vm1", 5173));
        assert!(poll!(stopping.as_mut()).is_pending());
        tokio::task::yield_now().await;
        assert!(poll!(stopping.as_mut()).is_pending());
        opening.release();
        cleanup.entered().await;
        assert!(poll!(stopping.as_mut()).is_pending());
        assert!(TcpListener::bind(("127.0.0.1", local)).is_err());
        cleanup.release();
        stopping.await.expect("fully stopped");
        assert!(TcpListener::bind(("127.0.0.1", local)).is_ok());
        assert!(!manager.preferences().forwards[0].enabled);
        assert!(manager.list().is_empty());
    }

    #[tokio::test]
    async fn cancelled_failed_write_rollback_holds_lane_until_cleanup_finishes() {
        let dir = tempfile::tempdir().expect("dir");
        let host = ScriptedHost::new(dir.path());
        let manager = forwards_manager(&host, dir.path());
        let local = free_port();
        manager
            .save("vm1", 5173, Some(local.into()), None)
            .await
            .expect("save");
        manager.stop("vm1", 5173).await.expect("pause");
        let config = dir.path().join("config");
        let backup = dir.path().join("backup");
        std::fs::rename(&config, &backup).expect("move");
        std::fs::write(&config, b"blocked").expect("block");
        let gate = host.pause_stop(5173);
        let mut opening = Box::pin(manager.open("vm1", 5173, None, false));
        assert!(poll!(opening.as_mut()).is_pending());
        gate.entered().await;
        drop(opening);
        std::fs::remove_file(&config).expect("unblock");
        std::fs::rename(&backup, &config).expect("restore");
        let mut forgetting = Box::pin(manager.forget("vm1", 5173));
        assert!(
            poll!(forgetting.as_mut()).is_pending(),
            "cleanup still owns lane after caller leaves"
        );
        gate.release();
        forgetting.await.expect("forget after cleanup");
        assert!(manager.preferences().forwards.is_empty());
        assert!(manager.list().is_empty());
        assert!(TcpListener::bind(("127.0.0.1", local)).is_ok());
    }
}
