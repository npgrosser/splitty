//! Native carrier adapter for the shared service. Backend, role and resource
//! assignment are supplied by the composition root, never by a peer envelope.
use crate::{
    client_session::{self, Message},
    service::{
        self,
        catalog::{self, Cancellation, Role},
        reply::Reply,
        State,
    },
};
use splitty_transport::{
    admission, negotiation, peer, queue, requests,
    wire::{self, Kind},
};
use std::{collections::HashMap, io, sync::Arc};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    task::{AbortHandle, Id, JoinSet},
    time::{self, Instant},
};

pub mod client;
#[cfg(unix)]
pub mod endpoint;
pub mod helper;
pub(crate) mod relay;

pub struct Assignment {
    pub role: Role,
    pub purpose: wire::Purpose,
    pub generation: u32,
    pub domain: Arc<admission::Domain>,
}
impl Default for Assignment {
    fn default() -> Self {
        Self {
            role: Role::Main,
            purpose: wire::Purpose::Direct,
            generation: 1,
            domain: admission::Domain::new(),
        }
    }
}

pub async fn serve<R, W>(
    mut read: R,
    mut write: W,
    state: Arc<State>,
    assignment: Assignment,
) -> io::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let admission = admission::Admission::global();
    let carrier = admission.carrier().map_err(busy)?;
    let opening = admission
        .begin(admission::Assignment::new(
            state.client_state.admission.clone(),
            assignment.domain.clone(),
        ))
        .map_err(busy)?;
    let context = queue::Context::default();
    negotiation::server(&mut read, &mut write, &context.budget).await?;
    serve_ready(read, write, state, assignment, context, opening, carrier).await
}

/// Accept only verified local peers, acquiring all opening/carrier capacity
/// before allocating a connection task. Shutdown revokes each delivery domain.
#[cfg(unix)]
pub async fn listen(
    listener: tokio::net::UnixListener,
    state: Arc<State>,
    shutdown: impl std::future::Future<Output = ()>,
) -> io::Result<()> {
    let mut tasks = JoinSet::new();
    let mut contexts = HashMap::<Id, queue::Context>::new();
    let admission = admission::Admission::global();
    // All accepted peers have this same OS owner. Repeated connections must
    // share its quota rather than receiving a new domain for every socket.
    let domain = admission::Domain::new();
    tokio::pin!(shutdown);
    let result = loop {
        tokio::select! { biased;
            _ = &mut shutdown => break Ok(()),
            completed = tasks.join_next_with_id(), if !tasks.is_empty() => {
                match completed { Some(Ok((id, ()))) => { contexts.remove(&id); }, Some(Err(e)) => { contexts.remove(&e.id()); }, None => {} }
            }
            accepted = listener.accept() => {
                let (stream, _) = match accepted {
                    Ok(value) => value,
                    Err(error) => {
                        if let Some(delay) = admission::accept_backoff(&error) {
                            tracing::warn!(%error, "native accept failed; retrying");
                            tokio::select! { biased;
                                _ = &mut shutdown => break Ok(()),
                                _ = time::sleep(delay) => {},
                            }
                        }
                        continue;
                    }
                };
                if endpoint::verify_peer(&stream).is_err() { continue; }
                let Ok(carrier) = admission.carrier() else { continue; };
                let assignment = Assignment { domain: domain.clone(), ..Assignment::default() };
                let Ok(opening) = admission.begin(admission::Assignment::new(state.client_state.admission.clone(), assignment.domain.clone())) else { continue; };
                let context = queue::Context::default();
                let closing = context.clone(); let state = state.clone();
                let handle = tasks.spawn(async move {
                    let (mut read, mut write) = stream.into_split();
                    let result = async {
                        let (host, _hello_bytes) = negotiation::server_main(&mut read, &mut write, &context.budget).await?.into_parts();
                        if let Some(host) = host {
                            // A host-open is intent on this owner endpoint,
                            // never a way to turn a remote carrier into a hub.
                            if host.owner_instance != state.instance {
                                let frame = splitty_transport::codec::Frame::control(Kind::Closed, &wire::Closed {
                                    code: wire::CloseCode::BackendLost, retryable: true,
                                    message: "local hub instance changed; refresh discovery".into(),
                                }, &context.budget)?;
                                time::timeout(wire::HANDSHAKE_TIMEOUT, splitty_transport::codec::write(&mut write, &frame)).await
                                    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "stale hub delivery timed out"))??;
                                return Err(io::Error::new(io::ErrorKind::InvalidData, negotiation::StaleInstance));
                            }
                            let target = splitty_remote::ssh::SshTarget::parse(&host.target).map_err(invalid)?;
                            let mut stopped = context.closing();
                            state.hub.serve_native_accepted((read, write), target, host.update,
                                async { let _ = stopped.wait_for(|closed| *closed).await; },
                                (context, carrier, Some(opening))).await
                        } else {
                            serve_ready(read, write, state, assignment, context, opening, carrier).await
                        }
                    }.await;
                    if let Err(error) = result { tracing::debug!(%error, "native client ended"); }
                });
                contexts.insert(handle.id(), closing);
            }
        }
    };
    // Stop accepting before waiting on admitted operations or releasing the
    // instance lock. A daemon replacement cannot race its predecessor here.
    drop(listener);
    for context in contexts.values() {
        context.close();
    }
    if time::timeout(wire::HANDSHAKE_TIMEOUT, async {
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

/// Used by an owned helper which negotiated framing before bounded backend
/// preparation. Admission and pipe ownership remain with that helper.
pub async fn serve_ready<R, W>(
    mut read: R,
    mut write: W,
    state: Arc<State>,
    assignment: Assignment,
    context: queue::Context,
    opening: admission::Opening,
    _carrier: admission::Carrier,
) -> io::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let ready = wire::Ready {
        generation: assignment.generation,
        instance: state.instance.clone(),
        mux_protocol: splitty_core::protocol::PROTOCOL_VERSION,
        purpose: assignment.purpose,
        limits: wire::Limits::default(),
    };
    negotiation::offer(&mut read, &mut write, &context.budget, &ready).await?;
    let stream = opening.admit().map_err(busy)?;
    let mut session = client_session::connect_admitted(
        state.client_state.clone(),
        context.clone(),
        stream.clone(),
        assignment.role == Role::Main,
    );
    let mut peer = peer::Peer::start(read, write, &ready, context.clone());
    let result = run(&state, assignment.role, &mut peer, &mut session).await;
    peer.shutdown().await;
    session.shutdown().await;
    // Stream and carrier capacity include all still-running admitted effects.
    drop(stream);
    result
}

fn busy(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::WouldBlock, error.to_string())
}
fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

