//! App-owned embedded helper with independently negotiated document streams.
use crate::{bridge::NativeBridge, ssh::SshTarget};
use serde::{Deserialize, Serialize};
use splitty_transport::{
    admission,
    budget::Charged,
    negotiation,
    peer::{Incoming, Peer, Sender},
    queue,
    wire::{self, Kind},
};
use std::{
    collections::HashMap,
    io,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::{mpsc, watch},
    task::JoinHandle,
};

const CHUNK: usize = 64 * 1024;
fn broken() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "embedded owner closed")
}
struct Route {
    input: mpsc::Sender<Charged<Incoming>>,
    context: queue::Context,
    released: watch::Sender<bool>,
    _capacity: Arc<Mutex<Capacity>>,
}
struct Capacity {
    opening: Option<admission::Opening>,
    stream: Option<admission::Stream>,
}
#[derive(Default)]
struct Routes {
    next: u32,
    streams: HashMap<u32, Route>,
}
/// One helper and runtime, shared across document replacement.
pub struct Owner {
    sender: Sender,
    mux: u32,
    pid: Option<u32>,
    routes: Arc<Mutex<Routes>>,
    assignment: admission::Assignment,
    exited: watch::Receiver<bool>,
}
impl Owner {
    pub async fn spawn(command: Command, expected_mux: u32) -> io::Result<Arc<Self>> {
        Self::spawn_context(command, expected_mux, queue::Context::default()).await
    }
    /// Cancellation during preparation returns only after the owned child is reaped.
    pub async fn spawn_context(
        command: Command,
        expected_mux: u32,
        context: queue::Context,
    ) -> io::Result<Arc<Self>> {
        let assignment =
            admission::Assignment::new(admission::Backend::new(), admission::Domain::new());
        let admission = admission::Admission::global();
        let capacity = (
            admission.carrier().map_err(io::Error::other)?,
            admission
                .begin(admission::Assignment::new(
                    admission::Backend::new(),
                    admission::Domain::new(),
                ))
                .map_err(io::Error::other)?,
        );
        let bridge = NativeBridge::spawn_context(
            command,
            capacity,
            negotiation::Expected {
                instance: None,
                purpose: &wire::Purpose::Owner,
                mux_protocol: expected_mux,
                preparation: wire::HANDSHAKE_TIMEOUT,
            },
            context,
            |_| {},
        )
        .await?;
        Ok(Self::start(bridge, expected_mux, assignment))
    }
    fn start(bridge: NativeBridge, mux: u32, assignment: admission::Assignment) -> Arc<Self> {
        let pid = bridge.id();
        let sender = bridge.sender();
        let routes = Arc::new(Mutex::new(Routes::default()));
        let (exited, exit) = watch::channel(false);
        let owner = Arc::new(Self {
            sender,
            mux,
            pid,
            routes: routes.clone(),
            assignment,
            exited: exit,
        });
        tokio::spawn(async move {
            let mut bridge = bridge;
            while let Some(message) = bridge.recv().await {
                if route(&routes, message, &bridge.sender().context().budget).is_err() {
                    break;
                }
            }
            // Cleanup evidence is helper exit, never just a lost carrier.
            let _ = bridge.finish().await;
            for (_, stream) in routes.lock().expect("owner routes").streams.drain() {
                stream.context.close();
                stream.released.send_replace(true);
            }
            exited.send_replace(true);
        });
        owner
    }
    /// Owned helper identity, retained for lifecycle diagnostics after exit.
    pub fn id(&self) -> Option<u32> {
        self.pid
    }
    /// Bytes currently retained by the physical carrier's bounded budget.
    pub fn retained_bytes(&self) -> usize {
        self.sender.context().budget.used()
    }
    /// Streams still awaiting helper cleanup confirmation.
    pub fn stream_count(&self) -> usize {
        self.routes.lock().expect("owner routes").streams.len()
    }
    pub fn is_closed(&self) -> bool {
        self.sender.context().is_closed()
    }
    /// Close the carrier and wait for the owned helper to be reaped. Dropping
    /// this wait leaves the supervisor running with its worker/admission slots.
    pub async fn close(&self) {
        self.sender.close();
        let mut exited = self.exited.clone();
        let _ = exited.wait_for(|done| *done).await;
    }
    pub async fn open_context(
        &self,
        target: Option<SshTarget>,
        update: bool,
        context: queue::Context,
        progress: impl FnMut(Charged<wire::Progress>),
    ) -> io::Result<Connection> {
        if self.is_closed() || context.is_closed() {
            return Err(broken());
        }
        if target
            .as_ref()
            .is_some_and(|target| target.as_str().len() > 4096)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "owner host target exceeds limit",
            ));
        }
        if update && target.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "update requires host",
            ));
        }
        let opening = admission::Admission::global()
            .begin(self.assignment.clone())
            .map_err(io::Error::other)?;
        let capacity = Arc::new(Mutex::new(Capacity {
            opening: Some(opening),
            stream: None,
        }));
        let buffers = context
            .budget
            .reserve(CHUNK * 3 + 4, false)
            .map_err(io::Error::other)?;
        let (service, relay) = tokio::io::duplex(CHUNK);
        let (input, incoming) = mpsc::channel(2);
        let (released, release) = watch::channel(false);
        let stream_context = context.child();
        let id = {
            let mut routes = self.routes.lock().expect("owner routes");
            if self.is_closed() || routes.streams.len() >= 32 {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "owner stream capacity exhausted",
                ));
            }
            let id = routes
                .next
                .checked_add(1)
                .ok_or_else(|| io::Error::other("owner stream ids exhausted"))?;
            #[derive(Serialize)]
            struct Open<'a> {
                r#type: &'static str,
                id: u32,
                target: Option<&'a str>,
                update: bool,
            }
            let command = Open {
                r#type: "open",
                id,
                target: target.as_ref().map(SshTarget::as_str),
                update,
            };
            if serde_json::to_vec(&command)?.len() > 4096 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "owner command exceeds limit",
                ));
            }
            self.sender.json(Kind::Text, &command)?;
            routes.next = id;
            routes.streams.insert(
                id,
                Route {
                    input,
                    context: stream_context.clone(),
                    released,
                    _capacity: capacity.clone(),
                },
            );
            id
        };
        let sender = self.sender.clone();
        let relay_context = stream_context.clone();
        let relay = tokio::spawn(async move {
            let _buffers = buffers;
            if relay_stream(
                id,
                relay,
                incoming,
                sender.clone(),
                relay_context.clone(),
                context,
            )
            .await
            .is_err()
            {
                relay_context.close();
            }
            if sender
                .control(
                    Kind::Cancel,
                    &wire::Cancel {
                        generation: sender.generation(),
                        id,
                    },
                )
                .is_err()
            {
                sender.close();
            }
        });
        // This guard survives cancellation while negotiation is pending.
        let mut connection = Connection {
            peer: None,
            ready: None,
            context: stream_context.clone(),
            relay: Some(relay),
            released: release,
            capacity: capacity.clone(),
        };
        let (mut read, mut write) = tokio::io::split(service);
        let purpose = target.map_or(wire::Purpose::Direct, |target| wire::Purpose::Host {
            target: target.as_str().to_owned(),
        });
        let mut closing = stream_context.closing();
        let ready = tokio::select! { biased;
            _ = closing.wait_for(|closed| *closed) => Err(broken()),
            result = async {
                negotiation::client(&mut read, &mut write, &stream_context.budget).await?;
                negotiation::accept(&mut read, &mut write, &stream_context.budget, negotiation::Expected {
                    instance: None, purpose: &purpose, mux_protocol: self.mux, preparation: Duration::from_secs(600),
                }, progress).await
            } => result,
        };
        let ready = match ready {
            Ok(ready) if !stream_context.is_closed() => ready,
            result => {
                let error = result.err().unwrap_or_else(broken);
                drop((read, write));
                let _ = connection.finish().await;
                return Err(error);
            }
        };
        {
            let mut capacity = capacity.lock().expect("stream capacity");
            capacity.stream = Some(
                capacity
                    .opening
                    .take()
                    .expect("opening capacity")
                    .admit()
                    .map_err(io::Error::other)?,
            );
        }
        connection.peer = Some(Peer::start(read, write, &ready, stream_context));
        connection.ready = Some(ready);
        Ok(connection)
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.sender.close();
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Closed {
    r#type: String,
    id: u32,
    message: String,
    retryable: bool,
}
fn route(
    routes: &Mutex<Routes>,
    message: Charged<Incoming>,
    budget: &splitty_transport::budget::ConnectionBudget,
) -> io::Result<()> {
    let mut routes = routes.lock().expect("owner routes");
    let bytes = message.frame.application()?.1;
    match message.frame.kind {
        Kind::Text => {
            if bytes.len() > 4096 {
                return Err(io::Error::other("oversized owner result"));
            }
            let closed = message.frame.decode_charged::<Closed>(budget)?;
            if closed.r#type != "closed" || closed.id == 0 || closed.id > routes.next {
                return Err(io::Error::other("invalid owner result"));
            }
            let _ = (&closed.message, closed.retryable);
            if let Some(stream) = routes.streams.remove(&closed.id) {
                stream.released.send_replace(true);
            }
        }
        Kind::Binary => {
            if bytes.len() <= 4 || bytes.len() > CHUNK + 4 {
                return Err(io::Error::other("invalid owner chunk"));
            }
            let id = u32::from_be_bytes(bytes[..4].try_into().expect("checked length"));
            if id == 0 || id > routes.next {
                return Err(io::Error::other("unknown owner stream"));
            }
            if let Some(stream) = routes.streams.get(&id) {
                if stream.input.try_send(message).is_err() {
                    stream.context.close();
                }
            }
        }
        _ => return Err(io::Error::other("unsupported owner operation")),
    }
    Ok(())
}
async fn relay_stream(
    id: u32,
    relay: tokio::io::DuplexStream,
    mut incoming: mpsc::Receiver<Charged<Incoming>>,
    sender: Sender,
    context: queue::Context,
    parent: queue::Context,
) -> io::Result<()> {
    let (mut read, mut write) = tokio::io::split(relay);
    let mut closing = context.closing();
    let mut owner = parent.closing();
    let input = async {
        while let Some(message) = incoming.recv().await {
            write
                .write_all(&message.frame.application()?.1[4..])
                .await?;
        }
        Ok::<_, io::Error>(())
    };
    let output = async {
        let mut bytes = vec![0; CHUNK + 4];
        bytes[..4].copy_from_slice(&id.to_be_bytes());
        loop {
            let length = read.read(&mut bytes[4..]).await?;
            if length == 0 {
                return Ok::<_, io::Error>(());
            }
            let receipt = sender.data_confirmed(Kind::Binary, &bytes[..length + 4])?;
            tokio::time::timeout(wire::LIVENESS_TIMEOUT, receipt)
                .await
                .map_err(io::Error::other)?
                .map_err(io::Error::other)?;
        }
    };
    tokio::select! { biased;
        _ = closing.wait_for(|closed| *closed) => Err(broken()),
        _ = owner.wait_for(|closed| *closed) => Err(broken()),
        result = input => result,
        result = output => result,
    }
}
/// One document's independent native connection. Completion includes the
/// helper's stream cleanup acknowledgement (or observed helper exit).
pub struct Connection {
    peer: Option<Peer>,
    ready: Option<Charged<wire::Ready>>,
    context: queue::Context,
    relay: Option<JoinHandle<()>>,
    released: watch::Receiver<bool>,
    capacity: Arc<Mutex<Capacity>>,
}
impl Connection {
    pub fn ready(&self) -> &wire::Ready {
        self.ready.as_ref().expect("negotiated owner stream")
    }
    pub fn sender(&self) -> Sender {
        self.peer
            .as_ref()
            .expect("negotiated owner stream")
            .sender
            .clone()
    }
    pub async fn recv(&mut self) -> Option<Charged<Incoming>> {
        self.peer.as_mut()?.recv().await
    }
    pub async fn finish(mut self) -> io::Result<()> {
        self.context.close();
        let peer = self.peer.take();
        let relay = self.relay.take();
        let mut released = self.released.clone();
        let capacity = self.capacity.clone();
        tokio::spawn(async move {
            let _capacity = capacity;
            let result = match peer {
                Some(peer) => peer.finish().await,
                None => Ok(()),
            };
            if let Some(relay) = relay {
                let _ = relay.await;
            }
            let _ = released.wait_for(|done| *done).await;
            result
        })
        .await
        .map_err(io::Error::other)?
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.context.close();
        let peer = self.peer.take();
        let relay = self.relay.take();
        let mut released = self.released.clone();
        let capacity = self.capacity.clone();
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        runtime.spawn(async move {
            let _capacity = capacity;
            if let Some(peer) = peer {
                let _ = peer.finish().await;
            }
            if let Some(relay) = relay {
                let _ = relay.await;
            }
            let _ = released.wait_for(|done| *done).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn peers() -> (Peer, Peer) {
        let ready = wire::Ready {
            generation: 1,
            instance: "a".repeat(32),
            mux_protocol: 1,
            purpose: wire::Purpose::Owner,
            limits: wire::Limits::default(),
        };
        let (a, b) = tokio::io::duplex(CHUNK);
        let (ar, aw) = tokio::io::split(a);
        let (br, bw) = tokio::io::split(b);
        (
            Peer::start(ar, aw, &ready, queue::Context::default()),
            Peer::start(br, bw, &ready, queue::Context::default()),
        )
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn preparation_cancellation_returns_after_child_exit() {
        let directory = tempfile::tempdir().unwrap();
        let pidfile = directory.path().join("pid");
        let mut command = Command::new("sh");
        command.args([
            "-c",
            &format!(
                "echo $$ > {}; exec sleep 30",
                crate::ssh::quote(pidfile.to_str().unwrap())
            ),
        ]);
        let context = queue::Context::default();
        let cancelling = context.clone();
        let observed_pid = pidfile.clone();
        let cancelled = tokio::spawn(async move {
            loop {
                if tokio::fs::read_to_string(&observed_pid).await.is_ok() {
                    cancelling.close();
                    break;
                }
                tokio::task::yield_now().await;
            }
        });
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            Owner::spawn_context(command, 1, context.clone()),
        )
        .await
        .unwrap();
        assert!(result.is_err());
        drop(result);
        cancelled.await.unwrap();
        let pid = std::fs::read_to_string(pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err());
        assert_eq!(context.budget.used(), 0);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn owner_close_reaps_an_uncooperative_child_within_three_seconds() {
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
        let owner = Owner::spawn(command, 1).await.unwrap();
        assert!(!owner.is_closed());
        tokio::time::timeout(Duration::from_secs(3), owner.close())
            .await
            .unwrap();
        assert!(owner.is_closed());
        assert!(
            *owner.exited.borrow(),
            "completion includes observed child exit"
        );
        owner.close().await;
    }
    #[tokio::test]
    async fn byte_exact_chunks_wait_for_receipts_and_cancellation_is_independent() {
        let (owner, mut helper) = peers();
        let a = queue::Context::default();
        let b = queue::Context::default();
        let (mut first, first_relay) = tokio::io::duplex(CHUNK);
        let (mut second, second_relay) = tokio::io::duplex(CHUNK);
        let (_a, ai) = mpsc::channel(2);
        let (_b, bi) = mpsc::channel(2);
        let at = tokio::spawn(relay_stream(
            1,
            first_relay,
            ai,
            owner.sender.clone(),
            a.clone(),
            a.clone(),
        ));
        let bt = tokio::spawn(relay_stream(
            2,
            second_relay,
            bi,
            owner.sender.clone(),
            b.clone(),
            b.clone(),
        ));
        first.write_all(b"held receipt").await.unwrap();
        let held = helper.recv().await.unwrap();
        let expected: Vec<_> = (0..CHUNK + 9).map(|i| i.to_le_bytes()[0]).collect();
        let bytes = expected.clone();
        let writer = tokio::spawn(async move {
            second.write_all(&bytes).await.unwrap();
            second
        });
        let mut actual = Vec::new();
        while actual.len() < expected.len() {
            let chunk = tokio::time::timeout(wire::HANDSHAKE_TIMEOUT, helper.recv())
                .await
                .unwrap()
                .unwrap();
            let bytes = chunk.frame.application().unwrap().1;
            assert_eq!(&bytes[..4], &2_u32.to_be_bytes());
            actual.extend_from_slice(&bytes[4..]);
        }
        assert_eq!(actual, expected);
        let _second = writer.await.unwrap();
        a.close();
        assert!(tokio::time::timeout(wire::HANDSHAKE_TIMEOUT, at)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        assert!(!owner.sender.context().is_closed());
        b.close();
        let _ = bt.await;
        drop(held);
        owner.shutdown().await;
        helper.shutdown().await;
    }
    #[tokio::test]
    async fn closed_result_does_not_revoke_buffered_inner_bytes() {
        let (mut owner, helper) = peers();
        let context = queue::Context::default();
        let (mut service, relay) = tokio::io::duplex(CHUNK);
        let (input, incoming) = mpsc::channel(2);
        let (released, mut release) = watch::channel(false);
        let capacity = Arc::new(Mutex::new(Capacity {
            opening: Some(
                admission::Admission::global()
                    .begin(admission::Assignment::new(
                        admission::Backend::new(),
                        admission::Domain::new(),
                    ))
                    .unwrap(),
            ),
            stream: None,
        }));
        let routes = Mutex::new(Routes {
            next: 7,
            streams: HashMap::from([(
                7,
                Route {
                    input,
                    context: context.clone(),
                    released,
                    _capacity: capacity,
                },
            )]),
        });
        let task = tokio::spawn(relay_stream(
            7,
            relay,
            incoming,
            owner.sender.clone(),
            context.clone(),
            context.clone(),
        ));
        helper
            .sender
            .data(Kind::Binary, b"\0\0\0\x07final\0\xff")
            .unwrap();
        let budget = owner.sender.context().budget;
        route(&routes, owner.recv().await.unwrap(), &budget).unwrap();
        helper
            .sender
            .data(
                Kind::Text,
                br#"{"type":"closed","id":7,"message":"done","retryable":false}"#,
            )
            .unwrap();
        route(&routes, owner.recv().await.unwrap(), &budget).unwrap();
        release.wait_for(|done| *done).await.unwrap();
        assert!(!context.is_closed());
        let mut bytes = Vec::new();
        service.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, b"final\0\xff");
        task.await.unwrap().unwrap();
        owner.shutdown().await;
        helper.shutdown().await;
    }
    #[tokio::test]
    async fn incoming_receipts_survive_until_bounded_ring_accepts_bytes() {
        let (mut owner, helper) = peers();
        let context = queue::Context::default();
        let (mut service, relay) = tokio::io::duplex(17);
        let (input, incoming) = mpsc::channel(2);
        let task = tokio::spawn(relay_stream(
            7,
            relay,
            incoming,
            owner.sender.clone(),
            context.clone(),
            context.clone(),
        ));
        let bytes = [0_u8, 255, 0, 128, 9].repeat(50);
        let mut payload = 7_u32.to_be_bytes().to_vec();
        payload.extend_from_slice(&bytes);
        let receipt = helper
            .sender
            .data_confirmed(Kind::Binary, &payload)
            .unwrap();
        let mut receipt = Box::pin(receipt);
        input.send(owner.recv().await.unwrap()).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut receipt)
                .await
                .is_err()
        );
        let mut actual = vec![0; bytes.len()];
        service.read_exact(&mut actual).await.unwrap();
        assert_eq!(actual, bytes);
        tokio::time::timeout(wire::HANDSHAKE_TIMEOUT, receipt)
            .await
            .unwrap()
            .unwrap();
        drop(input);
        task.await.unwrap().unwrap();
        // EOF drains the ring rather than revoking an inner peer prematurely.
        assert!(!context.is_closed());
        owner.shutdown().await;
        helper.shutdown().await;
    }
}
