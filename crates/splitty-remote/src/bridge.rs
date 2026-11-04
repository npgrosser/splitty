//! Owned native-v1 SSH/desktop pipes: negotiated service identity, bounded
//! delivery credit and one child lifetime. Browser credentials and TCP listeners
//! do not belong to this transport.

use std::io;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use splitty_transport::{
    admission,
    budget::{Charged, ConnectionBudget, Lease},
    negotiation,
    peer::{Incoming, Peer, Sender},
    queue, wire,
    worker::Worker,
};
use tokio::io::{AsyncRead, BufReader, ReadBuf};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::task::JoinHandle;
use tokio::time::{timeout, Instant};

use crate::install::Server;
use crate::ssh::{diagnostic_budget, keep_end, kept_end, kept_end_for, Diagnostic, Ssh, SshTarget};

/// The owned child and its bounded diagnostic drain.
pub struct BridgeProcess {
    child: Worker,
    stderr: Option<JoinHandle<Diagnostic>>,
    budget: ConnectionBudget,
}

/// How a bridge's process ended.
#[derive(Debug)]
pub struct Ended {
    /// Its exit status, unless waiting for it failed.
    pub status: io::Result<ExitStatus>,
    /// The end of its error output, trimmed.
    pub stderr: Diagnostic,
    /// Owner budget used for any derived retained diagnostics.
    pub budget: ConnectionBudget,
}

#[derive(Debug)]
struct OpenFailure {
    source: io::Error,
    ended: Ended,
}
impl std::fmt::Display for OpenFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.source)?;
        if !self.ended.stderr.is_empty() {
            write!(f, ": {}", self.ended.stderr)?;
        }
        Ok(())
    }
}
impl std::error::Error for OpenFailure {}

/// Keep SSH authentication/host-key diagnostics typed across pipe negotiation.
/// A framing error is not mistaken for an executable-spawn failure.
pub fn ssh_failure(host: &str, error: &io::Error) -> Option<crate::ssh::SshError> {
    // An observed framing/identity failure is more precise than a generic
    // process exit255. Keep stale identity typed for rediscovery too.
    if protocol_failure(error) || stale_instance(error) {
        return None;
    }
    let failure = error.get_ref()?.downcast_ref::<OpenFailure>()?;
    let status = failure.ended.status.as_ref().ok()?;
    (status.code() == Some(255)).then(|| {
        crate::ssh::classify_budgeted(host, *status, &failure.ended.stderr, &failure.ended.budget)
    })
}

pub fn stale_instance(error: &io::Error) -> bool {
    let source = error
        .get_ref()
        .and_then(|source| source.downcast_ref::<OpenFailure>())
        .map_or(error, |failure| &failure.source);
    negotiation::stale_instance(source)
}

pub fn protocol_failure(error: &io::Error) -> bool {
    !stale_instance(error)
        && matches!(
            error.kind(),
            io::ErrorKind::InvalidData
                | io::ErrorKind::InvalidInput
                | io::ErrorKind::Unsupported
                | io::ErrorKind::PermissionDenied
        )
}

// The buffer reservation belongs to the reader itself, including while a
// peer task is being cancelled after its process owner has been dropped.
struct BridgeOutput {
    inner: BufReader<ChildStdout>,
    _bytes: Lease,
}
impl AsyncRead for BridgeOutput {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_read(cx, buffer)
    }
}

#[cfg(test)]
fn spawn_pipes(command: Command) -> io::Result<(ChildStdin, BridgeOutput, BridgeProcess)> {
    spawn_pipes_budgeted(command, &ConnectionBudget::default())
}

fn spawn_pipes_budgeted(
    mut command: Command,
    budget: &ConnectionBudget,
) -> io::Result<(ChildStdin, BridgeOutput, BridgeProcess)> {
    const READER_BYTES: usize = 8192;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let diagnostic_bytes = diagnostic_budget(budget)?;
    let reader_bytes = budget
        .reserve(READER_BYTES, false)
        .map_err(|error| io::Error::new(io::ErrorKind::WouldBlock, error))?;
    let mut child = Worker::spawn(&mut command, &admission::Admission::global())?;
    let not_piped = || io::Error::other("the bridge's standard streams are not piped");
    let stdin = child.stdin().ok_or_else(not_piped)?;
    let stdout = child.stdout().ok_or_else(not_piped)?;
    let stderr = child
        .stderr()
        .map(|stderr| tokio::spawn(keep_end(stderr, diagnostic_bytes)));
    let reader = BridgeOutput {
        inner: BufReader::with_capacity(READER_BYTES, stdout),
        _bytes: reader_bytes,
    };
    Ok((
        stdin,
        reader,
        BridgeProcess {
            child,
            stderr,
            budget: budget.clone(),
        },
    ))
}

