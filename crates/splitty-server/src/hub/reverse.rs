//! Reverse forwards (ports of this machine shared with hosts) and their
//! opt-in rules, ordered per key as forwards are (`hub::forwards`). Only an
//! explicit POST and a host's successful readiness for an enabled rule open
//! one; nothing a host sends does. Short map and storage locks never span an
//! SSH wait.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use serde::{Deserialize, Serialize};
use splitty_remote::hosts;
use splitty_remote::ssh::SshTarget;
use splitty_remote::tunnel::{backoff, ReverseTunnelError};
use tokio::sync::{oneshot, Mutex as AsyncMutex};
use tokio::task::JoinHandle;

use super::forwards::{ForwardError, ForwardState, MAX_PER_HOST};
use super::preferences::{SavedError, MAX_TOTAL};
use super::{Connector, Forwarding, HubError};

/// Where the rules are kept, in the configuration directory. A file of its
/// own: `remote-forwards.json` is read with unknown fields refused.
pub const FILE: &str = "remote-reverse-forwards.json";

/// An active reverse forward, for `GET /api/remote/reverse-forwards`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReverseInfo {
    pub target: String,
    /// The host's port.
    pub remote: u16,
    /// The port here.
    pub local: u16,
    pub state: ForwardState,
}

/// A remembered reverse forward.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedReverse {
    pub target: String,
    pub remote: u16,
    pub local: u16,
    pub enabled: bool,
    /// Only the API attaches errors. They are never accepted from disk.
    #[serde(
        rename = "lastError",
        skip_deserializing,
        skip_serializing_if = "Option::is_none"
    )]
    pub last_error: Option<SavedError>,
}

/// The rules, for `GET /api/remote/reverse-forward-preferences` and the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReversePreferences {
    pub version: u32,
    pub forwards: Vec<SavedReverse>,
}

