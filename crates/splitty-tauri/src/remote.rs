//! Remote windows: a host's Splitty UI in a window of its own, through an SSH
//! tunnel this app holds (`desktop-remote`).
//!
//! The trusted UI (the main window on its backend's origin, or the Windows
//! setup page) calls `remote_open`. The flow of `splitty connect` runs here,
//! in the native process, and reports its steps to the caller; only once the
//! host's UI is ready does a window open, straight onto the single-use session
//! URL (D1 of remote-window, provisional). So no window that runs host code
//! ever showed a local page.
//!
//! A remote window has no native rights (`access-control`, Remote Window
//! Isolation): its label `remote-<n>` is in no capability, its cookie store is
//! its own and in memory only, and `navigation` pins it to its tunnel origin.
//! Its tunnel lives exactly as long as the window: the task that opened it
//! goes on to supervise the tunnel until the window is destroyed, and ends it
//! then (the `ssh` is also killed when the task is dropped, as on the app's
//! exit). The host's daemon and sessions keep running.
//!
//! The [`Registry`] keeps one entry per target: its window, and the open in
//! flight, which concurrent opens of that target share.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::Duration;

use serde::Serialize;
use splitty_remote::hosts::{self, KnownHost};
use splitty_remote::install;
use splitty_remote::ssh::{Ssh, SshTarget};
use splitty_remote::tunnel::{
    self, Connected, Event, Failure, Progress, SshRemote, SshTunnel, Step,
};
use tauri::async_runtime::JoinHandle;
use tauri::{
    AppHandle, Emitter, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
use tokio::sync::watch;

use crate::navigation;

/// The server the host gets: this app's own version.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The event that carries an open's progress to the window that asked.
const PROGRESS_EVENT: &str = "remote-progress";

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
fn failure(message: impl Into<String>) -> Failure {
    Failure {
        message: message.into(),
        hint: None,
    }
}

/// What a cancelled open rejects with.
fn cancelled() -> Failure {
    failure("cancelled")
}

/// The connection a remote window's title reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Connection {
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
    /// The origin the window may show, which moves with a moved daemon.
    origin: Arc<RwLock<String>>,
    /// Turns `true` once the window is destroyed.
    closed: watch::Receiver<bool>,
}

