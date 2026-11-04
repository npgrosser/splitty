//! One admitted document stream, joined to its native-assigned backend.
//! The WebView carrier only moves bounded chunks; framing, direction checks,
//! cancellation and process cleanup stay in this native task.
use super::{pipe, Attempt, BackendId};
use splitty_remote::bridge::NativeBridge;
use splitty_transport::{
    budget::Charged,
    peer::{Incoming, Peer, Sender},
    wire::{self, Kind},
};
use std::{io, sync::Arc};
use tokio::sync::oneshot;

pub(crate) enum Backend {
    #[cfg(not(target_os = "windows"))]
    Service(Box<splitty_server::native::client::Client>),
    #[cfg(test)]
    Child(Box<NativeBridge>),
    #[cfg(any(target_os = "windows", test))]
    Owned(Box<splitty_remote::owner::Connection>),
    Host(Box<AssignedHost>),
}
pub(crate) struct AssignedHost {
    bridge: NativeBridge,
    ready: Charged<wire::Ready>,
}
impl Backend {
    pub(super) fn host(bridge: NativeBridge, ready: Charged<wire::Ready>) -> Self {
        Self::Host(Box::new(AssignedHost { bridge, ready }))
    }
    pub(crate) fn ready(&self) -> &wire::Ready {
        match self {
            #[cfg(not(target_os = "windows"))]
            Self::Service(client) => &client.ready,
            #[cfg(test)]
            Self::Child(bridge) => bridge.ready(),
            #[cfg(any(target_os = "windows", test))]
            Self::Owned(connection) => connection.ready(),
            Self::Host(host) => &host.ready,
        }
    }
    fn sender(&self) -> Sender {
        match self {
            #[cfg(not(target_os = "windows"))]
            Self::Service(client) => client.sender(),
            #[cfg(test)]
            Self::Child(bridge) => bridge.sender(),
            Self::Host(host) => host.bridge.sender(),
            #[cfg(any(target_os = "windows", test))]
            Self::Owned(connection) => connection.sender(),
        }
    }
    async fn recv(&mut self) -> Option<Charged<Incoming>> {
        match self {
            #[cfg(not(target_os = "windows"))]
            Self::Service(client) => client.recv().await,
            #[cfg(test)]
            Self::Child(bridge) => bridge.recv().await,
            Self::Host(host) => host.bridge.recv().await,
            #[cfg(any(target_os = "windows", test))]
            Self::Owned(connection) => connection.recv().await,
        }
    }
    pub(crate) async fn finish(self) -> (io::Result<()>, bool) {
        match self {
            #[cfg(not(target_os = "windows"))]
            Self::Service(client) => (client.finish().await, true),
            #[cfg(any(target_os = "windows", test))]
            Self::Owned(connection) => (connection.finish().await, true),
            #[cfg(test)]
            Self::Child(bridge) => child_finish(*bridge).await,
            Self::Host(host) => child_finish(host.bridge).await,
        }
    }
}
async fn child_finish(bridge: NativeBridge) -> (io::Result<()>, bool) {
    let (result, ended) = bridge.finish().await;
    // A transport error does not imply failed reaping. Conversely,
    // a wait error must not release the document's worker quota.
    let reaped = ended.status.is_ok();
    let status = ended.status.map(|_| ());
    (result.and(status), reaped)
}

pub(crate) struct Opened {
    pub(crate) channel: pipe::Channel,
    pub(crate) ended: oneshot::Receiver<io::Result<()>>,
}

