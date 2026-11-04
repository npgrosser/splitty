//! Remote windows use the packaged application document with a fixed native
//! provider for their assigned host. Their transport capability carries no
//! browser credentials; native document proofs authorize each IPC operation.
//! The registry shares one connection attempt and outcome per target.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use serde::Serialize;
use splitty_remote::hosts::{self, KnownHost};
use splitty_remote::ssh::SshTarget;
/// Outcome metadata is shared with the carrier rather than cloned per waiter.
#[derive(Clone, Debug)]
pub(crate) enum Failure {
    Shared(Arc<splitty_transport::budget::Charged<splitty_remote::tunnel::Failure>>),
    Unavailable,
}
impl Failure {
    fn fields(
        &self,
    ) -> (
        &str,
        Option<&str>,
        Option<&splitty_remote::tunnel::MigrationOffer>,
    ) {
        match self {
            Self::Shared(value) => (
                &value.message,
                value.hint.as_deref(),
                value.migration.as_ref(),
            ),
            Self::Unavailable => ("Native diagnostic unavailable", None, None),
        }
    }
}
impl PartialEq for Failure {
    fn eq(&self, other: &Self) -> bool {
        self.fields() == other.fields()
    }
}
impl Eq for Failure {}
impl Serialize for Failure {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Borrowed<'a> {
            message: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            hint: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            migration: Option<&'a splitty_remote::tunnel::MigrationOffer>,
        }
        let (message, hint, migration) = self.fields();
        Borrowed {
            message,
            hint,
            migration,
        }
        .serialize(serializer)
    }
}
#[cfg(test)]
use splitty_remote::tunnel::{Progress, Step};
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};
use tokio::sync::watch;

use crate::client_ipc::{
    carrier::{Auth, BackendEvent, Native},
    provider::Provider,
    BackendId, Policy, Proof,
};
use crate::navigation;

/// How long the app's exit waits for the tunnels to be torn down.
const EXIT_GRACE: Duration = Duration::from_secs(2);

/// Runs in every page of a remote window, before the page's own code: the
/// page's leave prompt (`beforeunload`) is dropped (D12 of remote-window,
/// provisional). WebKitGTK shows it inside the window, and loading a new
/// session after the daemon restarted would wait for the user. Leaving a
/// host's page loses nothing, since the sessions belong to the host's daemon.
/// Its other `beforeunload` work still runs.
const NO_LEAVE_PROMPT: &str = r#"(() => {
  const proto = window.BeforeUnloadEvent && window.BeforeUnloadEvent.prototype;
  if (!proto) return;
  Object.defineProperty(proto, "preventDefault", { value() {}, configurable: false });
  Object.defineProperty(proto, "returnValue", { get() { return ""; }, set() {}, configurable: false });
})();"#;

/// What an open ends with, as the trusted UI receives it.
type Outcome = Result<(), Failure>;

/// A failure without a hint.
fn failure(message: impl std::fmt::Display) -> Failure {
    splitty_remote::tunnel::Failure::from_error_budgeted(
        &message,
        &splitty_transport::budget::ConnectionBudget::default(),
    )
    .map(|failure| Failure::Shared(Arc::new(failure)))
    .unwrap_or(Failure::Unavailable)
}

/// What a cancelled open rejects with.
fn cancelled() -> Failure {
    failure("cancelled")
}

/// The connection a remote window's title reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Connection {
    Connected,
    /// The tunnel is being rebuilt.
    Reconnecting,
    /// The supervisor gave up; opening the target again reconnects.
    Disconnected,
}

/// The title of the remote window of `target`.
fn title(target: &str, connection: Connection) -> String {
    match connection {
        Connection::Connected => format!("{target} — Splitty"),
        Connection::Reconnecting => format!("{target} (reconnecting…) — Splitty"),
        Connection::Disconnected => format!("{target} (disconnected) — Splitty"),
    }
}

/// What the app keeps of a remote window, for the guard and the supervisor.
#[derive(Clone)]
struct WindowLink {
    backend: BackendId,
    closed: watch::Receiver<bool>,
}

struct HostWindow {
    label: String,
    /// Whether its supervisor still keeps the tunnel, possibly rebuilding it.
    connected: bool,
    link: WindowLink,
}

struct Attempt {
    id: u64,
    update: bool,
    outcome: watch::Sender<Option<Outcome>>,
    /// The windows that asked for it, which get its progress.
    callers: Vec<String>,
    /// The flow succeeded and the window is being shown: too late to cancel
    /// (D14 of remote-window, provisional).
    committed: bool,
}

#[derive(Default)]
struct Host {
    window: Option<HostWindow>,
    attempt: Option<Attempt>,
}

/// What `remote_open` does for a target.
enum Begin {
    Rejected,
    /// Its window is open and connected: focus it.
    Focus {
        label: String,
    },
    /// An open of it runs already: wait for its outcome.
    Wait {
        outcome: watch::Receiver<Option<Outcome>>,
    },
    /// Run the flow as `attempt`, and wait for its outcome.
    Start {
        attempt: u64,
        outcome: watch::Receiver<Option<Outcome>>,
    },
}

/// Where an attempt whose flow succeeded shows the host.
#[cfg(test)]
enum Commit {
    /// In the target's window, whose connection gave up.
    Reuse { label: String, link: WindowLink },
    /// In a new window.
    New { label: String },
}

/// The remote windows by target, and the opens in flight (D3 of
/// remote-window, provisional: one window per target).
#[derive(Default)]
struct Registry {
    hosts: HashMap<String, Host>,
    attempts: u64,
    windows: u64,
}

impl Registry {
    /// An open of `target`, asked for by the window `caller`.
    fn begin(&mut self, target: &str, caller: &str) -> Begin {
        self.begin_with_update(target, caller, false)
    }
    fn begin_with_update(&mut self, target: &str, caller: &str, update: bool) -> Begin {
        let host = self.hosts.entry(target.to_owned()).or_default();
        if let Some(attempt) = &mut host.attempt {
            if attempt.update != update {
                return Begin::Rejected;
            }
            if !attempt.callers.iter().any(|known| known == caller) {
                attempt.callers.push(caller.to_owned());
            }
            return Begin::Wait {
                outcome: attempt.outcome.subscribe(),
            };
        }
        if let Some(window) = host.window.as_ref().filter(|window| window.connected) {
            if update {
                return Begin::Rejected;
            }
            return Begin::Focus {
                label: window.label.clone(),
            };
        }
        self.attempts += 1;
        let (sender, outcome) = watch::channel(None);
        host.attempt = Some(Attempt {
            id: self.attempts,
            update,
            outcome: sender,
            callers: vec![caller.to_owned()],
            committed: false,
        });
        Begin::Start {
            attempt: self.attempts,
            outcome,
        }
    }

    /// The windows that asked for `attempt`.
    fn callers(&self, target: &str, attempt: u64) -> Vec<String> {
        self.attempt(target, attempt)
            .map(|attempt| attempt.callers.clone())
            .unwrap_or_default()
    }

    /// The flow of `attempt` succeeded: where to show the host, or `None`
    /// when the open was cancelled meanwhile.
    fn mark_committed(&mut self, target: &str, attempt: u64) -> bool {
        let Some(current) = self
            .hosts
            .get_mut(target)
            .and_then(|host| host.attempt.as_mut())
            .filter(|current| current.id == attempt)
        else {
            return false;
        };
        current.committed = true;
        true
    }