impl Default for ReversePreferences {
    fn default() -> Self {
        Self {
            version: 1,
            forwards: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    target: String,
    remote: u16,
}

impl Key {
    fn matches(&self, row: &SavedReverse) -> bool {
        self.target == row.target && self.remote == row.remote
    }
}

enum Slot {
    Opening,
    Open(Supervised),
}

struct State {
    slots: HashMap<Key, Slot>,
    store: Store,
    errors: HashMap<Key, SavedError>,
    restoring: HashSet<Key>,
}

struct Shared {
    connector: Arc<dyn Connector>,
    state: Mutex<State>,
    lanes: Mutex<HashMap<Key, Weak<AsyncMutex<()>>>>,
    /// The local server's own port, never shared; 0 while unknown.
    reserved: AtomicU16,
}

pub struct Reverses {
    shared: Arc<Shared>,
    // Tasks own Shared, not Reverses: hub teardown cancels them all.
    operations: Mutex<Vec<JoinHandle<()>>>,
}

impl Reverses {
    pub fn new(connector: Arc<dyn Connector>, config_dir: PathBuf) -> Self {
        Self {
            shared: Arc::new(Shared {
                connector,
                state: Mutex::new(State {
                    slots: HashMap::new(),
                    store: Store::new(config_dir),
                    errors: HashMap::new(),
                    restoring: HashSet::new(),
                }),
                lanes: Mutex::new(HashMap::new()),
                reserved: AtomicU16::new(0),
            }),
            operations: Mutex::new(Vec::new()),
        }
    }

    /// The local server listens on `port`, which is never shared.
    pub fn reserve(&self, port: u16) {
        self.shared.reserved.store(port, Ordering::SeqCst);
    }

    /// Share `local` here as `port` of `target`, on the user's request.
    pub async fn open(
        &self,
        target: &str,
        port: i64,
        local: i64,
    ) -> Result<ReverseInfo, ForwardError> {
        let (target, key) = key(target, port)?;
        let local = check_port(local)?;
        self.shared.check_reserved(local)?;
        self.ordered(key, move |shared, key| async move {
            shared.open_ordered(&target, &key, Some(local), true).await
        })
        .await
    }

    pub fn list(&self) -> Vec<ReverseInfo> {
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

    pub fn preferences(&self) -> ReversePreferences {
        let state = lock(&self.shared.state);
        let mut document = state.store.list();
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

    /// Remember a rule, or change its local port. Never opens anything.
    pub async fn save(
        &self,
        target: &str,
        port: i64,
        local: Option<i64>,
    ) -> Result<SavedReverse, ForwardError> {
        let (_, key) = key(target, port)?;
        let local = local.map(check_port).transpose()?;
        if let Some(local) = local {
            self.shared.check_reserved(local)?;
        }
        let lane = self.shared.lane(&key);
        let _ordered = lane.lock().await;
        let mut state = lock(&self.shared.state);
        let active = state.slots.get(&key).and_then(|slot| match slot {
            Slot::Open(forward) => Some(forward.local),
            Slot::Opening => None,
        });
        check_active(local, active)?;
        let prior = state.store.get(&key);
        let mut row = prior.clone().unwrap_or_else(|| SavedReverse {
            target: key.target.clone(),
            remote: key.remote,
            local: active.unwrap_or(key.remote),
            enabled: true,
            last_error: None,
        });
        if let Some(local) = local {
            row.local = local;
        }
        self.shared.check_reserved(row.local)?;
        state.store.put(row.clone())?;
        if prior.is_some_and(|prior| prior.local != row.local) {
            state.errors.remove(&key);
        }
        row.last_error = state.errors.get(&key).cloned();
        Ok(row)
    }

    /// Remove a rule; an active reverse forward keeps running.
    pub async fn forget(&self, target: &str, port: i64) -> Result<(), ForwardError> {
        let (_, key) = key(target, port)?;
        let lane = self.shared.lane(&key);
        let _ordered = lane.lock().await;
        let mut state = lock(&self.shared.state);
        state.store.remove(&key)?;
        state.errors.remove(&key);
        Ok(())
    }

    /// End a reverse forward and pause its rule.
    pub async fn stop(&self, target: &str, port: i64) -> Result<(), ForwardError> {
        let (_, key) = key(target, port)?;
        self.ordered(
            key,
            |shared, key| async move { shared.stop_ordered(&key).await },
        )
        .await
    }

    /// Once admitted to a lane, an operation owns it through cleanup, even
    /// if the HTTP future is dropped.
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
            let _ = answer.send(action(shared, key).await);
        });
        {
            let mut tasks = lock(&self.operations);
            tasks.retain(|task| !task.is_finished());
            tasks.push(task);
        }
        response.await.map_err(|_| ForwardError::OperationEnded)?
    }

    /// Called only after `remote.ready` went out for `target`: open its
    /// enabled rules that are not active. Each task rechecks the rule in its
    /// lane.
    pub fn restore(&self, target: &str) {
        let keys: Vec<_> = {
            let mut state = lock(&self.shared.state);
            state
                .store
                .list()
                .forwards
                .into_iter()
                .filter_map(|row| {
                    let key = Key {
                        target: row.target,
                        remote: row.remote,
                    };
                    (key.target == target
                        && row.enabled
                        && !state.slots.contains_key(&key)
                        && state.restoring.insert(key.clone()))
                    .then_some(key)
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
                let local = {
                    let state = lock(&shared.state);
                    match state.store.get(&key) {
                        Some(row) if row.enabled && !state.slots.contains_key(&key) => row.local,
                        _ => return,
                    }
                };
                if let Ok(target) = SshTarget::parse(&key.target) {
                    let _ = shared.open_ordered(&target, &key, Some(local), false).await;
                }
            }));
        }
    }
}

impl Drop for Reverses {
    fn drop(&mut self) {
        for task in lock(&self.operations).drain(..) {
            task.abort();
        }
    }
}

