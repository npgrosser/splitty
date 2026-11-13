//! An app owns one embedded runtime, independently of its documents. The
//! inherited owner pipe carries bounded native substreams; there is no listener
//! or filesystem endpoint for these client connections. Only this launcher
//! carrier accepts opens; assigned daemon/host streams retain their own roles.
use crate::native;
use serde::{Deserialize, Serialize};
use splitty_transport::{
    admission,
    budget::Charged,
    negotiation,
    peer::{Incoming, Peer},
    queue,
    wire::{self, Kind},
};
use std::{
    collections::{HashMap, HashSet},
    io,
    sync::Arc,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::mpsc,
    task::JoinSet,
};

pub const CHUNK_BYTES: usize = 64 * 1024;
const STREAMS: usize = 32;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum Command {
    Open {
        id: u32,
        #[serde(default)]
        target: Option<String>,
        #[serde(default)]
        update: bool,
    },
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Closed<'a> {
    r#type: &'static str,
    id: u32,
    message: &'a str,
    retryable: bool,
}
struct Stream {
    input: mpsc::Sender<Charged<Incoming>>,
    context: queue::Context,
    routing: queue::Context,
}
impl Stream {
    fn close(&self) {
        self.context.close();
        self.routing.close();
    }
}

pub async fn embedded<R, W>(
    mut read: R,
    mut write: W,
    shutdown: impl std::future::Future<Output = ()>,
) -> io::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let admission = admission::Admission::global();
    let _carrier = admission.carrier().map_err(native::busy)?;
    let context = queue::Context::default();
    negotiation::server(&mut read, &mut write, &context.budget).await?;
    #[cfg(unix)]
    let control_path = crate::control::socket_path();
    #[cfg(not(unix))]
    let control_path = std::path::PathBuf::new();
    let runtime = crate::service::runtime::build(
        "embedded",
        None,
        Arc::new(splitty_core::pty::terminal::LocalTerminals),
        &control_path,
    );
    #[cfg(unix)]
    let control = crate::control::spawn_at(runtime.state.clone(), control_path);
    let ready = wire::Ready {
        generation: 1,
        instance: runtime.state.instance.clone(),
        mux_protocol: splitty_core::protocol::PROTOCOL_VERSION,
        purpose: wire::Purpose::Owner,
        limits: wire::Limits {
            frame_bytes: (CHUNK_BYTES + 4 + wire::HEADER_BYTES + wire::DELIVERY_BYTES) as u32,
            ..wire::Limits::default()
        },
    };
    let result = async {
        negotiation::offer(&mut read, &mut write, &context.budget, &ready).await?;
        let peer = Peer::start(read, write, &ready, context);
        serve(peer, runtime.state.clone(), shutdown).await
    }
    .await;
    let cleanup = runtime.close().await;
    #[cfg(unix)]
    control.shutdown();
    cleanup.and(result)
}