    #[cfg(test)]
    fn commit(&mut self, target: &str, attempt: u64) -> Option<Commit> {
        let host = self.hosts.get_mut(target)?;
        let current = host
            .attempt
            .as_mut()
            .filter(|current| current.id == attempt)?;
        current.committed = true;
        let reuse = host.window.as_ref().map(|window| Commit::Reuse {
            label: window.label.clone(),
            link: window.link.clone(),
        });
        Some(reuse.unwrap_or_else(|| Commit::New {
            label: self.new_label(),
        }))
    }

    /// A label for a new remote window.
    fn new_label(&mut self) -> String {
        self.windows += 1;
        format!("remote-{}", self.windows)
    }

    /// Settle `attempt`: every open waiting for it gets the outcome. With a
    /// window, that window now shows the target.
    fn finish(
        &mut self,
        target: &str,
        attempt: u64,
        result: Result<(String, WindowLink), Failure>,
    ) {
        let Some(host) = self.hosts.get_mut(target) else {
            return;
        };
        let Some(current) = host.attempt.take_if(|current| current.id == attempt) else {
            return;
        };
        let outcome = result.map(|(label, link)| {
            host.window = Some(HostWindow {
                label,
                connected: true,
                link,
            });
        });
        current.outcome.send_replace(Some(outcome));
        self.forget_if_unused(target);
    }

    /// Cancel the open of `target` in flight, unless it is past its flow.
    /// Every open waiting for it rejects with `cancelled`.
    fn cancel(&mut self, target: &str) -> bool {
        let Some(host) = self.hosts.get_mut(target) else {
            return false;
        };
        let Some(attempt) = host.attempt.take_if(|attempt| !attempt.committed) else {
            return false;
        };
        attempt.outcome.send_replace(Some(Err(cancelled())));
        self.forget_if_unused(target);
        true
    }

    fn connection_state(&mut self, label: &str, state: Connection) {
        if let Some(window) = self
            .hosts
            .values_mut()
            .filter_map(|host| host.window.as_mut())
            .find(|window| window.label == label)
        {
            window.connected = state != Connection::Disconnected;
        }
    }

    /// The supervisor of the window `label` gave up.
    #[cfg(test)]
    fn disconnected(&mut self, label: &str) {
        if let Some(window) = self
            .hosts
            .values_mut()
            .filter_map(|host| host.window.as_mut())
            .find(|window| window.label == label)
        {
            window.connected = false;
        }
    }

    /// The window `label` is gone.
    fn closed(&mut self, label: &str) {
        let Some(target) = self
            .hosts
            .iter_mut()
            .find(|(_, host)| host.window.as_ref().is_some_and(|w| w.label == label))
            .map(|(target, host)| {
                host.window = None;
                target.clone()
            })
        else {
            return;
        };
        self.forget_if_unused(&target);
    }

    fn attempt(&self, target: &str, attempt: u64) -> Option<&Attempt> {
        self.hosts
            .get(target)?
            .attempt
            .as_ref()
            .filter(|current| current.id == attempt)
    }

    fn forget_if_unused(&mut self, target: &str) {
        if self
            .hosts
            .get(target)
            .is_some_and(|host| host.window.is_none() && host.attempt.is_none())
        {
            self.hosts.remove(target);
        }
    }
}

/// The app's remote windows: the registry and the tasks that hold tunnels.
pub struct Remotes {
    registry: Mutex<Registry>,
    opening: Arc<splitty_transport::budget::Budget>,
    reads: Arc<splitty_transport::budget::Budget>,
    request_process: Arc<splitty_transport::budget::Budget>,
    request_documents: Mutex<HashMap<(String, u64), Weak<splitty_transport::budget::Budget>>>,
    requesters: Mutex<HashMap<String, Requester>>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    progress: Mutex<
        HashMap<String, Arc<splitty_transport::budget::Charged<splitty_transport::wire::Progress>>>,
    >,
}

#[derive(Clone)]
struct Requester {
    window: String,
    proof: Proof,
    references: usize,
}
/// Command waiters and connector cleanup have separate lifetimes. A coalesced
/// waiter must consume admission even though it never starts a connector.
struct RequestAdmission {
    _process: splitty_transport::budget::Permit,
    _document: splitty_transport::budget::Permit,
}

struct RequestGuard {
    remotes: Arc<Remotes>,
    key: String,
}
impl Drop for RequestGuard {
    fn drop(&mut self) {
        let mut requesters = self.remotes.requesters.lock().expect("remote requesters");
        let released = if let Some(requester) = requesters.get_mut(&self.key) {
            requester.references -= 1;
            requester.references == 0
        } else {
            true // A newer authorized request already purged this stale document.
        };
        if released {
            requesters.remove(&self.key);
            // Keep removal atomic with creation of another guard for this key.
            // Registry readers release their lock before taking requesters.
            for host in self.remotes.lock().hosts.values_mut() {
                if let Some(attempt) = &mut host.attempt {
                    attempt.callers.retain(|caller| caller != &self.key);
                }
            }
        }
    }
}

impl Default for Remotes {
    fn default() -> Self {
        Self {
            registry: Default::default(),
            requesters: Default::default(),
            tasks: Default::default(),
            progress: Default::default(),
            request_process: {
                static PROCESS: std::sync::OnceLock<Arc<splitty_transport::budget::Budget>> =
                    std::sync::OnceLock::new();
                PROCESS
                    .get_or_init(|| {
                        splitty_transport::budget::Budget::new(
                            splitty_transport::admission::OPENING,
                        )
                    })
                    .clone()
            },
            request_documents: Default::default(),
            reads: splitty_transport::budget::Budget::new(
                splitty_transport::admission::DOMAIN_OPENING,
            ),
            opening: splitty_transport::budget::Budget::new(
                splitty_transport::admission::DOMAIN_OPENING,
            ),
        }
    }
}

impl Remotes {
    fn lock(&self) -> MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn admit_request(
        &self,
        native: &Native,
        window: &str,
        proof: &Proof,
    ) -> Result<RequestAdmission, Failure> {
        native.broker.with_authority(window, proof, || {
            let process = self.request_process.acquire(1)
                .map_err(|_| failure("Too many remote requests are pending. Try again shortly."))?;
            let mut documents = self.request_documents.lock().expect("remote request admission");
            // Only weak bookkeeping lives here: live command permits retain
            // their exact document budget across revocation until actual drop.
            // Purging expired keys bounds reload bookkeeping by process slots.
            documents.retain(|_, budget| budget.strong_count() != 0);
            let entry = documents.entry((window.to_owned(), proof.document)).or_default();
            let budget = match entry.upgrade() {
                Some(budget) => budget,
                None => {
                    let budget = splitty_transport::budget::Budget::new(splitty_transport::admission::DOMAIN_OPENING);
                    *entry = Arc::downgrade(&budget);
                    budget
                }
            };
            let document = budget.acquire(1)
                .map_err(|_| failure("Too many remote requests are pending for this document. Try again shortly."))?;
            Ok(RequestAdmission { _process: process, _document: document })
        }).map_err(|_| failure("Native document denied"))?
    }

