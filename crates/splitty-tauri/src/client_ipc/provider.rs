//! Native-selected backend composition. Document input selects neither an
//! executable nor an endpoint, and every setup task retains its own cleanup.
use super::{
    session::{Backend, CleanupGuard},
    Attempt,
};
use splitty_remote::{
    install,
    ssh::{Ssh, SshTarget},
    tunnel,
};
use splitty_transport::{admission, budget::Charged, negotiation, queue, wire};
use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

#[cfg(any(target_os = "windows", test))]
use splitty_remote::owner;

pub(crate) const MUX_PROTOCOL: u32 = wire::MUX_PROTOCOL_VERSION;

#[derive(Clone)]
pub(crate) struct Provider {
    source: Source,
}
#[derive(Clone)]
enum Source {
    #[cfg(not(target_os = "windows"))]
    Embedded {
        state: Arc<splitty_server::service::State>,
        connector: Arc<splitty_server::native::client::Connector>,
    },
    #[cfg(any(target_os = "windows", test))]
    Owned(Arc<owner::Owner>),
    Remote {
        target: SshTarget,
        assignment: admission::Assignment,
        update: Arc<AtomicBool>,
    },
}
fn closed() -> io::Error {
    io::Error::new(
        io::ErrorKind::BrokenPipe,
        "native document preparation cancelled",
    )
}
impl Provider {
    #[cfg(not(target_os = "windows"))]
    pub(crate) fn embedded(state: Arc<splitty_server::service::State>) -> Self {
        Self {
            source: Source::Embedded {
                state,
                connector: Arc::new(Default::default()),
            },
        }
    }
    #[cfg(any(target_os = "windows", test))]
    pub(crate) fn owned(owner: Arc<owner::Owner>) -> Self {
        Self {
            source: Source::Owned(owner),
        }
    }
    pub(crate) fn remote(target: SshTarget) -> Self {
        Self::remote_with_update(target, false)
    }
    /// This one-shot intent belongs to the authenticated launcher. The remote
    /// document still has no update capability and reconnects cannot replay it.
    pub(crate) fn remote_with_update(target: SshTarget, update: bool) -> Self {
        Self {
            source: Source::Remote {
                target,
                update: Arc::new(AtomicBool::new(update)),
                assignment: admission::Assignment::new(
                    admission::Backend::new(),
                    admission::Domain::new(),
                ),
            },
        }
    }
    /// An abandoned handoff revokes the setup, while the private task keeps
    /// admission until all acquired backend resources have actually finished.
    pub(crate) async fn connect(
        &self,
        attempt: Arc<Attempt>,
        progress: impl FnMut(Charged<wire::Progress>) + Send + 'static,
    ) -> io::Result<Backend> {
        let mut cancellation = Cancellation(Some(attempt.context.clone()));
        let provider = self.clone();
        let task = tokio::spawn(async move {
            let mut guard = CleanupGuard(Some(attempt));
            match provider
                .prepare(guard.0.as_ref().expect("owned attempt"), progress)
                .await
            {
                Ok(backend) => Ok(Prepared {
                    backend: Some(backend),
                    guard: Some(guard),
                }),
                Err(error) => {
                    guard.0.take();
                    Err(error)
                }
            }
        });
        let mut prepared = task.await.map_err(io::Error::other)??;
        if !prepared.attempt().current() || prepared.attempt().context.is_closed() {
            prepared.discard().await;
            return Err(closed());
        }
        cancellation.0.take();
        prepared.guard.as_mut().expect("owned guard").0.take();
        Ok(prepared.backend.take().expect("owned backend"))
    }

