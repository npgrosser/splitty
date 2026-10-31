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
//! (`remembered`), their last observed directories (`catalog`), and forwards
//! ports of hosts on request (`forwards`), each
//! with an `ssh` of its own that lives until the forward is stopped.

mod catalog;
mod connector;
mod failure;
mod forwards;
mod preferences;
mod remembered;
#[cfg(all(test, unix))]
pub(crate) mod testing;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{close_code, CloseFrame, Message, WebSocket};
use futures::future::BoxFuture;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use serde::Serialize;
use splitty_core::protocol::PROTOCOL_VERSION;
use splitty_remote::bridge::{Bridge, Ended, Frame};
use splitty_remote::handshake::{Handshake, RunningDaemon, Screen};
use splitty_remote::hosts::{self, HostPreferences, KnownHost};
pub use splitty_remote::hosts::{HostOrder, HostPreference, PreferenceError};
use splitty_remote::install::{Prepared, Server};
use splitty_remote::ssh::{self, SshError, SshTarget};
use splitty_remote::tunnel::PortReservation;
use tokio::sync::mpsc;

pub use catalog::{Catalog, CatalogError, HostCatalog};
pub use connector::SshConnector;
pub use failure::{compatible, Code, Failure, HubError};
pub use forwards::{ForwardError, ForwardInfo, ForwardState, MAX_PER_HOST};
pub use preferences::{Preferences, SavedForward};
pub use remembered::{RememberError, Remembered};

/// This server's version: the one it installs on hosts, and the one
/// `update=1` brings a host's daemon to.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The close code of a relaying socket whose host side ended. The client
/// treats it like a dropped connection and reconnects.
pub const HOST_LOST: u16 = 4001;

/// The largest message a client may send: what one bridge frame carries.
pub const MAX_MESSAGE: usize = splitty_remote::bridge::MAX_FRAME;

/// How many messages a client may send before `remote.ready`; they are
/// relayed after it. A client that sends more is disconnected.
const MAX_EARLY: usize = 64;

/// How long the bridge may take to deliver the daemon's first message.
const BRIDGE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a bridge gets to end on its own once its input is closed.
const BRIDGE_GRACE: Duration = Duration::from_secs(3);

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
    ) -> BoxFuture<'a, Result<Prepared, HubError>>;

    /// The handshake with `--ensure-daemon`: a daemon that does not run is
    /// started on `port_hint`.
    fn handshake<'a>(
        &'a self,
        target: &'a SshTarget,
        prepared: &'a Prepared,
        port_hint: u16,
    ) -> BoxFuture<'a, Result<Handshake, HubError>>;

    /// Stop the host's running daemon with `server`'s `daemon stop`, which
    /// ends the daemon's sessions.
    fn stop_daemon<'a>(
        &'a self,
        target: &'a SshTarget,
        server: &'a Server,
    ) -> BoxFuture<'a, Result<(), HubError>>;

    /// Start `splitty remote-bridge` on the host with `server`.
    fn bridge(&self, target: &SshTarget, server: &Server) -> Result<Bridge, HubError>;

    /// Forward `127.0.0.1:<port>` here, the port `local` holds until then, to
    /// `localhost:<remote>` on `target`, once the forward is verified.
    fn forward<'a>(
        &'a self,
        target: &'a SshTarget,
        local: PortReservation,
        remote: u16,
    ) -> BoxFuture<'a, Result<Box<dyn Forwarding>, HubError>>;
}

