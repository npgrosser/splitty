//! The remote screen hub: `/ws/remote` gives the window a WebSocket to a
//! host's daemon, so local and remote screens share one window.
//!
//! For each socket the hub reaches the host as `splitty connect` does
//! (install, then the handshake with `--ensure-daemon`), checks the daemon's
//! protocol version, starts `splitty remote-bridge` there over SSH and relays
//! the daemon's messages both ways, text as text and binary as binary, without
//! reading them. Until the relay starts it speaks for itself: `remote.progress`
//! per step, then exactly one `remote.ready` or a `remote.error`
//! (`docs/protocol.ts`). Afterwards it sends nothing of its own, and when the
//! host side ends it closes the socket with [`HOST_LOST`].
//!
//! With `update=1`, a daemon of another version than this server's is stopped
//! before the bridge starts, and the handshake starts this version instead.
//! That ends the daemon's sessions, so a client asks the user first.
//!
//! Every socket has `ssh` processes of its own; closing the socket ends them,
//! and the host's daemon keeps running. Nothing goes to the host but the
//! commands of the install, the handshake, the update's `daemon stop` and the
//! bridge and the client's relayed messages: no cookie, ticket or token.
//!
//! The hub also keeps the client's list of remote screens across restarts
//! (`remembered`), their last observed directories (`catalog`), forwards
//! ports of hosts on request (`forwards`) and shares ports of this machine
//! with hosts on request (`reverse`), each with an `ssh` of its own that
//! lives until the forward is stopped.

mod catalog;
mod connector;
mod failure;
mod forwards;
mod native;
mod preferences;
mod remembered;
mod reverse;
#[cfg(all(test, unix))]
pub(crate) mod testing;

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use std::{collections::HashMap, io, path::PathBuf};

use axum::extract::ws::{close_code, CloseFrame, Message, WebSocket};
use futures::future::BoxFuture;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use serde::Serialize;
use splitty_core::protocol::PROTOCOL_VERSION;
use splitty_remote::bridge::{Ended, NativeBridge};
use splitty_remote::handshake::{Handshake, RunningDaemon, Screen};
use splitty_remote::hosts::{self, HostPreferences, KnownHost};
pub use splitty_remote::hosts::{HostOrder, HostPreference, PreferenceError};
use splitty_remote::install::{Prepared, Server};
use splitty_remote::ssh::{self, SshError, SshTarget};
use splitty_remote::tunnel::PortReservation;
use splitty_remote::tunnel::{update_plan, UpdatePlan, PREPARE_TIMEOUT};
use splitty_transport::{
    admission,
    budget::{Charged, ConnectionBudget},
    peer::Incoming,
    wire::{self, Kind},
};
use tokio::sync::mpsc;

pub use catalog::{Catalog, CatalogError, HostCatalog};
pub use connector::SshConnector;
pub use failure::{compatible, Code, Failure, HubError};
pub use forwards::{ForwardError, ForwardInfo, ForwardState, Forwards, MAX_PER_HOST};
pub use preferences::{Preferences, SavedForward};
pub use remembered::{RememberError, Remembered};
pub use reverse::{ReverseInfo, ReversePreferences, SavedReverse};

/// This server's version: the one it installs on hosts, and the one
/// `update=1` brings a host's daemon to.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The close code of a relaying socket whose host side ended. The client
/// treats it like a dropped connection and reconnects.
pub const HOST_LOST: u16 = 4001;

/// The largest message a client may send: what one bridge frame carries.
pub const MAX_MESSAGE: usize = wire::MAX_FRAME_BYTES - wire::HEADER_BYTES - wire::DELIVERY_BYTES;

/// How many messages a client may send before `remote.ready`; they are
/// relayed after it. A client that sends more is disconnected.
const MAX_EARLY: usize = 64;

/// How long the bridge may take to deliver the daemon's first message.
const BRIDGE_TIMEOUT: Duration = Duration::from_secs(30);

