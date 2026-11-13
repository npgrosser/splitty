//! Native client composition. Discovery, endpoint identity and quotas are
//! checked before any application input; there is no token or TCP fallback.
use crate::service::catalog::{Operation, Role};
use splitty_transport::{
    admission,
    budget::{Charged, ConnectionBudget, ProcessBudget},
    negotiation,
    peer::{Incoming, Peer, Sender},
    queue,
    wire::{self, Kind},
};
use std::{io, sync::Arc};

pub struct Client {
    pub ready: Charged<wire::Ready>,
    peer: Option<Peer>,
    resources: Option<(admission::Carrier, admission::Stream)>,
    service: Option<ServiceTask>,
    requests: splitty_transport::api_request::RequestState,
}

/// Embedded streams own both ends. Client completion includes the service's
/// cleanup, including admitted operations, rather than only closing its pipe.
struct ServiceTask {
    task: Option<tokio::task::JoinHandle<()>>,
    context: queue::Context,
    // Failed or abandoned negotiation keeps the caller-side opening too,
    // until the embedded service acknowledges setup cleanup.
    capacity: Option<(admission::Carrier, admission::Opening)>,
}
impl ServiceTask {
    async fn finish(mut self) -> io::Result<()> {
        self.context.close();
        let task = self.task.take();
        let capacity = self.capacity.take();
        tokio::spawn(async move {
            let result = match task {
                Some(task) => task.await.map_err(io::Error::other),
                None => Ok(()),
            };
            drop(capacity);
            result
        })
        .await
        .map_err(io::Error::other)?
    }
}
impl Drop for ServiceTask {
    fn drop(&mut self) {
        self.context.close();
        if let Some(task) = self.task.take() {
            let capacity = self.capacity.take();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = task.await;
                    drop(capacity);
                });
            }
        }
    }
}

impl Client {
    pub fn sender(&self) -> Sender {
        self.peer
            .as_ref()
            .expect("live native client")
            .sender
            .clone()
    }
    pub async fn recv(&mut self) -> Option<Charged<Incoming>> {
        self.peer.as_mut()?.recv().await
    }
    pub fn closed_reason(&self) -> Option<Arc<Charged<wire::Closed>>> {
        self.peer.as_ref()?.closed_reason()
    }
    pub async fn finish(mut self) -> io::Result<()> {
        let peer = self.peer.take();
        let service = self.service.take();
        let resources = self.resources.take();
        // Dropping the caller's wait cannot drop admission while peer/service
        // cleanup is still running. This task has no public abort handle.
        tokio::spawn(async move {
            let result = match peer {
                Some(peer) => peer.finish().await,
                None => Ok(()),
            };
            let service = match service {
                Some(service) => service.finish().await,
                None => Ok(()),
            };
            drop(resources);
            result.and(service)
        })
        .await
        .map_err(io::Error::other)?
    }
    pub async fn shutdown(self) {
        let _ = self.finish().await;
    }

    /// Used by API-only clients. Session clients consume recv directly so their
    /// welcome, output and events keep their original order and byte leases.
    pub async fn request(
        &mut self,
        operation: Operation,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> io::Result<Charged<wire::Reply>> {
        let request = crate::service::catalog::Request { operation, args };
        request.validate(Role::Main).map_err(io::Error::other)?;
        let name = serde_json::to_value(operation)?
            .as_str()
            .ok_or_else(|| io::Error::other("invalid operation"))?
            .to_owned();
        let deadline = operation
            .description()
            .deadline
            .duration()
            .map(|d| d + std::time::Duration::from_secs(1));
        self.requests
            .request(
                self.peer.as_mut().expect("admitted native client"),
                name,
                request.args,
                deadline,
            )
            .await
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Some(peer) = self.peer.take() {
            peer.sender.close();
            let resources = self.resources.take();
            let service = self.service.take();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    peer.shutdown().await;
                    if let Some(service) = service {
                        let _ = service.finish().await;
                    }
                    drop(resources);
                });
            }
        }
    }
}

/// Capacity acquired synchronously before starting a client setup task. The
/// same context accounts for browser input received during preparation.
pub struct Pending {
    carrier: admission::Carrier,
    opening: admission::Opening,
    pub(crate) context: queue::Context,
}