    fn requester(self: &Arc<Self>, native: &Native, window: &str, proof: Proof) -> RequestGuard {
        let key = format!("{window}:{}", proof.document);
        let mut requesters = self.requesters.lock().expect("remote requesters");
        requesters.retain(|_, requester| {
            native
                .broker
                .authorize(&requester.window, &requester.proof)
                .is_ok()
        });
        let requester = requesters.entry(key.clone()).or_insert(Requester {
            window: window.into(),
            proof,
            references: 0,
        });
        requester.references += 1;
        RequestGuard {
            remotes: self.clone(),
            key,
        }
    }
    fn current_requesters(
        &self,
        native: &Native,
        target: &str,
        attempt: u64,
    ) -> Vec<(String, Requester)> {
        let callers = self.lock().callers(target, attempt);
        let requesters: Vec<_> = {
            let registry = self.requesters.lock().expect("remote requesters");
            callers
                .into_iter()
                .filter_map(|key| {
                    registry
                        .get(&key)
                        .cloned()
                        .map(|requester| (key, requester))
                })
                .collect()
        };
        requesters
            .into_iter()
            .filter(|(_, requester)| {
                native
                    .broker
                    .authorize(&requester.window, &requester.proof)
                    .is_ok()
            })
            .collect()
    }

    fn admit_read(
        &self,
    ) -> Result<
        (
            splitty_transport::budget::Permit,
            splitty_transport::budget::Permit,
        ),
        Failure,
    > {
        static PROCESS: std::sync::OnceLock<Arc<splitty_transport::budget::Budget>> =
            std::sync::OnceLock::new();
        let process = PROCESS.get_or_init(|| {
            splitty_transport::budget::Budget::new(splitty_transport::admission::OPENING)
        });
        let process = process
            .acquire(1)
            .map_err(|_| failure("Too many native reads are running."))?;
        let domain = self
            .reads
            .acquire(1)
            .map_err(|_| failure("Too many native reads are running."))?;
        Ok((process, domain))
    }

    fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        let handle = tauri::async_runtime::spawn(task);
        let mut tasks = self.tasks.lock().unwrap_or_else(PoisonError::into_inner);
        tasks.retain(|task| !task.inner().is_finished());
        tasks.push(handle);
    }

    /// End every tunnel and every open in flight, for the app's exit.
    ///
    /// The process exits without running the destructors of tasks still
    /// alive, so the tasks are dropped here, which kills their `ssh`.
    pub fn shutdown(&self) {
        let tasks = std::mem::take(&mut *self.tasks.lock().unwrap_or_else(PoisonError::into_inner));
        for task in &tasks {
            task.abort();
        }
        tauri::async_runtime::block_on(async {
            let _ = tokio::time::timeout(EXIT_GRACE, async {
                for task in tasks {
                    let _ = task.await;
                }
            })
            .await;
        });
    }
}

/// Settles an attempt whose task ended without doing so (a panic, or the
/// app's exit). A no-op after `finish` or `cancel`.
struct Unsettled {
    remotes: Arc<Remotes>,
    target: String,
    attempt: u64,
}

impl Drop for Unsettled {
    fn drop(&mut self) {
        self.remotes.lock().finish(
            &self.target,
            self.attempt,
            Err(failure("The connection attempt ended unexpectedly.")),
        );
    }
}

/// The outcome of an attempt, once it has one. An attempt that went away
/// without one counts as cancelled.
async fn settled(outcome: &mut watch::Receiver<Option<Outcome>>) -> Outcome {
    match outcome.wait_for(Option::is_some).await {
        Ok(value) => value.clone().unwrap_or_else(|| Err(cancelled())),
        Err(_) => Err(cancelled()),
    }
}

/// Where the recent hosts are kept: Splitty's configuration directory, shared
/// with the CLI, and on Windows, which has no CLI, the app's own.
#[cfg(not(target_os = "windows"))]
fn config_dir(_app: &AppHandle) -> Result<PathBuf, String> {
    Ok(splitty_core::config::config_dir())
}

#[cfg(target_os = "windows")]
fn config_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map_err(|error| format!("could not resolve the configuration directory: {error}"))
}