/// How the hub reaches hosts: [`SshConnector`] with the system `ssh`, a
/// scripted host in tests.
pub trait Connector: Send + Sync {
    /// Make sure `target` has this client's server, installing it when this
    /// client carries one for the host. `on_install` gets the host's
    /// architecture before an upload starts.
    fn prepare<'a>(
        &'a self,
        target: &'a SshTarget,
        on_install: Box<dyn FnOnce(&str) + Send + 'a>,
        context: &'a splitty_transport::queue::Context,
    ) -> BoxFuture<'a, Result<Prepared, HubError>>;

    /// The format-2 handshake ensures a private daemon without a port hint.
    fn handshake<'a>(
        &'a self,
        target: &'a SshTarget,
        prepared: &'a Prepared,
        context: &'a splitty_transport::queue::Context,
    ) -> BoxFuture<'a, Result<Charged<Handshake>, HubError>>;

    /// Stop only an older daemon whose keepers preserve its sessions.
    fn stop_daemon<'a>(
        &'a self,
        target: &'a SshTarget,
        server: &'a Server,
        daemon: &'a RunningDaemon,
        context: &'a splitty_transport::queue::Context,
    ) -> BoxFuture<'a, Result<(), HubError>>;

    /// Start `splitty remote-bridge` on the host with `server`.
    fn bridge<'a>(
        &'a self,
        target: &'a SshTarget,
        server: &'a Server,
        daemon: &'a RunningDaemon,
        capacity: (admission::Carrier, admission::Opening),
        context: &'a splitty_transport::queue::Context,
    ) -> BoxFuture<'a, Result<NativeBridge, HubError>>;

    /// Forward `127.0.0.1:<port>` here, the port `local` holds until then, to
    /// `localhost:<remote>` on `target`, once the forward is verified.
    fn forward<'a>(
        &'a self,
        target: &'a SshTarget,
        local: PortReservation,
        remote: u16,
    ) -> BoxFuture<'a, Result<Box<dyn Forwarding>, HubError>>;

    /// Admission precedes all installation and SSH. Scripted connectors use
    /// no OS exposure; the real connector must return the durable share token.
    fn reserve_share(
        &self,
        local: u16,
    ) -> BoxFuture<'_, Result<Option<splitty_remote::tunnel::exposure::Share>, HubError>>;

    /// Share `localhost:<local>` here as `localhost:<port>` on `target`
    /// (a reverse forward), once the host's relay listens: the install's
    /// prepare, `remote-relay prepare`, then `ssh -R` with `remote-relay
    /// serve`.
    fn reverse<'a>(
        &'a self,
        target: &'a SshTarget,
        local: u16,
        port: u16,
        share: Option<&'a splitty_remote::tunnel::exposure::Share>,
    ) -> BoxFuture<'a, Result<Box<dyn Forwarding>, HubError>>;
}

/// A forward the hub holds, in either direction: an `ssh` with the system
/// `ssh`, a listener of the test in tests. Dropping it ends it.
pub trait Forwarding: Send {
    /// Resolves when the forward has ended, with why. Cancel-safe.
    fn ended(&mut self) -> BoxFuture<'_, String>;

    /// End the forward and wait until its local port is free.
    fn stop(&mut self) -> BoxFuture<'_, ()>;
}

/// The steps of `remote.progress`, in the order they can occur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Step {
    /// The server is being installed on the host.
    Install,
    /// The host is asked for its daemon (and starts a missing one).
    Daemon,
    /// With `update=1`: the host's daemon runs another version and is
    /// stopped, so that this version starts.
    Update,
    /// The host started its daemon for this socket.
    Started,
    /// The bridge is being opened.
    Bridge,
}

/// One `remote.progress`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Progress {
    step: Step,
    message: String,
}