impl Shared {
    fn check_reserved(&self, local: u16) -> Result<(), ForwardError> {
        if self.reserved.load(Ordering::SeqCst) == local {
            return Err(ForwardError::LocalPortReserved { port: local });
        }
        Ok(())
    }

    async fn stop_ordered(&self, key: &Key) -> Result<(), ForwardError> {
        let slot = {
            let mut state = lock(&self.state);
            if let Some(mut row) = state.store.get(key) {
                row.enabled = false;
                state.store.put(row)?;
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

    /// Open, recording a failure of a saved rule as its `lastError`.
    async fn open_ordered(
        self: &Arc<Self>,
        target: &SshTarget,
        key: &Key,
        local: Option<u16>,
        explicit: bool,
    ) -> Result<ReverseInfo, ForwardError> {
        let result = self.open_inner(target, key, local, explicit).await;
        let mut state = lock(&self.state);
        match &result {
            Ok(_) => {
                state.errors.remove(key);
            }
            Err(error)
                if state.store.get(key).is_some()
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
    ) -> Result<ReverseInfo, ForwardError> {
        let (saved, local) = {
            let mut state = lock(&self.state);
            let saved = state.store.get(key);
            if !explicit && saved.as_ref().is_some_and(|row| !row.enabled) {
                return Err(ForwardError::Paused);
            }
            if let Some(Slot::Open(forward)) = state.slots.get(key) {
                check_active(local, Some(forward.local))?;
                let info = forward.info(key);
                if explicit {
                    if let Some(mut row) = saved {
                        row.enabled = true;
                        state.store.put(row)?;
                    }
                }
                return Ok(info);
            }
            let local = local
                .or_else(|| saved.as_ref().map(|row| row.local))
                .ok_or_else(|| ForwardError::Invalid("a local port is required".into()))?;
            self.check_reserved(local)?;
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
            (saved, local)
        };
        let _opening = Opening {
            shared: self.clone(),
            key: key.clone(),
        };
        let forward = self.start(target, key.remote, local).await?;
        let info = forward.info(key);
        if explicit {
            if let Some(mut row) = saved {
                row.enabled = true;
                row.local = local;
                let commit = lock(&self.state).store.put(row);
                if let Err(error) = commit {
                    forward.stop().await;
                    return Err(error);
                }
            }
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
        local: u16,
    ) -> Result<Supervised, ForwardError> {
        let forwarding = self
            .connector
            .reverse(target, local, remote)
            .await
            .map_err(|error| reverse_error(target.as_str(), error))?;
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

/// A port in use on the host is the caller's conflict; the rest are the
/// hub's failures.
fn reverse_error(host: &str, error: HubError) -> ForwardError {
    match error {
        HubError::Reverse(ReverseTunnelError::HostPortConflict { port, detail }) => {
            // The relay's own words name the address; the caller's message
            // says what to do.
            tracing::info!("remote screen hub: port {port} is in use on {host:?}: {detail}");
            ForwardError::HostPortConflict {
                host: host.to_owned(),
                port,
            }
        }
        other => ForwardError::Failed(other),
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

/// An open reverse forward and the task that keeps it. Dropping it ends
/// both.
struct Supervised {
    local: u16,
    up: Arc<AtomicBool>,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl Supervised {
    fn info(&self, key: &Key) -> ReverseInfo {
        ReverseInfo {
            target: key.target.clone(),
            remote: key.remote,
            local: self.local,
            state: if self.up.load(Ordering::SeqCst) {
                ForwardState::Up
            } else {
                ForwardState::Down
            },
        }
    }

    /// End the reverse forward and wait until its `ssh` has ended.
    async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let _ = (&mut self.task).await;
    }
}

impl Drop for Supervised {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Keep `local` shared as `remote` of `target` until `stop` resolves,
/// rebuilding it whenever it ends. A rebuild after a lost connection may
/// find the host's port still held by the old relay until its heartbeat
/// runs out; it waits like any failed attempt.
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
        up.store(false, Ordering::SeqCst);
        tracing::info!(
            "remote screen hub: local port {local}, shared as port {remote} of {host}, ended \
             ({reason}); rebuilding it"
        );
        let mut failures = 0;
        forwarding = loop {
            let attempt = connector.reverse(&target, local, remote);
            tokio::pin!(attempt);
            let result = tokio::select! {
                _ = &mut stop => {
                    // Let the bounded attempt finish and close it, so an
                    // acknowledged Stop leaves no relay behind.
                    if let Ok(mut forwarding) = attempt.await {
                        forwarding.stop().await;
                    }
                    return;
                }
                result = &mut attempt => result,
            };
            match result {
                Ok(forwarding) => break forwarding,
                Err(error) => {
                    let wait = backoff(failures);
                    failures += 1;
                    tracing::info!(
                        "remote screen hub: local port {local} could not be shared as port \
                         {remote} of {host} again ({}); retrying in {} s",
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
            "remote screen hub: local port {local} is shared as port {remote} of {host} again"
        );
    }
}

/// The rules on disk. Reading never repairs a file or dials a host.
struct Store {
    path: PathBuf,
    document: ReversePreferences,
}

impl Store {
    fn new(path: PathBuf) -> Self {
        let document = match fs::read(path.join(FILE)) {
            Ok(bytes) => match serde_json::from_slice::<ReversePreferences>(&bytes) {
                Ok(mut document) if valid(&document) => {
                    document
                        .forwards
                        .sort_by(|a, b| (&a.target, a.remote).cmp(&(&b.target, b.remote)));
                    document
                }
                _ => {
                    tracing::warn!("invalid {FILE}; using no saved shared ports without rewriting");
                    ReversePreferences::default()
                }
            },
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("cannot read {FILE}: {error}");
                }
                ReversePreferences::default()
            }
        };
        Self { path, document }
    }

    fn list(&self) -> ReversePreferences {
        self.document.clone()
    }

    fn get(&self, key: &Key) -> Option<SavedReverse> {
        self.document
            .forwards
            .iter()
            .find(|row| key.matches(row))
            .cloned()
    }

    fn put(&mut self, mut row: SavedReverse) -> Result<(), ForwardError> {
        row.last_error = None;
        let mut next = self.document.clone();
        if let Some(old) = next
            .forwards
            .iter_mut()
            .find(|old| old.target == row.target && old.remote == row.remote)
        {
            *old = row;
        } else {
            if next.forwards.len() >= MAX_TOTAL
                || next
                    .forwards
                    .iter()
                    .filter(|old| old.target == row.target)
                    .count()
                    >= MAX_PER_HOST
            {
                return Err(ForwardError::PreferenceLimit);
            }
            next.forwards.push(row);
        }
        next.forwards
            .sort_by(|a, b| (&a.target, a.remote).cmp(&(&b.target, b.remote)));
        self.commit(next)
    }

    fn remove(&mut self, key: &Key) -> Result<(), ForwardError> {
        let mut next = self.document.clone();
        next.forwards.retain(|row| !key.matches(row));
        if next != self.document {
            self.commit(next)?;
        }
        Ok(())
    }

    fn commit(&mut self, next: ReversePreferences) -> Result<(), ForwardError> {
        let mut bytes = serde_json::to_vec_pretty(&next).map_err(std::io::Error::other)?;
        bytes.push(b'\n');
        hosts::write_private(&self.path, FILE, &bytes)?;
        self.document = next;
        Ok(())
    }
}

fn valid(document: &ReversePreferences) -> bool {
    if document.version != 1 || document.forwards.len() > MAX_TOTAL {
        return false;
    }
    let mut seen = HashSet::new();
    let mut per_host: HashMap<&str, usize> = HashMap::new();
    document.forwards.iter().all(|row| {
        let count = per_host.entry(&row.target).or_default();
        *count += 1;
        row.remote != 0
            && row.local != 0
            && *count <= MAX_PER_HOST
            && matches!(SshTarget::parse(&row.target), Ok(target) if target.as_str() == row.target)
            && seen.insert((&row.target, row.remote))
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::hub::testing::ScriptedHost;
    use std::net::{Ipv4Addr, TcpListener};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn free_port() -> u16 {
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .and_then(|listener| listener.local_addr())
            .expect("free port")
            .port()
    }

    fn manager(host: &ScriptedHost, dir: &std::path::Path) -> Reverses {
        Reverses::new(Arc::new(host.clone()), dir.join("config"))
    }

    /// A local service that answers `local <port>` to every connection.
    fn service() -> (u16, JoinHandle<()>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("service");
        listener.set_nonblocking(true).expect("nonblocking");
        let port = listener.local_addr().expect("address").port();
        let listener = tokio::net::TcpListener::from_std(listener).expect("tokio");
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let _ = stream.write_all(format!("local {port}\n").as_bytes()).await;
            }
        });
        (port, task)
    }

    async fn ask(port: u16) -> String {
        let mut stream = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .expect("connect");
        let mut answer = String::new();
        let _ =
            tokio::time::timeout(Duration::from_secs(5), stream.read_to_string(&mut answer)).await;
        answer
    }

    async fn until(mut done: impl FnMut() -> bool) {
        for _ in 0..200 {
            if done() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("condition not reached");
    }

    #[tokio::test]
    async fn an_explicit_share_reaches_the_local_service_and_is_reused() {
        let dir = tempfile::tempdir().expect("dir");
        let host = ScriptedHost::new(dir.path());
        let reverses = manager(&host, dir.path());
        let (local, _service) = service();
        let port = free_port();

        let info = reverses
            .open("vm", port.into(), local.into())
            .await
            .expect("shared");
        assert_eq!(
            info,
            ReverseInfo {
                target: "vm".into(),
                remote: port,
                local,
                state: ForwardState::Up,
            }
        );
        assert_eq!(ask(port).await, format!("local {local}\n"));
        assert_eq!(
            reverses
                .open("vm", port.into(), local.into())
                .await
                .expect("again"),
            info
        );
        assert_eq!(
            host.asked()
                .iter()
                .filter(|asked| asked.starts_with("reverse "))
                .count(),
            1,
            "the second request reuses the first"
        );
        let other = local.wrapping_add(1).max(1);
        assert!(matches!(
            reverses.open("vm", port.into(), other.into()).await,
            Err(ForwardError::ActiveConflict { local: active }) if active == local
        ));
        assert_eq!(reverses.list(), vec![info]);
        assert!(
            reverses.preferences().forwards.is_empty(),
            "POST never remembers"
        );
    }

    #[tokio::test]
    async fn the_own_port_and_invalid_ports_are_refused_without_ssh() {
        let dir = tempfile::tempdir().expect("dir");
        let host = ScriptedHost::new(dir.path());
        let reverses = manager(&host, dir.path());
        reverses.reserve(9800);
        assert!(matches!(
            reverses.open("vm", 9800, 9800).await,
            Err(ForwardError::LocalPortReserved { port: 9800 })
        ));
        assert!(matches!(
            reverses.save("vm", 9800, Some(9800)).await,
            Err(ForwardError::LocalPortReserved { port: 9800 })
        ));
        assert!(matches!(
            reverses.open("vm", 0, 11434).await,
            Err(ForwardError::Port(0))
        ));
        assert!(matches!(
            reverses.open("vm", 11434, 70000).await,
            Err(ForwardError::Port(70000))
        ));
        assert!(matches!(
            reverses.open("-oProxyCommand=x", 11434, 11434).await,
            Err(ForwardError::Target(_))
        ));
        assert!(host.asked().is_empty(), "{:?}", host.asked());
    }

    #[tokio::test]
    async fn a_busy_host_port_and_a_mac_host_are_refused() {
        let dir = tempfile::tempdir().expect("dir");
        let host = ScriptedHost::new(dir.path());
        let reverses = manager(&host, dir.path());
        host.set_busy(18080, true);
        assert!(matches!(
            reverses.open("vm", 18080, 8080).await,
            Err(ForwardError::HostPortConflict { port: 18080, .. })
        ));
        assert!(reverses.list().is_empty());

        let mac = ScriptedHost::new(dir.path()).with_kernel("Darwin");
        let reverses = manager(&mac, dir.path());
        match reverses.open("mac", 18081, 8080).await {
            Err(ForwardError::Failed(error)) => {
                assert_eq!(error.failure("mac").code, crate::hub::Code::UnsupportedHost)
            }
            other => panic!("expected unsupported_host, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stop_pauses_a_saved_rule_and_forget_keeps_the_forward() {
        let dir = tempfile::tempdir().expect("dir");
        let host = ScriptedHost::new(dir.path());
        let reverses = manager(&host, dir.path());
        let (local, _service) = service();
        let port = free_port();

        reverses
            .open("vm", port.into(), local.into())
            .await
            .expect("shared");
        let saved = reverses.save("vm", port.into(), None).await.expect("saved");
        assert_eq!(
            (saved.local, saved.enabled),
            (local, true),
            "the active local"
        );

        reverses.forget("vm", port.into()).await.expect("forgot");
        assert!(reverses.preferences().forwards.is_empty());
        assert_eq!(reverses.list().len(), 1, "Forget keeps it running");

        reverses.save("vm", port.into(), None).await.expect("saved");
        reverses.stop("vm", port.into()).await.expect("stopped");
        assert!(reverses.list().is_empty());
        let rows = reverses.preferences().forwards;
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].enabled, "Stop pauses the rule");
        assert!(
            TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok(),
            "the host's port is free"
        );
        // Stopping again is fine for a saved rule; an unknown one is 404.
        reverses
            .stop("vm", port.into())
            .await
            .expect("stopped again");
        assert!(matches!(
            reverses.stop("vm", 1).await,
            Err(ForwardError::NotFound { .. })
        ));

        // A restore leaves the paused rule alone; an explicit open resumes it.
        reverses.restore("vm");
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(reverses.list().is_empty());
        reverses
            .open("vm", port.into(), local.into())
            .await
            .expect("resumed");
        assert!(reverses.preferences().forwards[0].enabled);
    }

    #[tokio::test]
    async fn restore_opens_only_enabled_rules_of_that_host_and_survives_a_restart() {
        let dir = tempfile::tempdir().expect("dir");
        let host = ScriptedHost::new(dir.path());
        let (local, _service) = service();
        let (enabled, paused, elsewhere) = (free_port(), free_port(), free_port());
        {
            let reverses = manager(&host, dir.path());
            reverses
                .save("vm", enabled.into(), Some(local.into()))
                .await
                .expect("saved");
            reverses
                .save("vm", paused.into(), Some(local.into()))
                .await
                .expect("saved");
            reverses.stop("vm", paused.into()).await.expect("paused");
            reverses
                .save("other", elsewhere.into(), Some(local.into()))
                .await
                .expect("saved");
            assert!(reverses.list().is_empty(), "saving opens nothing");
        }
        assert!(host.asked().is_empty(), "{:?}", host.asked());

        // A new server reads the rules back and opens nothing by itself.
        let reverses = manager(&host, dir.path());
        assert_eq!(reverses.preferences().forwards.len(), 3);
        reverses.restore("vm");
        until(|| !reverses.list().is_empty()).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        let list = reverses.list();
        assert_eq!(list.len(), 1, "{list:?}");
        assert_eq!((list[0].remote, list[0].local), (enabled, local));
        assert_eq!(ask(enabled).await, format!("local {local}\n"));
    }

    #[tokio::test]
    async fn a_failed_restore_is_recorded_and_cleared_by_success() {
        let dir = tempfile::tempdir().expect("dir");
        let host = ScriptedHost::new(dir.path());
        let reverses = manager(&host, dir.path());
        let (local, _service) = service();
        let port = free_port();
        reverses
            .save("vm", port.into(), Some(local.into()))
            .await
            .expect("saved");
        host.fail_next_reverses(1);
        reverses.restore("vm");
        until(|| reverses.preferences().forwards[0].last_error.is_some()).await;
        let error = reverses.preferences().forwards[0]
            .last_error
            .clone()
            .expect("error");
        assert_eq!(error.code, "unreachable");

        reverses
            .open("vm", port.into(), local.into())
            .await
            .expect("shared");
        assert_eq!(reverses.preferences().forwards[0].last_error, None);
    }

    #[tokio::test]
    async fn an_ended_reverse_forward_is_rebuilt_with_the_same_ports() {
        let dir = tempfile::tempdir().expect("dir");
        let host = ScriptedHost::new(dir.path());
        let reverses = manager(&host, dir.path());
        let (local, _service) = service();
        let port = free_port();
        reverses
            .open("vm", port.into(), local.into())
            .await
            .expect("shared");

        host.fail_next_reverses(1);
        host.end_reverse(port);
        until(|| reverses.list()[0].state == ForwardState::Down).await;
        until(|| reverses.list()[0].state == ForwardState::Up).await;
        assert_eq!(reverses.list()[0].local, local);
        assert_eq!(ask(port).await, format!("local {local}\n"));
        let attempts = host
            .asked()
            .iter()
            .filter(|asked| *asked == &format!("reverse vm {port} from {local}"))
            .count();
        assert_eq!(attempts, 3, "the first, a failed and a successful rebuild");
    }

    #[tokio::test]
    async fn a_host_has_at_most_32_reverse_forwards() {
        let dir = tempfile::tempdir().expect("dir");
        let host = ScriptedHost::new(dir.path());
        let reverses = manager(&host, dir.path());
        let (local, _service) = service();
        let mut opened = Vec::new();
        while opened.len() < MAX_PER_HOST {
            let port = free_port();
            if opened.contains(&port) {
                continue;
            }
            reverses
                .open("vm", port.into(), local.into())
                .await
                .expect("shared");
            opened.push(port);
        }
        assert!(matches!(
            reverses.open("vm", free_port().into(), local.into()).await,
            Err(ForwardError::Limit { .. })
        ));
        reverses
            .open("vm2", free_port().into(), local.into())
            .await
            .expect("another host has its own limit");
    }

    #[test]
    fn rules_are_private_validated_and_never_store_errors() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("config");
        let mut store = Store::new(path.clone());
        store
            .put(SavedReverse {
                target: "vm".into(),
                remote: 11434,
                local: 11434,
                enabled: true,
                last_error: Some(SavedError {
                    code: "x".into(),
                    message: "y".into(),
                    hint: None,
                }),
            })
            .expect("saved");
        let text = fs::read_to_string(path.join(FILE)).expect("file");
        assert!(!text.contains("lastError"), "{text}");
        assert_eq!(Store::new(path.clone()).list(), store.list());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(path.join(FILE))
                .expect("meta")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        for invalid in [
            r#"{"version":2,"forwards":[]}"#,
            r#"{"version":1,"forwards":[{"target":"vm","remote":0,"local":1,"enabled":true}]}"#,
            r#"{"version":1,"forwards":[{"target":"-x","remote":1,"local":1,"enabled":true}]}"#,
            r#"{"version":1,"forwards":[{"target":"vm","remote":1,"local":1,"enabled":true,"label":""}]}"#,
            r#"{"version":1,"forwards":[{"target":"vm","remote":1,"local":1,"enabled":true},{"target":"vm","remote":1,"local":2,"enabled":true}]}"#,
        ] {
            fs::write(path.join(FILE), invalid).expect("write");
            assert!(
                Store::new(path.clone()).list().forwards.is_empty(),
                "{invalid}"
            );
            assert_eq!(
                fs::read_to_string(path.join(FILE)).expect("file"),
                invalid,
                "never rewritten"
            );
        }
    }
}