/// A step of an open as `connect --json` names it, or `notice`.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
enum ProgressStep {
    #[cfg(test)]
    Step(Step),
    #[cfg(test)]
    Other(&'static str),
    Named(String),
}

/// The payload of [`PROGRESS_EVENT`].
#[derive(Debug, Clone, Serialize)]
pub(crate) struct RemoteProgress {
    target: String,
    step: ProgressStep,
    message: String,
}

#[cfg(test)]
impl RemoteProgress {
    fn new(target: &str, progress: Progress) -> Self {
        let (step, message) = match progress {
            Progress::Step { step, message } => (ProgressStep::Step(step), message),
            Progress::Notice { message } => (ProgressStep::Other("notice"), message),
        };
        Self {
            target: target.to_owned(),
            step,
            message,
        }
    }
}

/// The known hosts: recent hosts first, then those of `~/.ssh/config`.
#[tauri::command]
pub async fn remote_hosts(
    app: AppHandle,
    webview: tauri::Webview,
    native: tauri::State<'_, Arc<Native>>,
    remotes: tauri::State<'_, Arc<Remotes>>,
    auth: Auth,
) -> Result<Vec<KnownHost>, Failure> {
    native.launcher_action(&webview, &auth).map_err(failure)?;
    let config_dir = config_dir(&app).map_err(failure)?;
    let permits = remotes.admit_read()?;
    tauri::async_runtime::spawn_blocking(move || {
        let _permits = permits;
        hosts::known(&config_dir, hosts::default_ssh_dir().as_deref())
    })
    .await
    .map_err(|error| failure(format_args!("could not read the known hosts: {error}")))
}

/// Open `target` in a remote window. Resolves once the window shows the host.
#[tauri::command]
pub async fn remote_open(
    app: AppHandle,
    webview: tauri::Webview,
    remotes: tauri::State<'_, Arc<Remotes>>,
    native: tauri::State<'_, Arc<Native>>,
    auth: Auth,
    target: String,
    update: Option<bool>,
) -> Result<(), Failure> {
    native.launcher_action(&webview, &auth).map_err(failure)?;
    let proof = auth.proof();
    // Nonwaiting admission precedes requester guards, outcome subscriptions
    // and registry work, including duplicate opens of the same target.
    let _admission = remotes.admit_request(&native, webview.label(), &proof)?;
    let caller = format!("{}:{}", webview.label(), proof.document);
    // Before anything runs, as `splitty connect` does (`access-control`).
    let target = SshTarget::parse(&target).map_err(failure)?;
    let remotes = Arc::clone(remotes.inner());
    let _request = remotes.requester(&native, webview.label(), proof.clone());
    loop {
        let update = update.unwrap_or(false);
        let begin = if update {
            remotes
                .lock()
                .begin_with_update(target.as_str(), &caller, true)
        } else {
            remotes.lock().begin(target.as_str(), &caller)
        };
        match begin {
            Begin::Rejected => return Err(failure("Another connection is active for this host. Wait for it to finish before updating.")),
            Begin::Focus { label } => {
                if let Some(window) = app.get_webview_window(&label) {
                    focus(&window);
                    return Ok(());
                }
                // Destroyed, and the registry not told yet.
                remotes.lock().closed(&label);
            }
            Begin::Wait { mut outcome } => return settled(&mut outcome).await,
            Begin::Start {
                attempt,
                mut outcome,
            } => {
                let opening = match remotes.opening.acquire(1) {
                    Ok(opening) => opening,
                    Err(_) => {
                        let error =
                            failure("Too many remote windows are opening. Try again shortly.");
                        remotes
                            .lock()
                            .finish(target.as_str(), attempt, Err(error.clone()));
                        return Err(error);
                    }
                };
                remotes.spawn(connect(
                    app.clone(),
                    Arc::clone(&remotes),
                    target,
                    attempt,
                    update,
                    outcome.clone(),
                    opening,
                ));
                return settled(&mut outcome).await;
            }
        }
    }
}

/// Cancel the open of `target` in flight; it then rejects with `cancelled`.
#[tauri::command]
pub fn remote_cancel(
    webview: tauri::Webview,
    native: tauri::State<'_, Arc<Native>>,
    auth: Auth,
    remotes: tauri::State<'_, Arc<Remotes>>,
    target: String,
) -> Result<(), String> {
    native.launcher_action(&webview, &auth)?;
    let caller = format!("{}:{}", webview.label(), auth.proof().document);
    if let Ok(target) = SshTarget::parse(&target) {
        let attempt = remotes
            .lock()
            .hosts
            .get(target.as_str())
            .and_then(|host| host.attempt.as_ref())
            .map(|attempt| attempt.id);
        if let Some(attempt) = attempt {
            let remaining: Vec<_> = remotes
                .current_requesters(&native, target.as_str(), attempt)
                .into_iter()
                .map(|(key, _)| key)
                .filter(|key| key != &caller)
                .collect();
            let mut registry = remotes.lock();
            if let Some(current) = registry
                .hosts
                .get_mut(target.as_str())
                .and_then(|host| host.attempt.as_mut())
                .filter(|current| current.id == attempt)
            {
                current.callers.retain(|key| remaining.contains(key));
                if current.callers.is_empty() {
                    registry.cancel(target.as_str());
                }
            }
        }
    }
    Ok(())
}

fn assigned_remote(native: &Native, label: &str, proof: &Proof) -> Result<SshTarget, String> {
    match native
        .broker
        .policy(label, proof)
        .map_err(|_| "native document denied")?
    {
        Policy::Remote { target, .. } => Ok(target),
        _ => Err("remote connection state denied".into()),
    }
}

/// A remote document reports the state of its own recovery machine. It cannot
/// name another window or host, and state changes carry no backend authority.
#[tauri::command]
pub(crate) fn native_connection_state(
    app: AppHandle,
    webview: tauri::Webview,
    native: tauri::State<'_, Arc<Native>>,
    remotes: tauri::State<'_, Arc<Remotes>>,
    auth: Auth,
    state: Connection,
) -> Result<(), String> {
    let proof = native.authenticate(&webview, &auth)?;
    let target = assigned_remote(&native, webview.label(), &proof)?;
    native
        .broker
        .with_authority(webview.label(), &proof, || {
            remotes.lock().connection_state(webview.label(), state);
            if let Some(window) = app.get_webview_window(webview.label()) {
                set_title(&window, target.as_str(), state);
            }
        })
        .map_err(|_| "native document denied".into())
}

/// A bounded latest-value progress snapshot, pulled by the launcher. Native
/// progress never accumulates an unacknowledged WebView event queue.
#[tauri::command]
pub fn remote_progress(
    webview: tauri::Webview,
    native: tauri::State<'_, Arc<Native>>,
    auth: Auth,
    remotes: tauri::State<'_, Arc<Remotes>>,
    target: String,
) -> Result<Option<RemoteProgress>, String> {
    native.launcher_action(&webview, &auth)?;
    Ok(remotes
        .progress
        .lock()
        .expect("remote progress")
        .get(&target)
        .map(|progress| RemoteProgress {
            target: target.clone(),
            step: ProgressStep::Named(progress.phase.clone()),
            message: progress.message.clone(),
        }))
}

async fn connect(
    app: AppHandle,
    remotes: Arc<Remotes>,
    target: SshTarget,
    attempt: u64,
    update: bool,
    mut outcome: watch::Receiver<Option<Outcome>>,
    opening: splitty_transport::budget::Permit,
) {
    let host = target.as_str().to_owned();
    let _unsettled = Unsettled {
        remotes: remotes.clone(),
        target: host.clone(),
        attempt,
    };
    let native = app.state::<Arc<Native>>().inner().clone();
    if outcome.borrow().is_some()
        || remotes
            .current_requesters(&native, &host, attempt)
            .is_empty()
    {
        return;
    }
    let mut existing = remotes
        .lock()
        .hosts
        .get(&host)
        .and_then(|host| host.window.as_ref())
        .map(|window| (window.label.clone(), window.link.clone()));
    if update {
        // A confirmation must not arm a document whose automatic reconnect
        // could consume it. Retire its fixed attachment before creating a new
        // provider with a single native-owned update intent.
        if let Some((label, link)) = existing.take() {
            native.destroyed(&label);
            if let Some(window) = app.get_webview_window(&label) {
                let _ = window.destroy();
            }
            native.remove_provider(link.backend);
            remotes.lock().closed(&label);
        }
    }
    let reused = existing.is_some();
    let (backend, label, link, mut events, ready_epoch) = if let Some((label, link)) = existing {
        let backend = link.backend;
        let Some(events) = native.watch_backend(backend) else {
            return;
        };
        let ready_epoch = native.ready_epoch(backend);
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.eval("window.dispatchEvent(new Event('splitty-native-retry'))");
        }
        (backend, label, link, events, ready_epoch)
    } else {
        let backend = BackendId::new();
        native.provider(
            backend,
            Arc::new(if update {
                Provider::remote_with_update(target.clone(), true)
            } else {
                Provider::remote(target.clone())
            }),
        );
        let label = remotes.lock().new_label();
        if native
            .broker
            .register(
                &label,
                Policy::Remote {
                    backend,
                    target: target.clone(),
                },
            )
            .is_err()
        {
            native.remove_provider(backend);
            remotes.lock().finish(
                &host,
                attempt,
                Err(failure("Native window capacity is exhausted.")),
            );
            return;
        }
        let Some(events) = native.watch_backend(backend) else {
            return;
        };
        let link = match build(&app, &remotes, &label, &host, backend) {
            Ok(link) => link,
            Err(error) => {
                native.destroyed(&label);
                native.remove_provider(backend);
                remotes.lock().finish(&host, attempt, Err(failure(error)));
                return;
            }
        };
        (backend, label, link, events, 0)
    };
    let mut check = tokio::time::interval(Duration::from_millis(100));
    let bootstrap_deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let result = loop {
        tokio::select! { biased;
            _ = settled(&mut outcome) => break Err(cancelled()),
            _ = bootstrap_timeout(&native, backend, bootstrap_deadline), if !reused => {
                break Err(failure("Remote window bootstrap timed out."));
            }
            _ = check.tick() => {
                if remotes.current_requesters(&native, &host, attempt).is_empty() { break Err(cancelled()); }
                if *link.closed.borrow() { break Err(cancelled()); }
                if native.ready_epoch(backend) > ready_epoch { break Ok(()); }
            }
            changed = events.changed() => {
                if changed.is_err() { break Err(failure("Native backend closed.")); }
                match events.borrow().clone() {
                    BackendEvent::Ready => break Ok(()),
                    BackendEvent::Failed(error) => break Err(Failure::Shared(error)),
                    BackendEvent::Closed => break Err(failure("Native backend failed; diagnostic unavailable")),
                    BackendEvent::Progress(progress) => {
                        remotes.progress.lock().expect("remote progress").insert(host.clone(), progress);
                    }
                    _ => {}
                }
            }
        }
    };
    remotes
        .progress
        .lock()
        .expect("remote progress")
        .remove(&host);
    if let Err(error) = result {
        if !reused {
            native.destroyed(&label);
            if let Some(window) = app.get_webview_window(&label) {
                let _ = window.destroy();
            }
            native.remove_provider(backend);
        }
        remotes.lock().finish(&host, attempt, Err(error));
        return;
    }
    // Commit linearizes against both requester revocation and cancellation.
    // The closure only touches the remote registry while broker authority is
    // locked; it never re-enters the broker.
    let requesters = remotes.current_requesters(&native, &host, attempt);
    let committed = requesters.into_iter().any(|(_, requester)| {
        native
            .broker
            .with_authority(&requester.window, &requester.proof, || {
                remotes.lock().mark_committed(&host, attempt)
            })
            .unwrap_or(false)
    });
    if !committed {
        if !reused {
            native.destroyed(&label);
            if let Some(window) = app.get_webview_window(&label) {
                let _ = window.destroy();
            }
            native.remove_provider(backend);
        }
        remotes.lock().finish(&host, attempt, Err(cancelled()));
        return;
    }
    record_recent(&app, &target);
    if let Some(window) = app.get_webview_window(&label) {
        focus(&window);
    }
    remotes
        .lock()
        .finish(&host, attempt, Ok((label.clone(), link.clone())));
    drop(opening);
    if reused {
        return;
    }
    // The renderer's bounded recovery machine owns its deadline. Only its
    // authenticated state callback changes the established window's title and
    // retry affordance; late pipe cleanup must not overwrite Disconnected.
    let mut closed = link.closed.clone();
    let _ = closed.wait_for(|gone| *gone).await;
    native.remove_provider(backend);
}