/// The hub's own messages, before the relay.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum HubMessage<'a> {
    #[serde(rename = "remote.progress")]
    Progress { step: Step, message: &'a str },
    #[serde(rename = "remote.ready")]
    Ready {
        target: &'a str,
        daemon: ReadyDaemon<'a>,
        screens: &'a [Screen],
    },
    #[serde(rename = "remote.error")]
    Error(&'a Failure),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadyDaemon<'a> {
    version: &'a str,
    protocol: u32,
    /// Whether its sessions outlive it, so an update ends none.
    keeps_sessions: bool,
    /// Whether this socket's handshake started it.
    started: bool,
}

/// A socket that reached the host: its bridge delivered the daemon's first
/// message.
struct Reached {
    target: SshTarget,
    bridge: NativeBridge,
    first: Charged<Incoming>,
    daemon: RunningDaemon,
    screens: Vec<Screen>,
    _metadata: splitty_transport::budget::Lease,
}

/// What became of a socket before its relay.
enum Setup {
    Reached(Box<Reached>),
    Failed(HubError),
    /// The client closed the socket, or it broke.
    Gone,
}

/// How a relay ended.
enum End {
    /// The bridge's output ended, or the bridge stopped taking input.
    Host,
    /// The client closed the socket, or it broke.
    Client,
}

type Sink = SplitSink<WebSocket, Message>;
type Stream = SplitStream<WebSocket>;

/// The hub of one server.
pub struct Hub {
    connector: Arc<dyn Connector>,
    /// The configuration directory, where the recent hosts and the
    /// remembered screens and host directories are kept.
    config_dir: PathBuf,
    /// The user's SSH directory, for the configured hosts.
    ssh_dir: Option<PathBuf>,
    /// Held while the remembered screens are written, so the list a `PUT`
    /// answers with is the one in the file.
    remembering: Mutex<()>,
    forwards: forwards::Forwards,
    reverses: reverse::Reverses,
    domain: Arc<admission::Domain>,
    backends: Mutex<HashMap<String, Weak<admission::Backend>>>,
}

impl Hub {
    pub fn new(
        connector: Box<dyn Connector>,
        config_dir: PathBuf,
        ssh_dir: Option<PathBuf>,
    ) -> Self {
        let connector: Arc<dyn Connector> = Arc::from(connector);
        Self {
            forwards: forwards::Forwards::new(connector.clone(), config_dir.clone()),
            reverses: reverse::Reverses::new(connector.clone(), config_dir.clone()),
            connector,
            config_dir,
            ssh_dir,
            remembering: Mutex::new(()),
            domain: admission::Domain::new(),
            backends: Mutex::default(),
        }
    }

    /// The hub of the user the server runs as: the system `ssh`, the
    /// configuration directory and `~/.ssh`.
    pub fn system() -> Self {
        Self::new(
            Box::new(SshConnector),
            splitty_core::config::config_dir(),
            hosts::default_ssh_dir(),
        )
    }

    /// The known hosts, for `GET /api/remote/hosts`.
    pub fn known_hosts(&self) -> Vec<KnownHost> {
        hosts::known(&self.config_dir, self.ssh_dir.as_deref())
    }

    pub fn host_preferences(&self) -> HostPreferences {
        hosts::preferences(&self.config_dir)
    }

    pub fn update_host_preference(
        &self,
        document: serde_json::Value,
    ) -> Result<HostPreference, PreferenceError> {
        hosts::update_preference(&self.config_dir, document)
    }

    /// The order of the client's host groups, for `GET /api/remote/host-order`.
    pub fn host_order(&self) -> HostOrder {
        hosts::host_order(&self.config_dir)
    }

    /// Replace the order of the host groups, for `PUT /api/remote/host-order`.
    pub fn set_host_order(
        &self,
        document: serde_json::Value,
    ) -> Result<HostOrder, PreferenceError> {
        hosts::set_host_order(&self.config_dir, document)
    }

    /// Forget metadata only: running connections, screens and forwards remain.
    pub fn forget_host(&self, document: serde_json::Value) -> Result<(), PreferenceError> {
        let object = document
            .as_object()
            .ok_or_else(|| PreferenceError::Invalid("expected a target object".into()))?;
        if object.len() != 1 {
            return Err(PreferenceError::Invalid("only target is accepted".into()));
        }
        let target = hosts::checked_target(object.get("target"))?;
        let _writing = hosts::lock(&self.config_dir)?;
        // Tombstone first: a partial failure cannot resurrect the host through
        // a cached directory or the user's unchanged SSH configuration.
        hosts::forget_locked(&self.config_dir, &target)?;
        catalog::forget(&self.config_dir, &target)?;
        Ok(())
    }

    /// The remembered remote screens, for `GET /api/remote/screens`.
    pub fn remembered_screens(&self) -> Remembered {
        remembered::read(&self.config_dir)
    }

    /// Replace the remembered remote screens with the list a client sent,
    /// for `PUT /api/remote/screens`, and return what is stored.
    pub fn remember_screens(
        &self,
        document: serde_json::Value,
    ) -> Result<Remembered, RememberError> {
        let remembered = remembered::check(document)?;
        // A poisoned lock only means that another write panicked; the rename
        // leaves the file whole either way.
        let _writing = self
            .remembering
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        remembered::write(&self.config_dir, &remembered)?;
        Ok(remembered)
    }

    /// The last observed host directories, for `GET /api/remote/catalog`.
    pub fn catalog(&self) -> Catalog {
        catalog::read(&self.config_dir)
    }

    /// Replace only one host's directory, for `PUT /api/remote/catalog`.
    /// Both catalog methods only access local metadata, never the connector.
    pub fn update_catalog(&self, document: serde_json::Value) -> Result<HostCatalog, CatalogError> {
        let update = catalog::check(document)?;
        let _writing = hosts::lock(&self.config_dir)?;
        catalog::update(&self.config_dir, update)
    }

    /// Open a forward of `port` on `target`, for `POST /api/remote/forwards`,
    /// or return the one that exists.
    pub async fn open_forward(&self, target: &str, port: i64) -> Result<ForwardInfo, ForwardError> {
        self.forwards.open(target, port, None, false).await
    }

    pub async fn open_forward_with(
        &self,
        target: &str,
        port: i64,
        local: Option<i64>,
        automatic: bool,
    ) -> Result<ForwardInfo, ForwardError> {
        self.forwards.open(target, port, local, automatic).await
    }

    pub fn forward_preferences(&self) -> Preferences {
        self.forwards.preferences()
    }

    pub async fn save_forward(
        &self,
        target: &str,
        port: i64,
        local: Option<i64>,
        label: Option<String>,
    ) -> Result<SavedForward, ForwardError> {
        self.forwards.save(target, port, local, label).await
    }

    pub async fn forget_forward(&self, target: &str, port: i64) -> Result<(), ForwardError> {
        self.forwards.forget(target, port).await
    }

    /// Every forward, for `GET /api/remote/forwards`.
    pub fn forwards(&self) -> Vec<ForwardInfo> {
        self.forwards.list()
    }

    /// Stop a forward, for `DELETE /api/remote/forwards`.
    pub async fn stop_forward(&self, target: &str, port: i64) -> Result<(), ForwardError> {
        self.forwards.stop(target, port).await
    }

    /// The local server listens on `port`, which is never shared with a
    /// host.
    pub fn reserve_local_port(&self, port: u16) {
        self.reverses.reserve(port);
    }

    /// Share local port `local` as `port` of `target`, for `POST
    /// /api/remote/reverse-forwards`, or return the one that exists.
    pub async fn open_reverse(
        &self,
        target: &str,
        port: i64,
        local: i64,
    ) -> Result<ReverseInfo, ForwardError> {
        self.reverses.open(target, port, local).await
    }

    /// Every reverse forward, for `GET /api/remote/reverse-forwards`.
    pub fn reverses(&self) -> Vec<ReverseInfo> {
        self.reverses.list()
    }

    /// Stop a reverse forward, for `DELETE /api/remote/reverse-forwards`.
    pub async fn stop_reverse(&self, target: &str, port: i64) -> Result<(), ForwardError> {
        self.reverses.stop(target, port).await
    }

    pub fn reverse_preferences(&self) -> ReversePreferences {
        self.reverses.preferences()
    }

    pub async fn save_reverse(
        &self,
        target: &str,
        port: i64,
        local: Option<i64>,
    ) -> Result<SavedReverse, ForwardError> {
        self.reverses.save(target, port, local).await
    }

    pub async fn forget_reverse(&self, target: &str, port: i64) -> Result<(), ForwardError> {
        self.reverses.forget(target, port).await
    }

    /// Serve one upgraded `/ws/remote` socket for `target`, as the client
    /// sent it, until either side ends. With `update`, a daemon of another
    /// version is replaced by one of this version first.
    pub async fn serve(&self, socket: WebSocket, target: String, update: bool) {
        let (mut sink, mut stream) = socket.split();
        let mut early = Vec::new();
        let budget = ConnectionBudget::default();
        match self
            .setup(&target, update, &mut sink, &mut stream, &mut early, &budget)
            .await
        {
            Setup::Gone => {}
            Setup::Failed(error) => {
                let Ok(failure) = error.failure_budgeted(&target, &budget) else {
                    return;
                };
                tracing::info!(
                    "remote screen hub: {target:?} failed ({}): {}",
                    failure.code.as_str(),
                    failure.message
                );
                if say(&mut sink, &HubMessage::Error(&failure), &budget).await {
                    let _ = sink
                        .send(Message::Close(Some(CloseFrame {
                            code: close_code::NORMAL,
                            reason: failure.code.as_str().into(),
                        })))
                        .await;
                }
            }
            Setup::Reached(reached) => {
                let Reached {
                    target,
                    bridge,
                    first,
                    daemon,
                    screens,
                    _metadata,
                } = *reached;
                let ready = HubMessage::Ready {
                    target: target.as_str(),
                    daemon: ReadyDaemon {
                        version: &daemon.version,
                        protocol: daemon.protocol,
                        keeps_sessions: daemon.keeps_sessions,
                        started: daemon.started,
                    },
                    screens: &screens,
                };
                // Keep the attachment's cleanup inside this client lifetime.
                if !say(&mut sink, &ready, &budget).await {
                    let _ = bridge.finish().await;
                    return;
                }
                tracing::info!(
                    "remote screen hub: relaying {} (Splitty {}, protocol {})",
                    target.as_str(),
                    daemon.version,
                    daemon.protocol
                );
                if let Err(error) = hosts::record_recent_if_visible(&self.config_dir, &target) {
                    tracing::warn!(
                        "remote screen hub: could not record {} as a recent host: {error}",
                        target.as_str()
                    );
                }
                self.forwards.restore(target.as_str());
                self.reverses.restore(target.as_str());
                relay(target.as_str(), sink, stream, bridge, first, early).await;
            }
        }
    }

    /// Reach the host while the client hears of each step. The client's
    /// messages in the meantime are kept in `early`; closing the socket
    /// cancels the setup, which ends every `ssh` it started.
    async fn setup(
        &self,
        target: &str,
        update: bool,
        sink: &mut Sink,
        stream: &mut Stream,
        early: &mut Vec<Charged<Message>>,
        budget: &ConnectionBudget,
    ) -> Setup {
        let target = match SshTarget::parse(target) {
            Ok(target) => target,
            Err(error) => return Setup::Failed(error.into()),
        };
        let (progress, mut reports) = mpsc::channel(8);
        let context = splitty_transport::queue::Context::new(budget.clone());
        let reach = self.reach(target, update, progress, &context);
        tokio::pin!(reach);
        let mut completed = false;
        let deadline = tokio::time::Instant::now() + PREPARE_TIMEOUT;
        let outcome = async {
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => return Setup::Failed(HubError::PreparationTimeout),
                result = &mut reach => {
                    completed = true;
                    // Every step before the outcome.
                    while let Ok(report) = reports.try_recv() {
                        if !report_progress(sink, &report, budget).await {
                            if let Ok(reached) = result { let _ = reached.bridge.finish().await; }
                            return Setup::Gone;
                        }
                    }
                    return match result {
                        Ok(reached) => Setup::Reached(Box::new(reached)),
                        Err(error) => Setup::Failed(error),
                    };
                }
                Some(report) = reports.recv() => {
                    if !report_progress(sink, &report, budget).await {
                        return Setup::Gone;
                    }
                }
                message = stream.next() => match message {
                    Some(Ok(message @ (Message::Text(_) | Message::Binary(_))))
                        if early.len() < MAX_EARLY =>
                    {
                        let size = match &message { Message::Text(text) => text.len(), Message::Binary(bytes) => bytes.len(), _ => 0 };
                        let Ok(message) = budget.charge(size + wire::HEADER_BYTES, false, || message) else { return Setup::Gone; };
                        early.push(message);
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    _ => return Setup::Gone,
                },
            }
        }
        }.await;
        if !completed {
            context.close();
            if let Ok(reached) = reach.await {
                let _ = reached.bridge.finish().await;
            }
        }
        outcome
    }

    /// Install, handshake, the update when asked for, protocol check and
    /// bridge, until the daemon's first message arrived through the bridge.
    async fn reach(
        &self,
        target: SshTarget,
        update: bool,
        progress: mpsc::Sender<Charged<Progress>>,
        context: &splitty_transport::queue::Context,
    ) -> Result<Reached, HubError> {
        // Trusted hub/domain quota keys, shared by every active stream to a
        // host. Weak entries disappear when the last connection/opening ends.
        let admission = admission::Admission::global();
        let carrier = admission
            .carrier()
            .map_err(|e| HubError::Capacity(e.to_string()))?;
        let backend = {
            let mut backends = self
                .backends
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            backends.retain(|_, value| value.strong_count() != 0);
            match backends.get(target.as_str()).and_then(Weak::upgrade) {
                Some(backend) => backend,
                None => {
                    let backend = admission::Backend::new();
                    backends.insert(target.as_str().into(), Arc::downgrade(&backend));
                    backend
                }
            }
        };
        let opening = admission
            .begin(admission::Assignment::new(backend, self.domain.clone()))
            .map_err(|e| HubError::Capacity(e.to_string()))?;
        let host = target.as_str().to_owned();
        let report = |step: Step, message: std::fmt::Arguments<'_>| -> Result<(), HubError> {
            let report = context
                .budget
                .display(&message, true)
                .map_err(HubError::NativeBridge)?
                .map(|message| Progress { step, message });
            let _ = progress.try_send(report);
            Ok(())
        };

        let install_progress = progress.clone();
        let install_host = host.clone();
        let install_context = context.clone();
        let prepared = self
            .connector
            .prepare(
                &target,
                Box::new(move |arch: &str| {
                    match install_context.budget.display(
                        &format_args!("Installing Splitty {VERSION} on {install_host} ({arch})..."),
                        true,
                    ) {
                        Ok(message) => {
                            let _ = install_progress.try_send(message.map(|message| Progress {
                                step: Step::Install,
                                message,
                            }));
                        }
                        Err(_) => install_context.close(),
                    }
                }),
                context,
            )
            .await?;

        report(
            Step::Daemon,
            format_args!("Asking {host} for its Splitty daemon..."),
        )?;
        let handshake = self
            .connector
            .handshake(&target, &prepared, context)
            .await?;
        let (mut handshake, reservation) = handshake.into_parts();
        let mut metadata = Some(reservation);
        let result = async {
            let mut daemon = running(&host, &handshake)?;
            // The update needs the running daemon's version, which this handshake
            // tells: it starts no daemon when one runs, so asking first would cost
            // a call. A daemon it started is the answering server's own.
            if update && !daemon.started && daemon.version != VERSION {
                if let UpdatePlan::Notice(message) =
                    update_plan(daemon, &host, VERSION, Some(&handshake.server.version))
                {
                    return Err(HubError::UpdateRefused { message });
                }
                report(
                    Step::Update,
                    format_args!(
                        "Updating Splitty {} on {host}; its sessions keep running...",
                        daemon.version
                    ),
                )?;
                self.connector
                    .stop_daemon(&target, &prepared.server, daemon, context)
                    .await?;
                let updated = self
                    .connector
                    .handshake(&target, &prepared, context)
                    .await?;
                let (updated, reservation) = updated.into_parts();
                handshake = updated;
                metadata = Some(reservation);
                daemon = running(&host, &handshake)?;
                // `daemon stop` waits only briefly for the daemon to end.
                if daemon.version != VERSION {
                    return Err(HubError::NotStopped {
                        host,
                        version: daemon.version.clone(),
                    });
                }
            }
            if daemon.started {
                report(
                    Step::Started,
                    format_args!("Started a private Splitty daemon on {host}."),
                )?;
            }
            // The window's protocol is this server's: the UI comes from it. A
            // daemon outside the window keeps running untouched.
            if !compatible(daemon.protocol, PROTOCOL_VERSION) {
                return Err(HubError::Incompatible {
                    host,
                    version: daemon.version.clone(),
                    protocol: daemon.protocol,
                    keeps_sessions: daemon.keeps_sessions,
                    ours: PROTOCOL_VERSION,
                    screens: handshake.screens,
                });
            }

            if !daemon.supports_native() {
                return Err(HubError::NativeTransport { host });
            }
            report(
                Step::Bridge,
                format_args!("Opening the bridge to {host}..."),
            )?;
            let mut bridge = self
                .connector
                .bridge(
                    &target,
                    &prepared.server,
                    daemon,
                    (carrier, opening),
                    context,
                )
                .await?;
            // The daemon greets every connection with `mux.welcome`. Until it
            // arrives, a bridge that ends failed, and its error output says why.
            let first = match tokio::time::timeout(BRIDGE_TIMEOUT, bridge.recv()).await {
                Ok(Some(frame)) => frame,
                Ok(None) => {
                    let (protocol, ended) = bridge.finish().await;
                    let protocol = match protocol {
                        Err(error) if splitty_remote::bridge::protocol_failure(&error) => {
                            return Err(HubError::NativeBridge(error))
                        }
                        result => result,
                    };
                    if ended
                        .status
                        .as_ref()
                        .is_ok_and(|status| status.code() == Some(255))
                    {
                        return Err(HubError::Bridge(bridge_failure(&host, ended)));
                    }
                    if let Err(error) = protocol {
                        return Err(HubError::NativeBridge(error));
                    }
                    return Err(HubError::Bridge(bridge_failure(&host, ended)));
                }
                Err(_) => {
                    let _ = bridge.finish().await;
                    return Err(HubError::BridgeSilent {
                        host,
                        seconds: BRIDGE_TIMEOUT.as_secs(),
                    });
                }
            };
            #[derive(serde::Deserialize)]
            struct Welcome<'a> {
                #[serde(rename = "type")]
                kind: &'a str,
            }
            let welcome = first.frame.kind == Kind::Text
                && first
                    .frame
                    .application()
                    .ok()
                    .and_then(|(_, bytes)| serde_json::from_slice::<Welcome<'_>>(bytes).ok())
                    .is_some_and(|message| message.kind == "mux.welcome");
            if !welcome {
                let _ = bridge.finish().await;
                return Err(HubError::NativeBridge(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the native host did not begin with mux.welcome",
                )));
            }
            Ok(Reached {
                target,
                bridge,
                first,
                daemon: handshake.daemon.daemon.expect("validated running daemon"),
                screens: handshake.screens,
                _metadata: metadata.take().expect("metadata reservation"),
            })
        }
        .await;
        result.map_err(|error| HubError::Retained {
            error: Box::new(error),
            allocation: Box::new(metadata.take().expect("failed metadata reservation")),
        })
    }
}