struct Running {
    id: u32,
    abort: AbortHandle,
    mutation: bool,
}

fn failure(
    sender: &peer::Sender,
    id: u32,
    status: u16,
    error: &str,
    uncertain: bool,
) -> io::Result<()> {
    sender.json(
        Kind::Reply,
        &wire::Reply {
            id,
            status,
            content_type: wire::ContentType::Json,
            body: serde_json::json!({"error":error}).to_string(),
            uncertain,
        },
    )
}
fn reply(sender: &peer::Sender, id: u32, result: Reply) -> io::Result<()> {
    let Reply {
        status,
        body,
        content_type,
        bytes: _response_bytes,
    } = result;
    let content_type = match content_type {
        Some("application/json") => wire::ContentType::Json,
        Some(_) => wire::ContentType::Text,
        None => wire::ContentType::Empty,
    };
    sender.json(
        Kind::Reply,
        &wire::Reply {
            id,
            status,
            content_type,
            body: String::from_utf8(body).map_err(invalid)?,
            uncertain: false,
        },
    )
}

async fn run(
    state: &Arc<State>,
    role: Role,
    peer: &mut peer::Peer,
    session: &mut client_session::Connection,
) -> io::Result<()> {
    let sender = peer.sender.clone();
    let generation = sender.generation();
    // The common loop publishes welcome first. Queue it before servicing any
    // peer requests, even when the peer pipelines input after ReadyAccept.
    let welcome = time::timeout(wire::HANDSHAKE_TIMEOUT, session.output.recv())
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "session welcome timed out"))?
        .ok_or_else(|| invalid("session closed before welcome"))?;
    let Message::Text(text) = &welcome.value else {
        return Err(invalid("missing session welcome"));
    };
    sender.data(Kind::Text, text.as_bytes())?;
    drop(welcome);
    let mut requests = requests::Requests::new(generation);
    let mut tasks = JoinSet::<(u32, Reply)>::new();
    let mut running = HashMap::<Id, Running>::new();
    let result = async {
        loop {
            let deadline = requests.next_deadline();
            tokio::select! { biased;
                completed = tasks.join_next_with_id(), if !tasks.is_empty() => {
                    match completed {
                        Some(Ok((task_id, (id, result)))) => {
                            running.remove(&task_id);
                            if requests.finish(generation, id) { reply(&sender, id, result)?; }
                        }
                        Some(Err(error)) => {
                            if let Some(task) = running.remove(&error.id()) {
                                if requests.finish(generation, task.id) { failure(&sender, task.id, 500, "request interrupted", task.mutation)?; }
                            }
                        }
                        None => {}
                    }
                }
                _ = time::sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => {
                    for cancelled in requests.expire(Instant::now()) {
                        if !cancelled.uncertain {
                            if let Some(task) = running.values().find(|t| t.id == cancelled.id) { task.abort.abort(); }
                        }
                        failure(&sender, cancelled.id, 408, "request timed out", cancelled.uncertain)?;
                    }
                }
                message = peer.recv() => {
                    let Some(message) = message else { break; };
                    match message.frame.kind {
                        Kind::Text => {
                            let bytes = message.frame.application()?.1;
                            let validated = catalog::validate_session(bytes, &sender.context().budget).map_err(invalid)?;
                            if role != Role::Main && validated["type"] == "hub.catalog.observe" {
                                return Err(invalid("host observations require local hub authority"));
                            }
                            drop(validated);
                            std::str::from_utf8(bytes).map_err(invalid)?;
                            // Transfer the carrier allocation and its lease. A
                            // full-size image must not coexist with another
                            // wire copy when the session parses its JSON.
                            session.input.forward(message.map(|mut incoming| {
                                let mut bytes = std::mem::take(&mut incoming.frame.payload);
                                bytes.truncate(bytes.len() - 16);
                                bytes.drain(..4);
                                Message::Text(String::from_utf8(bytes).expect("validated session UTF-8"))
                            }))?;
                        }
                        Kind::Request => {
                            let bytes = message.frame.application()?.1;
                            if bytes.len() > wire::MAX_API_BYTES { return Err(invalid("API request exceeds limit")); }
                            let (request, decoded_bytes) = message.frame.decode_charged::<wire::Request>(&sender.context().budget)?.into_parts();
                            let operation: Result<catalog::Operation, _> = serde_json::from_value(serde_json::Value::String(request.operation));
                            let (operation, validation) = match operation {
                                Ok(operation) => {
                                    let call = catalog::Request { operation, args: request.args };
                                    let valid = call.validate(role); (Some(call), valid)
                                }
                                Err(error) => (None, Err(catalog::Rejection::Invalid(error.to_string()))),
                            };
                            let description = operation.as_ref().map(|o| o.operation.description());
                            let mutation = description.is_some_and(|d| d.cancellation == Cancellation::Mutation);
                            let deadline = description.and_then(|d| d.deadline.duration()).map(|d| Instant::now() + d);
                            match requests.admit(generation, request.id, mutation, deadline) {
                                Ok(()) => {},
                                Err(requests::Rejection::Busy) => { failure(&sender, request.id, 503, "too many pending requests", false)?; continue; }
                                Err(requests::Rejection::Cancelled) => continue,
                                Err(error) => return Err(invalid(format!("invalid request identity: {error:?}"))),
                            }
                            if let Err(error) = validation {
                                requests.finish(generation, request.id);
                                let status = match error { catalog::Rejection::Forbidden => 403, catalog::Rejection::TooLarge => 413, catalog::Rejection::Invalid(_) => 400 };
                                failure(&sender, request.id, status, &error.to_string(), false)?; continue;
                            }
                            let state = state.clone(); let budget = sender.context().budget; let id = request.id;
                            let handle = tasks.spawn(async move {
                                let _request_bytes = message;
                                let _decoded_bytes = decoded_bytes;
                                let reply = service::reply::with_budget(budget, state.execute(operation.expect("validated operation"), role)).await;
                                (id, reply)
                            });
                            running.insert(handle.id(), Running { id, abort: handle, mutation });
                        }
                        Kind::Cancel => {
                            let cancel = message.frame.decode_control::<wire::Cancel>(&sender.context().budget)?;
                            if let Some(cancelled) = requests.cancel(cancel.generation, cancel.id).map_err(|error| invalid(format!("cancel admission: {error:?}")))? {
                                if !cancelled.uncertain {
                                    if let Some(task) = running.values().find(|t| t.id == cancel.id) { task.abort.abort(); }
                                }
                                failure(&sender, cancel.id, 499, "request cancelled", cancelled.uncertain)?;
                            }
                        }
                        _ => return Err(invalid("unsupported client-to-service frame")),
                    }
                }
                message = session.output.recv() => {
                    let Some(message) = message else { break; };
                    match &message.value {
                        Message::Text(text) => sender.data(Kind::Text, text.as_bytes())?,
                        Message::Binary(bytes) => sender.data(Kind::Binary, bytes)?,
                        Message::Close => break,
                    }
                }
            }
        }
        Ok(())
    }.await;
    sender.close();
    for task in running.values() {
        if !task.mutation {
            task.abort.abort();
        }
    }
    while tasks.join_next().await.is_some() {}
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A native client of `state` over an in-process pipe, after framing and
    /// the service handshake: its raw ends, Ready and the service task.
    async fn connected(
        state: Arc<State>,
    ) -> (
        (
            tokio::io::ReadHalf<tokio::io::DuplexStream>,
            tokio::io::WriteHalf<tokio::io::DuplexStream>,
        ),
        wire::Ready,
        tokio::task::JoinHandle<io::Result<()>>,
    ) {
        let (client, server) = tokio::io::duplex(1024 * 1024);
        let (server_read, server_write) = tokio::io::split(server);
        let instance = state.instance.clone();
        let task = tokio::spawn(serve(
            server_read,
            server_write,
            state,
            Assignment::default(),
        ));
        let (mut read, mut write) = tokio::io::split(client);
        let budget = splitty_transport::budget::ConnectionBudget::default();
        negotiation::client(&mut read, &mut write, &budget)
            .await
            .unwrap();
        let ready = negotiation::accept(
            &mut read,
            &mut write,
            &budget,
            negotiation::Expected {
                instance: Some(&instance),
                purpose: &wire::Purpose::Direct,
                mux_protocol: splitty_core::protocol::PROTOCOL_VERSION,
                preparation: wire::HANDSHAKE_TIMEOUT,
            },
            |_| {},
        )
        .await
        .unwrap();
        ((read, write), (*ready).clone(), task)
    }

    fn registered(state: &State) -> usize {
        state.client_state.clients.lock().unwrap().len()
    }

    async fn until_registered(state: &State, count: usize) {
        time::timeout(Duration::from_secs(10), async {
            while registered(state) != count {
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the session registers");
    }

    /// mux-protocol, Heartbeat and Liveness (Silent client, Responsive
    /// client, Peer of the previous release): the session loop keeps no
    /// heartbeat, so the native carrier's transport pings alone decide. A
    /// peer that answers them stays registered however long it sends
    /// nothing; a peer that stops is ended within 45 seconds and its session
    /// released. In-process with paused time once the session runs.
    #[tokio::test]
    async fn the_native_carrier_alone_ends_a_silent_peer_and_keeps_a_responsive_one() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = service::runtime::build(
            "embedded",
            None,
            Arc::new(splitty_core::pty::terminal::LocalTerminals),
            &directory.path().join("control.sock"),
        );
        let state = runtime.state.clone();
        let ((read, write), ready, responsive_task) = connected(state.clone()).await;
        let mut responsive = peer::Peer::start(read, write, &ready, queue::Context::default());
        let welcome = responsive.recv().await.expect("welcome");
        assert_eq!(welcome.frame.kind, Kind::Text);
        drop(welcome);
        let drain = tokio::spawn(async move {
            while let Some(message) = responsive.recv().await {
                drop(message);
            }
            responsive
        });
        until_registered(&state, 1).await;
        // Never read again, never answer a ping.
        let (silent_io, _ready, silent_task) = connected(state.clone()).await;
        until_registered(&state, 2).await;

        time::pause();
        let silent_since = Instant::now();
        time::timeout(Duration::from_secs(46), async {
            while registered(&state) != 1 {
                time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("the silent peer is ended within 45 seconds");
        // Counted from acceptance, a moment before the clock was paused.
        let silent_for = silent_since.elapsed();
        assert!(
            silent_for + Duration::from_secs(1) >= wire::LIVENESS_TIMEOUT,
            "ended after {silent_for:?}"
        );
        let _ended = time::timeout(Duration::from_secs(1), silent_task)
            .await
            .expect("the silent peer's service task ends")
            .expect("service task");
        drop(silent_io);

        for _ in 0..60 {
            time::sleep(Duration::from_secs(5)).await;
        }
        assert_eq!(registered(&state), 1, "the idle responsive peer is kept");
        assert!(!responsive_task.is_finished());

        drain.abort();
        let _ = drain.await;
        let _ = time::timeout(Duration::from_secs(1), responsive_task).await;
        runtime.close().await.unwrap();
    }
}