impl WindowLink {
    fn origin(&self) -> String {
        self.origin
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_origin(&self, origin: String) {
        *self.origin.write().unwrap_or_else(PoisonError::into_inner) = origin;
    }
}

struct HostWindow {
    label: String,
    /// Whether its supervisor still keeps the tunnel, possibly rebuilding it.
    connected: bool,
    link: WindowLink,
}

struct Attempt {
    id: u64,
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
    /// Its window is open and connected: focus it.
    Focus { label: String },
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
        let host = self.hosts.entry(target.to_owned()).or_default();
        if let Some(attempt) = &mut host.attempt {
            if !attempt.callers.iter().any(|known| known == caller) {
                attempt.callers.push(caller.to_owned());
            }
            return Begin::Wait {
                outcome: attempt.outcome.subscribe(),
            };
        }
        if let Some(window) = host.window.as_ref().filter(|window| window.connected) {
            return Begin::Focus {
                label: window.label.clone(),
            };
        }
        self.attempts += 1;
        let (sender, outcome) = watch::channel(None);
        host.attempt = Some(Attempt {
            id: self.attempts,
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

    /// The supervisor of the window `label` gave up.
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
#[derive(Default)]
pub struct Remotes {
    registry: Mutex<Registry>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl Remotes {
    fn lock(&self) -> MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(PoisonError::into_inner)
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
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(untagged)]
enum ProgressStep {
    Step(Step),
    Other(&'static str),
}

/// The payload of [`PROGRESS_EVENT`].
#[derive(Debug, Clone, Serialize)]
struct RemoteProgress {
    target: String,
    step: ProgressStep,
    message: String,
}

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
pub async fn remote_hosts(app: AppHandle) -> Result<Vec<KnownHost>, Failure> {
    let config_dir = config_dir(&app).map_err(failure)?;
    tauri::async_runtime::spawn_blocking(move || {
        hosts::known(&config_dir, hosts::default_ssh_dir().as_deref())
    })
    .await
    .map_err(|error| failure(format!("could not read the known hosts: {error}")))
}

/// Open `target` in a remote window. Resolves once the window shows the host.
#[tauri::command]
pub async fn remote_open(
    app: AppHandle,
    webview: tauri::Webview,
    remotes: tauri::State<'_, Arc<Remotes>>,
    target: String,
) -> Result<(), Failure> {
    // Before anything runs, as `splitty connect` does (`access-control`).
    let target = SshTarget::parse(&target).map_err(|error| failure(error.to_string()))?;
    let remotes = Arc::clone(remotes.inner());
    loop {
        let begin = remotes.lock().begin(target.as_str(), webview.label());
        match begin {
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
                remotes.spawn(connect(
                    app.clone(),
                    Arc::clone(&remotes),
                    target,
                    attempt,
                    outcome.clone(),
                ));
                return settled(&mut outcome).await;
            }
        }
    }
}

/// Cancel the open of `target` in flight; it then rejects with `cancelled`.
#[tauri::command]
pub fn remote_cancel(remotes: tauri::State<'_, Arc<Remotes>>, target: String) {
    // An invalid target cannot have an open in flight.
    if let Ok(target) = SshTarget::parse(&target) {
        remotes.lock().cancel(target.as_str());
    }
}

/// The task of one attempt: run the flow, show the host, then keep its tunnel
/// for as long as the window lives.
async fn connect(
    app: AppHandle,
    remotes: Arc<Remotes>,
    target: SshTarget,
    attempt: u64,
    mut outcome: watch::Receiver<Option<Outcome>>,
) {
    let host = target.as_str().to_owned();
    let _unsettled = Unsettled {
        remotes: Arc::clone(&remotes),
        target: host.clone(),
        attempt,
    };

    let on_progress = |progress: Progress| {
        let progress = RemoteProgress::new(&host, progress);
        let callers = remotes.lock().callers(&host, attempt);
        for caller in callers {
            if let Err(error) = app.emit_to(caller.as_str(), PROGRESS_EVENT, progress.clone()) {
                tracing::warn!("could not report the progress of {host}: {error}");
            }
        }
    };
    let flow = tunnel::open(
        Ssh::new(target.clone()),
        VERSION,
        install::payload_for,
        on_progress,
    );
    // A cancel settles the attempt; dropping the flow then kills every `ssh`
    // it started.
    let (remote, connected) = tokio::select! {
        result = flow => match result {
            Ok(opened) => opened,
            Err(error) => {
                tracing::warn!("could not connect to {host}: {error}");
                remotes.lock().finish(&host, attempt, Err(error.failure()));
                return;
            }
        },
        _ = settled(&mut outcome) => return,
    };

    // Dropping `connected` on the way out closes the tunnel.
    let Some(commit) = remotes.lock().commit(&host, attempt) else {
        return;
    };
    record_recent(&app, &target);
    let (label, link) = match show(&app, &remotes, commit, &host, &connected.url) {
        Ok(shown) => shown,
        Err(failure) => {
            remotes.lock().finish(&host, attempt, Err(failure));
            return;
        }
    };
    remotes
        .lock()
        .finish(&host, attempt, Ok((label.clone(), link.clone())));
    keep(&app, &remotes, &host, &label, &link, &remote, connected).await;
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

/// Show the host's session `url`: in its window when that is still open
/// (a reconnect after giving up), otherwise in a new one.
fn show(
    app: &AppHandle,
    remotes: &Arc<Remotes>,
    commit: Commit,
    target: &str,
    url: &str,
) -> Result<(String, WindowLink), Failure> {
    let url: Url = url
        .parse()
        .map_err(|_| failure("The host returned an invalid session URL."))?;
    let label = match commit {
        Commit::Reuse { label, link } => {
            if let Some(window) = app.get_webview_window(&label) {
                load(&window, &link, url);
                set_title(&window, target, Connection::Connected);
                focus(&window);
                return Ok((label, link));
            }
            remotes.lock().closed(&label);
            remotes.lock().new_label()
        }
        Commit::New { label } => label,
    };
    build(app, remotes, &label, target, url)
        .map(|link| (label, link))
        .map_err(|error| failure(format!("Could not open a window for {target}: {error}")))
}

/// A new remote window `label` on the session `url` of `target`.
fn build(
    app: &AppHandle,
    remotes: &Arc<Remotes>,
    label: &str,
    target: &str,
    url: Url,
) -> tauri::Result<WindowLink> {
    let (closed_sender, closed) = watch::channel(false);
    let link = WindowLink {
        origin: Arc::new(RwLock::new(navigation::tunnel_origin(&url))),
        closed,
    };
    let navigation_app = app.clone();
    let navigation_link = link.clone();
    let new_window_app = app.clone();
    let new_window_link = link.clone();

    // Native decorations: the UI's own title bar needs window commands, which
    // a remote window does not get.
    let builder = WebviewWindowBuilder::new(app, label, WebviewUrl::External(url))
        .title(title(target, Connection::Connected))
        .inner_size(1200.0, 800.0)
        // A cookie store of its own, in memory only: the host's session
        // cookie reaches no other server, and no other cookie reaches it.
        .incognito(true)
        .disable_drag_drop_handler()
        .initialization_script(NO_LEAVE_PROMPT)
        .on_navigation(move |url| {
            let decision = navigation::remote_navigation(url, &navigation_link.origin());
            navigation::follow(&navigation_app, url, decision)
        })
        .on_new_window(move |url, _features| {
            let decision = navigation::remote_new_window(&url, &new_window_link.origin());
            navigation::follow_new_window(&new_window_app, &url, decision)
        })
        // A host's UI saves nothing on this computer: file offers are taken by
        // the hub's window (file-transfer D10).
        .on_download(|_webview, _event| false);
    #[cfg(target_os = "windows")]
    let builder = builder.data_directory(webview_data_dir(app, label)?);
    let window = builder.build()?;

    let remotes = Arc::clone(remotes);
    let window_label = label.to_owned();
    window.on_window_event(move |event| {
        if let WindowEvent::Destroyed = event {
            closed_sender.send_replace(true);
            remotes.lock().closed(&window_label);
        }
    });
    Ok(link)
}

/// A WebView2 user data folder per remote window (D13 of remote-window,
/// provisional). WebView2 keeps one InPrivate session per user data folder
/// and profile, shared by every InPrivate webview in it, so `incognito` alone
/// would let two hosts' windows share their cookies. Labels repeat across
/// runs; the folder keeps no browsing data, since the webview is InPrivate.
#[cfg(target_os = "windows")]
fn webview_data_dir(app: &AppHandle, label: &str) -> tauri::Result<PathBuf> {
    Ok(app
        .path()
        .app_local_data_dir()?
        .join("remote-windows")
        .join(label))
}

/// Keep the tunnel of the window `label` until the window is destroyed.
async fn keep(
    app: &AppHandle,
    remotes: &Remotes,
    target: &str,
    label: &str,
    link: &WindowLink,
    remote: &SshRemote,
    connected: Connected<SshTunnel>,
) {
    let mut closed = link.closed.clone();
    let shutdown = async move {
        let _ = closed.wait_for(|gone| *gone).await;
    };
    let on_event = |event: Event| {
        let window = app.get_webview_window(label);
        let connection = match event {
            Event::Lost { reason } => {
                tracing::info!("the tunnel to {target} was lost ({reason}), reconnecting");
                Connection::Reconnecting
            }
            Event::RetryFailed { error, retry_in } => {
                tracing::info!(
                    "reconnecting to {target} failed, next attempt in {} s: {error}",
                    retry_in.as_secs()
                );
                Connection::Reconnecting
            }
            Event::Restored => Connection::Connected,
            // The daemon was restarted or moved: the window's session is gone.
            Event::NewUrl { url } => {
                match (url.parse::<Url>(), &window) {
                    (Ok(url), Some(window)) => load(window, link, url),
                    (Err(error), _) => {
                        tracing::warn!("{target} sent an invalid session URL: {error}")
                    }
                    (Ok(_), None) => {}
                }
                Connection::Connected
            }
        };
        if let Some(window) = &window {
            set_title(window, target, connection);
        }
    };
    let supervised =
        tunnel::supervise(remote, connected, tunnel::GIVE_UP_AFTER, shutdown, on_event).await;
    if let Err(error) = supervised {
        // The window stays, showing the host's UI in its own disconnected
        // state; opening the target again reconnects it (D11 of
        // remote-window, provisional).
        tracing::warn!("{error}");
        remotes.lock().disconnected(label);
        if let Some(window) = app.get_webview_window(label) {
            set_title(&window, target, Connection::Disconnected);
        }
    }
}

/// Load a new session `url` into `window`, admitting its origin first.
fn load(window: &WebviewWindow, link: &WindowLink, url: Url) {
    link.set_origin(navigation::tunnel_origin(&url));
    if let Err(error) = window.navigate(url) {
        tracing::warn!("could not load the new session: {error}");
    }
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

    fn link() -> WindowLink {
        let (_closed, closed) = watch::channel(false);
        WindowLink {
            origin: Arc::new(RwLock::new("http://127.0.0.1:9800".to_owned())),
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
}