/// The daemon the handshake found running or started.
fn running<'a>(host: &str, handshake: &'a Handshake) -> Result<&'a RunningDaemon, HubError> {
    match &handshake.daemon.daemon {
        Some(daemon) if handshake.daemon.running => Ok(daemon),
        _ => Err(HubError::NoDaemon {
            host: host.to_owned(),
        }),
    }
}

/// A bridge that ended, as an `ssh` failure: its status and error output.
fn bridge_failure(host: &str, ended: Ended) -> SshError {
    match ended.status {
        Ok(status) => ssh::classify_budgeted(host, status, &ended.stderr, &ended.budget),
        Err(error) => {
            let size = match splitty_transport::budget::display_size(&error)
                .ok()
                .and_then(|bytes| bytes.checked_add(host.len()))
                .and_then(|bytes| bytes.checked_add(ended.stderr.len()))
                .and_then(|bytes| bytes.checked_mul(8))
                .and_then(|bytes| bytes.checked_add(4096))
            {
                Some(size) => size,
                None => {
                    return SshError::Spawn(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "bridge diagnostic capacity exhausted",
                    ))
                }
            };
            let storage = match ended.budget.reserve(size, false) {
                Ok(storage) => storage,
                Err(error) => {
                    return SshError::Spawn(io::Error::new(io::ErrorKind::WouldBlock, error))
                }
            };
            let message = format!("the bridge to {host} could not be watched: {error}");
            let stderr = ended.stderr.to_string();
            let retained = message.capacity()
                + stderr.capacity()
                + std::mem::size_of::<SshError>()
                + std::mem::size_of::<splitty_transport::budget::Lease>();
            SshError::Failed {
                allocation: Some(Box::new(storage.shrink(retained))),
                message,
                status: None,
                stderr,
                hint: None,
            }
        }
    }
}

