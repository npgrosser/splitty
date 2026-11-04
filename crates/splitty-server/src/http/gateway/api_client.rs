//! One bounded, descriptor-bound API carrier. WebSocket clients remain separate.
use super::{catalog, wire, Backend, Connection, Descriptor, Kind, Lease, Sender, Ticket};
use splitty_transport::budget::{Budget, Permit};
use std::{
    collections::BTreeMap,
    io,
    sync::{Arc, Mutex},
};
use tokio::sync::{oneshot, watch, Notify};
use tokio::time::Instant;

type Reply = (wire::Reply, Lease);

pub(super) struct Pool {
    capacity: Arc<Budget>,
    current: tokio::sync::Mutex<Option<Arc<Client>>>,
}
impl Default for Pool {
    fn default() -> Self {
        Self {
            capacity: Budget::new(wire::MAX_PENDING_REQUESTS),
            current: tokio::sync::Mutex::new(None),
        }
    }
}
impl Pool {
    pub(super) fn admit(&self) -> io::Result<Permit> {
        self.capacity.acquire(1).map_err(|_| {
            io::Error::new(io::ErrorKind::WouldBlock, "gateway API capacity exhausted")
        })
    }
    pub(super) async fn connection(
        &self,
        backend: &Backend,
        ticket: Ticket,
    ) -> io::Result<Arc<Client>> {
        let mut revision = ticket.revision.clone();
        let mut current = tokio::select! { biased;
            _ = revision.changed() => return Err(changed()),
            current = self.current.lock() => current,
        };
        if revision.has_changed().unwrap_or(true) {
            return Err(changed());
        }
        if let Some(client) = current.as_ref().filter(|client| client.usable()) {
            return Ok(client.clone());
        }
        if let Some(previous) = current.as_ref() {
            previous.sender.close();
            let mut stopped = previous.stopped.clone();
            // Serialize replacement through actual cleanup, including owned SSH
            // exit. No failed request is transferred to the next connection.
            tokio::select! { biased;
                _ = revision.changed() => return Err(changed()),
                _ = stopped.wait_for(|stopped| *stopped) => {},
            }
        }
        current.take();
        let (connection, revision) = backend.open(ticket).await?;
        let client = Client::start(connection, revision)?;
        *current = Some(client.clone());
        Ok(client)
    }
}

struct Pending {
    response: Option<oneshot::Sender<io::Result<Reply>>>,
    cancel_deadline: Option<Instant>,
    _capacity: Permit,
}
#[derive(Default)]
struct Requests {
    last: u32,
    closed: bool,
    pending: BTreeMap<u32, Pending>,
}

pub(super) struct Client {
    sender: Sender,
    revision: watch::Receiver<Option<Descriptor>>,
    requests: Arc<Mutex<Requests>>,
    changed: Arc<Notify>,
    stopped: watch::Receiver<bool>,
    limit: usize,
    storage: Arc<Lease>,
}
impl Client {
    fn start(
        connection: Connection,
        revision: watch::Receiver<Option<Descriptor>>,
    ) -> io::Result<Arc<Self>> {
        let limit = usize::from(match &connection {
            Connection::Remote(client) => client.ready().limits.pending_requests,
            Connection::Local(client) => client.ready.limits.pending_requests,
        });
        let sender = connection.sender();
        // Fixed overhead for all 64 routes, oneshots, cancellation guards and
        // map nodes is admitted before allocation, and retained through cleanup.
        let storage = sender
            .context()
            .budget
            .reserve(4096 * wire::MAX_PENDING_REQUESTS, false)
            .map_err(io::Error::other)?;
        let storage = Arc::new(storage);
        let requests = Arc::new(Mutex::new(Requests::default()));
        let changed = Arc::new(Notify::new());
        let (finished, stopped) = watch::channel(false);
        let client = Arc::new(Self {
            sender,
            revision: revision.clone(),
            requests: requests.clone(),
            changed: changed.clone(),
            stopped,
            limit,
            storage: storage.clone(),
        });
        tokio::spawn(async move {
            supervise(connection, revision, requests, changed).await;
            drop(storage);
            finished.send_replace(true);
        });
        Ok(client)
    }
    fn usable(&self) -> bool {
        !self.sender.context().is_closed()
            && !self.revision.has_changed().unwrap_or(true)
            && !self.requests.lock().expect("gateway requests").closed
    }
    pub(super) fn begin(&self, request: &catalog::Request, capacity: Permit) -> io::Result<Call> {
        let mut requests = self.requests.lock().expect("gateway requests");
        if requests.closed
            || self.sender.context().is_closed()
            || self.revision.has_changed().unwrap_or(true)
        {
            return Err(changed());
        }
        if requests.pending.len() >= self.limit {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "backend API capacity exhausted",
            ));
        }
        let Some(id) = requests.last.checked_add(1) else {
            self.sender.close();
            return Err(io::Error::other("gateway API request IDs exhausted"));
        };
        requests.last = id;
        #[derive(serde::Serialize)]
        struct Request<'a> {
            id: u32,
            operation: &'static str,
            args: &'a serde_json::Map<String, serde_json::Value>,
        }
        let (response, receive) = oneshot::channel();
        requests.pending.insert(
            id,
            Pending {
                response: Some(response),
                cancel_deadline: None,
                _capacity: capacity,
            },
        );
        // Keep ID assignment and enqueue ordered, even for concurrent callers.
        if let Err(error) = self.sender.json(
            Kind::Request,
            &Request {
                id,
                operation: request.operation.description().name,
                args: &request.args,
            },
        ) {
            requests.pending.remove(&id);
            return Err(error);
        }
        Ok(Call {
            id,
            receive,
            sender: self.sender.clone(),
            requests: self.requests.clone(),
            changed: self.changed.clone(),
            answered: false,
            _storage: self.storage.clone(),
        })
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.sender.close();
    }
}