/// One trusted connector shares these quota keys across every stream it opens.
/// Its streams count against the process-wide admission and byte budget,
/// except for a connector made by `isolated()`.
pub struct Connector {
    backend: Arc<admission::Backend>,
    domain: Arc<admission::Domain>,
    server_domain: Arc<admission::Domain>,
    admission: Arc<admission::Admission>,
    process: Arc<ProcessBudget>,
}
impl Default for Connector {
    fn default() -> Self {
        Self::with_limits(admission::Admission::global(), ProcessBudget::global())
    }
}

impl Connector {
    fn with_limits(admission: Arc<admission::Admission>, process: Arc<ProcessBudget>) -> Self {
        Self {
            backend: admission::Backend::new(),
            domain: admission::Domain::new(),
            server_domain: admission::Domain::new(),
            admission,
            process,
        }
    }

    /// A connector with its own admission and byte budget, each at the
    /// process-wide limits. For in-process test services: the tests of one
    /// binary run concurrently and would otherwise exhaust each other's
    /// process-wide quota. Production keeps `Connector::default()`, so all
    /// streams of a process share one set of limits.
    pub fn isolated() -> Self {
        Self::with_limits(
            Arc::new(admission::Admission::default()),
            ProcessBudget::new(wire::PROCESS_BYTES, wire::PROCESS_CONTROL_BYTES),
        )
    }

    /// A new owner context charged to this connector's process byte budget.
    pub fn context(&self) -> queue::Context {
        queue::Context::new(ConnectionBudget::new(
            self.process.clone(),
            wire::CONNECTION_BYTES,
            wire::CONNECTION_CONTROL_BYTES,
        ))
    }

    pub fn admit(&self) -> io::Result<Pending> {
        self.admit_context(self.context())
    }