async fn serve(
    mut peer: Peer,
    state: Arc<crate::service::State>,
    shutdown: impl std::future::Future<Output = ()>,
) -> io::Result<()> {
    let mut streams = HashMap::<u32, Stream>::new();
    let mut tasks = JoinSet::new();
    let domain = admission::Domain::new();
    let admission = admission::Admission::global();
    let mut last_id = 0;
    let mut cancelled = HashSet::new();
    tokio::pin!(shutdown);
    let result = async {
        loop {
            tokio::select! { biased;
                _ = &mut shutdown => break,
                completed = tasks.join_next(), if !tasks.is_empty() => {
                    let (id, result): (u32, io::Result<()>) = completed.expect("running stream").map_err(io::Error::other)?;
                    streams.remove(&id);
                    peer.sender.json(Kind::Text, &Closed { r#type: "closed", id,
                        retryable: result.is_err(),
                        message: if result.is_ok() { "stream closed" } else { "stream ended" } })?;
                }
                message = peer.recv() => {
                    let Some(message) = message else { break; };
                    match message.frame.kind {
                        Kind::Text => {
                            if message.frame.application()?.1.len() > 4096 { return Err(native::invalid("owner command exceeds limit")); }
                            let (command, _decoded) = message.frame.decode_charged::<Command>(&peer.sender.context().budget)?.into_parts();
                            match command {
                                Command::Open { id, target, update } => {
                                    if id == 0 || id <= last_id { return Err(native::invalid("owner stream ids must increase")); }
                                    last_id = id;
                                    let was_cancelled = cancelled.remove(&id);
                                    cancelled.retain(|cancelled| *cancelled > id);
                                    if was_cancelled {
                                        peer.sender.json(Kind::Text, &Closed { r#type: "closed", id, message: "stream cancelled before opening", retryable: false })?;
                                        continue;
                                    }
                                    let target = match target.map(|target| splitty_remote::ssh::SshTarget::parse(&target)).transpose() {
                                        Ok(target) if !update || target.is_some() => target,
                                        _ => { peer.sender.json(Kind::Text, &Closed { r#type: "closed", id, message: "invalid owner host target", retryable: false })?; continue; }
                                    };
                                    let resources = (|| {
                                        if streams.len() >= STREAMS { return Err(native::busy("owner stream limit")); }
                                        let carrier = admission.carrier().map_err(native::busy)?;
                                        let opening = admission.begin(admission::Assignment::new(state.client_state.admission.clone(), domain.clone())).map_err(native::busy)?;
                                        let context = queue::Context::default();
                                        // Two duplex ring buffers plus one outgoing read scratch buffer.
                                        let buffers = context.budget.reserve(CHUNK_BYTES * 3 + 4, false).map_err(native::busy)?;
                                        Ok((carrier, opening, context, buffers))
                                    })();
                                    let (carrier, opening, context, buffers) = match resources {
                                        Ok(resources) => resources,
                                        Err(_) => { peer.sender.json(Kind::Text, &Closed { r#type: "closed", id, message: "owner stream capacity exhausted", retryable: true })?; continue; }
                                    };
                                    let (input, incoming) = mpsc::channel(2);
                                    let (service, relay) = tokio::io::duplex(CHUNK_BYTES);
                                    // Service EOF drains its final typed failure before the
                                    // carrier announces closure. Explicit cancellation instead
                                    // revokes both halves immediately.
                                    let routing = queue::Context::new(context.budget.clone());
                                    streams.insert(id, Stream { input, context: context.clone(), routing: routing.clone() });
                                    let sender = peer.sender.clone(); let state = state.clone(); let domain = domain.clone();
                                    tasks.spawn(async move {
                                        let _buffers = buffers;
                                        let (mut read, mut write) = tokio::io::split(service);
                                        let serving = async {
                                            let mut closing = context.closing();
                                            tokio::select! { biased;
                                                _ = closing.wait_for(|closed| *closed) => return Ok(()),
                                                result = negotiation::server(&mut read, &mut write, &context.budget) => result?,
                                            }
                                            if let Some(target) = target {
                                                state.hub.serve_native_accepted((read, write), target, update,
                                                    async { let _ = closing.wait_for(|closed| *closed).await; },
                                                    (context.clone(), carrier, Some(opening))).await
                                            } else {
                                                native::serve_ready(read, write, state,
                                                    native::Assignment { domain, ..native::Assignment::default() },
                                                    context.clone(), opening, carrier).await
                                            }
                                        };
                                        let serving = async { let result = serving.await; context.close(); result };
                                        let relaying = async {
                                            let result = relay_stream(id, relay, incoming, sender, routing).await;
                                            context.close(); result
                                        };
                                        let (served, relayed) = tokio::join!(serving, relaying);
                                        (id, served.and(relayed))
                                    });
                                }
                            }
                        }
                        Kind::Binary => {
                            let bytes = message.frame.application()?.1;
                            if bytes.len() <= 4 || bytes.len() > CHUNK_BYTES + 4 { return Err(native::invalid("invalid owner stream chunk")); }
                            let id = u32::from_be_bytes(bytes[..4].try_into().expect("checked length"));
                            if id == 0 || id > last_id { return Err(native::invalid("unknown owner stream")); }
                            if let Some(stream) = streams.get(&id) {
                                // One frozen stream cannot hold the owner's reader. Its own
                                // ordered input is revoked, not silently truncated and resumed.
                                if stream.input.try_send(message).is_err() { stream.close(); }
                            }
                        }
                        Kind::Cancel => {
                            // Closing a substream uses reserved control capacity,
                            // even when all owner data receipts are outstanding.
                            let cancel = message.frame.decode_control::<wire::Cancel>(&peer.sender.context().budget)?;
                            if cancel.id == 0 { return Err(native::invalid("invalid owner cancellation")); }
                            if cancel.id > last_id {
                                // Reserved controls may overtake the queued Open.
                                // Retain at most one tombstone per possible stream.
                                if !cancelled.contains(&cancel.id) && cancelled.len() >= STREAMS { return Err(native::busy("owner cancellation capacity exhausted")); }
                                cancelled.insert(cancel.id);
                            } else if let Some(stream) = streams.get(&cancel.id) { stream.close(); }
                        }
                        _ => return Err(native::invalid("unsupported owner carrier operation")),
                    }
                }
            }
        }
        Ok(())
    }.await;
    for stream in streams.values() {
        stream.close();
    }
    peer.shutdown().await;
    if tokio::time::timeout(wire::HANDSHAKE_TIMEOUT, async {
        while tasks.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }
    result
}

async fn relay_stream(
    id: u32,
    relay: tokio::io::DuplexStream,
    mut incoming: mpsc::Receiver<Charged<Incoming>>,
    sender: splitty_transport::peer::Sender,
    context: queue::Context,
) -> io::Result<()> {
    let (mut read, mut write) = tokio::io::split(relay);
    let mut closing = context.closing();
    let input = async {
        while let Some(message) = incoming.recv().await {
            let bytes = message.frame.application()?.1;
            write.write_all(&bytes[4..]).await?;
            // Drop the outer receipt only after its bytes reached the bounded
            // inner ring buffer, whose allocation has its own retained lease.
        }
        Ok::<_, io::Error>(())
    };
    let output = async {
        let mut bytes = vec![0; CHUNK_BYTES + 4];
        bytes[..4].copy_from_slice(&id.to_be_bytes());
        loop {
            let length = read.read(&mut bytes[4..]).await?;
            if length == 0 {
                break;
            }
            let delivered = sender.data_confirmed(Kind::Binary, &bytes[..length + 4])?;
            tokio::time::timeout(wire::LIVENESS_TIMEOUT, delivered)
                .await
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::TimedOut, "owner stream consumer stalled")
                })?
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "owner carrier closed"))?;
        }
        Ok::<_, io::Error>(())
    };
    let result = tokio::select! { biased;
        _ = closing.wait_for(|closed| *closed) => Ok(()),
        result = input => result,
        result = output => result,
    };
    context.close();
    result
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
        let (a, b) = tokio::io::duplex(CHUNK_BYTES);
        let (ar, aw) = tokio::io::split(a);
        let (br, bw) = tokio::io::split(b);
        (
            Peer::start(ar, aw, &ready, queue::Context::default()),
            Peer::start(br, bw, &ready, queue::Context::default()),
        )
    }
    #[tokio::test]
    async fn service_eof_drains_final_bytes_before_stream_closure() {
        let (owner, mut consumer) = peers();
        let context = queue::Context::default();
        let (mut service, relay) = tokio::io::duplex(CHUNK_BYTES);
        let (_input, incoming) = mpsc::channel(2);
        let relaying = tokio::spawn(relay_stream(
            7,
            relay,
            incoming,
            owner.sender.clone(),
            context,
        ));
        // Includes a split second chunk and non-UTF-8; a final typed failure
        // uses the same drain path as these bytes after its service returns.
        let expected: Vec<_> = (0..CHUNK_BYTES + 17).map(|index| index as u8).collect();
        let bytes = expected.clone();
        let writing = tokio::spawn(async move {
            service.write_all(&bytes).await.unwrap();
            service.shutdown().await.unwrap();
        });
        let mut received = Vec::new();
        while received.len() < expected.len() {
            let message = tokio::time::timeout(wire::HANDSHAKE_TIMEOUT, consumer.recv())
                .await
                .unwrap()
                .unwrap();
            let bytes = message.frame.application().unwrap().1;
            assert_eq!(&bytes[..4], &7_u32.to_be_bytes());
            received.extend_from_slice(&bytes[4..]);
        }
        assert_eq!(received, expected);
        writing.await.unwrap();
        tokio::time::timeout(wire::HANDSHAKE_TIMEOUT, relaying)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        owner.shutdown().await;
        consumer.shutdown().await;
    }
    #[tokio::test]
    async fn unconsumed_stream_does_not_block_another_stream_or_its_cancellation() {
        let (owner, mut consumer) = peers();
        let first = queue::Context::default();
        let second = queue::Context::default();
        let (mut a, ar) = tokio::io::duplex(CHUNK_BYTES);
        let (mut b, br) = tokio::io::duplex(CHUNK_BYTES);
        let (_at, ai) = mpsc::channel(2);
        let (_bt, bi) = mpsc::channel(2);
        let a_task = tokio::spawn(relay_stream(1, ar, ai, owner.sender.clone(), first.clone()));
        let b_task = tokio::spawn(relay_stream(2, br, bi, owner.sender.clone(), second));
        a.write_all(b"held").await.unwrap();
        let held = consumer.recv().await.unwrap();
        b.write_all(b"independent").await.unwrap();
        b.shutdown().await.unwrap();
        let delivered = tokio::time::timeout(wire::HANDSHAKE_TIMEOUT, consumer.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            delivered.frame.application().unwrap().1,
            b"\0\0\0\x02independent"
        );
        drop(delivered);
        tokio::time::timeout(wire::HANDSHAKE_TIMEOUT, b_task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        first.close();
        tokio::time::timeout(wire::HANDSHAKE_TIMEOUT, a_task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!owner.sender.context().is_closed());
        drop(held);
        owner.shutdown().await;
        consumer.shutdown().await;
    }
    #[test]
    fn owner_commands_have_no_renderer_selected_authority_fields() {
        for invalid in [
            r#"{"type":"open","id":1,"role":"main"}"#,
            r#"{"type":"open","id":1,"url":"http://127.0.0.1:80"}"#,
            r#"{"type":"open","id":1,"path":"/tmp/client.sock"}"#,
            r#"{"type":"open","id":1,"command":"ssh vm"}"#,
        ] {
            assert!(serde_json::from_str::<Command>(invalid).is_err());
        }
        assert!(serde_json::from_str::<Command>(
            r#"{"type":"open","id":1,"target":"vm","update":false}"#
        )
        .is_ok());
    }
}