/// Relay until either side ends, then end the other: a bridge whose host
/// side ended closes the socket with [`HOST_LOST`], a closed socket ends the
/// bridge.
async fn relay(
    host: &str,
    mut sink: Sink,
    mut stream: Stream,
    bridge: NativeBridge,
    first: Charged<Incoming>,
    early: Vec<Charged<Message>>,
) {
    let to_host = bridge.sender();
    let budget = to_host.context().budget.clone();
    let mut bridge = bridge;
    let downstream = async {
        let mut frame = first;
        loop {
            let message = match message_of(&frame, &budget) {
                Ok(message) => message,
                Err(error) => {
                    tracing::warn!(%error, "invalid host session event");
                    return End::Host;
                }
            };
            let (message, _delivery) = message.into_parts();
            if !matches!(
                tokio::time::timeout(wire::LIVENESS_TIMEOUT, sink.send(message)).await,
                Ok(Ok(()))
            ) {
                return End::Client;
            }
            // The host's receipt is credited only after browser delivery.
            drop(frame);
            frame = match bridge.recv().await {
                Some(frame) => frame,
                None => return End::Host,
            };
        }
    };
    let upstream = async {
        for message in early {
            if forward_message(&to_host, &message).is_err() {
                return End::Host;
            }
        }
        loop {
            match stream.next().await {
                Some(Ok(message @ (Message::Text(_) | Message::Binary(_)))) => {
                    if forward_message(&to_host, &message).is_err() {
                        return End::Host;
                    }
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                Some(Ok(Message::Close(_)) | Err(_)) | None => return End::Client,
            }
        }
    };
    let end = tokio::select! { end = downstream => end, end = upstream => end };
    to_host.close();
    let (_, ended) = bridge.finish().await;
    match end {
        End::Host => {
            let _ = tokio::time::timeout(
                wire::HANDSHAKE_TIMEOUT,
                sink.send(Message::Close(Some(CloseFrame {
                    code: HOST_LOST,
                    reason: "host connection lost".into(),
                }))),
            )
            .await;
            tracing::info!(
                "remote screen hub: the connection to {host} ended ({})",
                describe(&ended)
            );
        }
        End::Client => {
            let _ = tokio::time::timeout(wire::HANDSHAKE_TIMEOUT, sink.close()).await;
        }
    }
}

fn describe(ended: &Ended) -> String {
    let status = match &ended.status {
        Ok(status) => status.to_string(),
        Err(error) => format!("not watched: {error}"),
    };
    if ended.stderr.is_empty() {
        format!("ssh {status}")
    } else {
        format!("ssh {status}: {}", ended.stderr)
    }
}

fn message_of(frame: &Incoming, budget: &ConnectionBudget) -> io::Result<Charged<Message>> {
    let bytes = frame.frame.application()?.1;
    match frame.frame.kind {
        Kind::Text => {
            let text = std::str::from_utf8(bytes).map_err(io::Error::other)?;
            budget
                .charge(bytes.len(), false, || Message::Text(text.to_owned().into()))
                .map_err(io::Error::other)
        }
        Kind::Binary => budget
            .charge(bytes.len(), false, || {
                Message::Binary(bytes.to_vec().into())
            })
            .map_err(io::Error::other),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected host session frame",
        )),
    }
}