    /// The native owner supplies the same context used for IPC delivery and
    /// revocation, so transport setup cannot create an independent byte budget.
    pub fn admit_context(&self, context: queue::Context) -> io::Result<Pending> {
        if context.is_closed() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "native owner closed",
            ));
        }
        let admission = &self.admission;
        let carrier = admission.carrier().map_err(super::busy)?;
        let opening = admission
            .begin(admission::Assignment::new(
                self.backend.clone(),
                self.domain.clone(),
            ))
            .map_err(super::busy)?;
        Ok(Pending {
            carrier,
            opening,
            context,
        })
    }
    /// A bounded in-process carrier to an explicitly supplied service. This
    /// never reads daemon/keeper discovery or opens a listening endpoint.
    pub async fn embedded(
        &self,
        state: Arc<crate::service::State>,
        role: Role,
    ) -> io::Result<Client> {
        self.embedded_admitted(state, role, self.admit()?).await
    }

    /// Use capacity acquired before scheduling setup, retaining the owner's
    /// cancellation signal and byte budget through the live peer.
    pub async fn embedded_admitted(
        &self,
        state: Arc<crate::service::State>,
        role: Role,
        pending: Pending,
    ) -> io::Result<Client> {
        let Pending {
            carrier,
            opening,
            context,
        } = pending;
        if context.is_closed() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "native owner closed",
            ));
        }
        let admission = &self.admission;
        let server_carrier = admission.carrier().map_err(super::busy)?;
        let server_opening = admission
            .begin(admission::Assignment::new(
                state.client_state.admission.clone(),
                self.server_domain.clone(),
            ))
            .map_err(super::busy)?;
        let pipe_bytes = context
            .budget
            .reserve(2 * 64 * 1024, false)
            .map_err(super::busy)?;
        let instance = state.instance.clone();
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (read, write) = tokio::io::split(client);
        let (mut server_read, mut server_write) = tokio::io::split(server);
        let assignment = super::Assignment {
            role,
            domain: self.server_domain.clone(),
            ..Default::default()
        };
        // Exactly one service task per admitted stream. It owns its capacity
        // and pipe charge until cleanup of admitted operations is complete.
        let server_context = self.context();
        let task_context = server_context.clone();
        let task = tokio::spawn(async move {
            let _pipe_bytes = pipe_bytes;
            let context = task_context;
            let result = async {
                let mut closing = context.closing();
                tokio::select! { biased;
                    _ = closing.wait_for(|closed| *closed) => return Ok(()),
                    result = negotiation::server(&mut server_read, &mut server_write, &context.budget) => result?,
                }
                super::serve_ready(
                    server_read,
                    server_write,
                    state,
                    assignment,
                    context,
                    server_opening,
                    server_carrier,
                )
                .await
            }
            .await;
            if let Err(error) = result {
                tracing::debug!(%error, "embedded native stream ended");
            }
        });
        let mut service = ServiceTask {
            task: Some(task),
            context: server_context,
            capacity: Some((carrier, opening)),
        };
        let result = self
            .negotiate(
                (read, write),
                negotiation::Expected {
                    instance: Some(&instance),
                    purpose: &wire::Purpose::Direct,
                    mux_protocol: splitty_core::protocol::PROTOCOL_VERSION,
                    preparation: wire::HANDSHAKE_TIMEOUT,
                },
                &mut service.capacity,
                context,
                None,
                |_| {},
            )
            .await;
        attach_service(result, service).await
    }

    /// Separate fixed-host carrier through this embedded service's hub. The
    /// main broker supplies the validated target; remote roles cannot call it.
    pub async fn host(
        &self,
        state: Arc<crate::service::State>,
        target: splitty_remote::ssh::SshTarget,
        update: bool,
    ) -> io::Result<Client> {
        self.host_admitted(state, target, update, self.admit()?, |_| {})
            .await
    }

    pub async fn host_admitted(
        &self,
        state: Arc<crate::service::State>,
        target: splitty_remote::ssh::SshTarget,
        update: bool,
        pending: Pending,
        progress: impl FnMut(Charged<wire::Progress>),
    ) -> io::Result<Client> {
        let Pending {
            carrier,
            opening,
            context,
        } = pending;
        if context.is_closed() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "native owner closed",
            ));
        }
        let admission = &self.admission;
        let server_carrier = admission.carrier().map_err(super::busy)?;
        let server_opening = admission
            .begin(admission::Assignment::new(
                state.client_state.admission.clone(),
                self.server_domain.clone(),
            ))
            .map_err(super::busy)?;
        let pipe_bytes = context
            .budget
            .reserve(2 * 64 * 1024, false)
            .map_err(super::busy)?;
        let purpose = wire::Purpose::Host {
            target: target.as_str().into(),
        };
        // The relay needs the owning hub, not the rest of the local service.
        let hub: Arc<crate::hub::Hub> = state.hub.clone();
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (read, write) = tokio::io::split(client);
        let (mut server_read, mut server_write) = tokio::io::split(server);
        let server_context = self.context();
        let task_context = server_context.clone();
        let task = tokio::spawn(async move {
            let _pipe_bytes = pipe_bytes;
            let context = task_context;
            let result = async {
                let mut closing = context.closing();
                tokio::select! { biased;
                    _ = closing.wait_for(|closed| *closed) => return Ok(()),
                    result = negotiation::server(&mut server_read, &mut server_write, &context.budget) => result?,
                }
                hub.serve_native_accepted(
                    (server_read, server_write),
                    target,
                    update,
                    async move { let _ = closing.wait_for(|closed| *closed).await; },
                    (context, server_carrier, Some(server_opening)),
                )
                .await
            }
            .await;
            if let Err(error) = result {
                tracing::debug!(%error, "embedded native host stream ended");
            }
        });
        let mut service = ServiceTask {
            task: Some(task),
            context: server_context,
            capacity: Some((carrier, opening)),
        };
        let result = self
            .negotiate(
                (read, write),
                negotiation::Expected {
                    instance: None,
                    purpose: &purpose,
                    mux_protocol: splitty_core::protocol::PROTOCOL_VERSION,
                    preparation: splitty_remote::tunnel::PREPARE_TIMEOUT,
                },
                &mut service.capacity,
                context,
                None,
                progress,
            )
            .await;
        attach_service(result, service).await
    }

    #[cfg(unix)]
    pub async fn local(&self, info: &splitty_core::daemon::DaemonInfo) -> io::Result<Client> {
        self.local_open(info, None, self.admit()?, |_| {}).await
    }
    #[cfg(unix)]
    pub async fn local_host(
        &self,
        info: &splitty_core::daemon::DaemonInfo,
        target: splitty_remote::ssh::SshTarget,
        update: bool,
    ) -> io::Result<Client> {
        self.local_host_admitted(info, target, update, self.admit()?, |_| {})
            .await
    }
    #[cfg(unix)]
    pub async fn local_host_admitted(
        &self,
        info: &splitty_core::daemon::DaemonInfo,
        target: splitty_remote::ssh::SshTarget,
        update: bool,
        pending: Pending,
        progress: impl FnMut(Charged<wire::Progress>),
    ) -> io::Result<Client> {
        let open = wire::OpenHost {
            owner_instance: info.instance.clone().unwrap_or_default(),
            target: target.as_str().into(),
            update,
        };
        self.local_open(info, Some(open), pending, progress).await
    }
    #[cfg(unix)]
    async fn local_open(
        &self,
        info: &splitty_core::daemon::DaemonInfo,
        open_host: Option<wire::OpenHost>,
        pending: Pending,
        progress: impl FnMut(Charged<wire::Progress>),
    ) -> io::Result<Client> {
        info.validate()?;
        if info.legacy() {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "legacy daemon has no native endpoint; explicitly restart/update it before attaching"));
        }
        if info.transport_version != Some(u32::from(wire::VERSION))
            || !(info.protocol == splitty_core::protocol::PROTOCOL_VERSION
                || splitty_core::protocol::PROTOCOL_VERSION.checked_sub(1) == Some(info.protocol))
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "incompatible daemon transport or mux protocol; explicitly update the daemon",
            ));
        }
        let Pending {
            carrier,
            opening,
            context,
        } = pending;
        let path = info
            .client_socket
            .as_ref()
            .ok_or_else(|| io::Error::other("missing native endpoint"))?;
        let stream = super::endpoint::connect(std::path::Path::new(path)).await?;
        let (read, write) = stream.into_split();
        let purpose =
            open_host
                .as_ref()
                .map_or(wire::Purpose::Direct, |host| wire::Purpose::Host {
                    target: host.target.clone(),
                });
        let expected = negotiation::Expected {
            instance: if open_host.is_some() {
                None
            } else {
                info.instance.as_deref()
            },
            purpose: &purpose,
            mux_protocol: if open_host.is_some() {
                splitty_core::protocol::PROTOCOL_VERSION
            } else {
                info.protocol
            },
            preparation: if open_host.is_some() {
                splitty_remote::tunnel::PREPARE_TIMEOUT
            } else {
                wire::HANDSHAKE_TIMEOUT
            },
        };
        self.negotiate(
            (read, write),
            expected,
            &mut Some((carrier, opening)),
            context,
            open_host,
            progress,
        )
        .await
    }

    async fn negotiate<R, W>(
        &self,
        (mut read, mut write): (R, W),
        expected: negotiation::Expected<'_>,
        capacity: &mut Option<(admission::Carrier, admission::Opening)>,
        context: queue::Context,
        open_host: Option<wire::OpenHost>,
        progress: impl FnMut(Charged<wire::Progress>),
    ) -> io::Result<Client>
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
        W: tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let mut closing = context.closing();
        let ready = tokio::select! { biased;
            _ = closing.wait_for(|closed| *closed) => {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "native owner closed"));
            }
            ready = async {
                negotiation::client_with_host(&mut read, &mut write, &context.budget, open_host).await?;
                negotiation::accept(&mut read, &mut write, &context.budget, expected, progress).await
            } => ready?,
        };
        // Progress and Ready may complete in the same poll as owner revocation.
        if context.is_closed() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "native owner closed",
            ));
        }
        let (carrier, opening) = capacity.take().expect("reserved native opening");
        let stream = opening.admit().map_err(super::busy)?;
        let peer = Peer::start(read, write, &ready, context);
        Ok(Client {
            ready,
            peer: Some(peer),
            resources: Some((carrier, stream)),
            service: None,
            requests: Default::default(),
        })
    }
}

