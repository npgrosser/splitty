//! Native SSH preparation and reconnection. No listener, browser credential,
//! URL or automatic application replay belongs to this layer.
use super::RemoteError;
use crate::handshake::RunningDaemon;
use serde::Serialize;
use splitty_transport::budget::{Charged, ConnectionBudget};
use std::sync::Arc;
use std::{future::Future, time::Duration};
use tokio::time::{sleep, Instant};

pub const GIVE_UP_AFTER: Duration = Duration::from_secs(10 * 60);
pub const PREPARE_TIMEOUT: Duration = Duration::from_secs(120);

pub trait Remote: Sync {
    type Client: Send;
    type Pending: Send;
    fn host(&self) -> &str;
    fn version(&self) -> &str;
    fn protocol(&self) -> u32;
    fn context(&self) -> splitty_transport::queue::Context {
        splitty_transport::queue::Context::default()
    }
    fn admit(&self) -> Result<Self::Pending, RemoteError>;
    fn prepare(
        &self,
        on_install: impl FnOnce(&str) + Send,
    ) -> impl Future<Output = Result<(), RemoteError>> + Send;
    fn daemon(&self) -> impl Future<Output = Result<Charged<RunningDaemon>, RemoteError>> + Send;
    fn server_version(&self) -> Option<Arc<Charged<String>>>;
    fn stop_daemon(
        &self,
        daemon: &RunningDaemon,
    ) -> impl Future<Output = Result<(), RemoteError>> + Send;
    fn open_bridge(
        &self,
        daemon: &RunningDaemon,
        pending: Self::Pending,
    ) -> impl Future<Output = Result<Self::Client, RemoteError>> + Send;
}