/// The deadline only bounds creation of an authenticated native stream, not SSH
/// installation/preparation after that stream has begun. Initial watch Pending
/// is not proof that the document ever invoked native_open.
async fn bootstrap_timeout(native: &Native, backend: BackendId, deadline: tokio::time::Instant) {
    tokio::time::sleep_until(deadline).await;
    if native.backend_started(backend) {
        std::future::pending::<()>().await;
    }
}

fn record_recent(app: &AppHandle, target: &SshTarget) {
    let recorded = config_dir(app)
        .and_then(|dir| hosts::record_recent(&dir, target).map_err(|error| error.to_string()));
    if let Err(error) = recorded {
        tracing::warn!(
            "could not record {} as a recent host: {error}",
            target.as_str()
        );
    }
}

/// A host receives packaged executable UI and one fixed native provider.
fn build(
    app: &AppHandle,
    remotes: &Arc<Remotes>,
    label: &str,
    target: &str,
    backend: BackendId,
) -> tauri::Result<WindowLink> {
    let (closed_sender, closed) = watch::channel(false);
    let link = WindowLink { backend, closed };
    let builder = WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title(title(target, Connection::Reconnecting))
        .inner_size(1200.0, 800.0)
        .visible(false)
        .incognito(true)
        .disable_drag_drop_handler()
        .initialization_script(NO_LEAVE_PROMPT);
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    let builder = builder.data_directory(webview_data_dir(app, label)?);
    let builder = crate::configure_window(
        builder,
        app.clone(),
        label,
        navigation::DocumentRole::Remote,
    );
    let window = builder.build()?;
    let remotes = remotes.clone();
    let native = app.state::<Arc<Native>>().inner().clone();
    let window_label = label.to_owned();
    window.on_window_event(move |event| {
        if let WindowEvent::Destroyed = event {
            closed_sender.send_replace(true);
            native.destroyed(&window_label);
            remotes.lock().closed(&window_label);
        }
    });
    Ok(link)
}

/// A separate context key per remote window. On Linux, Tauri caches custom
/// protocol registration by this path, while Wry creates a fresh ephemeral
/// context for incognito windows. Reusing Main's key would omit the packaged
/// protocol handler from that fresh context, preventing the UI from loading.
/// WebView2 keeps one InPrivate session per user data folder
/// and profile, shared by every InPrivate webview in it, so `incognito` alone
/// would let two hosts' windows share their cookies. Labels repeat across
/// runs; the folder keeps no browsing data, since the webview is InPrivate.
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn webview_data_dir(app: &AppHandle, label: &str) -> tauri::Result<PathBuf> {
    Ok(app
        .path()
        .app_local_data_dir()?
        .join("remote-windows")
        .join(label))
}

fn set_title(window: &WebviewWindow, target: &str, connection: Connection) {
    if let Err(error) = window.set_title(&title(target, connection)) {
        tracing::warn!("could not set the title of the window of {target}: {error}");
    }
}