async fn attach_service(result: io::Result<Client>, service: ServiceTask) -> io::Result<Client> {
    match result {
        Ok(mut client) => {
            client.service = Some(service);
            Ok(client)
        }
        Err(error) => {
            let _ = service.finish().await;
            Err(error)
        }
    }
}

/// A terminal's existing session dispatcher can write to native framing
/// without a second relay queue or a task per message.
pub struct SessionSink {
    sender: Sender,
    flush: Option<tokio::sync::oneshot::Receiver<()>>,
}
impl SessionSink {
    pub fn new(sender: Sender) -> Self {
        Self {
            sender,
            flush: None,
        }
    }
}
impl futures::Sink<tokio_tungstenite::tungstenite::Message> for SessionSink {
    type Error = io::Error;
    fn poll_ready(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(if self.sender.context().is_closed() {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "native session closed",
            ))
        } else {
            Ok(())
        })
    }
    fn start_send(
        self: std::pin::Pin<&mut Self>,
        message: tokio_tungstenite::tungstenite::Message,
    ) -> io::Result<()> {
        match message {
            tokio_tungstenite::tungstenite::Message::Text(text) => {
                self.sender.data(Kind::Text, text.as_bytes())
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unsupported native session input",
            )),
        }
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        use std::{future::Future, task::Poll};
        if self.flush.is_none() {
            self.flush = Some(self.sender.flush()?);
        }
        match std::pin::Pin::new(self.flush.as_mut().expect("flush started")).poll(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                self.flush = None;
                Poll::Ready(
                    result.map_err(|_| {
                        io::Error::new(io::ErrorKind::BrokenPipe, "native writer closed")
                    }),
                )
            }
        }
    }
    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        self.poll_flush(context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_isolated_connector_charges_its_own_admission_and_byte_budget() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = crate::service::runtime::build(
            "embedded",
            None,
            Arc::new(splitty_core::pty::terminal::LocalTerminals),
            &directory.path().join("control.sock"),
        );
        assert!(Arc::ptr_eq(
            &Connector::default().admission,
            &admission::Admission::global()
        ));
        let connector = Connector::isolated();
        assert!(!Arc::ptr_eq(
            &connector.admission,
            &admission::Admission::global()
        ));
        let client = connector
            .embedded(runtime.state.clone(), Role::Main)
            .await
            .unwrap();
        // Both ends of the embedded pipe hold a carrier and a stream slot.
        let counts = connector.admission.counts();
        assert_eq!((counts.carriers, counts.streams), (2, 2));
        let budget = client.sender().context().budget;
        assert!(Arc::ptr_eq(&budget.process, &connector.process));
        assert!(connector.process.used() > 0);
        client.finish().await.unwrap();
        assert_eq!(
            connector.admission.counts(),
            admission::Counts {
                carriers: 0,
                streams: 0,
                opening: 0,
                workers: 0
            }
        );
        assert_eq!(connector.process.used(), 0);
        runtime.close().await.unwrap();
    }

    /// The embedded service of the native client ends through the runtime's
    /// `close` alone: it closes the streams of every connected client.
    #[cfg(unix)]
    #[tokio::test]
    async fn runtime_close_closes_every_client_stream() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = crate::service::runtime::build_in(
            "embedded",
            None,
            Arc::new(splitty_core::pty::terminal::LocalTerminals),
            &directory.path().join("control.sock"),
            crate::service::runtime::Home {
                config_dir: directory.path().join("config"),
                ssh_dir: None,
                connector: Box::new(crate::hub::testing::ScriptedHost::new(directory.path())),
            },
        );
        let connector = Connector::isolated();
        let client = connector
            .embedded(runtime.state.clone(), Role::Main)
            .await
            .unwrap();
        let sender = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let registered = runtime
                    .state
                    .client_state
                    .clients
                    .lock()
                    .unwrap()
                    .values()
                    .next()
                    .map(|client| client.sender.clone());
                if let Some(sender) = registered {
                    break sender;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the session client registers");
        assert!(!*sender.closing().borrow());
        runtime.close().await.unwrap();
        assert!(*sender.closing().borrow());
        drop(client);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn admitted_host_stream_uses_fixed_target_context_and_progress() {
        use crate::hub::{testing::ScriptedHost, Hub};
        let directory = tempfile::tempdir().unwrap();
        let host = ScriptedHost::new(directory.path())
            .with_keeping_daemon("0.0.0", splitty_core::protocol::PROTOCOL_VERSION);
        let runtime = crate::service::runtime::build(
            "embedded",
            None,
            Arc::new(splitty_core::pty::terminal::LocalTerminals),
            &directory.path().join("control.sock"),
        );
        let mut state = runtime.state.as_ref().clone();
        state.hub = Arc::new(Hub::new(
            Box::new(host.clone()),
            directory.path().join("config"),
            None,
        ));
        let state = Arc::new(state);
        let connector = Connector::default();
        let revoked = queue::Context::default();
        let pending = connector.admit_context(revoked.clone()).unwrap();
        revoked.close();
        let rejected = connector
            .host_admitted(
                state.clone(),
                splitty_remote::ssh::SshTarget::parse("not-dialed").unwrap(),
                false,
                pending,
                |_| panic!("revoked setup cannot publish progress"),
            )
            .await;
        assert!(matches!(rejected, Err(error) if error.kind() == io::ErrorKind::BrokenPipe));
        assert!(
            host.asked().is_empty(),
            "revoked admission cannot prepare a host"
        );
        let context = queue::Context::default();
        let mut phases = Vec::new();
        let first = connector
            .host_admitted(
                state.clone(),
                splitty_remote::ssh::SshTarget::parse("vm1").unwrap(),
                false,
                connector.admit_context(context.clone()).unwrap(),
                |progress| phases.push(progress.phase.clone()),
            )
            .await
            .unwrap();
        assert_eq!(
            first.ready.purpose,
            wire::Purpose::Host {
                target: "vm1".into()
            }
        );
        assert!(!phases.is_empty());
        assert!(Arc::ptr_eq(
            &first.sender().context().budget.data,
            &context.budget.data
        ));
        assert!(Arc::ptr_eq(
            &first.sender().context().budget.control,
            &context.budget.control
        ));
        assert!(host.asked().iter().any(|call| call == "prepare vm1"));
        assert!(!host.asked().iter().any(|call| call == "prepare vm2"));
        assert!(
            !host
                .asked()
                .iter()
                .any(|call| call == "stop" || call == "migrate"),
            "opening a compatible older daemon must not force an update"
        );

        let mut second = connector
            .host_admitted(
                state.clone(),
                splitty_remote::ssh::SshTarget::parse("vm2").unwrap(),
                false,
                connector.admit_context(queue::Context::default()).unwrap(),
                |_| {},
            )
            .await
            .unwrap();
        first.finish().await.unwrap();
        assert!(context.is_closed());
        second
            .sender()
            .json(Kind::Text, &serde_json::json!({"type":"mux.resync"}))
            .unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(3), second.recv())
                .await
                .unwrap()
                .is_some()
        );
        second.finish().await.unwrap();
        // The same admitted embedded adapter forwards an explicit update, then
        // returns to connect-only behavior for the next independent intent.
        for update in [true, false] {
            let context = queue::Context::default();
            let client = connector
                .host_admitted(
                    state.clone(),
                    splitty_remote::ssh::SshTarget::parse("vm1").unwrap(),
                    update,
                    connector.admit_context(context.clone()).unwrap(),
                    |_| {},
                )
                .await
                .unwrap();
            assert_eq!(
                client.ready.purpose,
                wire::Purpose::Host {
                    target: "vm1".into()
                }
            );
            assert_eq!(
                host.asked().iter().filter(|call| *call == "stop").count(),
                1
            );
            client.finish().await.unwrap();
            assert!(context.is_closed());
        }
        runtime.close().await.unwrap();
    }

    #[tokio::test]
    async fn closed_reason_passes_through_after_accepted_data_and_keeps_its_charge() {
        use splitty_transport::{
            codec::{self, Frame},
            wire::Delivery,
        };
        let context = queue::Context::default();
        let ready = wire::Ready {
            generation: 3,
            instance: "a".repeat(32),
            mux_protocol: 1,
            purpose: wire::Purpose::Direct,
            limits: wire::Limits::default(),
        };
        let (io, mut raw) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(io);
        let mut client = Client {
            ready: context.budget.charge(0, true, || ready.clone()).unwrap(),
            peer: Some(Peer::start(read, write, &ready, context.clone())),
            resources: None,
            service: None,
            requests: Default::default(),
        };
        codec::write(
            &mut raw,
            &Frame::data(Kind::Text, Delivery::new(3), b"last").unwrap(),
        )
        .await
        .unwrap();
        codec::write(
            &mut raw,
            &Frame::json(
                Kind::Closed,
                &wire::Closed {
                    code: wire::CloseCode::BackendLost,
                    message: "lost".into(),
                    retryable: true,
                },
            )
            .unwrap(),
        )
        .await
        .unwrap();
        let frame = client.recv().await.unwrap();
        assert_eq!(frame.frame.application().unwrap().1, b"last");
        drop(frame);
        assert!(client.recv().await.is_none());
        let reason = client.closed_reason().unwrap();
        assert_eq!(reason.code, wire::CloseCode::BackendLost);
        assert!(reason.retryable);
        client.finish().await.unwrap();
        assert!(context.budget.control.used() > 0);
        drop(reason);
        assert_eq!(context.budget.used(), 0);
    }

    #[tokio::test]
    async fn abandoned_negotiation_retains_its_opening_until_service_cleanup() {
        use tokio::io::AsyncReadExt;
        let admission = Arc::new(admission::Admission::default());
        let capacity = Some((
            admission.carrier().unwrap(),
            admission
                .begin(admission::Assignment::new(
                    admission::Backend::new(),
                    admission::Domain::new(),
                ))
                .unwrap(),
        ));
        let context = queue::Context::default();
        let service_context = queue::Context::default();
        let mut closing = service_context.closing();
        let (started, cleaning) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let mut service = ServiceTask {
            context: service_context,
            capacity,
            task: Some(tokio::spawn(async move {
                let _ = closing.wait_for(|closed| *closed).await;
                started.send(()).unwrap();
                released.await.unwrap();
            })),
        };
        let (io, mut other_end) = tokio::io::duplex(1024);
        let (read, write) = tokio::io::split(io);
        let task_context = context.clone();
        let wait = tokio::spawn(async move {
            let result = Connector::default()
                .negotiate(
                    (read, write),
                    negotiation::Expected {
                        instance: None,
                        purpose: &wire::Purpose::Direct,
                        mux_protocol: 1,
                        preparation: wire::HANDSHAKE_TIMEOUT,
                    },
                    &mut service.capacity,
                    task_context,
                    None,
                    |_| {},
                )
                .await;
            attach_service(result, service).await
        });
        let mut magic = [0; wire::MAGIC.len()];
        other_end.read_exact(&mut magic).await.unwrap();
        assert_eq!(&magic, wire::MAGIC);
        wait.abort();
        let _ = wait.await;
        tokio::time::timeout(wire::HANDSHAKE_TIMEOUT, cleaning)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(admission.counts().carriers, 1);
        assert_eq!(admission.counts().streams, 1);
        assert_eq!(admission.counts().opening, 1);
        release.send(()).unwrap();
        tokio::time::timeout(wire::HANDSHAKE_TIMEOUT, async {
            while admission.counts().carriers != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(admission.counts().streams, 0);
        assert_eq!(admission.counts().opening, 0);
        assert_eq!(context.budget.used(), 0);
    }

    #[tokio::test]
    async fn abandoned_finish_wait_retains_capacity_until_owned_service_cleanup() {
        let admission = Arc::new(admission::Admission::default());
        let carrier = admission.carrier().unwrap();
        let stream = admission
            .begin(admission::Assignment::new(
                admission::Backend::new(),
                admission::Domain::new(),
            ))
            .unwrap()
            .admit()
            .unwrap();
        let context = queue::Context::default();
        let service_context = queue::Context::default();
        let mut closing = service_context.closing();
        let (started, cleaning) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let service = ServiceTask {
            context: service_context,
            capacity: None,
            task: Some(tokio::spawn(async move {
                let _ = closing.wait_for(|closed| *closed).await;
                started.send(()).unwrap();
                released.await.unwrap();
            })),
        };
        let ready = wire::Ready {
            generation: 1,
            instance: "a".repeat(32),
            mux_protocol: 1,
            purpose: wire::Purpose::Direct,
            limits: wire::Limits::default(),
        };
        let (io, _other_end) = tokio::io::duplex(1024);
        let (read, write) = tokio::io::split(io);
        let peer = Peer::start(read, write, &ready, context.clone());
        let client = Client {
            ready: context.budget.charge(1024, true, || ready).unwrap(),
            peer: Some(peer),
            resources: Some((carrier, stream)),
            service: Some(service),
            requests: Default::default(),
        };
        let wait = tokio::spawn(client.finish());
        tokio::time::timeout(wire::HANDSHAKE_TIMEOUT, cleaning)
            .await
            .unwrap()
            .unwrap();
        wait.abort();
        let _ = wait.await;
        assert_eq!(admission.counts().carriers, 1);
        assert_eq!(admission.counts().streams, 1);
        release.send(()).unwrap();
        tokio::time::timeout(wire::HANDSHAKE_TIMEOUT, async {
            while admission.counts().carriers != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(admission.counts().streams, 0);
        assert_eq!(context.budget.used(), 0);
    }

    #[tokio::test]
    async fn owner_revocation_interrupts_an_incomplete_handshake() {
        use tokio::io::AsyncReadExt;
        let connector = Connector::default();
        let context = queue::Context::default();
        let Pending {
            carrier, opening, ..
        } = connector.admit_context(context.clone()).unwrap();
        let (client, mut server) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client);
        let revoked = context.clone();
        let task = tokio::spawn(async move {
            connector
                .negotiate(
                    (read, write),
                    negotiation::Expected {
                        instance: None,
                        purpose: &wire::Purpose::Direct,
                        mux_protocol: 1,
                        preparation: wire::HANDSHAKE_TIMEOUT,
                    },
                    &mut Some((carrier, opening)),
                    revoked,
                    None,
                    |_| {},
                )
                .await
        });
        let mut magic = [0; wire::MAGIC.len()];
        server.read_exact(&mut magic).await.unwrap();
        assert_eq!(&magic, wire::MAGIC);
        context.close();
        let error = match tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
        {
            Ok(_) => panic!("revoked setup must not finish admission"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(context.budget.used(), 0);
        let mut tail = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            server.read_to_end(&mut tail),
        )
        .await
        .unwrap()
        .unwrap();
    }
    #[tokio::test]
    async fn progress_revocation_rejects_ready_in_the_same_read_poll() {
        use splitty_transport::codec::{self, Frame};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let connector = Connector::default();
        let context = queue::Context::default();
        let Pending {
            carrier, opening, ..
        } = connector.admit_context(context.clone()).unwrap();
        let (client, server) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client);
        let server = tokio::spawn(async move {
            let (mut read, mut write) = tokio::io::split(server);
            let budget = splitty_transport::budget::ConnectionBudget::default();
            negotiation::server(&mut read, &mut write, &budget)
                .await
                .unwrap();
            let mut bytes = Frame::json(
                Kind::Progress,
                &wire::Progress {
                    phase: "connecting".into(),
                    message: "prepared".into(),
                },
            )
            .unwrap()
            .encode()
            .unwrap();
            bytes.extend_from_slice(
                &Frame::json(
                    Kind::Ready,
                    &wire::Ready {
                        generation: 1,
                        instance: "a".repeat(32),
                        mux_protocol: 1,
                        purpose: wire::Purpose::Direct,
                        limits: wire::Limits::default(),
                    },
                )
                .unwrap()
                .encode()
                .unwrap(),
            );
            write.write_all(&bytes).await.unwrap();
            // Keep the write half live until the client closes. Neither Ready
            // completion nor cancellation is confused with an early pipe EOF.
            let mut tail = Vec::new();
            read.read_to_end(&mut tail).await.unwrap();
            drop(write);
            tail
        });
        let mut reported = false;
        let result = connector
            .negotiate(
                (read, write),
                negotiation::Expected {
                    instance: None,
                    purpose: &wire::Purpose::Direct,
                    mux_protocol: 1,
                    preparation: wire::HANDSHAKE_TIMEOUT,
                },
                &mut Some((carrier, opening)),
                context.clone(),
                None,
                |_| {
                    reported = true;
                    context.close();
                },
            )
            .await;
        assert!(reported);
        let error = match result {
            Ok(_) => panic!("revoked Ready admitted"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(context.budget.used(), 0);
        let tail = server.await.unwrap();
        // ReadyAccept proves that this fixture reached the last successful
        // negotiation poll; the owner check must still deny live admission.
        let frame = codec::read(&mut &tail[..], &context.budget)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(frame.kind, Kind::ReadyAccept);
    }
}