/// Native composition supplies the backend identity and an already negotiated
/// connection using this Attempt's context. No address or policy comes from
/// renderer data here. Taking ownership is synchronous: even a refused handoff
/// schedules actual cleanup while retaining admission.
pub(crate) fn open(
    attempt: Arc<Attempt>,
    backend_id: BackendId,
    backend: Backend,
    expected_mux: u32,
) -> io::Result<Opened> {
    let mut owner = Owner(Some(Running {
        guard: CleanupGuard(Some(attempt.clone())),
        backend,
        frontend: None,
    }));
    let runtime = tokio::runtime::Handle::try_current().map_err(io::Error::other)?;
    let backend = &owner.0.as_ref().expect("owned connection").backend;
    let context = backend.sender().context();
    if backend_id != attempt.plan.backend
        || !Arc::ptr_eq(&context.budget.data, &attempt.context.budget.data)
        || !Arc::ptr_eq(&context.budget.control, &attempt.context.budget.control)
    {
        return Err(invalid("native backend assignment mismatch"));
    }
    let (channel, io) = pipe::pair(attempt.clone())?;
    attempt
        .ready(backend.ready(), expected_mux)
        .map_err(|_| invalid("native backend readiness rejected"))?;
    let (read, write) = tokio::io::split(io);
    let frontend = Peer::start(read, write, backend.ready(), attempt.context.clone());
    owner.0.as_mut().expect("owned connection").frontend = Some(frontend);
    let (ended, result) = oneshot::channel();
    runtime.spawn(async move {
        let result = owner.0.as_mut().expect("owned connection").relay().await;
        let cleanup = owner.finish().await;
        let _ = ended.send(result.and(cleanup));
    });
    Ok(Opened {
        channel,
        ended: result,
    })
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Cancellation never declares cleanup complete. If a cleanup future itself
/// is destroyed (for example during runtime shutdown), retain admission rather
/// than allowing new work on the assumption that its worker has exited.
pub(super) struct CleanupGuard(pub(super) Option<Arc<Attempt>>);
impl Drop for CleanupGuard {
    fn drop(&mut self) {
        if let Some(attempt) = self.0.take() {
            std::mem::forget(attempt);
        }
    }
}
struct Running {
    guard: CleanupGuard,
    backend: Backend,
    frontend: Option<Peer>,
}
impl Running {
    fn close(&self) {
        self.backend.sender().close();
        if let Some(frontend) = &self.frontend {
            frontend.sender.close();
        }
        if let Some(attempt) = &self.guard.0 {
            attempt.context.close();
        }
    }
    async fn relay(&mut self) -> io::Result<()> {
        let frontend = self.frontend.as_mut().expect("admitted frontend");
        let to_backend = self.backend.sender();
        let to_frontend = frontend.sender.clone();
        let context = to_backend.context();
        let mut closing = context.closing();
        loop {
            tokio::select! { biased;
                _ = closing.wait_for(|closed| *closed) => return Ok(()),
                incoming = frontend.recv() => {
                    let Some(message) = incoming else { return Ok(()); };
                    match message.frame.kind {
                        Kind::Text | Kind::Request => to_backend.data_frame(message.map(Incoming::into_frame))?,
                        Kind::Cancel => {
                            let cancel = message.frame.decode_control::<wire::Cancel>(&context.budget)?;
                            to_backend.control(Kind::Cancel, &wire::Cancel { generation: to_backend.generation(), id: cancel.id })?;
                        }
                        _ => return Err(invalid("unexpected native document input")),
                    }
                }
                outgoing = self.backend.recv() => {
                    let Some(message) = outgoing else { return Ok(()); };
                    if !matches!(message.frame.kind, Kind::Text | Kind::Binary | Kind::Reply) {
                        return Err(invalid("unexpected native backend output"));
                    }
                    to_frontend.data_frame(message.map(Incoming::into_frame))?;
                }
            }
        }
    }
    async fn finish(mut self) -> io::Result<()> {
        self.close();
        let frontend = async {
            match self.frontend.take() {
                Some(frontend) => frontend.finish().await,
                None => Ok(()),
            }
        };
        let (frontend, (backend, cleaned)) = tokio::join!(frontend, self.backend.finish());
        if cleaned {
            self.guard.0.take();
        }
        frontend.and(backend)
    }
}

struct Owner(Option<Running>);
impl Owner {
    async fn finish(mut self) -> io::Result<()> {
        self.0.take().expect("owned connection").finish().await
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        if let Some(running) = self.0.take() {
            running.close();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = running.finish().await;
                });
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::client_ipc::{Broker, Policy};
    use splitty_transport::{admission, codec::Frame, negotiation, queue};
    use std::time::Duration;
    use tokio::process::Command;

    const WAIT: Duration = Duration::from_secs(10);
    fn ready() -> wire::Ready {
        wire::Ready {
            generation: 7,
            instance: "a".repeat(32),
            mux_protocol: 1,
            purpose: wire::Purpose::Direct,
            limits: wire::Limits::default(),
        }
    }
    fn attempt(broker: &Broker, label: &str) -> (BackendId, Arc<Attempt>) {
        let id = BackendId::new();
        broker
            .register(label, Policy::Main { backend: id })
            .unwrap();
        let proof = broker.committed(label).unwrap();
        (
            id,
            Arc::new(broker.open(label, &proof, 1, None, false).unwrap()),
        )
    }
    async fn child(context: queue::Context) -> (NativeBridge, Arc<admission::Admission>) {
        let ready = ready();
        let mut command = Command::new("python3");
        command
            .args([
                "-u",
                "-c",
                include_str!("../../../splitty-remote/tests/native_bridge.py"),
            ])
            .arg(serde_json::to_string(&ready).unwrap())
            .arg(r#"{"type":"mux.welcome"}"#);
        let capacity = Arc::new(admission::Admission::default());
        let bridge = NativeBridge::spawn_context(
            command,
            (
                capacity.carrier().unwrap(),
                capacity
                    .begin(admission::Assignment::new(
                        admission::Backend::new(),
                        admission::Domain::new(),
                    ))
                    .unwrap(),
            ),
            negotiation::Expected {
                instance: Some(&ready.instance),
                purpose: &ready.purpose,
                mux_protocol: 1,
                preparation: wire::HANDSHAKE_TIMEOUT,
            },
            context,
            |_| {},
        )
        .await
        .unwrap();
        (bridge, capacity)
    }
    async fn send(input: &pipe::Input, frame: Frame) {
        let mut bytes = vec![frame.kind as u8];
        bytes.extend_from_slice(&u32::try_from(frame.payload.len()).unwrap().to_be_bytes());
        bytes.extend_from_slice(&frame.payload);
        for chunk in bytes.chunks(pipe::CHUNK_BYTES) {
            tokio::time::timeout(WAIT, input.write(chunk).unwrap())
                .await
                .unwrap()
                .unwrap();
        }
    }
    // Test document: copies each native chunk and acknowledges only after that
    // copy. Native frame credit is separate and only sent on frame consumption.
    struct Document {
        channel: pipe::Channel,
        bytes: Vec<u8>,
    }
    impl Document {
        fn new(channel: pipe::Channel) -> Self {
            channel.input().start().unwrap();
            Self {
                channel,
                bytes: Vec::new(),
            }
        }
        async fn receive(&mut self) -> Frame {
            tokio::time::timeout(WAIT, async {
                loop {
                    if self.bytes.len() >= wire::HEADER_BYTES {
                        let size = usize::try_from(u32::from_be_bytes(
                            self.bytes[1..5].try_into().unwrap(),
                        ))
                        .unwrap();
                        if self.bytes.len() >= wire::HEADER_BYTES + size {
                            let kind = Kind::try_from(self.bytes[0]).unwrap();
                            let payload = self.bytes[5..5 + size].to_vec();
                            self.bytes.drain(..5 + size);
                            return Frame { kind, payload };
                        }
                    }
                    let chunk = self.channel.next().await.unwrap().expect("native output");
                    self.bytes.extend_from_slice(chunk.bytes());
                    let token = chunk.token();
                    drop(chunk);
                    self.channel.input().consumed(token).unwrap();
                }
            })
            .await
            .expect("document delivery deadline")
        }
        async fn data(&mut self) -> Frame {
            loop {
                let frame = self.receive().await;
                match frame.kind {
                    Kind::Credit => continue,
                    Kind::Ping => {
                        send(
                            &self.channel.input(),
                            Frame::json(Kind::Pong, &frame.decode::<wire::Ping>().unwrap())
                                .unwrap(),
                        )
                        .await;
                    }
                    _ => {
                        let delivery = frame.application().unwrap().0;
                        send(
                            &self.channel.input(),
                            Frame::json(Kind::Credit, &splitty_transport::flow::credit(delivery))
                                .unwrap(),
                        )
                        .await;
                        return frame;
                    }
                }
            }
        }
    }
    fn reaped(pid: u32) {
        assert!(
            !std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success(),
            "owned child remains alive"
        );
    }

    // Runtime construction reads process configuration. Isolate that environment
    // in this test binary's child rather than mutating parallel tests or using
    // the real desktop configuration directory.
    #[tokio::test]
    async fn embedded_service_roundtrip_and_close_use_the_same_owner_budget() {
        const CHILD: &str = "SPLITTY_TEST_TAURI_SESSION";
        if std::env::var_os(CHILD).is_none() {
            let config = tempfile::tempdir().unwrap();
            let result = tokio::time::timeout(WAIT, Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "client_ipc::session::tests::embedded_service_roundtrip_and_close_use_the_same_owner_budget", "--nocapture"])
                .env(CHILD, "1").env("SPLITTY_CONFIG_DIR", config.path())
                .kill_on_drop(true).output()).await.unwrap().unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            return;
        }
        let broker = Broker::default();
        let (id, attempt) = attempt(&broker, "main");
        let context = attempt.context.clone();
        let control = std::path::PathBuf::from(std::env::var_os("SPLITTY_CONFIG_DIR").unwrap())
            .join("unused.sock");
        let runtime = splitty_server::service::runtime::build(
            "embedded",
            None,
            Arc::new(splitty_core::pty::terminal::LocalTerminals),
            &control,
        );
        let backend = super::super::provider::Provider::embedded(runtime.state.clone())
            .connect(attempt.clone(), |_| {})
            .await
            .unwrap();
        let generation = backend.ready().generation;
        let mux = backend.ready().mux_protocol;
        let Opened { channel, ended } = open(attempt, id, backend, mux).unwrap();
        let mut document = Document::new(channel);
        let welcome = document.data().await;
        let welcome: serde_json::Value =
            serde_json::from_slice(welcome.application().unwrap().1).unwrap();
        assert_eq!(welcome["type"], "mux.welcome");
        send(
            &document.channel.input(),
            Frame::data_json(
                Kind::Request,
                wire::Delivery::new(generation),
                &wire::Request {
                    id: 1,
                    operation: "mux.status".into(),
                    args: Default::default(),
                },
            )
            .unwrap(),
        )
        .await;
        let reply = loop {
            let frame = document.data().await;
            if frame.kind == Kind::Reply {
                break frame.decode::<wire::Reply>().unwrap();
            }
        };
        assert_eq!(reply.id, 1);
        assert_eq!(reply.status, 200);
        drop(document);
        tokio::time::timeout(WAIT, ended)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(context.budget.used(), 0);
        assert_eq!(broker.counts(), (0, 0));
        runtime.close().await.unwrap();
    }

    #[tokio::test]
    async fn assigned_host_wraps_verified_upstream_and_reaps_rejected_identity() {
        for valid in [false, true] {
            let broker = Broker::default();
            let id = BackendId::new();
            let target = splitty_remote::ssh::SshTarget::parse("assigned.invalid").unwrap();
            broker
                .register(
                    "remote",
                    Policy::Remote {
                        backend: id,
                        target: target.clone(),
                    },
                )
                .unwrap();
            let proof = broker.committed("remote").unwrap();
            let attempt = Arc::new(broker.open("remote", &proof, 1, None, false).unwrap());
            let context = attempt.context.clone();
            let (bridge, capacity) = child(context.clone()).await;
            let pid = bridge.id().unwrap();
            let instance = if valid {
                "a".repeat(32)
            } else {
                "b".repeat(32)
            };
            let backend = super::super::provider::assigned_host(
                bridge,
                Some(&instance),
                1,
                &target,
                &context,
            )
            .await;
            if !valid {
                assert!(backend.is_err());
                drop(attempt);
                reaped(pid);
                assert_eq!(context.budget.used(), 0);
                assert_eq!(broker.counts(), (0, 0));
                assert_eq!(capacity.counts().streams, 0);
                continue;
            }
            let backend = backend.unwrap();
            assert_eq!(
                backend.ready().purpose,
                wire::Purpose::Host {
                    target: target.as_str().into()
                }
            );
            assert_eq!(backend.ready().instance, instance);
            let Opened { channel, ended } = open(attempt, id, backend, 1).unwrap();
            let mut document = Document::new(channel);
            assert_eq!(
                document.data().await.application().unwrap().1,
                br#"{"type":"mux.welcome"}"#
            );
            send(
                &document.channel.input(),
                Frame::data(Kind::Text, wire::Delivery::new(7), b"{}").unwrap(),
            )
            .await;
            assert_eq!(document.data().await.application().unwrap().1, b"{}");
            drop(document);
            tokio::time::timeout(WAIT, ended)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            reaped(pid);
            assert_eq!(context.budget.used(), 0);
            assert_eq!(broker.counts(), (0, 0));
            assert_eq!(capacity.counts().streams, 0);
        }
    }

    #[tokio::test]
    async fn child_roundtrip_spans_chunks_preserves_binary_and_reaps_before_completion() {
        let broker = Broker::default();
        let (id, attempt) = attempt(&broker, "main");
        let context = attempt.context.clone();
        let (bridge, capacity) = child(context.clone()).await;
        let pid = bridge.id().unwrap();
        let Opened { channel, ended } =
            open(attempt, id, Backend::Child(Box::new(bridge)), 1).unwrap();
        assert_eq!(broker.counts(), (1, 0));
        let mut document = Document::new(channel);
        let welcome = document.data().await;
        assert_eq!(
            welcome.application().unwrap().1,
            br#"{"type":"mux.welcome"}"#
        );
        let text =
            serde_json::json!({"type":"input", "data":"x".repeat(1024 * 1024 + 17)}).to_string();
        send(
            &document.channel.input(),
            Frame::data(Kind::Text, wire::Delivery::new(7), text.as_bytes()).unwrap(),
        )
        .await;
        let echoed = document.data().await;
        assert_eq!(echoed.application().unwrap().1, text.as_bytes());
        let binary = document.data().await;
        assert_eq!(binary.kind, Kind::Binary);
        assert_eq!(binary.application().unwrap().1, &[0, 1, 2, 255]);
        drop(document);
        tokio::time::timeout(WAIT, ended)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(context.budget.used(), 0);
        assert_eq!(broker.counts(), (0, 0));
        assert_eq!(capacity.counts().carriers, 0);
        assert_eq!(capacity.counts().streams, 0);
        reaped(pid);
    }

    #[tokio::test]
    async fn forbidden_input_closes_only_its_stream_and_retains_admission_until_reaped() {
        let broker = Broker::default();
        let (good_id, good) = attempt(&broker, "good");
        let (good_bridge, _) = child(good.context.clone()).await;
        let Opened {
            channel,
            ended: good_end,
        } = open(good, good_id, Backend::Child(Box::new(good_bridge)), 1).unwrap();
        let mut healthy = Document::new(channel);
        healthy.data().await;
        for kind in [Kind::Binary, Kind::Reply] {
            let label = format!("bad-{kind:?}");
            let (id, attempt) = attempt(&broker, &label);
            let context = attempt.context.clone();
            let (bridge, capacity) = child(context.clone()).await;
            let pid = bridge.id().unwrap();
            let Opened { channel, ended } =
                open(attempt, id, Backend::Child(Box::new(bridge)), 1).unwrap();
            let mut document = Document::new(channel);
            document.data().await;
            send(
                &document.channel.input(),
                Frame::data(kind, wire::Delivery::new(7), b"forbidden").unwrap(),
            )
            .await;
            let error = tokio::time::timeout(WAIT, ended)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            reaped(pid);
            assert_eq!(capacity.counts().streams, 0);
            assert_eq!(
                broker.counts(),
                (2, 0),
                "the native document handle also retains admission"
            );
            drop(document);
            assert_eq!(context.budget.used(), 0);
            assert_eq!(broker.counts(), (1, 0));
        }
        send(
            &healthy.channel.input(),
            Frame::data(Kind::Text, wire::Delivery::new(7), b"{}").unwrap(),
        )
        .await;
        assert_eq!(healthy.data().await.application().unwrap().1, b"{}");
        drop(healthy);
        tokio::time::timeout(WAIT, good_end)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(broker.counts(), (0, 0));
    }

    #[tokio::test]
    async fn refused_handoffs_and_navigation_reap_the_owned_child() {
        for mismatch in [0, 1, 2] {
            let broker = Broker::default();
            let (id, attempt) = attempt(&broker, "main");
            let context = attempt.context.clone();
            let child_context = if mismatch == 1 {
                queue::Context::default()
            } else {
                context.clone()
            };
            let child_budget = child_context.budget.clone();
            let (bridge, capacity) = child(child_context).await;
            let pid = bridge.id().unwrap();
            let identity = if mismatch == 0 { BackendId::new() } else { id };
            let expected_mux = if mismatch == 2 { 9 } else { 1 };
            assert!(open(
                attempt,
                identity,
                Backend::Child(Box::new(bridge)),
                expected_mux
            )
            .is_err());
            tokio::time::timeout(WAIT, async {
                while broker.counts() != (0, 0) {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert_eq!(context.budget.used(), 0);
            assert_eq!(child_budget.used(), 0);
            assert_eq!(capacity.counts().carriers, 0);
            reaped(pid);
        }
        let broker = Broker::default();
        let (id, attempt) = attempt(&broker, "main");
        let context = attempt.context.clone();
        let (bridge, _) = child(context.clone()).await;
        let pid = bridge.id().unwrap();
        let Opened { mut channel, ended } =
            open(attempt, id, Backend::Child(Box::new(bridge)), 1).unwrap();
        channel.input().start().unwrap();
        let held = channel.next().await.unwrap().unwrap();
        broker.navigating("main");
        // Stale delivery may be retained by the adapter, but cannot be acked
        // or used to release quota until that actual allocation is gone.
        assert!(channel.input().consumed(held.token()).is_err());
        drop(channel);
        let _ = tokio::time::timeout(WAIT, ended).await.unwrap().unwrap();
        reaped(pid);
        assert_eq!(broker.counts(), (1, 0));
        assert_eq!(context.budget.used(), 2 * pipe::CHUNK_BYTES);
        drop(held);
        assert_eq!(broker.counts(), (0, 0));
        assert_eq!(context.budget.used(), 0);
    }
}