/// A forward the hub holds: an `ssh -N` with the system `ssh`, a listener of
/// the test in tests. Dropping it ends it.
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
    bridge: Bridge,
    first: Frame,
    daemon: RunningDaemon,
    screens: Vec<Screen>,
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
            connector,
            config_dir,
            ssh_dir,
            remembering: Mutex::new(()),
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

    /// Serve one upgraded `/ws/remote` socket for `target`, as the client
    /// sent it, until either side ends. With `update`, a daemon of another
    /// version is replaced by one of this version first.
    pub async fn serve(&self, socket: WebSocket, target: String, update: bool) {
        let (mut sink, mut stream) = socket.split();
        let mut early = Vec::new();
        match self
            .setup(&target, update, &mut sink, &mut stream, &mut early)
            .await
        {
            Setup::Gone => {}
            Setup::Failed(error) => {
                let failure = error.failure(&target);
                tracing::info!(
                    "remote screen hub: {target:?} failed ({}): {}",
                    failure.code.as_str(),
                    failure.message
                );
                if say(&mut sink, &HubMessage::Error(&failure)).await {
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
                // A client gone by now drops the bridge, which kills its ssh.
                if !say(&mut sink, &ready).await {
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
        early: &mut Vec<Message>,
    ) -> Setup {
        let target = match SshTarget::parse(target) {
            Ok(target) => target,
            Err(error) => return Setup::Failed(error.into()),
        };
        let (progress, mut reports) = mpsc::unbounded_channel();
        let reach = self.reach(target, update, progress);
        tokio::pin!(reach);
        loop {
            tokio::select! {
                result = &mut reach => {
                    // Every step before the outcome.
                    while let Ok(report) = reports.try_recv() {
                        if !report_progress(sink, &report).await {
                            return Setup::Gone;
                        }
                    }
                    return match result {
                        Ok(reached) => Setup::Reached(Box::new(reached)),
                        Err(error) => Setup::Failed(error),
                    };
                }
                Some(report) = reports.recv() => {
                    if !report_progress(sink, &report).await {
                        return Setup::Gone;
                    }
                }
                message = stream.next() => match message {
                    Some(Ok(message @ (Message::Text(_) | Message::Binary(_))))
                        if early.len() < MAX_EARLY =>
                    {
                        early.push(message);
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    _ => return Setup::Gone,
                },
            }
        }
    }

    /// Install, handshake, the update when asked for, protocol check and
    /// bridge, until the daemon's first message arrived through the bridge.
    async fn reach(
        &self,
        target: SshTarget,
        update: bool,
        progress: mpsc::UnboundedSender<Progress>,
    ) -> Result<Reached, HubError> {
        let host = target.as_str().to_owned();
        let report = |step: Step, message: String| {
            // The receiver lives as long as the setup that awaits this.
            let _ = progress.send(Progress { step, message });
        };

        let install_progress = progress.clone();
        let install_host = host.clone();
        let prepared = self
            .connector
            .prepare(
                &target,
                Box::new(move |arch: &str| {
                    let _ = install_progress.send(Progress {
                        step: Step::Install,
                        message: format!(
                            "Installing Splitty {VERSION} on {install_host} ({arch})..."
                        ),
                    });
                }),
            )
            .await?;

        // A port free on this side, held during the handshake: a daemon the
        // host starts now gets it, so a later `splitty connect` to the host
        // can forward the same port and the origin matches.
        let hint = PortReservation::acquire().map_err(HubError::Port)?;
        report(
            Step::Daemon,
            format!("Asking {host} for its Splitty daemon..."),
        );
        let mut handshake = self
            .connector
            .handshake(&target, &prepared, hint.port())
            .await?;
        let mut daemon = running(&host, &handshake)?;
        // The update needs the running daemon's version, which this handshake
        // tells: it starts no daemon when one runs, so asking first would cost
        // a call. A daemon it started is the answering server's own.
        if update && !daemon.started && daemon.version != VERSION {
            // The handshake after the stop starts the answering server's
            // version. Without this version on the host, stopping would end
            // the sessions for nothing.
            if handshake.server.version != VERSION {
                return Err(HubError::NoUpdate {
                    host,
                    version: daemon.version,
                    arch: prepared.probe.arch,
                });
            }
            report(
                Step::Update,
                format!("Stopping Splitty {} on {host}...", daemon.version),
            );
            self.connector
                .stop_daemon(&target, &prepared.server)
                .await?;
            handshake = self
                .connector
                .handshake(&target, &prepared, hint.port())
                .await?;
            daemon = running(&host, &handshake)?;
            // `daemon stop` waits only briefly for the daemon to end.
            if daemon.version != VERSION {
                return Err(HubError::NotStopped {
                    host,
                    version: daemon.version,
                });
            }
        }
        drop(hint);
        if daemon.started {
            report(
                Step::Started,
                format!("Started a Splitty daemon on {host} (port {}).", daemon.port),
            );
        }
        // The window's protocol is this server's: the UI comes from it. A
        // daemon outside the window keeps running untouched.
        if !compatible(daemon.protocol, PROTOCOL_VERSION) {
            return Err(HubError::Incompatible {
                host,
                version: daemon.version,
                protocol: daemon.protocol,
                keeps_sessions: daemon.keeps_sessions,
                ours: PROTOCOL_VERSION,
                screens: handshake.screens,
            });
        }

        report(Step::Bridge, format!("Opening the bridge to {host}..."));
        let mut bridge = self.connector.bridge(&target, &prepared.server)?;
        // The daemon greets every connection with `mux.welcome`. Until it
        // arrives, a bridge that ends failed, and its error output says why.
        let first = match tokio::time::timeout(BRIDGE_TIMEOUT, bridge.recv()).await {
            Ok(Ok(Some(frame))) => frame,
            Ok(Ok(None) | Err(_)) => {
                let ended = bridge.process().finish(BRIDGE_GRACE).await;
                return Err(HubError::Bridge(bridge_failure(&host, ended)));
            }
            Err(_) => {
                return Err(HubError::BridgeSilent {
                    host,
                    seconds: BRIDGE_TIMEOUT.as_secs(),
                })
            }
        };
        Ok(Reached {
            target,
            bridge,
            first,
            daemon,
            screens: handshake.screens,
        })
    }
}

/// The daemon the handshake found running or started.
fn running(host: &str, handshake: &Handshake) -> Result<RunningDaemon, HubError> {
    match &handshake.daemon.daemon {
        Some(daemon) if handshake.daemon.running => Ok(daemon.clone()),
        _ => Err(HubError::NoDaemon {
            host: host.to_owned(),
        }),
    }
}

/// A bridge that ended, as an `ssh` failure: its status and error output.
fn bridge_failure(host: &str, ended: Ended) -> SshError {
    match ended.status {
        Ok(status) => ssh::classify(host, status, &ended.stderr),
        Err(error) => SshError::Failed {
            message: format!("the bridge to {host} could not be watched: {error}"),
            status: None,
            stderr: ended.stderr,
            hint: None,
        },
    }
}

/// Relay until either side ends, then end the other: a bridge whose host
/// side ended closes the socket with [`HOST_LOST`], a closed socket ends the
/// bridge.
async fn relay(
    host: &str,
    mut sink: Sink,
    mut stream: Stream,
    bridge: Bridge,
    first: Frame,
    early: Vec<Message>,
) {
    let (mut to_host, mut from_host, mut process) = bridge.split();
    let downstream = async {
        let mut frame = first;
        loop {
            if sink.send(message_of(frame)).await.is_err() {
                return End::Client;
            }
            frame = match from_host.recv().await {
                Ok(Some(frame)) => frame,
                Ok(None) => return End::Host,
                Err(error) => {
                    tracing::warn!("remote screen hub: bad frame from {host}: {error}");
                    return End::Host;
                }
            };
        }
    };
    let upstream = async {
        for message in early {
            if let Some(frame) = frame_of(message) {
                if to_host.send(&frame).await.is_err() {
                    return End::Host;
                }
            }
        }
        loop {
            let frame = match stream.next().await {
                Some(Ok(message @ (Message::Text(_) | Message::Binary(_)))) => {
                    match frame_of(message) {
                        Some(frame) => frame,
                        None => continue,
                    }
                }
                // Pings are answered by the WebSocket itself.
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                Some(Ok(Message::Close(_)) | Err(_)) | None => return End::Client,
            };
            if to_host.send(&frame).await.is_err() {
                return End::Host;
            }
        }
    };
    let end = tokio::select! {
        end = downstream => end,
        end = upstream => end,
    };

    // The end of its input ends the bridge on the host cleanly.
    drop(to_host);
    match end {
        End::Host => {
            let _ = sink
                .send(Message::Close(Some(CloseFrame {
                    code: HOST_LOST,
                    reason: "host connection lost".into(),
                })))
                .await;
            let ended = process.finish(BRIDGE_GRACE).await;
            tracing::info!(
                "remote screen hub: the connection to {host} ended ({})",
                describe(&ended)
            );
        }
        End::Client => {
            let _ = sink.close().await;
            let ended = process.finish(BRIDGE_GRACE).await;
            tracing::debug!(
                "remote screen hub: closed the bridge to {host} ({})",
                describe(&ended)
            );
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

fn message_of(frame: Frame) -> Message {
    match frame {
        Frame::Text(text) => Message::Text(text.into()),
        Frame::Binary(bytes) => Message::Binary(bytes.into()),
    }
}

/// The frame of a client's text or binary message.
fn frame_of(message: Message) -> Option<Frame> {
    match message {
        Message::Text(text) => Some(Frame::Text(text.as_str().to_owned())),
        Message::Binary(bytes) => Some(Frame::Binary(bytes.into())),
        Message::Ping(_) | Message::Pong(_) | Message::Close(_) => None,
    }
}

async fn report_progress(sink: &mut Sink, progress: &Progress) -> bool {
    say(
        sink,
        &HubMessage::Progress {
            step: progress.step,
            message: &progress.message,
        },
    )
    .await
}

/// Send one of the hub's own messages; `false` when the client is gone.
async fn say(sink: &mut Sink, message: &HubMessage<'_>) -> bool {
    match serde_json::to_string(message) {
        Ok(text) => sink.send(Message::Text(text.into())).await.is_ok(),
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

    /// A client's messages become frames of the same kind and bytes, and the
    /// host's frames messages.
    #[test]
    fn messages_and_frames_map_one_to_one() {
        assert_eq!(
            frame_of(Message::Text("{\"type\":\"mux.resync\"}".into())),
            Some(Frame::Text("{\"type\":\"mux.resync\"}".into()))
        );
        assert_eq!(
            frame_of(Message::Binary(vec![0, 255].into())),
            Some(Frame::Binary(vec![0, 255]))
        );
        assert_eq!(frame_of(Message::Ping(vec![1].into())), None);
        assert_eq!(
            message_of(Frame::Text("t".into())),
            Message::Text("t".into())
        );
        assert_eq!(
            message_of(Frame::Binary(vec![7])),
            Message::Binary(vec![7].into())
        );
    }
}