fn focus(window: &WebviewWindow) {
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn bootstrap_deadline_ends_missing_ui_but_not_started_ssh_preparation() {
        let native = Arc::new(Native::default());
        let missing = BackendId::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        let waiting = native.clone();
        let timeout =
            tokio::spawn(async move { bootstrap_timeout(&waiting, missing, deadline).await });
        tokio::time::advance(Duration::from_secs(19)).await;
        assert!(!timeout.is_finished());
        tokio::time::advance(Duration::from_secs(1)).await;
        timeout.await.unwrap();
        let started = BackendId::new();
        native.provider(
            started,
            Arc::new(Provider::remote(SshTarget::parse("host").unwrap())),
        );
        native.preparing(started);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        let waiting = native.clone();
        let timeout =
            tokio::spawn(async move { bootstrap_timeout(&waiting, started, deadline).await });
        tokio::time::advance(Duration::from_secs(600)).await;
        tokio::task::yield_now().await;
        assert!(
            !timeout.is_finished(),
            "SSH preparation has its own timeout"
        );
        timeout.abort();
        assert!(timeout.await.unwrap_err().is_cancelled());
    }

    fn link() -> WindowLink {
        let (_closed, closed) = watch::channel(false);
        WindowLink {
            backend: BackendId::new(),
            closed,
        }
    }

    fn outcome_of(receiver: &watch::Receiver<Option<Outcome>>) -> Option<Outcome> {
        receiver.borrow().clone()
    }

    /// Start an open of `target` from `main` and return its attempt.
    fn start(registry: &mut Registry, target: &str) -> (u64, watch::Receiver<Option<Outcome>>) {
        match registry.begin(target, "main") {
            Begin::Start { attempt, outcome } => (attempt, outcome),
            _ => panic!("expected a new attempt"),
        }
    }

    #[test]
    fn update_confirmation_never_coalesces_with_an_ordinary_open() {
        let mut registry = Registry::default();
        let (ordinary, _) = start(&mut registry, "host");
        assert!(matches!(
            registry.begin_with_update("host", "confirming", true),
            Begin::Rejected
        ));
        assert_eq!(registry.callers("host", ordinary), vec!["main"]);
        registry.finish("host", ordinary, Err(cancelled()));
        let Begin::Start { attempt, .. } = registry.begin_with_update("host", "confirming", true)
        else {
            panic!("explicit migration did not start");
        };
        assert!(matches!(
            registry.begin("host", "ordinary"),
            Begin::Rejected
        ));
        assert_eq!(registry.callers("host", attempt), vec!["confirming"]);
        registry.cancel("host");
        assert!(matches!(
            registry.begin("host", "ordinary"),
            Begin::Start { .. }
        ));
    }

    #[test]
    fn update_confirmation_cannot_be_consumed_by_focusing_a_connected_window() {
        let mut registry = Registry::default();
        open(&mut registry, "host");
        assert!(matches!(
            registry.begin_with_update("host", "main", true),
            Begin::Rejected
        ));
        assert!(matches!(
            registry.begin("host", "main"),
            Begin::Focus { .. }
        ));
    }

    #[test]
    fn launcher_serializes_the_native_offer_without_migration_authority() {
        let budget = splitty_transport::budget::ConnectionBudget::default();
        let value = budget
            .display(&"Update required", false)
            .unwrap()
            .map(|message| splitty_remote::tunnel::Failure {
                message,
                hint: None,
                migration: Some(splitty_remote::tunnel::MigrationOffer {
                    available: true,
                    from_version: "0.1.27".into(),
                    to_version: "0.2.0-dev.2".into(),
                    sessions: 3,
                }),
            });
        let json = serde_json::to_value(Failure::Shared(Arc::new(value))).unwrap();
        assert_eq!(
            json["migration"],
            serde_json::json!({
                "available": true, "fromVersion": "0.1.27", "toVersion": "0.2.0-dev.2", "sessions": 3,
            })
        );
        assert!(json.get("token").is_none());
    }

    /// Open `target` in a new window and return its label.
    fn open(registry: &mut Registry, target: &str) -> String {
        let (attempt, _) = start(registry, target);
        let Some(Commit::New { label }) = registry.commit(target, attempt) else {
            panic!("expected a new window");
        };
        registry.finish(target, attempt, Ok((label.clone(), link())));
        label
    }

    #[test]
    fn recovery_deadline_state_enables_deliberate_retry_in_the_existing_window() {
        let mut registry = Registry::default();
        let label = open(&mut registry, "vm1");
        registry.connection_state(&label, Connection::Reconnecting);
        assert!(matches!(registry.begin("vm1", "main"), Begin::Focus { .. }));
        registry.connection_state(&label, Connection::Disconnected);
        let (attempt, _) = start(&mut registry, "vm1");
        assert!(
            matches!(registry.commit("vm1", attempt), Some(Commit::Reuse { label: reused, .. }) if reused == label)
        );
        registry.finish("vm1", attempt, Ok((label.clone(), link())));
        registry.connection_state(&label, Connection::Connected);
        assert!(
            matches!(registry.begin("vm1", "main"), Begin::Focus { label: focused } if focused == label)
        );
    }

    #[test]
    fn remote_state_authority_rejects_main_setup_other_windows_and_stale_documents() {
        let native = Native::default();
        let backend = BackendId::new();
        let target = SshTarget::parse("vm1").unwrap();
        for (label, policy) in [
            ("main", Policy::Main { backend }),
            ("setup", Policy::Setup),
            (
                "remote-1",
                Policy::Remote {
                    backend,
                    target: target.clone(),
                },
            ),
        ] {
            native.broker.register(label, policy).unwrap();
        }
        let main = native.broker.committed("main").unwrap();
        let setup = native.broker.committed("setup").unwrap();
        let remote = native.broker.committed("remote-1").unwrap();
        assert!(assigned_remote(&native, "main", &main).is_err());
        assert!(assigned_remote(&native, "setup", &setup).is_err());
        assert!(assigned_remote(&native, "remote-1", &main).is_err());
        assert_eq!(
            assigned_remote(&native, "remote-1", &remote).unwrap(),
            target
        );
        native.broker.navigating("remote-1");
        assert!(assigned_remote(&native, "remote-1", &remote).is_err());
    }

    #[test]
    fn coalesced_open_survives_its_first_requesting_document_being_replaced() {
        let native = Native::default();
        native
            .broker
            .register(
                "main",
                Policy::Main {
                    backend: BackendId::new(),
                },
            )
            .unwrap();
        let remotes = Arc::new(Remotes::default());
        let first = native.broker.committed("main").unwrap();
        let first_request = remotes.requester(&native, "main", first.clone());
        let attempt = match remotes.lock().begin("vm1", &first_request.key) {
            Begin::Start { attempt, .. } => attempt,
            _ => panic!("first open"),
        };
        native.broker.navigating("main");
        let second = native.broker.committed("main").unwrap();
        let second_request = remotes.requester(&native, "main", second.clone());
        assert!(matches!(
            remotes.lock().begin("vm1", &second_request.key),
            Begin::Wait { .. }
        ));
        let current = remotes.current_requesters(&native, "vm1", attempt);
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].0, second_request.key);
        assert!(native
            .broker
            .with_authority("main", &first, || remotes
                .lock()
                .mark_committed("vm1", attempt))
            .is_err());
        assert!(native
            .broker
            .with_authority("main", &second, || remotes
                .lock()
                .mark_committed("vm1", attempt))
            .unwrap());
        drop(first_request);
        assert_eq!(remotes.current_requesters(&native, "vm1", attempt).len(), 1);
        drop(second_request);
        assert!(remotes
            .current_requesters(&native, "vm1", attempt)
            .is_empty());
    }

    #[test]
    fn cancellation_and_commit_have_one_native_order() {
        let mut registry = Registry::default();
        let (attempt, _) = start(&mut registry, "vm1");
        assert!(registry.cancel("vm1"));
        assert!(!registry.mark_committed("vm1", attempt));
        let (next, _) = start(&mut registry, "vm1");
        assert!(registry.mark_committed("vm1", next));
        assert!(!registry.cancel("vm1"));
        assert!(!registry.mark_committed("vm1", attempt));
    }

    #[test]
    fn blocking_read_capacity_follows_the_worker_owned_permit() {
        let remotes = Remotes::default();
        let mut workers = Vec::new();
        for _ in 0..splitty_transport::admission::DOMAIN_OPENING {
            workers.push(remotes.admit_read().unwrap());
        }
        assert!(remotes.admit_read().is_err());
        drop(workers.pop());
        workers.push(remotes.admit_read().unwrap());
        assert!(remotes.admit_read().is_err());
        drop(workers);
        assert!(remotes.admit_read().is_ok());
    }

    #[test]
    fn titles_name_the_target_and_the_connection() {
        assert_eq!(title("vm1", Connection::Connected), "vm1 — Splitty");
        assert_eq!(
            title("me@vm1:2222", Connection::Reconnecting),
            "me@vm1:2222 (reconnecting…) — Splitty"
        );
        assert_eq!(
            title("vm1", Connection::Disconnected),
            "vm1 (disconnected) — Splitty"
        );
    }

    #[test]
    fn a_new_target_gets_a_new_window() {
        let mut registry = Registry::default();
        let (attempt, outcome) = start(&mut registry, "vm1");
        assert_eq!(outcome_of(&outcome), None);
        assert!(matches!(
            registry.commit("vm1", attempt),
            Some(Commit::New { label }) if label == "remote-1"
        ));
        registry.finish("vm1", attempt, Ok(("remote-1".to_owned(), link())));
        assert_eq!(outcome_of(&outcome), Some(Ok(())));
    }

    #[test]
    fn a_connected_window_is_focused() {
        let mut registry = Registry::default();
        let label = open(&mut registry, "vm1");
        assert!(matches!(
            registry.begin("vm1", "main"),
            Begin::Focus { label: focused } if focused == label
        ));
    }

    /// One window per target, but each target its own.
    #[test]
    fn targets_get_windows_of_their_own() {
        let mut registry = Registry::default();
        assert_eq!(open(&mut registry, "vm1"), "remote-1");
        assert_eq!(open(&mut registry, "vm2"), "remote-2");
    }

    #[test]
    fn concurrent_opens_share_one_attempt() {
        let mut registry = Registry::default();
        let (attempt, first) = start(&mut registry, "vm1");
        let Begin::Wait { outcome: second } = registry.begin("vm1", "launcher") else {
            panic!("expected to wait for the attempt in flight");
        };
        assert_eq!(registry.callers("vm1", attempt), ["main", "launcher"]);

        let failed = failure("ssh: connect to host vm1 port 22: Connection refused");
        registry.finish("vm1", attempt, Err(failed.clone()));
        assert_eq!(outcome_of(&first), Some(Err(failed.clone())));
        assert_eq!(outcome_of(&second), Some(Err(failed)));
    }

    /// A failed open leaves no window behind, so the next one starts afresh.
    #[test]
    fn a_failed_open_can_be_retried() {
        let mut registry = Registry::default();
        let (attempt, _) = start(&mut registry, "vm1");
        registry.finish("vm1", attempt, Err(failure("Permission denied")));
        let (retry, _) = start(&mut registry, "vm1");
        assert_ne!(retry, attempt);
    }

    #[test]
    fn a_window_that_gave_up_is_reconnected_in_place() {
        let mut registry = Registry::default();
        let label = open(&mut registry, "vm1");
        registry.disconnected(&label);

        let (attempt, outcome) = start(&mut registry, "vm1");
        let Some(Commit::Reuse {
            label: reused,
            link,
        }) = registry.commit("vm1", attempt)
        else {
            panic!("expected the window of vm1");
        };
        assert_eq!(reused, label);
        registry.finish("vm1", attempt, Ok((reused, link)));
        assert_eq!(outcome_of(&outcome), Some(Ok(())));
        assert!(matches!(
            registry.begin("vm1", "main"),
            Begin::Focus { label: focused } if focused == label
        ));
    }

    /// A failed reconnect leaves the window as it was: disconnected.
    #[test]
    fn a_failed_reconnect_keeps_the_window() {
        let mut registry = Registry::default();
        let label = open(&mut registry, "vm1");
        registry.disconnected(&label);
        let (attempt, _) = start(&mut registry, "vm1");
        registry.finish("vm1", attempt, Err(failure("Connection timed out")));

        let (attempt, _) = start(&mut registry, "vm1");
        assert!(matches!(
            registry.commit("vm1", attempt),
            Some(Commit::Reuse { label: reused, .. }) if reused == label
        ));
    }

    #[test]
    fn cancel_rejects_every_open_waiting() {
        let mut registry = Registry::default();
        let (attempt, first) = start(&mut registry, "vm1");
        let Begin::Wait { outcome: second } = registry.begin("vm1", "main") else {
            panic!("expected to wait for the attempt in flight");
        };

        assert!(registry.cancel("vm1"));
        assert_eq!(outcome_of(&first), Some(Err(cancelled())));
        assert_eq!(outcome_of(&second), Some(Err(cancelled())));
        // The attempt's task finds it gone and shows nothing.
        assert!(registry.commit("vm1", attempt).is_none());
        assert!(registry.callers("vm1", attempt).is_empty());
        // Nothing to cancel any more.
        assert!(!registry.cancel("vm1"));
    }

    /// A late result of a cancelled attempt changes nothing.
    #[test]
    fn a_cancelled_attempt_cannot_finish() {
        let mut registry = Registry::default();
        let (cancelled_attempt, _) = start(&mut registry, "vm1");
        registry.cancel("vm1");
        let (attempt, outcome) = start(&mut registry, "vm1");

        registry.finish(
            "vm1",
            cancelled_attempt,
            Ok(("remote-9".to_owned(), link())),
        );
        assert_eq!(outcome_of(&outcome), None);
        assert!(registry.commit("vm1", attempt).is_some());
    }

    /// Once the window is being shown, the open succeeds.
    #[test]
    fn a_committed_attempt_is_not_cancelled() {
        let mut registry = Registry::default();
        let (attempt, outcome) = start(&mut registry, "vm1");
        let Some(Commit::New { label }) = registry.commit("vm1", attempt) else {
            panic!("expected a new window");
        };
        assert!(!registry.cancel("vm1"));
        registry.finish("vm1", attempt, Ok((label, link())));
        assert_eq!(outcome_of(&outcome), Some(Ok(())));
    }

    #[test]
    fn a_closed_window_is_forgotten() {
        let mut registry = Registry::default();
        let label = open(&mut registry, "vm1");
        registry.closed(&label);

        let (attempt, _) = start(&mut registry, "vm1");
        assert!(matches!(
            registry.commit("vm1", attempt),
            Some(Commit::New { label: new }) if new != label
        ));
    }

    #[test]
    fn progress_carries_the_step_as_connect_json_names_it() {
        let step = RemoteProgress::new(
            "vm1",
            Progress::Step {
                step: Step::Install,
                message: "Installing Splitty 0.1.0 on vm1 (x86_64)...".to_owned(),
            },
        );
        assert_eq!(
            serde_json::to_value(step).ok(),
            Some(serde_json::json!({
                "target": "vm1",
                "step": "install",
                "message": "Installing Splitty 0.1.0 on vm1 (x86_64)...",
            }))
        );
        let notice = RemoteProgress::new(
            "vm1",
            Progress::Notice {
                message: "The daemon on vm1 runs Splitty 0.0.9".to_owned(),
            },
        );
        assert_eq!(
            serde_json::to_value(notice)
                .ok()
                .and_then(|v| v.get("step").cloned()),
            Some(serde_json::json!("notice"))
        );
    }

    /// The rejection the trusted UI gets.
    #[test]
    fn a_rejection_is_a_message_with_an_optional_hint() {
        assert_eq!(
            serde_json::to_value(cancelled()).ok(),
            Some(serde_json::json!({ "message": "cancelled" }))
        );
    }
    #[tokio::test]
    async fn remote_outcome_waiters_share_diagnostic_lease_through_last_release() {
        let budget = splitty_transport::budget::ConnectionBudget::default();
        let charged = Arc::new(
            splitty_remote::tunnel::Failure::from_error_budgeted(
                &"shared sensitive failure",
                &budget,
            )
            .unwrap(),
        );
        let mut registry = Registry::default();
        let (attempt, first) = start(&mut registry, "vm1");
        let Begin::Wait { outcome: second } = registry.begin("vm1", "other") else {
            panic!("not shared");
        };
        registry.finish("vm1", attempt, Err(Failure::Shared(charged.clone())));
        let Err(Failure::Shared(one)) = outcome_of(&first).unwrap() else {
            panic!("missing failure");
        };
        let Err(Failure::Shared(two)) = outcome_of(&second).unwrap() else {
            panic!("missing failure");
        };
        assert!(Arc::ptr_eq(&charged, &one));
        assert!(Arc::ptr_eq(&one, &two));
        drop(charged);
        drop(first);
        drop(second);
        drop(registry);
        drop(one);
        tokio::task::yield_now().await;
        assert!(budget.used() > 0);
        assert_eq!(
            serde_json::to_value(Failure::Shared(two.clone())).unwrap(),
            serde_json::json!({"message":"shared sensitive failure"})
        );
        drop(two);
        assert_eq!(budget.used(), 0);
    }
    #[tokio::test]
    async fn duplicate_remote_open_flood_is_admitted_before_waiter_allocation_and_released_on_abort(
    ) {
        let native = Native::default();
        native
            .broker
            .register(
                "main",
                crate::client_ipc::Policy::Main {
                    backend: BackendId::new(),
                },
            )
            .unwrap();
        let proof = native.broker.committed("main").unwrap();
        let remotes = Arc::new(Remotes {
            request_process: splitty_transport::budget::Budget::new(
                splitty_transport::admission::OPENING,
            ),
            ..Remotes::default()
        });
        let mut held = Vec::new();
        let mut rejected = 0;
        let mut attempt_id = 0;
        for _ in 0..128 {
            let Ok(admission) = remotes.admit_request(&native, "main", &proof) else {
                rejected += 1;
                continue;
            };
            let request = remotes.requester(&native, "main", proof.clone());
            let begin = remotes
                .lock()
                .begin("vm1", &format!("main:{}", proof.document));
            let outcome = match begin {
                Begin::Start { attempt, outcome } => {
                    attempt_id = attempt;
                    outcome
                }
                Begin::Wait { outcome } => outcome,
                Begin::Focus { .. } | Begin::Rejected => panic!("unexpected window or update"),
            };
            held.push((admission, request, outcome));
        }
        assert_eq!(held.len(), splitty_transport::admission::DOMAIN_OPENING);
        assert_eq!(rejected, 124);
        assert_eq!(
            remotes
                .lock()
                .attempt("vm1", attempt_id)
                .unwrap()
                .outcome
                .receiver_count(),
            4
        );
        assert_eq!(
            remotes
                .requesters
                .lock()
                .unwrap()
                .values()
                .next()
                .unwrap()
                .references,
            4
        );
        assert_eq!(remotes.request_process.used(), 4);
        assert_eq!(
            remotes.opening.used(),
            0,
            "waiter admission does not replace independent connector cleanup admission"
        );
        let waiting = held.pop().unwrap();
        let (started, ready) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let (_admission, _request, mut outcome) = waiting;
            started.send(()).unwrap();
            settled(&mut outcome).await
        });
        ready.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(remotes.request_process.used(), 3);
        assert_eq!(
            remotes
                .lock()
                .attempt("vm1", attempt_id)
                .unwrap()
                .outcome
                .receiver_count(),
            3
        );
        let replacement = remotes.admit_request(&native, "main", &proof).unwrap();
        assert!(remotes.admit_request(&native, "main", &proof).is_err());
        drop(replacement);
        drop(held);
        assert_eq!(remotes.request_process.used(), 0);
        assert!(remotes.requesters.lock().unwrap().is_empty());
        assert_eq!(
            remotes
                .lock()
                .attempt("vm1", attempt_id)
                .unwrap()
                .outcome
                .receiver_count(),
            0
        );
        assert!(remotes.lock().callers("vm1", attempt_id).is_empty());
        for _ in 0..64 {
            native.broker.navigating("main");
            let proof = native.broker.committed("main").unwrap();
            let admission = remotes.admit_request(&native, "main", &proof).unwrap();
            let request = remotes.requester(&native, "main", proof.clone());
            let Begin::Wait { outcome } = remotes
                .lock()
                .begin("vm1", &format!("main:{}", proof.document))
            else {
                panic!("lost shared attempt");
            };
            assert_eq!(remotes.lock().callers("vm1", attempt_id).len(), 1);
            drop(outcome);
            drop(request);
            drop(admission);
            assert!(
                remotes.lock().callers("vm1", attempt_id).is_empty(),
                "completed commands must not accumulate caller keys"
            );
        }
    }

    #[test]
    fn remote_request_admission_is_document_bound_and_process_bounded_across_reload() {
        let native = Native::default();
        for window in ["main", "other", "third", "fourth"] {
            native
                .broker
                .register(
                    window,
                    crate::client_ipc::Policy::Main {
                        backend: BackendId::new(),
                    },
                )
                .unwrap();
        }
        let remotes = Remotes {
            request_process: splitty_transport::budget::Budget::new(
                splitty_transport::admission::OPENING,
            ),
            ..Remotes::default()
        };
        let old = native.broker.committed("main").unwrap();
        let old_commands: Vec<_> = (0..4)
            .map(|_| remotes.admit_request(&native, "main", &old).unwrap())
            .collect();
        native.broker.navigating("main");
        let current = native.broker.committed("main").unwrap();
        assert!(remotes.admit_request(&native, "main", &old).is_err());
        let mut commands: Vec<_> = (0..4)
            .map(|_| remotes.admit_request(&native, "main", &current).unwrap())
            .collect();
        for window in ["other", "third"] {
            let proof = native.broker.committed(window).unwrap();
            commands
                .extend((0..4).map(|_| remotes.admit_request(&native, window, &proof).unwrap()));
        }
        assert_eq!(
            remotes.request_process.used(),
            16,
            "revocation retains old command permits until actual drop"
        );
        let fourth = native.broker.committed("fourth").unwrap();
        assert!(remotes.admit_request(&native, "fourth", &fourth).is_err());
        drop(old_commands);
        let available: Vec<_> = (0..4)
            .map(|_| remotes.admit_request(&native, "fourth", &fourth).unwrap())
            .collect();
        assert!(remotes.admit_request(&native, "fourth", &fourth).is_err());
        assert_eq!(remotes.request_process.used(), 16);
        drop(available);
        drop(commands);
        assert_eq!(remotes.request_process.used(), 0);
        for _ in 0..100 {
            native.broker.navigating("main");
            let proof = native.broker.committed("main").unwrap();
            let command = remotes.admit_request(&native, "main", &proof).unwrap();
            assert_eq!(
                remotes.request_documents.lock().unwrap().len(),
                1,
                "expired document bookkeeping must not grow with reloads"
            );
            drop(command);
        }
        assert_eq!(remotes.request_process.used(), 0);
    }
}
