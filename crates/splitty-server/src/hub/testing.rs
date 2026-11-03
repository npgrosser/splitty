//! A scripted host for in-process tests of the hub: what preparing it and
//! its handshake answer is set by the test, and its bridge is a local `sh`
//! script, so the relay runs against a real process and real pipes. Its
//! forwards are listeners of the test process on the forward's local port,
//! which answer every connection with `forwarded <port>` and end when the
//! test says their `ssh` exited. Its reverse forwards are listeners on the
//! host's port that relay to the local port, as the host's relay would.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use splitty_core::protocol::PROTOCOL_VERSION;
use splitty_remote::bridge::NativeBridge;
use splitty_remote::handshake::{self, Handshake, RunningDaemon};
use splitty_remote::install::{Prepared, Probe, Server};
use splitty_remote::ssh::{SshError, SshTarget};
use splitty_remote::tunnel::{PortReservation, PortTunnelError, ReverseTunnelError};
use splitty_transport::{admission, negotiation, wire};
use tokio::io::AsyncWriteExt;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use super::{Connector, Forwarding, HubError, VERSION};

/// The daemon's first message on every bridge of a scripted host.
pub const WELCOME: &str = r#"{"type":"mux.welcome","clientId":"c1","screens":[]}"#;

/// What a scripted host does when the hub prepares it.
#[derive(Clone, Copy)]
pub enum Prepare {
    /// The server is there.
    Installed,
    /// The server is installed for this architecture.
    Installs(&'static str),
    /// `ssh` fails with status 255 and this error output.
    SshFails(&'static str),
}

/// A daemon on a scripted host.
#[derive(Clone)]
struct Daemon {
    version: String,
    protocol: u32,
    keeps_sessions: bool,
}

/// A host whose answers a test sets. Clones share the host's daemon and what
/// the host was asked.
#[derive(Clone)]
pub struct ScriptedHost {
    prepare: Prepare,
    preparation_gate: Option<Arc<ForwardGate>>,
    preparation_cleanup: Option<Arc<ForwardGate>>,
    /// The version of the server the handshake runs, which also starts the
    /// daemon when none runs.
    server_version: &'static str,
    /// The protocol version of that server.
    protocol: u32,
    /// Whether `daemon stop` ends the daemon.
    stops: bool,
    /// An optional failure script, run instead of the native fixture.
    bridge: Option<&'static str>,
    welcome: &'static str,
    daemon: Arc<Mutex<Option<Daemon>>>,
    asked: Arc<Mutex<Vec<String>>>,
    bridge_pid: Arc<Mutex<Option<u32>>>,
    /// When set, a forward's `ssh` fails with status 255 and this error
    /// output.
    forward_fails: Option<&'static str>,
    /// How many of the next forwards fail as if the host were unreachable.
    failing_forwards: Arc<AtomicUsize>,
    /// What ends the running forward of each port of the host.
    forwards: Arc<Mutex<HashMap<u16, Arc<Notify>>>>,
    forward_gates: Arc<Mutex<HashMap<u16, Arc<ForwardGate>>>>,
    stop_gates: Arc<Mutex<HashMap<u16, Arc<ForwardGate>>>>,
    /// The kernel the probe reports.
    kernel: &'static str,
    /// The host's ports in use there.
    busy: Arc<Mutex<HashSet<u16>>>,
    /// How many of the next reverse forwards fail as if the host were
    /// unreachable.
    failing_reverses: Arc<AtomicUsize>,
    /// What ends the running reverse forward of each port of the host.
    reverses: Arc<Mutex<HashMap<u16, Arc<Notify>>>>,
}

impl ScriptedHost {
    /// A host without Splitty: this version is installed, the handshake
    /// starts a daemon of it with one screen, and the bridge greets with
    /// [`WELCOME`] and then echoes every frame. `dir` holds its files.
    pub fn new(_dir: &std::path::Path) -> Self {
        Self {
            prepare: Prepare::Installs("x86_64"),
            preparation_gate: None,
            preparation_cleanup: None,
            server_version: VERSION,
            protocol: PROTOCOL_VERSION,
            stops: true,
            bridge: None,
            welcome: WELCOME,
            daemon: Arc::default(),
            asked: Arc::default(),
            bridge_pid: Arc::default(),
            forward_fails: None,
            failing_forwards: Arc::default(),
            forwards: Arc::default(),
            forward_gates: Arc::default(),
            stop_gates: Arc::default(),
            kernel: "Linux",
            busy: Arc::default(),
            failing_reverses: Arc::default(),
            reverses: Arc::default(),
        }
    }

    /// The probe reports `kernel`, such as `Darwin`.
    pub fn with_kernel(mut self, kernel: &'static str) -> Self {
        self.kernel = kernel;
        self
    }

    pub fn with_preparation_gate(mut self, gate: Arc<ForwardGate>) -> Self {
        self.preparation_gate = Some(gate);
        self
    }
    pub fn with_preparation_cleanup(mut self, gate: Arc<ForwardGate>) -> Self {
        self.preparation_cleanup = Some(gate);
        self
    }

    /// Port `port` of the host is in use there, or free again.
    pub fn set_busy(&self, port: u16, busy: bool) {
        let mut ports = self.busy.lock().expect("busy");
        if busy {
            ports.insert(port);
        } else {
            ports.remove(&port);
        }
    }

    /// The next `count` reverse forwards fail, as if the host were
    /// unreachable.
    pub fn fail_next_reverses(&self, count: usize) {
        self.failing_reverses.store(count, Ordering::SeqCst);
    }

    /// The `ssh` of the reverse forward to the host's `port` exits.
    pub fn end_reverse(&self, port: u16) {
        if let Some(ended) = self.reverses.lock().expect("reverses").get(&port) {
            ended.notify_one();
        }
    }

    pub fn preparing(mut self, prepare: Prepare) -> Self {
        self.prepare = prepare;
        self
    }

    /// The protocol of the server, and so of the daemon it starts.
    pub fn with_protocol(mut self, protocol: u32) -> Self {
        self.protocol = protocol;
        self
    }

    /// The version of the server the handshake runs: another than this
    /// one's when the host has only its own `splitty` in its `PATH`.
    pub fn with_server_version(mut self, version: &'static str) -> Self {
        self.server_version = version;
        self
    }

    /// A daemon of `version` and `protocol` already runs, one whose sessions
    /// end with it.
    pub fn with_daemon(self, version: &str, protocol: u32) -> Self {
        *self.daemon.lock().expect("daemon") = Some(Daemon {
            version: version.to_owned(),
            protocol,
            keeps_sessions: false,
        });
        self
    }

    /// A daemon of `version` and `protocol` already runs, one whose sessions
    /// outlive it in keepers.
    pub fn with_keeping_daemon(self, version: &str, protocol: u32) -> Self {
        *self.daemon.lock().expect("daemon") = Some(Daemon {
            version: version.to_owned(),
            protocol,
            keeps_sessions: true,
        });
        self
    }

    /// `daemon stop` leaves the daemon running, as one that takes longer to
    /// end than the command waits.
    pub fn not_stopping(mut self) -> Self {
        self.stops = false;
        self
    }

    pub fn with_welcome(mut self, welcome: &'static str) -> Self {
        self.welcome = welcome;
        self
    }

    pub fn with_bridge(mut self, script: &'static str) -> Self {
        self.bridge = Some(script);
        self
    }

    /// Every forward's `ssh` fails with status 255 and `stderr`.
    pub fn forwards_failing(mut self, stderr: &'static str) -> Self {
        self.forward_fails = Some(stderr);
        self
    }

    /// Pause one attempt before it binds, for deterministic ordering tests.
    pub fn pause_forward(&self, remote: u16) -> Arc<ForwardGate> {
        let gate = Arc::new(ForwardGate::default());
        self.forward_gates
            .lock()
            .expect("gates")
            .insert(remote, gate.clone());
        gate
    }

    pub fn pause_stop(&self, remote: u16) -> Arc<ForwardGate> {
        let gate = Arc::new(ForwardGate::default());
        self.stop_gates
            .lock()
            .expect("stop gates")
            .insert(remote, gate.clone());
        gate
    }

    /// The next `count` forwards fail, as if the host were unreachable.
    pub fn fail_next_forwards(&self, count: usize) {
        self.failing_forwards.store(count, Ordering::SeqCst);
    }

    /// The `ssh` of the forward of the host's `port` exits, which frees its
    /// local port.
    pub fn end_forward(&self, port: u16) {
        if let Some(ended) = self.forwards.lock().expect("forwards").get(&port) {
            ended.notify_one();
        }
    }

    /// What the hub asked the host, in order: `prepare <target>`,
    /// `handshake`, `stop`, `bridge`, `forward <port> at <local
    /// port>`, `reverse <target> <port> from <local port>`.
    pub fn asked(&self) -> Vec<String> {
        self.asked.lock().expect("asked").clone()
    }

    /// The bridge's process id, once one was started.
    pub fn bridge_pid(&self) -> Option<u32> {
        *self.bridge_pid.lock().expect("bridge pid")
    }

    fn ask(&self, what: String) {
        self.asked.lock().expect("asked").push(what);
    }
}

fn exit_status(code: i32) -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    std::process::ExitStatus::from_raw(code << 8)
}

impl Connector for ScriptedHost {
    fn prepare<'a>(
        &'a self,
        target: &'a SshTarget,
        on_install: Box<dyn FnOnce(&str) + Send + 'a>,
        context: &'a splitty_transport::queue::Context,
    ) -> BoxFuture<'a, Result<Prepared, HubError>> {
        Box::pin(async move {
            self.ask(format!("prepare {}", target.as_str()));
            let arch = match self.prepare {
                Prepare::Installed => "x86_64",
                Prepare::Installs(arch) => {
                    on_install(arch);
                    arch
                }
                Prepare::SshFails(stderr) => {
                    let error: SshError =
                        splitty_remote::ssh::classify(target.as_str(), exit_status(255), stderr);
                    return Err(HubError::Install(error.into()));
                }
            };
            if let Some(gate) = &self.preparation_gate {
                gate.entered.notify_one();
                let mut closing = context.closing();
                tokio::select! { biased;
                    _ = async { let _ = closing.wait_for(|closed| *closed).await; } => {
                        if let Some(cleanup) = &self.preparation_cleanup {
                            cleanup.entered.notify_one();
                            cleanup.release.notified().await;
                        }
                        return Err(HubError::NativeBridge(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "setup owner closed")));
                    },
                    _ = gate.release.notified() => {},
                }
            }
            Ok(Prepared {
                server: Server::Installed {
                    path: format!("/home/u/.local/share/splitty/remote/{VERSION}/splitty"),
                    config_dir: None,
                },
                probe: Probe {
                    kernel: self.kernel.into(),
                    arch: arch.into(),
                    home: "/home/u".into(),
                    marker: None,
                },
                uploaded: matches!(self.prepare, Prepare::Installs(_)),
                had_payload: true,
            })
        })
    }

    fn handshake<'a>(
        &'a self,
        target: &'a SshTarget,
        _prepared: &'a Prepared,
        context: &'a splitty_transport::queue::Context,
    ) -> BoxFuture<'a, Result<splitty_transport::budget::Charged<Handshake>, HubError>> {
        Box::pin(async move {
            if context.is_closed() {
                return Err(HubError::NativeBridge(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "setup owner closed",
                )));
            }
            self.ask("handshake".into());
            let (daemon, started) = {
                let mut running = self.daemon.lock().expect("daemon");
                let started = running.is_none();
                // A daemon this build starts keeps its sessions.
                let daemon = running.get_or_insert_with(|| Daemon {
                    version: self.server_version.to_owned(),
                    protocol: self.protocol,
                    keeps_sessions: true,
                });
                (daemon.clone(), started)
            };
            let answer = serde_json::json!({
                "handshake": 2,
                "server": {"version": self.server_version, "protocol": self.protocol, "path": "/p", "transportVersions":[1]},
                "daemon": {
                    "running": true, "pid": 4711, "instance":"0123456789abcdef0123456789abcdef", "transportVersion":1, "version": daemon.version,
                    "protocol": daemon.protocol, "startedAt": "1760",
                    "keepsSessions": daemon.keeps_sessions, "started": started,
                },
                "screens": [{"id": "s1", "name": "api", "panes": 3}],
            });
            let answer = serde_json::to_vec(&answer).expect("JSON");
            Ok(handshake::parse_budgeted(
                target.as_str(),
                &answer,
                &context.budget,
            )?)
        })
    }

    fn stop_daemon<'a>(
        &'a self,
        _target: &'a SshTarget,
        _server: &'a Server,
        _daemon: &'a RunningDaemon,
        context: &'a splitty_transport::queue::Context,
    ) -> BoxFuture<'a, Result<(), HubError>> {
        Box::pin(async move {
            if context.is_closed() {
                return Err(HubError::NativeBridge(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "setup owner closed",
                )));
            }
            self.ask("stop".to_owned());
            if self.stops {
                *self.daemon.lock().expect("daemon") = None;
            }
            Ok(())
        })
    }

    fn bridge<'a>(
        &'a self,
        _target: &'a SshTarget,
        _server: &'a Server,
        daemon: &'a RunningDaemon,
        capacity: (admission::Carrier, admission::Opening),
        context: &'a splitty_transport::queue::Context,
    ) -> BoxFuture<'a, Result<NativeBridge, HubError>> {
        Box::pin(async move {
            self.ask("bridge".to_owned());
            let mut command = if let Some(script) = self.bridge {
                let mut command = tokio::process::Command::new("sh");
                command.args(["-c", script]);
                command
            } else {
                let ready = wire::Ready {
                    generation: 1,
                    instance: daemon.instance.clone().unwrap(),
                    mux_protocol: daemon.protocol,
                    purpose: wire::Purpose::Direct,
                    limits: wire::Limits::default(),
                };
                let mut command = tokio::process::Command::new("python3");
                command
                    .args([
                        "-u",
                        "-c",
                        include_str!("../../../splitty-remote/tests/native_bridge.py"),
                    ])
                    .arg(serde_json::to_string(&ready).unwrap())
                    .arg(self.welcome);
                command
            };
            command.kill_on_drop(true);
            let bridge = NativeBridge::spawn_context(
                command,
                capacity,
                negotiation::Expected {
                    instance: daemon.instance.as_deref(),
                    purpose: &wire::Purpose::Direct,
                    mux_protocol: daemon.protocol,
                    preparation: wire::HANDSHAKE_TIMEOUT,
                },
                context.clone(),
                |_| {},
            )
            .await
            .map_err(HubError::NativeBridge)?;
            *self.bridge_pid.lock().expect("bridge pid") = bridge.id();
            Ok(bridge)
        })
    }

    fn forward<'a>(
        &'a self,
        target: &'a SshTarget,
        local: PortReservation,
        remote: u16,
    ) -> BoxFuture<'a, Result<Box<dyn Forwarding>, HubError>> {
        Box::pin(async move {
            let port = local.port();
            self.ask(format!("forward {} {remote} at {port}", target.as_str()));
            let gate = self.forward_gates.lock().expect("gates").remove(&remote);
            if let Some(gate) = gate {
                gate.entered.notify_one();
                gate.release.notified().await;
            }
            // As long as `ssh` takes to log in, so requests can overlap.
            tokio::time::sleep(Duration::from_millis(50)).await;
            let failing = self
                .failing_forwards
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                    count.checked_sub(1)
                })
                .is_ok();
            let stderr = match (self.forward_fails, failing) {
                (Some(stderr), _) => stderr,
                (None, true) => "ssh: connect to host vm port 22: Connection refused",
                (None, false) => "",
            };
            if !stderr.is_empty() {
                let error =
                    splitty_remote::ssh::classify(target.as_str(), exit_status(255), stderr);
                return Err(HubError::Forward(PortTunnelError::Ssh(error)));
            }

            // This in-process fake can inherit the reservation itself. Dropping
            // and rebinding introduced unrelated ephemeral-port races between
            // parallel fixtures; real SSH handoff is covered by process tests.
            let listener = local.into_listener().map_err(HubError::ForwardPort)?;
            listener
                .set_nonblocking(true)
                .map_err(HubError::ForwardPort)?;
            let listener =
                tokio::net::TcpListener::from_std(listener).map_err(HubError::ForwardPort)?;
            let answer = format!("forwarded {remote}\n");
            let listener = tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    let _ = stream.write_all(answer.as_bytes()).await;
                }
            });
            let ended = Arc::new(Notify::new());
            self.forwards
                .lock()
                .expect("forwards")
                .insert(remote, ended.clone());
            Ok(Box::new(ScriptedForward {
                ended,
                listener,
                remote,
                stop_gates: self.stop_gates.clone(),
            }) as Box<dyn Forwarding>)
        })
    }

    fn reserve_share(
        &self,
        _local: u16,
    ) -> BoxFuture<'_, Result<Option<splitty_remote::tunnel::exposure::Share>, HubError>> {
        Box::pin(async { Ok(None) })
    }

    fn reverse<'a>(
        &'a self,
        target: &'a SshTarget,
        local: u16,
        port: u16,
        _share: Option<&'a splitty_remote::tunnel::exposure::Share>,
    ) -> BoxFuture<'a, Result<Box<dyn Forwarding>, HubError>> {
        Box::pin(self.scripted_reverse(target, local, port))
    }
}