/// A negotiated native exec bridge. It uses the shared framing, admission,
/// delivery-credit and liveness implementation, including for API operations.
pub struct NativeBridge {
    ready: Option<Charged<wire::Ready>>,
    live: Option<NativeConnection>,
}

struct NativeConnection {
    peer: Option<Peer>,
    owner_watch: Option<JoinHandle<()>>,
    process: BridgeProcess,
    owner_cleanup: bool,
    _carrier: admission::Carrier,
    opening: Option<admission::Opening>,
    stream: Option<admission::Stream>,
}

impl NativeConnection {
    async fn finish(mut self) -> (io::Result<()>, Ended) {
        // Pipe/task shutdown and diagnostics consume the same app-owned exit
        // budget. Reserve a small margin within the public three-second bound.
        let owner_deadline = self
            .owner_cleanup
            .then(|| Instant::now() + Duration::from_millis(2900));
        let result = match self.peer.take() {
            Some(peer) => peer.finish().await,
            None => Ok(()),
        };
        if let Some(watch) = self.owner_watch.take() {
            let _ = watch.await;
        }
        let ended = match owner_deadline {
            Some(deadline) => self.process.finish_owner(deadline).await,
            None => self.process.finish(Duration::from_millis(500)).await,
        };
        (result, ended)
    }
}

impl NativeBridge {
    /// `assignment` comes from the owning broker, never from host metadata.
    /// Expected instance and mux version come from a checked host handshake.
    pub async fn open(
        target: &SshTarget,
        server: &Server,
        assignment: admission::Assignment,
        expected: negotiation::Expected<'_>,
    ) -> io::Result<Self> {
        let admission = admission::Admission::global();
        let carrier = admission.carrier().map_err(io::Error::other)?;
        let opening = admission.begin(assignment).map_err(io::Error::other)?;
        Self::open_admitted(target, server, (carrier, opening), expected).await
    }

    pub async fn open_admitted(
        target: &SshTarget,
        server: &Server,
        capacity: (admission::Carrier, admission::Opening),
        expected: negotiation::Expected<'_>,
    ) -> io::Result<Self> {
        let ssh = Ssh::stdio_only(target.clone());
        Self::open_with_ssh(&ssh, server, capacity, expected).await
    }

    pub async fn open_context(
        target: &SshTarget,
        server: &Server,
        capacity: (admission::Carrier, admission::Opening),
        context: queue::Context,
        expected: negotiation::Expected<'_>,
    ) -> io::Result<Self> {
        let ssh = Ssh::stdio_only(target.clone()).with_context(context);
        Self::open_with_ssh(&ssh, server, capacity, expected).await
    }

    pub(crate) async fn open_with_ssh(
        ssh: &Ssh,
        server: &Server,
        capacity: (admission::Carrier, admission::Opening),
        expected: negotiation::Expected<'_>,
    ) -> io::Result<Self> {
        let command = open_command(ssh, server);
        Self::spawn_context(command, capacity, expected, ssh.context(), |_| {}).await
    }

    /// Also used by owned desktop helpers; the caller constructs the executable
    /// and arguments without interpreting a renderer-provided command line.
    pub async fn spawn(
        command: Command,
        assignment: admission::Assignment,
        expected: negotiation::Expected<'_>,
        progress: impl FnMut(Charged<wire::Progress>),
    ) -> io::Result<Self> {
        let admission = admission::Admission::global();
        let carrier = admission.carrier().map_err(io::Error::other)?;
        let opening = admission.begin(assignment).map_err(io::Error::other)?;
        Self::spawn_admitted(command, (carrier, opening), expected, progress).await
    }

    pub async fn spawn_admitted(
        command: Command,
        (carrier, opening): (admission::Carrier, admission::Opening),
        expected: negotiation::Expected<'_>,
        progress: impl FnMut(Charged<wire::Progress>),
    ) -> io::Result<Self> {
        Self::spawn_context(
            command,
            (carrier, opening),
            expected,
            queue::Context::default(),
            progress,
        )
        .await
    }