pub struct Connected<C> {
    pub daemon: Charged<RunningDaemon>,
    pub client: C,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Step {
    Install,
    Daemon,
    Started,
    Tunnel,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    Step { step: Step, message: String },
    Notice { message: String },
}
#[derive(Debug)]
pub enum Event {
    Lost {
        reason: String,
    },
    RetryFailed {
        error: RemoteError,
        retry_in: Duration,
    },
    Restored,
    Replaced {
        instance: String,
    },
}

/// What `connect` does about a daemon of another version than the client's.
#[derive(Debug, PartialEq, Eq)]
pub enum UpdatePlan {
    /// Stop it and start this client's: it is older, its sessions outlive it,
    /// and the host's answering server is this client's version.
    Update,
    /// Use it as it is, and tell the user why.
    Notice(String),
}

pub fn update_plan(
    daemon: &RunningDaemon,
    host: &str,
    version: &str,
    server: Option<&str>,
) -> UpdatePlan {
    let host_version = &daemon.version;
    if !older(host_version, version) {
        // Never replace a newer daemon: it could fall back to a version
        // without keepers, which would start fresh shells and leave the
        // kept ones unadopted.
        return UpdatePlan::Notice(format!(
            "The daemon on {host} runs Splitty {host_version}, this client is {version}; \
             it is used as it is."
        ));
    }
    if !daemon.keeps_sessions {
        return UpdatePlan::Notice(format!(
            "The daemon on {host} runs Splitty {host_version}, this client is {version}. \
             Updating it ends its sessions: to update, stop it there (`daemon stop`) and \
             connect again."
        ));
    }
    if daemon.instance.is_none() {
        return UpdatePlan::Notice(format!(
            "The legacy daemon on {host} has no verifiable service instance. Explicitly restart it on the host before connecting; its kept sessions remain available."
        ));
    }
    if server != Some(version) {
        return UpdatePlan::Notice(format!(
            "The daemon on {host} runs Splitty {host_version}, this client is {version}. \
             `splitty daemon restart` there, with a newer splitty, updates it without ending \
             its sessions."
        ));
    }
    UpdatePlan::Update
}

/// Compare SemVer precedence, including prereleases but excluding build
/// metadata. One optional `v` prefix is accepted; invalid versions never
/// authorize replacing a daemon.
fn older(a: &str, b: &str) -> bool {
    fn parse(version: &str) -> Result<semver::Version, semver::Error> {
        semver::Version::parse(version.strip_prefix('v').unwrap_or(version))
    }
    match (parse(a), parse(b)) {
        (Ok(a), Ok(b)) => a.cmp_precedence(&b).is_lt(),
        _ => false,
    }
}

pub fn backoff(failures: u32) -> Duration {
    Duration::from_secs(1u64.checked_shl(failures).unwrap_or(u64::MAX).min(10))
}

pub async fn connect<R: Remote>(
    remote: &R,
    on_progress: impl FnMut(Charged<Progress>) + Send,
) -> Result<Connected<R::Client>, RemoteError> {
    tokio::time::timeout(PREPARE_TIMEOUT, attempts(remote, on_progress))
        .await
        .map_err(|_| RemoteError::PreparationTimeout)?
}

// An owned setup supplies the deadline and awaits cancellation itself, so an
// inner timeout must not drop its process cleanup future.
pub(super) async fn attempts<R: Remote>(
    remote: &R,
    mut on_progress: impl FnMut(Charged<Progress>) + Send,
) -> Result<Connected<R::Client>, RemoteError> {
    for attempt in 0..3 {
        let pending = remote.admit()?;
        let result = prepare_connection(remote, pending, &mut on_progress).await;
        let stale = matches!(&result, Err(RemoteError::DiscoveryChanged))
            || matches!(&result, Err(RemoteError::Native(error)) if crate::bridge::stale_instance(error));
        if !stale || attempt == 2 {
            return result;
        }
    }
    unreachable!("bounded discovery attempts return above")
}

fn step(
    budget: &ConnectionBudget,
    step: Step,
    message: std::fmt::Arguments<'_>,
) -> Result<Charged<Progress>, RemoteError> {
    budget
        .display(&message, true)
        .map(|message| message.map(|message| Progress::Step { step, message }))
        .map_err(RemoteError::Spawn)
}
async fn prepare_connection<R: Remote>(
    remote: &R,
    pending: R::Pending,
    mut on_progress: impl FnMut(Charged<Progress>) + Send,
) -> Result<Connected<R::Client>, RemoteError> {
    let host = remote.host();
    let version = remote.version();
    let context = remote.context();
    let budget = &context.budget;
    let mut report_error = None;
    remote
        .prepare(|arch| {
            match step(
                budget,
                Step::Install,
                format_args!("Installing Splitty {version} on {host} ({arch})..."),
            ) {
                Ok(report) => on_progress(report),
                Err(error) => {
                    report_error = Some(error);
                    context.close();
                }
            }
        })
        .await?;
    if let Some(error) = report_error {
        return Err(error);
    }
    on_progress(step(
        budget,
        Step::Daemon,
        format_args!("Asking {host} for its Splitty daemon..."),
    )?);
    let mut daemon = remote.daemon().await?;
    if daemon.started {
        on_progress(step(
            budget,
            Step::Started,
            format_args!("Started a private Splitty daemon on {host}."),
        )?);
    }
    if daemon.version != version {
        let server = remote.server_version();
        let bytes = (host.len()
            + daemon.version.len()
            + server.as_ref().map_or(0, |version| version.len()))
        .checked_mul(8)
        .and_then(|bytes| bytes.checked_add(4096))
        .ok_or_else(|| {
            RemoteError::Spawn(std::io::Error::other("update diagnostic exceeds capacity"))
        })?;
        let allocation = budget.reserve(bytes, false).map_err(|error| {
            RemoteError::Spawn(std::io::Error::new(std::io::ErrorKind::WouldBlock, error))
        })?;
        let plan = allocation.charge(update_plan(
            &daemon,
            host,
            version,
            server.as_deref().map(|version| version.as_str()),
        ));
        match &*plan {
            UpdatePlan::Update => {
                on_progress(step(
                    budget,
                    Step::Daemon,
                    format_args!(
                        "Updating Splitty on {host} to {version}; its sessions keep running..."
                    ),
                )?);
                remote.stop_daemon(&daemon).await?;
                daemon = remote.daemon().await?;
                if daemon.version != version {
                    return Err(RemoteError::InvalidAnswer {
                        allocation: None,
                        host: host.into(),
                        detail: "the old daemon did not stop or the selected update did not start"
                            .into(),
                    });
                }
            }
            UpdatePlan::Notice(_) => on_progress(plan.map(|plan| match plan {
                UpdatePlan::Notice(message) => Progress::Notice { message },
                UpdatePlan::Update => unreachable!("matched notice"),
            })),
        }
    }
    if !daemon.supports_native() {
        return Err(RemoteError::InvalidAnswer { allocation: None, host: host.into(), detail: "the running daemon does not support native transport v1; explicit migration is required".into() });
    }
    if !daemon.supports_mux(remote.protocol()) {
        return Err(RemoteError::InvalidAnswer {
            allocation: None,
            host: host.into(),
            detail: format!("incompatible daemon mux protocol {}", daemon.protocol),
        });
    }
    on_progress(step(
        budget,
        Step::Tunnel,
        format_args!("Opening the native SSH connection to {host}..."),
    )?);
    let client = remote.open_bridge(&daemon, pending).await?;
    Ok(Connected { daemon, client })
}

/// The owner consumes each connection once. Reconnection creates a fresh
/// application generation; this layer stores or replays no API/session input.
pub async fn supervise<R, Use, Used>(
    remote: &R,
    mut connected: Connected<R::Client>,
    give_up_after: Duration,
    shutdown: impl Future<Output = ()> + Send,
    mut use_connection: Use,
    mut on_event: impl FnMut(Event) + Send,
) -> Result<(), RemoteError>
where
    R: Remote,
    Use: FnMut(Connected<R::Client>) -> Used + Send,
    Used: Future<Output = Result<String, RemoteError>> + Send,
{
    tokio::pin!(shutdown);
    loop {
        let previous = connected.daemon.instance.clone();
        let reason = tokio::select! { biased;
            _ = &mut shutdown => return Ok(()),
            reason = use_connection(connected) => reason,
        };
        let reason = match reason {
            Ok(reason) => reason,
            Err(error) if error.retryable() => error.to_string(),
            Err(error) => return Err(error),
        };
        on_event(Event::Lost { reason });
        let deadline = Instant::now() + give_up_after;
        let mut failures = 0;
        loop {
            if Instant::now() >= deadline {
                return Err(RemoteError::GaveUp {
                    host: remote.host().into(),
                    after: give_up_after,
                    last: Box::new(RemoteError::PreparationTimeout),
                });
            }
            let result = tokio::select! { biased;
                _ = &mut shutdown => return Ok(()),
                result = tokio::time::timeout_at(deadline, connect(remote, |_| {})) => result,
            };
            match result {
                Ok(Ok(next)) => {
                    if next.daemon.instance == previous {
                        on_event(Event::Restored);
                    } else {
                        on_event(Event::Replaced {
                            instance: next
                                .daemon
                                .instance
                                .clone()
                                .expect("admitted native daemon"),
                        });
                    }
                    connected = next;
                    break;
                }
                Ok(Err(error)) => {
                    // Credentials and host identity require an explicit user fix;
                    // never reconnect by weakening SSH's authentication policy.
                    if !error.retryable() {
                        return Err(error);
                    }
                    if Instant::now() >= deadline {
                        return Err(RemoteError::GaveUp {
                            host: remote.host().into(),
                            after: give_up_after,
                            last: Box::new(error),
                        });
                    }
                    let retry_in =
                        backoff(failures).min(deadline.saturating_duration_since(Instant::now()));
                    failures = failures.saturating_add(1);
                    on_event(Event::RetryFailed { error, retry_in });
                    tokio::select! { biased; _ = &mut shutdown => return Ok(()), _ = sleep(retry_in) => {} }
                }
                Err(_) => {
                    return Err(RemoteError::GaveUp {
                        host: remote.host().into(),
                        after: give_up_after,
                        last: Box::new(RemoteError::InvalidAnswer {
                            allocation: None,
                            host: remote.host().into(),
                            detail: "reconnection deadline exceeded".into(),
                        }),
                    })
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    };
    struct Fake {
        context: splitty_transport::queue::Context,
        daemons: Mutex<VecDeque<RunningDaemon>>,
        version: &'static str,
        server: &'static str,
        calls: Mutex<Vec<&'static str>>,
        admitted: AtomicUsize,
        active: Arc<AtomicUsize>,
        errors: Mutex<VecDeque<RemoteError>>,
        stalled: AtomicBool,
        reject: AtomicBool,
    }
    impl Fake {
        fn new(daemon: RunningDaemon) -> Self {
            Self {
                context: Default::default(),
                daemons: Mutex::new(VecDeque::from([daemon])),
                version: "0.1.30",
                server: "0.1.30",
                calls: Mutex::new(Vec::new()),
                admitted: AtomicUsize::new(0),
                active: Arc::default(),
                errors: Mutex::default(),
                stalled: AtomicBool::new(false),
                reject: AtomicBool::new(false),
            }
        }
    }
    struct Pending(Arc<AtomicUsize>);
    impl Drop for Pending {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    impl Remote for Fake {
        type Client = String;
        type Pending = Pending;
        fn host(&self) -> &str {
            "host"
        }
        fn version(&self) -> &str {
            self.version
        }
        fn protocol(&self) -> u32 {
            1
        }
        fn context(&self) -> splitty_transport::queue::Context {
            self.context.clone()
        }
        fn admit(&self) -> Result<Pending, RemoteError> {
            if self.reject.load(Ordering::SeqCst) {
                return Err(RemoteError::Spawn(std::io::Error::from(
                    std::io::ErrorKind::WouldBlock,
                )));
            }
            self.admitted.fetch_add(1, Ordering::SeqCst);
            self.calls.lock().unwrap().push("admit");
            self.active.fetch_add(1, Ordering::SeqCst);
            Ok(Pending(self.active.clone()))
        }
        async fn prepare(&self, on_install: impl FnOnce(&str) + Send) -> Result<(), RemoteError> {
            self.calls.lock().unwrap().push("prepare");
            on_install("x86_64");
            if self.stalled.load(Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            if let Some(error) = self.errors.lock().unwrap().pop_front() {
                return Err(error);
            }
            Ok(())
        }
        async fn daemon(&self) -> Result<Charged<RunningDaemon>, RemoteError> {
            self.calls.lock().unwrap().push("daemon");
            let mut daemons = self.daemons.lock().unwrap();
            Ok(self
                .context
                .budget
                .charge(4096, false, || {
                    if daemons.len() > 1 {
                        daemons.pop_front().unwrap()
                    } else {
                        daemons.front().unwrap().clone()
                    }
                })
                .unwrap())
        }
        fn server_version(&self) -> Option<Arc<Charged<String>>> {
            Some(Arc::new(
                self.context
                    .budget
                    .charge(256, false, || self.server.into())
                    .unwrap(),
            ))
        }
        async fn stop_daemon(&self, _: &RunningDaemon) -> Result<(), RemoteError> {
            self.calls.lock().unwrap().push("stop");
            Ok(())
        }
        async fn open_bridge(
            &self,
            daemon: &RunningDaemon,
            _: Pending,
        ) -> Result<String, RemoteError> {
            self.calls.lock().unwrap().push("bridge");
            Ok(daemon.instance.clone().unwrap())
        }
    }
    fn daemon(version: &str) -> RunningDaemon {
        RunningDaemon {
            pid: 42,
            version: version.into(),
            protocol: 1,
            started_at: "1".into(),
            keeps_sessions: true,
            started: false,
            instance: Some("0123456789abcdef0123456789abcdef".into()),
            transport_version: Some(1),
            legacy: false,
        }
    }

    #[tokio::test]
    async fn queued_progress_keeps_its_bytes_after_setup_is_cancelled() {
        let remote = Fake::new(daemon("0.1.30"));
        remote.stalled.store(true, Ordering::SeqCst);
        let budget = remote.context.budget.clone();
        let (reports, received) = tokio::sync::mpsc::channel(8);
        let mut opening = Box::pin(connect(&remote, move |report| {
            reports.try_send(report).unwrap();
        }));
        std::future::poll_fn(|cx| {
            assert!(opening.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        assert!(budget.control.used() > 0);
        drop(opening);
        assert_eq!(remote.active.load(Ordering::SeqCst), 0);
        assert!(
            budget.control.used() > 0,
            "receiver still owns the queued progress allocation"
        );
        drop(received);
        assert_eq!(budget.used(), 0);
    }

    #[tokio::test]
    async fn only_an_older_kept_daemon_with_the_matching_payload_is_updated() {
        for (version, client, keeps, server, update) in [
            ("0.1.29", "0.1.30", true, "0.1.30", true),
            ("0.1.31", "0.1.30", true, "0.1.30", false),
            ("0.1.29", "0.1.30", false, "0.1.30", false),
            ("0.1.29", "0.1.30", true, "0.1.29", false),
            ("unknown", "0.1.30", true, "0.1.30", false),
            ("0.1.30", "0.2.0-dev.1", true, "0.2.0-dev.1", true),
            ("0.2.0-dev.1", "0.2.0-dev.2", true, "0.2.0-dev.2", true),
            ("0.2.0-dev.2", "0.2.0-dev.1", true, "0.2.0-dev.1", false),
            ("0.2.0-dev.1", "0.2.0", true, "0.2.0", true),
            ("0.2.0", "0.2.0-dev.1", true, "0.2.0-dev.1", false),
            ("0.2.0-dev.9", "0.2.0-dev.10", true, "0.2.0-dev.10", true),
            ("0.2.0-dev.10", "0.2.0-dev.9", true, "0.2.0-dev.9", false),
            ("0.2.0+aaa", "0.2.0+zzz", true, "0.2.0+zzz", false),
            ("0.2.0+zzz", "0.2.0+aaa", true, "0.2.0+aaa", false),
            (
                "0.2.0-dev.1+aaa",
                "0.2.0-dev.1+zzz",
                true,
                "0.2.0-dev.1+zzz",
                false,
            ),
            ("v0.1.30", "v0.2.0-dev.1", true, "v0.2.0-dev.1", true),
            ("vv0.1.30", "0.2.0-dev.1", true, "0.2.0-dev.1", false),
            ("0.2.0-dev.01", "0.2.0", true, "0.2.0", false),
            ("0.2.0-dev.1", "0.2.0-dev.02", true, "0.2.0-dev.02", false),
            ("0.1", "0.2.0", true, "0.2.0", false),
            ("0.1.30", "unknown", true, "unknown", false),
            ("0.2.0-dev.1", "0.2.0-dev.2", false, "0.2.0-dev.2", false),
            ("0.2.0-dev.1", "0.2.0-dev.2", true, "0.2.0-dev.1", false),
            (
                "0.2.0-dev.1",
                "0.2.0-dev.2+aaa",
                true,
                "0.2.0-dev.2+zzz",
                false,
            ),
        ] {
            let mut old = daemon(version);
            old.keeps_sessions = keeps;
            let mut remote = Fake::new(old);
            remote.version = client;
            remote.server = server;
            if update {
                remote.daemons.lock().unwrap().push_back(daemon(client));
            }
            let connected = connect(&remote, |_| {}).await.unwrap();
            assert_eq!(
                connected.daemon.version,
                if update { client } else { version },
                "daemon {version}, client {client}, keeps {keeps}, payload {server}"
            );
            let calls = remote.calls.lock().unwrap();
            assert_eq!(calls[0], "admit");
            assert_eq!(
                calls.contains(&"stop"),
                update,
                "daemon {version}, client {client}, keeps {keeps}, payload {server}"
            );
            assert_eq!(calls.last(), Some(&"bridge"));
        }
    }

    #[tokio::test]
    async fn an_older_daemon_without_native_identity_requires_explicit_migration() {
        for legacy in [false, true] {
            let mut old = daemon("0.1.30");
            old.instance = None;
            old.legacy = legacy;
            let mut remote = Fake::new(old);
            remote.version = "0.2.0-dev.1";
            remote.server = remote.version;
            assert!(matches!(
                connect(&remote, |_| {}).await,
                Err(RemoteError::InvalidAnswer { .. })
            ));
            assert_eq!(
                *remote.calls.lock().unwrap(),
                ["admit", "prepare", "daemon"]
            );
        }
    }

    #[tokio::test]
    async fn incompatible_transport_or_mux_never_opens_a_bridge_or_tcp_fallback() {
        for (transport, protocol, legacy) in
            [(Some(999), 1, false), (Some(1), 2, false), (None, 1, true)]
        {
            let mut bad = daemon("0.1.30");
            bad.transport_version = transport;
            bad.protocol = protocol;
            bad.legacy = legacy;
            if legacy {
                bad.instance = None;
            }
            let remote = Fake::new(bad);
            assert!(connect(&remote, |_| {}).await.is_err());
            assert_eq!(
                *remote.calls.lock().unwrap(),
                ["admit", "prepare", "daemon"]
            );
        }
    }
    #[tokio::test]
    async fn reconnect_reports_a_new_instance_without_replaying_a_connection() {
        let remote = Fake::new(daemon("0.1.30"));
        let initial = connect(&remote, |_| {}).await.unwrap();
        remote.daemons.lock().unwrap()[0].instance =
            Some("abcdef0123456789abcdef0123456789".into());
        let events = Mutex::new(Vec::new());
        let used = AtomicUsize::new(0);
        let (stop, shutdown) = tokio::sync::oneshot::channel();
        let mut stop = Some(stop);
        supervise(
            &remote,
            initial,
            GIVE_UP_AFTER,
            async {
                let _ = shutdown.await;
            },
            |connection| {
                let first = used.fetch_add(1, Ordering::SeqCst) == 0;
                if !first {
                    stop.take().unwrap().send(()).unwrap();
                }
                async move {
                    drop(connection);
                    if !first {
                        std::future::pending::<()>().await;
                    }
                    Ok("lost".into())
                }
            },
            |event| events.lock().unwrap().push(event),
        )
        .await
        .unwrap();
        assert_eq!(used.load(Ordering::SeqCst), 2);
        assert!(matches!(
            events.lock().unwrap().as_slice(),
            [Event::Lost { .. }, Event::Replaced { .. }]
        ));
    }
    #[test]
    fn backoff_is_bounded() {
        assert_eq!(
            (0..6).map(|n| backoff(n).as_secs()).collect::<Vec<_>>(),
            [1, 2, 4, 8, 10, 10]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn setup_admission_timeout_and_cancellation_release_pending_capacity() {
        let remote = Fake::new(daemon("0.1.30"));
        remote.reject.store(true, Ordering::SeqCst);
        assert!(connect(&remote, |_| {}).await.is_err());
        assert!(remote.calls.lock().unwrap().is_empty());
        remote.reject.store(false, Ordering::SeqCst);
        remote.stalled.store(true, Ordering::SeqCst);
        let started = Instant::now();
        assert!(matches!(
            connect(&remote, |_| {}).await,
            Err(RemoteError::PreparationTimeout)
        ));
        assert_eq!(started.elapsed(), PREPARE_TIMEOUT);
        assert_eq!(remote.active.load(Ordering::SeqCst), 0);
        let mut attempt = Box::pin(connect(&remote, |_| {}));
        std::future::poll_fn(|cx| {
            assert!(attempt.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        assert_eq!(remote.active.load(Ordering::SeqCst), 1);
        drop(attempt);
        assert_eq!(remote.active.load(Ordering::SeqCst), 0);
    }

    fn ssh_failure(stderr: &str) -> crate::ssh::SshError {
        crate::ssh::SshError::Failed {
            allocation: None,
            status: Some(255),
            message: stderr.into(),
            stderr: stderr.into(),
            hint: None,
        }
    }

    #[tokio::test]
    async fn reconnect_stops_for_auth_host_identity_and_protocol_failures_at_every_setup_stage() {
        for error in [
            RemoteError::Ssh(ssh_failure("Permission denied (publickey).")),
            RemoteError::Ssh(ssh_failure("Host key verification failed.")),
            RemoteError::Install(crate::install::InstallError::Ssh(ssh_failure(
                "Permission denied (publickey).",
            ))),
            RemoteError::Install(crate::install::InstallError::Ssh(ssh_failure(
                "REMOTE HOST IDENTIFICATION HAS CHANGED",
            ))),
            RemoteError::InvalidAnswer {
                allocation: None,
                host: "host".into(),
                detail: "incompatible native transport".into(),
            },
        ] {
            let remote = Fake::new(daemon("0.1.30"));
            let initial = connect(&remote, |_| {}).await.unwrap();
            remote.errors.lock().unwrap().push_back(error);
            let events = Mutex::new(Vec::new());
            let result = supervise(
                &remote,
                initial,
                GIVE_UP_AFTER,
                std::future::pending(),
                |_| async { Ok("lost".into()) },
                |event| events.lock().unwrap().push(event),
            )
            .await;
            assert!(result.is_err());
            assert!(matches!(
                events.lock().unwrap().as_slice(),
                [Event::Lost { .. }]
            ));
            assert_eq!(remote.admitted.load(Ordering::SeqCst), 2);
            assert_eq!(remote.active.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn an_admitted_permanent_carrier_failure_does_not_start_reconnection() {
        for kind in [
            std::io::ErrorKind::InvalidData,
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::Unsupported,
        ] {
            let remote = Fake::new(daemon("0.1.30"));
            let initial = connect(&remote, |_| {}).await.unwrap();
            let calls = remote.calls.lock().unwrap().len();
            let result = supervise(
                &remote,
                initial,
                GIVE_UP_AFTER,
                std::future::pending(),
                |_| async move {
                    Err(RemoteError::Native(std::io::Error::new(
                        kind,
                        "invalid admitted carrier",
                    )))
                },
                |_| panic!("permanent failure must not announce a retry"),
            )
            .await;
            assert!(matches!(result, Err(RemoteError::Native(error)) if error.kind() == kind));
            assert_eq!(remote.calls.lock().unwrap().len(), calls);
            assert_eq!(remote.admitted.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn transient_failures_retry_within_one_deadline_and_same_instance_restores() {
        let remote = Fake::new(daemon("0.1.30"));
        let initial = connect(&remote, |_| {}).await.unwrap();
        remote
            .errors
            .lock()
            .unwrap()
            .push_back(RemoteError::Ssh(ssh_failure("Connection refused")));
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let mut stop = Some(stop);
        let used = AtomicUsize::new(0);
        let events = Mutex::new(Vec::new());
        let start = Instant::now();
        supervise(
            &remote,
            initial,
            GIVE_UP_AFTER,
            async {
                let _ = stopped.await;
            },
            |_| {
                let first = used.fetch_add(1, Ordering::SeqCst) == 0;
                if !first {
                    stop.take().unwrap().send(()).unwrap();
                }
                async move {
                    if !first {
                        std::future::pending::<()>().await;
                    }
                    Ok("lost".into())
                }
            },
            |event| events.lock().unwrap().push(event),
        )
        .await
        .unwrap();
        assert_eq!(start.elapsed(), Duration::from_secs(1));
        assert!(matches!(
            events.lock().unwrap().as_slice(),
            [
                Event::Lost { .. },
                Event::RetryFailed { .. },
                Event::Restored
            ]
        ));

        let initial = connect(&remote, |_| {}).await.unwrap();
        remote
            .errors
            .lock()
            .unwrap()
            .extend((0..10).map(|_| RemoteError::Ssh(ssh_failure("Connection timed out"))));
        let start = Instant::now();
        let result = supervise(
            &remote,
            initial,
            Duration::from_secs(4),
            std::future::pending(),
            |_| async { Ok("lost".into()) },
            |_| {},
        )
        .await;
        assert!(matches!(result, Err(RemoteError::GaveUp { .. })));
        assert_eq!(start.elapsed(), Duration::from_secs(4));
        assert_eq!(remote.active.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn stale_identity_and_setup_capacity_retry_but_invalid_protocol_does_not() {
        let stale = std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            splitty_transport::negotiation::StaleInstance,
        );
        assert!(RemoteError::Native(stale).retryable());
        assert!(RemoteError::DiscoveryChanged.retryable());
        for kind in [
            std::io::ErrorKind::WouldBlock,
            std::io::ErrorKind::TimedOut,
            std::io::ErrorKind::ConnectionReset,
        ] {
            assert!(RemoteError::Install(crate::install::InstallError::Ssh(
                crate::ssh::SshError::Spawn(std::io::Error::from(kind))
            ))
            .retryable());
        }
        assert!(
            !RemoteError::Native(std::io::Error::from(std::io::ErrorKind::InvalidData)).retryable()
        );
        assert!(!RemoteError::Install(crate::install::InstallError::Ssh(
            crate::ssh::SshError::Spawn(std::io::Error::from(std::io::ErrorKind::NotFound))
        ))
        .retryable());
    }
}