impl ScriptedHost {
    async fn scripted_reverse(
        &self,
        target: &SshTarget,
        local: u16,
        port: u16,
    ) -> Result<Box<dyn Forwarding>, HubError> {
        self.ask(format!("reverse {} {port} from {local}", target.as_str()));
        // As long as `ssh` takes to log in, so requests can overlap.
        tokio::time::sleep(Duration::from_millis(50)).await;
        if self.kernel != "Linux" {
            return Err(HubError::ReverseUnsupported {
                host: target.as_str().to_owned(),
                kernel: self.kernel.to_owned(),
            });
        }
        let failing = self
            .failing_reverses
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                count.checked_sub(1)
            })
            .is_ok();
        if failing {
            let error = splitty_remote::ssh::classify(
                target.as_str(),
                exit_status(255),
                "ssh: connect to host vm port 22: Connection refused",
            );
            return Err(HubError::Reverse(ReverseTunnelError::Ssh(error)));
        }
        if self.busy.lock().expect("busy").contains(&port) {
            return Err(HubError::Reverse(ReverseTunnelError::HostPortConflict {
                port,
                detail: format!("port {port} is already in use on this host (127.0.0.1)"),
            }));
        }
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .map_err(|error| {
                HubError::Reverse(ReverseTunnelError::HostPortConflict {
                    port,
                    detail: error.to_string(),
                })
            })?;
        let listener = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    if let Ok(mut upstream) =
                        tokio::net::TcpStream::connect(("127.0.0.1", local)).await
                    {
                        let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
                    }
                });
            }
        });
        let ended = Arc::new(Notify::new());
        self.reverses
            .lock()
            .expect("reverses")
            .insert(port, ended.clone());
        Ok(Box::new(ScriptedForward {
            ended,
            listener,
            remote: port,
            stop_gates: Arc::default(),
        }) as Box<dyn Forwarding>)
    }
}