fn forward_message(sender: &splitty_transport::peer::Sender, message: &Message) -> io::Result<()> {
    match message {
        Message::Text(text) => {
            crate::service::catalog::validate_session(text.as_bytes(), &sender.context().budget)
                .map_err(io::Error::other)?;
            sender.data(Kind::Text, text.as_bytes())
        }
        Message::Binary(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "binary client input is not a session operation",
        )),
        _ => Ok(()),
    }
}

async fn report_progress(sink: &mut Sink, progress: &Progress, budget: &ConnectionBudget) -> bool {
    say(
        sink,
        &HubMessage::Progress {
            step: progress.step,
            message: &progress.message,
        },
        budget,
    )
    .await
}
async fn say(sink: &mut Sink, message: &HubMessage<'_>, budget: &ConnectionBudget) -> bool {
    let limit = wire::MAX_FRAME_BYTES - wire::HEADER_BYTES - wire::DELIVERY_BYTES;
    let result = (|| {
        let size = splitty_transport::codec::json_size(message, limit)?;
        // JSON storage plus the WebSocket writer's encoded copy coexist until
        // the physical send completes; neither is an unbounded hidden queue.
        let storage = budget
            .reserve(2 * size + wire::HEADER_BYTES, false)
            .map_err(io::Error::other)?;
        let bytes = splitty_transport::codec::json_bytes(message, limit)?;
        let text = String::from_utf8(bytes).map_err(io::Error::other)?;
        Ok::<_, io::Error>((text, storage))
    })();
    match result {
        Ok((text, _storage)) => matches!(
            tokio::time::timeout(
                wire::HANDSHAKE_TIMEOUT,
                sink.send(Message::Text(text.into()))
            )
            .await,
            Ok(Ok(()))
        ),
        Err(error) => {
            tracing::warn!("remote screen hub: cannot encode a message: {error}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(message: &HubMessage<'_>) -> serde_json::Value {
        serde_json::to_value(message).expect("serialize")
    }

    /// The shapes `docs/protocol.ts` names.
    #[test]
    fn the_hubs_messages_encode_as_the_protocol_names_them() {
        assert_eq!(
            json(&HubMessage::Progress {
                step: Step::Install,
                message: "Installing"
            }),
            serde_json::json!({"type": "remote.progress", "step": "install", "message": "Installing"})
        );
        for (step, name) in [
            (Step::Install, "install"),
            (Step::Daemon, "daemon"),
            (Step::Update, "update"),
            (Step::Started, "started"),
            (Step::Bridge, "bridge"),
        ] {
            assert_eq!(
                serde_json::to_value(step).expect("step"),
                serde_json::json!(name)
            );
        }

        let screens = [
            Screen {
                id: "s1".into(),
                name: Some("api".into()),
                panes: 3,
            },
            Screen {
                id: "s2".into(),
                name: None,
                panes: 1,
            },
        ];
        assert_eq!(
            json(&HubMessage::Ready {
                target: "user@vm1",
                daemon: ReadyDaemon {
                    version: "0.1.0",
                    protocol: 1,
                    keeps_sessions: true,
                    started: true
                },
                screens: &screens,
            }),
            serde_json::json!({
                "type": "remote.ready",
                "target": "user@vm1",
                "daemon": {"version": "0.1.0", "protocol": 1, "keepsSessions": true, "started": true},
                "screens": [{"id": "s1", "name": "api", "panes": 3}, {"id": "s2", "panes": 1}],
            })
        );

        let incompatible = HubError::Incompatible {
            host: "vm1".into(),
            version: "0.9.0".into(),
            protocol: 3,
            keeps_sessions: true,
            ours: 1,
            screens: vec![Screen {
                id: "s1".into(),
                name: None,
                panes: 2,
            }],
        }
        .failure("vm1");
        assert_eq!(
            json(&HubMessage::Error(&incompatible)),
            serde_json::json!({
                "type": "remote.error",
                "code": "incompatible",
                "message": "vm1 runs Splitty 0.9.0 (protocol 3); this Splitty needs protocol 1.",
                "hint": "Update this Splitty to use vm1. Its daemon keeps running.",
                "version": "0.9.0",
                "protocol": 3,
                "keepsSessions": true,
                "screens": [{"id": "s1", "panes": 2}],
            })
        );
    }
}