    /// Desktop brokers supply their document-owned context so revocation also
    /// interrupts child preparation, and IPC and pipe bytes share one budget.
    pub async fn spawn_context(
        command: Command,
        (carrier, opening): (admission::Carrier, admission::Opening),
        expected: negotiation::Expected<'_>,
        context: queue::Context,
        progress: impl FnMut(Charged<wire::Progress>),
    ) -> io::Result<Self> {
        if context.is_closed() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "native owner closed",
            ));
        }
        // All opening/carrier and OS-worker capacity is held before spawning.
        let (mut input, mut output, process) = spawn_pipes_budgeted(command, &context.budget)?;
        let mut bridge = Self {
            ready: None,
            live: Some(NativeConnection {
                peer: None,
                owner_watch: None,
                process,
                owner_cleanup: matches!(expected.purpose, wire::Purpose::Owner),
                _carrier: carrier,
                opening: Some(opening),
                stream: None,
            }),
        };
        let mut closing = context.closing();
        let result = tokio::select! { biased;
            _ = closing.wait_for(|closed| *closed) => Err(io::Error::new(io::ErrorKind::BrokenPipe, "native owner closed")),
            ready = async {
                negotiation::client(&mut output, &mut input, &context.budget).await?;
                negotiation::accept(&mut output, &mut input, &context.budget, expected, progress).await
            } => ready,
        };
        // A progress callback can revoke the owner during the same poll that
        // consumes an already-buffered Ready; select does not repoll close.
        let result = result.and_then(|ready| {
            if context.is_closed() {
                Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "native owner closed",
                ))
            } else {
                Ok(ready)
            }
        });
        let ready = match result {
            Ok(ready) => ready,
            Err(error) => {
                drop(input);
                drop(output);
                let (_, ended) = bridge.finish().await;
                return Err(io::Error::new(
                    error.kind(),
                    OpenFailure {
                        source: error,
                        ended,
                    },
                ));
            }
        };
        let live = bridge.live.as_mut().expect("owned bridge");
        live.stream = Some(
            live.opening
                .take()
                .expect("opening capacity")
                .admit()
                .map_err(io::Error::other)?,
        );
        // Owner cancellation flows into this hop, not backwards. A malformed
        // host frame must remain a reportable backend failure rather than
        // masquerading as cancellation of the document preparing that host.
        let peer_context = context.child();
        let peer_closing = peer_context.clone();
        live.owner_watch = Some(tokio::spawn(async move {
            let mut owner = context.closing();
            let mut peer = peer_closing.closing();
            tokio::select! { biased;
                _ = owner.wait_for(|closed| *closed) => peer_closing.close(),
                _ = peer.wait_for(|closed| *closed) => {},
            }
        }));
        live.peer = Some(Peer::start(output, input, &ready, peer_context));
        bridge.ready = Some(ready);
        Ok(bridge)
    }

    pub fn ready(&self) -> &wire::Ready {
        self.ready.as_ref().expect("admitted native bridge")
    }
    pub fn sender(&self) -> Sender {
        self.live
            .as_ref()
            .and_then(|live| live.peer.as_ref())
            .expect("admitted native bridge")
            .sender
            .clone()
    }
    pub async fn recv(&mut self) -> Option<Charged<Incoming>> {
        self.live.as_mut()?.peer.as_mut()?.recv().await
    }
    pub fn id(&self) -> Option<u32> {
        self.live.as_ref().and_then(|live| live.process.id())
    }
    pub async fn finish(mut self) -> (io::Result<()>, Ended) {
        let live = self.live.take().expect("owned bridge");
        // Abandoning a completion wait must not release the stream/carrier
        // before peer cleanup and observed child exit. The worker alone only
        // retains the OS-worker quota, not the surrounding connection slots.
        match tokio::spawn(live.finish()).await {
            Ok(ended) => ended,
            Err(error) => (
                Err(io::Error::other("native cleanup task failed")),
                Ended {
                    status: Err(io::Error::other(error)),
                    stderr: Default::default(),
                    budget: ConnectionBudget::default(),
                },
            ),
        }
    }
}

impl Drop for NativeBridge {
    fn drop(&mut self) {
        if let Some(live) = self.live.take() {
            if let Some(peer) = &live.peer {
                peer.sender.close();
            }
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = live.finish().await;
                });
            }
        }
    }
}

