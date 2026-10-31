//! A scripted host for in-process tests of the hub: what preparing it and
//! its handshake answer is set by the test, and its bridge is a local `sh`
//! script, so the relay runs against a real process and real pipes. Its
//! forwards are listeners of the test process on the forward's local port,
//! which answer every connection with `forwarded <port>` and end when the
//! test says their `ssh` exited.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use splitty_core::protocol::PROTOCOL_VERSION;
use splitty_remote::bridge::{encode, Bridge, Frame};
use splitty_remote::handshake::{self, Handshake};
use splitty_remote::install::{Prepared, Probe, Server};
use splitty_remote::ssh::{SshError, SshTarget};
use splitty_remote::tunnel::{PortReservation, PortTunnelError};
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
    /// The version of the server the handshake runs, which also starts the
    /// daemon when none runs.
    server_version: &'static str,
    /// The protocol version of that server.
    protocol: u32,
    /// Whether `daemon stop` ends the daemon.
    stops: bool,
    /// The bridge, run by `sh -c`; `$1` is a file with the daemon's
    /// [`WELCOME`] as a frame.
    bridge: &'static str,
    welcome: Arc<PathBuf>,
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
}

impl ScriptedHost {
    /// A host without Splitty: this version is installed, the handshake
    /// starts a daemon of it with one screen, and the bridge greets with
    /// [`WELCOME`] and then echoes every frame. `dir` holds its files.
    pub fn new(dir: &std::path::Path) -> Self {
        let welcome = dir.join("welcome.frame");
        std::fs::write(
            &welcome,
            encode(&Frame::Text(WELCOME.to_owned())).expect("frame"),
        )
        .expect("write the welcome frame");
        Self {
            prepare: Prepare::Installs("x86_64"),
            server_version: VERSION,
            protocol: PROTOCOL_VERSION,
            stops: true,
            bridge: r#"cat "$1" && exec cat"#,
            welcome: Arc::new(welcome),
            daemon: Arc::default(),
            asked: Arc::default(),
            bridge_pid: Arc::default(),
            forward_fails: None,
            failing_forwards: Arc::default(),
            forwards: Arc::default(),
            forward_gates: Arc::default(),
            stop_gates: Arc::default(),
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

    pub fn with_bridge(mut self, script: &'static str) -> Self {
        self.bridge = script;
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
    /// `handshake <port hint>`, `stop`, `bridge`, `forward <port> at <local
    /// port>`.
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
            Ok(Prepared {
                server: Server::Installed {
                    path: format!("/home/u/.local/share/splitty/remote/{VERSION}/splitty"),
                    config_dir: None,
                },
                probe: Probe {
                    kernel: "Linux".into(),
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
        port_hint: u16,
    ) -> BoxFuture<'a, Result<Handshake, HubError>> {
        Box::pin(async move {
            self.ask(format!("handshake {port_hint}"));
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
                "handshake": 1,
                "server": {"version": self.server_version, "protocol": self.protocol, "path": "/p"},
                "daemon": {
                    "running": true, "pid": 4711, "port": port_hint, "version": daemon.version,
                    "protocol": daemon.protocol, "startedAt": "1760",
                    "keepsSessions": daemon.keeps_sessions, "started": started,
                },
                "screens": [{"id": "s1", "name": "api", "panes": 3}],
            });
            let answer = serde_json::to_vec(&answer).expect("JSON");
            Ok(handshake::parse(target.as_str(), &answer)?)
        })
    }

    fn stop_daemon<'a>(
        &'a self,
        _target: &'a SshTarget,
        _server: &'a Server,
    ) -> BoxFuture<'a, Result<(), HubError>> {
        Box::pin(async move {
            self.ask("stop".to_owned());
            if self.stops {
                *self.daemon.lock().expect("daemon") = None;
            }
            Ok(())
        })
    }

    fn bridge(&self, _target: &SshTarget, _server: &Server) -> Result<Bridge, HubError> {
        self.ask("bridge".to_owned());
        let mut command = tokio::process::Command::new("sh");
        command
            .arg("-c")
            .arg(self.bridge)
            .arg("bridge")
            .arg(self.welcome.as_path());
        let mut bridge =
            Bridge::spawn(command).map_err(|e| HubError::Bridge(SshError::Spawn(e)))?;
        *self.bridge_pid.lock().expect("bridge pid") = bridge.process().id();
        Ok(bridge)
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

            drop(local);
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
                .await
                .map_err(HubError::ForwardPort)?;
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