/// A forward of a scripted host: the test's listener on its local port.
struct ScriptedForward {
    remote: u16,
    stop_gates: Arc<Mutex<HashMap<u16, Arc<ForwardGate>>>>,
    ended: Arc<Notify>,
    listener: JoinHandle<()>,
}

impl ScriptedForward {
    /// Close the listener and wait until its port is free.
    async fn close(&mut self) {
        self.listener.abort();
        let _ = (&mut self.listener).await;
    }
}

impl Forwarding for ScriptedForward {
    fn ended(&mut self) -> BoxFuture<'_, String> {
        Box::pin(async move {
            self.ended.notified().await;
            self.close().await;
            "ssh exited with exit status: 255".to_owned()
        })
    }

    fn stop(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let gate = self
                .stop_gates
                .lock()
                .expect("stop gates")
                .remove(&self.remote);
            if let Some(gate) = gate {
                gate.entered.notify_one();
                gate.release.notified().await;
            }
            self.close().await;
        })
    }
}

impl Drop for ScriptedForward {
    fn drop(&mut self) {
        self.listener.abort();
    }
}

/// One-shot attempt barrier; no global connector lock spans its wait.
#[derive(Default)]
pub struct ForwardGate {
    entered: Notify,
    release: Notify,
}
impl ForwardGate {
    pub async fn entered(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.entered.notified())
            .await
            .expect("forward operation did not reach its barrier");
    }
    pub fn release(&self) {
        self.release.notify_one();
    }
}