pub(super) struct Call {
    id: u32,
    receive: oneshot::Receiver<io::Result<Reply>>,
    sender: Sender,
    requests: Arc<Mutex<Requests>>,
    changed: Arc<Notify>,
    answered: bool,
    _storage: Arc<Lease>,
}
impl Call {
    pub(super) async fn recv(&mut self) -> io::Result<Reply> {
        let result = (&mut self.receive).await.map_err(io::Error::other)?;
        self.answered = true;
        result
    }
}
impl Drop for Call {
    fn drop(&mut self) {
        if self.answered {
            return;
        }
        let mut requests = self.requests.lock().expect("gateway requests");
        if let Some(pending) = requests.pending.get_mut(&self.id) {
            // Cancellation releases the HTTP wait, not proof of upstream
            // completion. A non-answering but ping-responsive backend cannot
            // permanently consume this carrier's pending slots.
            pending.cancel_deadline = Some(Instant::now() + wire::HANDSHAKE_TIMEOUT);
            if self
                .sender
                .control(
                    Kind::Cancel,
                    &wire::Cancel {
                        generation: self.sender.generation(),
                        id: self.id,
                    },
                )
                .is_err()
            {
                self.sender.close();
            }
            self.changed.notify_one();
        }
    }
}

fn changed() -> io::Error {
    io::Error::new(
        io::ErrorKind::ConnectionAborted,
        "backend changed; the request was not replayed",
    )
}
async fn supervise(
    mut connection: Connection,
    mut revision: watch::Receiver<Option<Descriptor>>,
    requests: Arc<Mutex<Requests>>,
    notification: Arc<Notify>,
) {
    let sender = connection.sender();
    let error = loop {
        let deadline = requests
            .lock()
            .expect("gateway requests")
            .pending
            .values()
            .filter_map(|pending| pending.cancel_deadline)
            .min();
        let message = tokio::select! { biased;
            _ = revision.changed() => break changed(),
            _ = tokio::time::sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() =>
                break io::Error::new(io::ErrorKind::TimedOut, "backend did not acknowledge API cancellation"),
            _ = notification.notified() => continue,
            message = connection.recv() => message,
        };
        let Some(message) = message else {
            break io::Error::new(io::ErrorKind::ConnectionAborted, "backend connection ended");
        };
        if message.frame.kind != Kind::Reply {
            // API clients do not subscribe to sessions. Consume initial welcome
            // and other service broadcasts without retaining them or granting
            // any WebSocket identity to a stateless HTTP request.
            continue;
        }
        let (reply, lease) = match message
            .frame
            .decode_charged::<wire::Reply>(&sender.context().budget)
        {
            Ok(reply) => reply.into_parts(),
            Err(error) => break error,
        };
        let pending = requests
            .lock()
            .expect("gateway requests")
            .pending
            .remove(&reply.id);
        let Some(mut pending) = pending else {
            break io::Error::new(io::ErrorKind::InvalidData, "unexpected native API reply");
        };
        if let Some(response) = pending.response.take() {
            let _ = response.send(Ok((reply, lease)));
        }
    };
    let mut pending = {
        let mut requests = requests.lock().expect("gateway requests");
        requests.closed = true;
        std::mem::take(&mut requests.pending)
    };
    sender.close();
    for request in pending.values_mut() {
        if let Some(response) = request.response.take() {
            let _ = response.send(Err(io::Error::new(error.kind(), error.to_string())));
        }
    }
    connection.finish().await;
    // Retain cancelled/failed request capacity until carrier/worker cleanup.
    drop(pending);
}