    async fn prepare(
        &self,
        attempt: &Arc<Attempt>,
        mut progress: impl FnMut(Charged<wire::Progress>) + Send,
    ) -> io::Result<Backend> {
        if !attempt.current() || attempt.context.is_closed() {
            return Err(closed());
        }
        let context = attempt.context.clone();
        match &self.source {
            #[cfg(not(target_os = "windows"))]
            Source::Embedded { state, connector } => {
                let pending = connector.admit_context(context)?;
                let client = if let Some(target) = &attempt.plan.target {
                    connector
                        .host_admitted(
                            state.clone(),
                            target.clone(),
                            attempt.plan.update,
                            pending,
                            progress,
                        )
                        .await?
                } else {
                    connector
                        .embedded_admitted(
                            state.clone(),
                            splitty_server::service::catalog::Role::Main,
                            pending,
                        )
                        .await?
                };
                Ok(Backend::Service(Box::new(client)))
            }
            #[cfg(any(target_os = "windows", test))]
            Source::Owned(owner) => owner
                .open_context(
                    attempt.plan.target.clone(),
                    attempt.plan.update,
                    context,
                    progress,
                )
                .await
                .map(|connection| Backend::Owned(Box::new(connection))),
            Source::Remote {
                target,
                assignment,
                update,
            } => {
                if attempt.plan.target.as_ref() != Some(target) || attempt.plan.update {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "native host assignment mismatch",
                    ));
                }
                let result = tunnel::open_with_update(
                    Ssh::stdio_only(target.clone()).with_context(context.clone()),
                    env!("CARGO_PKG_VERSION"),
                    MUX_PROTOCOL,
                    install::payload_for,
                    assignment.clone(),
                    update.swap(false, Ordering::AcqRel),
                    |event| {
                        if context.is_closed() {
                            return;
                        }
                        let event = event.map(|event| {
                            let (phase, message) = match event {
                                tunnel::Progress::Step { step, message } => (
                                    match step {
                                        tunnel::Step::Install => "install",
                                        tunnel::Step::Daemon => "daemon",
                                        tunnel::Step::Started => "started",
                                        tunnel::Step::Tunnel => "tunnel",
                                    },
                                    message,
                                ),
                                tunnel::Progress::Notice { message } => ("notice", message),
                            };
                            wire::Progress {
                                phase: phase.into(),
                                message,
                            }
                        });
                        progress(event);
                    },
                )
                .await;
                let (_, connected) = result.map_err(|error| {
                    io::Error::new(
                        if error.retryable() {
                            io::ErrorKind::ConnectionAborted
                        } else {
                            io::ErrorKind::PermissionDenied
                        },
                        error,
                    )
                })?;
                let bridge = connected.client;

                if context.is_closed() || !attempt.current() {
                    let _ = bridge.finish().await;
                    return Err(closed());
                }
                assigned_host(
                    bridge,
                    connected.daemon.instance.as_deref(),
                    connected.daemon.protocol,
                    target,
                    &context,
                )
                .await
            }
        }
    }
}
/// Upstream identity is checked before native composition assigns the host
/// purpose. The counted copy survives alongside the upstream counted Ready.
pub(super) async fn assigned_host(
    bridge: splitty_remote::bridge::NativeBridge,
    instance: Option<&str>,
    mux: u32,
    target: &SshTarget,
    context: &queue::Context,
) -> io::Result<Backend> {
    let ready = negotiation::validate_ready(bridge.ready(), instance, &wire::Purpose::Direct, mux)
        .and_then(|()| negotiation::host_ready(bridge.ready(), target.as_str(), &context.budget));
    match ready {
        Ok(ready) => Ok(Backend::host(bridge, ready)),
        Err(error) => {
            let _ = bridge.finish().await;
            Err(error)
        }
    }
}
struct Cancellation(Option<queue::Context>);
impl Drop for Cancellation {
    fn drop(&mut self) {
        if let Some(context) = self.0.take() {
            context.close();
        }
    }
}
struct Prepared {
    backend: Option<Backend>,
    guard: Option<CleanupGuard>,
}
impl Prepared {
    fn attempt(&self) -> &Arc<Attempt> {
        self.guard
            .as_ref()
            .and_then(|guard| guard.0.as_ref())
            .expect("owned attempt")
    }
    async fn discard(mut self) {
        self.attempt().context.close();
        let backend = self.backend.take().expect("owned backend");
        let guard = self.guard.take().expect("owned guard");
        let _ = tokio::spawn(dispose(backend, guard)).await;
    }
}
async fn dispose(backend: Backend, mut guard: CleanupGuard) {
    let (_, cleaned) = backend.finish().await;
    if cleaned {
        guard.0.take();
    }
}
impl Drop for Prepared {
    fn drop(&mut self) {
        if let Some(backend) = self.backend.take() {
            self.attempt().context.close();
            let guard = self.guard.take().expect("owned guard");
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(dispose(backend, guard));
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::client_ipc::{BackendId, Broker, Policy};
    use std::time::Duration;
    const WAIT: Duration = Duration::from_secs(10);
    fn attempt(broker: &Broker, target: Option<SshTarget>) -> Arc<Attempt> {
        let backend = BackendId::new();
        let policy = target.map_or(Policy::Main { backend }, |target| Policy::Remote {
            backend,
            target,
        });
        broker.register("provider", policy).unwrap();
        let proof = broker.committed("provider").unwrap();
        Arc::new(broker.open("provider", &proof, 1, None, false).unwrap())
    }
    #[tokio::test]
    async fn remote_provider_rejects_another_native_assignment_before_ssh() {
        let broker = Broker::default();
        let attempt = attempt(&broker, Some(SshTarget::parse("assigned.invalid").unwrap()));
        let context = attempt.context.clone();
        let provider = Provider::remote(SshTarget::parse("different.invalid").unwrap());
        let error = match provider.connect(attempt, |_| {}).await {
            Ok(_) => panic!("foreign host admitted"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(broker.counts(), (0, 0));
        assert_eq!(context.budget.used(), 0);
    }
    #[tokio::test]
    async fn abandoned_owner_preparation_retains_attempt_until_actual_cleanup() {
        let ready = wire::Ready {
            generation: 1,
            instance: "a".repeat(32),
            mux_protocol: MUX_PROTOCOL,
            purpose: wire::Purpose::Owner,
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
            .arg("__IGNORE_EOF__");
        let owner = owner::Owner::spawn(command, MUX_PROTOCOL).await.unwrap();
        let broker = Broker::default();
        let attempt = attempt(&broker, None);
        let context = attempt.context.clone();
        let provider = Provider::owned(owner.clone());
        let waiting = tokio::spawn(async move { provider.connect(attempt, |_| {}).await });
        tokio::time::timeout(WAIT, async {
            while owner.stream_count() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(context.budget.used() > 0, "live relay buffers are charged");
        waiting.abort();
        let _ = waiting.await;
        assert!(context.is_closed());
        assert_eq!(
            broker.counts(),
            (1, 1),
            "cancelled setup still owns cleanup admission"
        );
        owner.close().await;
        tokio::time::timeout(WAIT, async {
            while broker.counts() != (0, 0) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(context.budget.used(), 0);
        assert_eq!(owner.retained_bytes(), 0);
    }
}