/// `ssh <options> <target> -- '<server>' remote-bridge`.
fn open_command(ssh: &Ssh, server: &Server) -> Command {
    let line = server
        .command()
        .word("remote-bridge")
        .word("--transport")
        .word("native-v1");
    let mut command = ssh.command();
    command
        .arg(ssh.target().as_str())
        .arg("--")
        .arg(line.as_str());
    command
}

impl BridgeProcess {
    /// The process id, while it runs.
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    /// Wait for the process to end.
    pub async fn ended(&mut self) -> Ended {
        let status = self.child.wait().await;
        let stderr = kept_end(self.stderr.take()).await;
        Ended {
            status,
            stderr,
            budget: self.budget.clone(),
        }
    }

    /// Kill the process and wait for it.
    pub async fn kill(&mut self) -> Ended {
        // Fails only when the process has already been waited for.
        let _ = self.child.start_kill();
        self.ended().await
    }

    async fn finish_owner(&mut self, deadline: Instant) -> Ended {
        // Owned PTY lifecycle retains its child through a 500ms escalation
        // grace before reaping. The relay's identical 500ms kill races cleanup.
        let grace =
            Duration::from_millis(2750).min(deadline.saturating_duration_since(Instant::now()));
        let status = match timeout(grace, self.child.wait()).await {
            Ok(status) => status,
            Err(_) => {
                let _ = self.child.start_kill();
                self.child.wait().await
            }
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        let stderr = kept_end_for(self.stderr.take(), remaining).await;
        Ended {
            status,
            stderr,
            budget: self.budget.clone(),
        }
    }

    /// End a bridge whose input is closed: it gets `grace` to
    /// finish on its own, then it is killed.
    pub async fn finish(&mut self, grace: Duration) -> Ended {
        match timeout(grace, self.child.wait()).await {
            Ok(_) => self.ended().await,
            Err(_) => self.kill().await,
        }
    }
}

impl Drop for BridgeProcess {
    fn drop(&mut self) {
        if let Some(task) = self.stderr.take() {
            task.abort();
        }
        // Worker retains its capacity until its reaper observes exit.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bridge runs the installed server by its quoted path, over a
    /// connection that forwards nothing.
    #[test]
    fn the_bridge_runs_the_hosts_server_over_a_stdio_only_connection() {
        let ssh = Ssh::stdio_only(SshTarget::parse("user@vm").expect("target"));
        let server = Server::Installed {
            path: "/home/it's me/.local/share/splitty/remote/0.1.0/splitty".into(),
            config_dir: None,
        };
        let args: Vec<String> = open_command(&ssh, &server)
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let mut expected = crate::ssh::hardening_options();
        expected.extend(crate::ssh::stdio_only_options());
        expected.extend([
            "user@vm".to_owned(),
            "--".to_owned(),
            r"'/home/it'\''s me/.local/share/splitty/remote/0.1.0/splitty' remote-bridge --transport native-v1"
                .to_owned(),
        ]);
        assert_eq!(args, expected);
    }

    #[cfg(unix)]
    fn sh(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.arg("-c").arg(script);
        command
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_native_bridge_negotiates_and_relays_in_order_until_pipe_eof() {
        let ready = wire::Ready {
            generation: 1,
            instance: "a".repeat(32),
            mux_protocol: 1,
            purpose: wire::Purpose::Direct,
            limits: wire::Limits::default(),
        };
        let mut command = Command::new("python3");
        command
            .args(["-u", "-c", include_str!("../tests/native_bridge.py")])
            .arg(serde_json::to_string(&ready).unwrap())
            .arg(r#"{"type":"mux.welcome"}"#);
        let context = queue::Context::default();
        let admission = admission::Admission::global();
        let capacity = (
            admission.carrier().unwrap(),
            admission
                .begin(admission::Assignment::new(
                    admission::Backend::new(),
                    admission::Domain::new(),
                ))
                .unwrap(),
        );
        let mut bridge = NativeBridge::spawn_context(
            command,
            capacity,
            negotiation::Expected {
                instance: Some(&ready.instance),
                purpose: &ready.purpose,
                mux_protocol: 1,
                preparation: wire::HANDSHAKE_TIMEOUT,
            },
            context.clone(),
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(bridge.ready(), &ready);
        assert!(std::sync::Arc::ptr_eq(
            &context.budget.data,
            &bridge.sender().context().budget.data
        ));
        assert!(std::sync::Arc::ptr_eq(
            &context.budget.control,
            &bridge.sender().context().budget.control
        ));
        let welcome = bridge.recv().await.unwrap();
        assert_eq!(
            welcome.frame.application().unwrap().1,
            br#"{"type":"mux.welcome"}"#
        );
        drop(welcome);
        let input = br#"{"type":"input","sessionId":"p","data":"test"}"#;
        bridge.sender().data(wire::Kind::Text, input).unwrap();
        let echo = bridge.recv().await.unwrap();
        assert_eq!(echo.frame.application().unwrap().1, input);
        drop(echo);
        let binary = bridge.recv().await.unwrap();
        assert_eq!(binary.frame.kind, wire::Kind::Binary);
        assert_eq!(binary.frame.application().unwrap().1, &[0, 1, 2, 255]);
        drop(binary);
        let (result, ended) = bridge.finish().await;
        result.unwrap();
        assert!(ended.status.unwrap().success(), "{}", ended.stderr);
    }

    /// The app-owned helper must finish its lifecycle reaper after EOF. An
    /// ordinary SSH relay retains the shorter teardown grace.
    #[cfg(unix)]
    #[tokio::test]
    async fn embedded_owner_cleanup_outlives_the_ordinary_relay_grace() {
        for (purpose, cleans_up) in [(wire::Purpose::Owner, true), (wire::Purpose::Direct, false)] {
            let directory = tempfile::tempdir().unwrap();
            let marker = directory.path().join("cleaned");
            let ready = wire::Ready {
                generation: 1,
                instance: "a".repeat(32),
                mux_protocol: 1,
                purpose,
                limits: wire::Limits::default(),
            };
            let mut command = Command::new("python3");
            command
                .args(["-u", "-c", include_str!("../tests/native_bridge.py")])
                .arg(serde_json::to_string(&ready).unwrap())
                .arg("__OWNER_CLEANUP__")
                .arg(&marker);
            let admission = admission::Admission::global();
            let capacity = (
                admission.carrier().unwrap(),
                admission
                    .begin(admission::Assignment::new(
                        admission::Backend::new(),
                        admission::Domain::new(),
                    ))
                    .unwrap(),
            );
            let context = queue::Context::default();
            let bridge = NativeBridge::spawn_context(
                command,
                capacity,
                negotiation::Expected {
                    instance: Some(&ready.instance),
                    purpose: &ready.purpose,
                    mux_protocol: 1,
                    preparation: wire::HANDSHAKE_TIMEOUT,
                },
                context.clone(),
                |_| {},
            )
            .await
            .unwrap();
            let (_, ended) = tokio::time::timeout(Duration::from_secs(3), bridge.finish())
                .await
                .unwrap();
            assert_eq!(
                ended.status.as_ref().unwrap().success(),
                cleans_up,
                "{ended:?}"
            );
            assert_eq!(
                marker.exists(),
                cleans_up,
                "only owner cleanup completes before observed child exit"
            );
            drop(ended);
            assert_eq!(context.budget.used(), 0);
        }
    }

    /// A second exact owned process holds a real stderr pipe open. Its drain
    /// must consume the owner's remaining deadline, not add two more seconds.
    #[cfg(unix)]
    #[tokio::test]
    async fn owner_exit_deadline_includes_a_stderr_pipe_held_open_after_child_exit() {
        let ready = wire::Ready {
            generation: 1,
            instance: "a".repeat(32),
            mux_protocol: 1,
            purpose: wire::Purpose::Owner,
            limits: wire::Limits::default(),
        };
        let mut command = Command::new("python3");
        command
            .args(["-u", "-c", include_str!("../tests/native_bridge.py")])
            .arg(serde_json::to_string(&ready).unwrap())
            .arg("__IGNORE_EOF__");
        let admission = admission::Admission::global();
        let capacity = (
            admission.carrier().unwrap(),
            admission
                .begin(admission::Assignment::new(
                    admission::Backend::new(),
                    admission::Domain::new(),
                ))
                .unwrap(),
        );
        let context = queue::Context::default();
        let mut bridge = NativeBridge::spawn_context(
            command,
            capacity,
            negotiation::Expected {
                instance: None,
                purpose: &ready.purpose,
                mux_protocol: 1,
                preparation: wire::HANDSHAKE_TIMEOUT,
            },
            context.clone(),
            |_| {},
        )
        .await
        .unwrap();
        let mut holder = Command::new("sleep")
            .arg("30")
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let process = &mut bridge.live.as_mut().unwrap().process;
        let original = process.stderr.take().unwrap();
        original.abort();
        let _ = original.await;
        process.stderr = Some(tokio::spawn(keep_end(
            holder.stderr.take().unwrap(),
            diagnostic_budget(&context.budget).unwrap(),
        )));
        let result = tokio::time::timeout(Duration::from_secs(3), bridge.finish()).await;
        // Retain the exact holder handle and reap it even if the assertion fails.
        holder.kill().await.unwrap();
        let (_, ended) =
            result.expect("child exit plus pending stderr drain fit the owner deadline");
        assert!(!ended.status.as_ref().unwrap().success());
        drop(ended);
        assert_eq!(
            context.budget.used(),
            0,
            "aborted drain releases its actual storage"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn backend_failure_does_not_revoke_its_owner_but_owner_close_rejects_input_immediately() {
        for malformed in [true, false] {
            let ready = wire::Ready {
                generation: 1,
                instance: "a".repeat(32),
                mux_protocol: 1,
                purpose: wire::Purpose::Direct,
                limits: wire::Limits::default(),
            };
            let mut command = Command::new("python3");
            command
                .args(["-u", "-c", include_str!("../tests/native_bridge.py")])
                .arg(serde_json::to_string(&ready).unwrap())
                .arg(if malformed {
                    "__INVALID_FRAME_EXIT_255__"
                } else {
                    "__IGNORE_EOF__"
                });
            let context = queue::Context::default();
            let admission = std::sync::Arc::new(admission::Admission::default());
            let capacity = (
                admission.carrier().unwrap(),
                admission
                    .begin(admission::Assignment::new(
                        admission::Backend::new(),
                        admission::Domain::new(),
                    ))
                    .unwrap(),
            );
            let mut bridge = NativeBridge::spawn_context(
                command,
                capacity,
                negotiation::Expected {
                    instance: Some(&ready.instance),
                    purpose: &ready.purpose,
                    mux_protocol: 1,
                    preparation: wire::HANDSHAKE_TIMEOUT,
                },
                context.clone(),
                |_| {},
            )
            .await
            .unwrap();
            if malformed {
                assert!(tokio::time::timeout(Duration::from_secs(3), bridge.recv())
                    .await
                    .unwrap()
                    .is_none());
                assert!(
                    !context.is_closed(),
                    "the owner can still deliver the backend error"
                );
                let (result, ended) = bridge.finish().await;
                assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
                assert!(ended.status.is_ok());
            } else {
                context.close();
                assert!(
                    bridge.sender().data(wire::Kind::Text, b"stale").is_err(),
                    "revocation needs no watcher scheduling turn"
                );
                let _ = bridge.finish().await;
            }
            assert_eq!(context.budget.used(), 0);
            assert_eq!(admission.counts().streams, 0);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn abandoned_finish_wait_keeps_connection_slots_until_child_exit() {
        let ready = wire::Ready {
            generation: 1,
            instance: "a".repeat(32),
            mux_protocol: 1,
            purpose: wire::Purpose::Direct,
            limits: wire::Limits::default(),
        };
        let mut command = Command::new("python3");
        command
            .args(["-u", "-c", include_str!("../tests/native_bridge.py")])
            .arg(serde_json::to_string(&ready).unwrap())
            .arg("__IGNORE_EOF__");
        let context = queue::Context::default();
        let admission = std::sync::Arc::new(admission::Admission::default());
        let capacity = (
            admission.carrier().unwrap(),
            admission
                .begin(admission::Assignment::new(
                    admission::Backend::new(),
                    admission::Domain::new(),
                ))
                .unwrap(),
        );
        let bridge = NativeBridge::spawn_context(
            command,
            capacity,
            negotiation::Expected {
                instance: Some(&ready.instance),
                purpose: &ready.purpose,
                mux_protocol: 1,
                preparation: wire::HANDSHAKE_TIMEOUT,
            },
            context.clone(),
            |_| {},
        )
        .await
        .unwrap();
        let pid = bridge.id().unwrap();
        let peer_context = bridge.sender().context();
        let wait = tokio::spawn(bridge.finish());
        let mut closing = peer_context.closing();
        closing.wait_for(|closed| *closed).await.unwrap();
        wait.abort();
        let _ = wait.await;
        assert_eq!(admission.counts().carriers, 1);
        assert_eq!(admission.counts().streams, 1);
        tokio::time::timeout(Duration::from_secs(3), async {
            while admission.counts().carriers != 0 || context.budget.used() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("background cleanup finishes despite abandoned completion wait");
        assert_eq!(admission.counts().streams, 0);
        assert!(!Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .await
            .unwrap()
            .success());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bridge_buffers_are_admitted_before_spawn_and_retained_by_their_consumers() {
        use splitty_transport::budget::ProcessBudget;
        let total = 1024 * 1024;
        let aggregate = ProcessBudget::new(total, 0);
        let budget = ConnectionBudget::new(aggregate.clone(), total, 0);
        let held = aggregate.data.acquire(total).unwrap();
        let fixture = tempfile::tempdir().unwrap();
        let marker = fixture.path().join("started");
        let mut command = Command::new("touch");
        command.arg(&marker);
        let error = match spawn_pipes_budgeted(command, &budget) {
            Ok(_) => panic!("a full byte budget must refuse the child"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert!(!marker.exists());
        assert_eq!(budget.used(), 0);
        drop(held);
        let mut command = Command::new("python3");
        command.args(["-c", "import sys; sys.stderr.buffer.write(b'\\xff' * 8192)"]);
        let (input, output, mut process) = spawn_pipes_budgeted(command, &budget).unwrap();
        drop(input);
        let ended = process.ended().await;
        assert!(ended.status.as_ref().unwrap().success());
        drop(process);
        assert_eq!(ended.stderr.as_str(), "\u{fffd}".repeat(8192));
        let diagnostic_bytes = ended.stderr.capacity() + std::mem::size_of::<String>();
        assert_eq!(budget.used(), 8192 + diagnostic_bytes);
        drop(output);
        assert_eq!(
            aggregate.used(),
            diagnostic_bytes,
            "process exit does not release an unconsumed diagnostic"
        );
        drop(ended);
        assert_eq!(aggregate.used(), 0);

        let (input, output, process) = spawn_pipes_budgeted(sh("exec sleep 30"), &budget).unwrap();
        drop((input, output, process));
        tokio::time::timeout(Duration::from_secs(3), async {
            while aggregate.used() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("aborted stderr readers release their storage after cancellation");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn owner_revocation_rejects_spawn_and_reaps_an_incomplete_bridge() {
        use splitty_transport::budget::ProcessBudget;
        let aggregate = ProcessBudget::new(wire::PROCESS_BYTES, wire::PROCESS_CONTROL_BYTES);
        let context = queue::Context::new(ConnectionBudget::new(
            aggregate.clone(),
            wire::CONNECTION_BYTES,
            wire::CONNECTION_CONTROL_BYTES,
        ));
        let capacity = std::sync::Arc::new(admission::Admission::default());
        let reserve = || {
            (
                capacity.carrier().unwrap(),
                capacity
                    .begin(admission::Assignment::new(
                        admission::Backend::new(),
                        admission::Domain::new(),
                    ))
                    .unwrap(),
            )
        };
        let expected = || negotiation::Expected {
            instance: None,
            purpose: &wire::Purpose::Direct,
            mux_protocol: 1,
            preparation: wire::HANDSHAKE_TIMEOUT,
        };
        let fixture = tempfile::tempdir().unwrap();
        let marker = fixture.path().join("child.pid");
        let command = || {
            let mut child = Command::new("sh");
            child
                .args(["-c", "echo $$ > \"$1\"; exec sleep 30", "sh"])
                .arg(&marker);
            child
        };
        let closed = queue::Context::default();
        closed.close();
        let error =
            match NativeBridge::spawn_context(command(), reserve(), expected(), closed, |_| {})
                .await
            {
                Ok(_) => panic!("revoked owner must not spawn"),
                Err(error) => error,
            };
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert!(!marker.exists());
        assert_eq!(capacity.counts().carriers, 0);
        assert_eq!(capacity.counts().streams, 0);
        let pending =
            NativeBridge::spawn_context(command(), reserve(), expected(), context.clone(), |_| {});
        let cancel = async {
            let pid = tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if let Ok(pid) = tokio::fs::read_to_string(&marker).await {
                        if let Ok(pid) = pid.trim().parse::<u32>() {
                            break pid;
                        }
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("child started");
            context.close();
            pid
        };
        let (result, pid) = tokio::join!(pending, cancel);
        let error = match result {
            Ok(_) => panic!("revoked handshake admitted"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        let failure = error
            .get_ref()
            .unwrap()
            .downcast_ref::<OpenFailure>()
            .unwrap();
        assert!(!failure.ended.status.as_ref().unwrap().success());
        assert!(
            !Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stderr(Stdio::null())
                .status()
                .await
                .unwrap()
                .success(),
            "cleanup observed child exit before returning"
        );
        drop(error);
        assert_eq!(aggregate.used(), 0);
        assert_eq!(capacity.counts().carriers, 0);
        assert_eq!(capacity.counts().streams, 0);
        assert_eq!(capacity.counts().opening, 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn revocation_in_progress_rejects_buffered_ready_and_reaps_before_return() {
        let ready = wire::Ready {
            generation: 1,
            instance: "a".repeat(32),
            mux_protocol: 1,
            purpose: wire::Purpose::Direct,
            limits: wire::Limits::default(),
        };
        let mut command = Command::new("python3");
        command
            .args(["-u", "-c", include_str!("../tests/native_bridge.py")])
            .arg(serde_json::to_string(&ready).unwrap())
            .arg("__REVOKE_DURING_PROGRESS__");
        let context = queue::Context::default();
        let admission = std::sync::Arc::new(admission::Admission::default());
        let capacity = (
            admission.carrier().unwrap(),
            admission
                .begin(admission::Assignment::new(
                    admission::Backend::new(),
                    admission::Domain::new(),
                ))
                .unwrap(),
        );
        let mut reported = false;
        let result = NativeBridge::spawn_context(
            command,
            capacity,
            negotiation::Expected {
                instance: Some(&ready.instance),
                purpose: &ready.purpose,
                mux_protocol: 1,
                preparation: wire::HANDSHAKE_TIMEOUT,
            },
            context.clone(),
            |_| {
                reported = true;
                context.close();
            },
        )
        .await;
        assert!(reported);
        let error = match result {
            Ok(_) => panic!("revoked buffered Ready admitted"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        let failure = error
            .get_ref()
            .unwrap()
            .downcast_ref::<OpenFailure>()
            .unwrap();
        assert!(
            !failure.ended.status.as_ref().unwrap().success(),
            "cleanup awaited the uncooperative child"
        );
        drop(error);
        assert_eq!(admission.counts().carriers, 0);
        assert_eq!(admission.counts().streams, 0);
        assert_eq!(admission.counts().opening, 0);
        assert_eq!(context.budget.used(), 0);
    }

    /// What the host's bridge says when it fails is kept for the message.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_bridge_leaves_its_status_and_error_output() {
        let (input, output, mut process) = spawn_pipes(sh(
            "echo 'No Splitty daemon is running on this host.' >&2; exit 1",
        ))
        .expect("spawn");
        drop(input);
        drop(output);
        let ended = process.ended().await;
        assert_eq!(ended.status.expect("status").code(), Some(1));
        assert_eq!(ended.stderr, "No Splitty daemon is running on this host.");
    }

    /// Only the end of a long error output is kept.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_end_of_a_long_error_output_is_kept() {
        let (input, output, mut process) = spawn_pipes(sh(
            "i=0; while [ $i -lt 2000 ]; do echo 'debug1: noise' >&2; i=$((i+1)); done; \
             echo 'the reason' >&2",
        ))
        .expect("spawn");
        drop(input);
        drop(output);
        let ended = process.ended().await;
        assert!(
            ended.stderr.len() <= crate::ssh::STDERR_KEPT,
            "{}",
            ended.stderr.len()
        );
        assert!(ended.stderr.ends_with("the reason"), "{}", ended.stderr);
    }

    /// A bridge that does not end with its input is killed after the grace
    /// period.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_bridge_that_ignores_its_input_is_killed() {
        let (input, output, mut process) = spawn_pipes(sh("exec sleep 30")).expect("spawn");
        drop(input);
        drop(output);
        let started = std::time::Instant::now();
        let ended = process.finish(Duration::from_millis(200)).await;
        assert!(!ended.status.expect("status").success(), "killed");
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(process.id(), None, "the process is gone");
    }
}
