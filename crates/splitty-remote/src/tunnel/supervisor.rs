//! Connecting, and rebuilding the tunnel when it drops.
//!
//! The supervisor talks to the host only through [`Remote`], so the reconnect
//! rules (when to reuse the browser's session, when to mint a new URL, how long
//! to wait, when to give up) are tested with a scripted host and paused time.

use std::future::Future;
use std::io;
use std::time::Duration;

use serde::Serialize;
use tokio::time::{sleep, Instant};

use super::forward::reserve;
use super::{session_url, Forward, PortForward, PortReservation, RemoteError, Reservations};
use crate::handshake::RunningDaemon;

/// How long `splitty connect` tries to rebuild a dropped tunnel before it
/// gives up.
pub const GIVE_UP_AFTER: Duration = Duration::from_secs(10 * 60);

/// What a host offers a client that connects to it.
pub trait Remote: Sync {
    type Tunnel: Tunnel + Send;

    /// The target as the user typed it, for messages and the session URL.
    fn host(&self) -> &str;

    /// This client's version: the server the host gets.
    fn version(&self) -> &str;

    /// The host's ports to forward next to the daemon's, each to the local
    /// port asked for when that one is free here.
    fn forwards(&self) -> &[PortForward] {
        &[]
    }

    /// Make sure the host has this client's server, installing it when the
    /// client has one for the host. `on_install` gets the host's architecture
    /// before an upload starts.
    fn prepare(
        &self,
        on_install: impl FnOnce(&str) + Send,
    ) -> impl Future<Output = Result<(), RemoteError>> + Send;

    /// The host's daemon. A missing one is started on `port_hint`, a port the
    /// caller holds free on this side, so both ends of the forward match.
    fn daemon(
        &self,
        port_hint: u16,
    ) -> impl Future<Output = Result<RunningDaemon, RemoteError>> + Send;

    /// The version of the host's server that answered the last handshake: a
    /// daemon started after a stop is that one's.
    fn server_version(&self) -> Option<String>;

    /// Stop the host's daemon with `daemon stop`.
    fn stop_daemon(&self) -> impl Future<Output = Result<(), RemoteError>> + Send;

    /// Release `ports` and forward them in one tunnel: the daemon's local port
    /// to the daemon's `port`, and each forward's to its port on the host.
    /// Verified before it returns.
    fn open_tunnel(
        &self,
        ports: Reservations,
        port: u16,
    ) -> impl Future<Output = Result<Self::Tunnel, RemoteError>> + Send;

    /// A single-use bootstrap code minted by the host's daemon.
    fn issue_bootstrap(&self) -> impl Future<Output = Result<String, RemoteError>> + Send;
}

/// An open forward.
pub trait Tunnel {
    /// Resolves when the forward has ended, with a description of why.
    /// Cancel-safe: the supervisor polls it again after other events.
    fn closed(&mut self) -> impl Future<Output = String> + Send;

    /// End the forward.
    fn close(&mut self) -> impl Future<Output = ()> + Send;
}

/// An established connection: the daemon it reaches (with whether the
/// connect started it), its tunnel, the URL that opens a session there, and
/// the forwards the tunnel holds next to the daemon's port.
pub struct Connected<T> {
    pub daemon: RunningDaemon,
    pub tunnel: T,
    pub url: String,
    pub forwards: Vec<Forward>,
}

/// The steps of [`connect`], in the order they can occur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Step {
    /// The server is being installed on the host.
    Install,
    /// The host is asked for its daemon (and starts a missing one).
    Daemon,
    /// The host started its daemon for this connect.
    Started,
    /// The forward is being opened.
    Tunnel,
}

/// What [`connect`] reports while it runs, each with a message for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    Step {
        step: Step,
        message: String,
    },
    /// Something the user should know, such as a daemon of another version.
    Notice {
        message: String,
    },
}

/// What the supervisor reports while it keeps the tunnel up.
#[derive(Debug)]
pub enum Event {
    /// The forward ended; the supervisor starts rebuilding it.
    Lost { reason: String },
    /// An attempt failed; the next one follows after `retry_in`.
    RetryFailed {
        error: RemoteError,
        retry_in: Duration,
    },
    /// The same daemon is reachable again; the browser's session still works.
    Restored,
    /// A different daemon (restarted, or on another port) is reachable; the
    /// browser needs this new URL.
    NewUrl { url: String },
}

/// What `connect` does about a daemon of another version than the client's.
#[derive(Debug, PartialEq, Eq)]
enum Plan {
    /// Stop it and start this client's: it is older, its sessions outlive it,
    /// and the host's answering server is this client's version.
    Update,
    /// Use it as it is, and tell the user why.
    Notice(String),
}

fn update_plan(daemon: &RunningDaemon, host: &str, version: &str, server: Option<&str>) -> Plan {
    let host_version = &daemon.version;
    if !older(host_version, version) {
        // Never replace a newer daemon: it could fall back to a version
        // without keepers, which would start fresh shells and leave the
        // kept ones unadopted.
        return Plan::Notice(format!(
            "The daemon on {host} runs Splitty {host_version}, this client is {version}; \
             it is used as it is."
        ));
    }
    if !daemon.keeps_sessions {
        return Plan::Notice(format!(
            "The daemon on {host} runs Splitty {host_version}, this client is {version}. \
             Updating it ends its sessions: to update, stop it there (`daemon stop`) and \
             connect again."
        ));
    }
    if server != Some(version) {
        return Plan::Notice(format!(
            "The daemon on {host} runs Splitty {host_version}, this client is {version}. \
             `splitty daemon restart` there, with a newer splitty, updates it without ending \
             its sessions."
        ));
    }
    Plan::Update
}

/// Whether version `a` is older than `b`, comparing their dot-separated
/// numbers. A version that does not parse is never older: nothing is replaced
/// on a doubt.
fn older(a: &str, b: &str) -> bool {
    fn numbers(version: &str) -> Option<Vec<u64>> {
        let core = version.trim_start_matches('v');
        let core = core.split(['-', '+']).next().unwrap_or(core);
        core.split('.').map(|part| part.parse().ok()).collect()
    }
    match (numbers(a), numbers(b)) {
        (Some(a), Some(b)) => a < b,
        _ => false,
    }
}

/// The wait after the `failures`-th failed attempt in a row: 1, 2, 4, 8, then
/// 10 seconds.
pub fn backoff(failures: u32) -> Duration {
    Duration::from_secs(1u64.checked_shl(failures).unwrap_or(u64::MAX).min(10))
}

/// Make sure the host has the server, ask it for its daemon, forward the
/// daemon's port and mint the first URL, reporting each step to `on_progress`.
pub async fn connect<R: Remote>(
    remote: &R,
    mut on_progress: impl FnMut(Progress) + Send,
) -> Result<Connected<R::Tunnel>, RemoteError> {
    let host = remote.host();
    let version = remote.version();
    remote
        .prepare(|arch| {
            on_progress(Progress::Step {
                step: Step::Install,
                message: format!("Installing Splitty {version} on {host} ({arch})..."),
            })
        })
        .await?;

    // The local end must use the *same* port as the remote daemon: the browser
    // sends the local URL as its `Origin`, and the remote server accepts only
    // its own origin (SR-07). A daemon the host starts now gets a port that is
    // free here, held until the forward takes it over.
    let hint = PortReservation::acquire()
        .map_err(|source| RemoteError::PortUnavailable { port: 0, source })?;
    on_progress(Progress::Step {
        step: Step::Daemon,
        message: format!("Asking {host} for its Splitty daemon..."),
    });
    let mut daemon = remote.daemon(hint.port()).await?;
    if daemon.started {
        on_progress(Progress::Step {
            step: Step::Started,
            message: format!("Started a Splitty daemon on {host} (port {}).", daemon.port),
        });
    }
    if daemon.version != version {
        match update_plan(&daemon, host, version, remote.server_version().as_deref()) {
            Plan::Update => {
                // Its sessions outlive it: the daemon this client's server
                // starts next adopts them (`session-keepers`).
                on_progress(Progress::Step {
                    step: Step::Daemon,
                    message: format!(
                        "Updating Splitty {} on {host} to {version}; its sessions keep running...",
                        daemon.version
                    ),
                });
                remote.stop_daemon().await?;
                daemon = remote.daemon(hint.port()).await?;
                if daemon.started {
                    on_progress(Progress::Step {
                        step: Step::Started,
                        message: format!(
                            "Started a Splitty daemon on {host} (port {}).",
                            daemon.port
                        ),
                    });
                }
                if daemon.version != version {
                    on_progress(Progress::Notice {
                        message: format!(
                            "The daemon on {host} still runs Splitty {}; it is used as it is.",
                            daemon.version
                        ),
                    });
                }
            }
            Plan::Notice(message) => on_progress(Progress::Notice { message }),
        }
    }
    let reservation = if daemon.port == hint.port() {
        hint
    } else {
        drop(hint);
        PortReservation::acquire_port(daemon.port).map_err(|source| {
            RemoteError::PortUnavailable {
                port: daemon.port,
                source,
            }
        })?
    };
    // The forwards come after the daemon's port, which has to match the
    // host's; theirs may move.
    let forwards = remote
        .forwards()
        .iter()
        .map(|forward| reserve(*forward))
        .collect::<io::Result<Vec<_>>>()
        .map_err(|source| RemoteError::PortUnavailable { port: 0, source })?;
    let opened: Vec<Forward> = forwards.iter().map(|(forward, _)| *forward).collect();
    on_progress(Progress::Step {
        step: Step::Tunnel,
        message: format!(
            "Opening the SSH tunnel 127.0.0.1:{port} -> {host}:{port}...",
            port = daemon.port
        ),
    });
    let ports = Reservations {
        daemon: reservation,
        forwards,
    };
    let mut tunnel = remote.open_tunnel(ports, daemon.port).await?;
    // Only now, against a verified forward, is a credential requested.
    match mint_url(remote, daemon.port).await {
        Ok(url) => Ok(Connected {
            daemon,
            tunnel,
            url,
            forwards: opened,
        }),
        Err(error) => {
            tunnel.close().await;
            Err(error)
        }
    }
}

async fn mint_url<R: Remote>(remote: &R, port: u16) -> Result<String, RemoteError> {
    let code = remote.issue_bootstrap().await?;
    Ok(session_url(port, &code, remote.host()))
}

/// A local port the supervisor keeps while no `ssh` holds it (SR-13).
struct Held {
    port: u16,
    reservation: Option<PortReservation>,
}

impl Held {
    fn new(port: u16) -> Self {
        Self {
            port,
            reservation: None,
        }
    }

    /// Reserve the port, unless that is already done. Failing is not fatal
    /// here: the next attempt reports it.
    fn ensure(&mut self) {
        if self.reservation.is_none() {
            self.reservation = PortReservation::acquire_port(self.port).ok();
        }
    }

    fn take(&mut self) -> io::Result<PortReservation> {
        match self.reservation.take() {
            Some(reservation) => Ok(reservation),
            None => PortReservation::acquire_port(self.port),
        }
    }
}

/// The local ports of a dropped tunnel: the daemon's and every forward's. The
/// forwards keep theirs, so the pages open on them keep working.
struct Hold {
    daemon: Held,
    forwards: Vec<(Forward, Held)>,
}

impl Hold {
    fn new(port: u16, forwards: &[Forward]) -> Self {
        Self {
            daemon: Held::new(port),
            forwards: forwards
                .iter()
                .map(|forward| (*forward, Held::new(forward.local)))
                .collect(),
        }
    }

    fn port(&self) -> u16 {
        self.daemon.port
    }

    /// Reserve every port not reserved yet.
    fn ensure(&mut self) {
        self.daemon.ensure();
        for (_, held) in &mut self.forwards {
            held.ensure();
        }
    }

    /// The daemon now listens on `port`; let go of the old one.
    fn move_to(&mut self, port: u16) {
        if self.daemon.port != port {
            self.daemon = Held::new(port);
        }
    }

    /// Every port, reserved for the next tunnel, or the first one that is not
    /// free. Then the ports taken so far stay held.
    fn take(&mut self) -> Result<Reservations, RemoteError> {
        let daemon = self
            .daemon
            .take()
            .map_err(|source| RemoteError::PortUnavailable {
                port: self.daemon.port,
                source,
            })?;
        let mut forwards = Vec::with_capacity(self.forwards.len());
        let mut failure = None;
        for (forward, held) in &mut self.forwards {
            match held.take() {
                Ok(reservation) => forwards.push((*forward, reservation)),
                Err(source) => {
                    failure = Some(RemoteError::ForwardPortUnavailable {
                        local: forward.local,
                        remote: forward.remote,
                        source,
                    });
                    break;
                }
            }
        }
        let Some(error) = failure else {
            return Ok(Reservations { daemon, forwards });
        };
        self.daemon.reservation = Some(daemon);
        for ((_, held), (_, reservation)) in self.forwards.iter_mut().zip(forwards) {
            held.reservation = Some(reservation);
        }
        Err(error)
    }
}

/// One attempt to rebuild the tunnel to the daemon `previous` reached.
async fn reconnect<R: Remote>(
    remote: &R,
    previous: &RunningDaemon,
    hold: &mut Hold,
) -> Result<(RunningDaemon, R::Tunnel, Option<String>), RemoteError> {
    // A daemon that is gone (a rebooted host) is started on the previous
    // port, so the origin stays the same.
    let daemon = remote.daemon(hold.port()).await?;
    hold.move_to(daemon.port);
    let ports = hold.take()?;
    let mut tunnel = remote.open_tunnel(ports, daemon.port).await?;
    let same = daemon.pid == previous.pid
        && daemon.started_at == previous.started_at
        && daemon.port == previous.port;
    if same {
        // The browser's cookie belongs to this daemon's session store, and the
        // origin is unchanged: the open tab simply reconnects.
        return Ok((daemon, tunnel, None));
    }
    match mint_url(remote, daemon.port).await {
        Ok(url) => Ok((daemon, tunnel, Some(url))),
        Err(error) => {
            tunnel.close().await;
            Err(error)
        }
    }
}

/// Keep the tunnel up until `shutdown` resolves (`Ok`), rebuilding it when it
/// drops, or until it could not be rebuilt for `give_up_after`
/// ([`RemoteError::GaveUp`]; `splitty connect` passes [`GIVE_UP_AFTER`]).
pub async fn supervise<R: Remote>(
    remote: &R,
    mut connected: Connected<R::Tunnel>,
    give_up_after: Duration,
    shutdown: impl Future<Output = ()>,
    mut on_event: impl FnMut(Event),
) -> Result<(), RemoteError> {
    tokio::pin!(shutdown);
    loop {
        let reason = tokio::select! {
            () = &mut shutdown => {
                connected.tunnel.close().await;
                return Ok(());
            }
            reason = connected.tunnel.closed() => reason,
        };

        // Take the ports back before anything else can (SR-13).
        let mut hold = Hold::new(connected.daemon.port, &connected.forwards);
        hold.ensure();
        on_event(Event::Lost { reason });

        let started = Instant::now();
        let mut failures = 0;
        loop {
            match reconnect(remote, &connected.daemon, &mut hold).await {
                Ok((daemon, tunnel, url)) => {
                    match &url {
                        Some(url) => on_event(Event::NewUrl { url: url.clone() }),
                        None => on_event(Event::Restored),
                    }
                    connected = Connected {
                        daemon,
                        tunnel,
                        url: url.unwrap_or(connected.url),
                        forwards: connected.forwards,
                    };
                    break;
                }
                Err(error) => {
                    let retry_in = backoff(failures);
                    failures += 1;
                    if started.elapsed() + retry_in > give_up_after {
                        return Err(RemoteError::GaveUp {
                            host: remote.host().to_owned(),
                            after: give_up_after,
                            last: Box::new(error),
                        });
                    }
                    hold.ensure();
                    on_event(Event::RetryFailed { error, retry_in });
                    tokio::select! {
                        () = &mut shutdown => return Ok(()),
                        () = sleep(retry_in) => {}
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::sync::{mpsc, oneshot, watch};

    /// What the scripted host has: a daemon, none (the handshake then starts
    /// one on the port hint), or no network.
    #[derive(Clone)]
    enum Answer {
        Daemon(RunningDaemon),
        NoDaemon,
        Unreachable,
    }

    /// A host whose daemon, reachability and tunnels the test controls.
    struct FakeHost {
        /// The client's version.
        version: &'static str,
        /// Whether the host lacks the server, so that `prepare` installs it.
        lacks_server: bool,
        answer: Mutex<Answer>,
        codes: AtomicUsize,
        refuse_codes: Mutex<bool>,
        /// The ends of the open tunnels; sending a reason ends that tunnel.
        tunnels: Mutex<Vec<watch::Sender<Option<String>>>>,
        closed: AtomicUsize,
        /// The port hints of every handshake.
        hints: Mutex<Vec<u16>>,
        /// The forwards the client asks for.
        forwards: Vec<PortForward>,
        /// The forwards of every tunnel opened, in order.
        opened: Mutex<Vec<Vec<Forward>>>,
        /// The version of the host's answering server; a daemon started after
        /// a stop is that one's.
        server: Mutex<String>,
        /// How often the client stopped the host's daemon.
        stops: AtomicUsize,
    }

    struct FakeTunnel {
        ended: watch::Receiver<Option<String>>,
        host: Arc<FakeHost>,
    }

    impl Tunnel for FakeTunnel {
        async fn closed(&mut self) -> String {
            loop {
                if let Some(reason) = self.ended.borrow().clone() {
                    return reason;
                }
                if self.ended.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            }
        }

        async fn close(&mut self) {
            self.host.closed.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl Remote for Arc<FakeHost> {
        type Tunnel = FakeTunnel;

        fn host(&self) -> &str {
            "user@vm"
        }

        fn version(&self) -> &str {
            self.version
        }

        fn forwards(&self) -> &[PortForward] {
            &self.forwards
        }

        async fn prepare(&self, on_install: impl FnOnce(&str) + Send) -> Result<(), RemoteError> {
            if self.lacks_server {
                on_install("x86_64");
            }
            Ok(())
        }

        async fn daemon(&self, port_hint: u16) -> Result<RunningDaemon, RemoteError> {
            self.hints.lock().expect("hints").push(port_hint);
            let mut answer = self.answer.lock().expect("answer");
            match answer.clone() {
                Answer::Daemon(daemon) => Ok(RunningDaemon {
                    started: false,
                    ..daemon
                }),
                Answer::NoDaemon => {
                    let pid = 100 + self.hints.lock().expect("hints").len();
                    let started = RunningDaemon {
                        port: port_hint,
                        version: self.server.lock().expect("server").clone(),
                        ..daemon(u32::try_from(pid).expect("a small pid"))
                    };
                    *answer = Answer::Daemon(started.clone());
                    Ok(RunningDaemon {
                        started: true,
                        ..started
                    })
                }
                Answer::Unreachable => Err(RemoteError::Ssh {
                    message: "ssh: connect to host vm port 22: Network is unreachable".into(),
                    hint: None,
                }),
            }
        }

        fn server_version(&self) -> Option<String> {
            Some(self.server.lock().expect("server").clone())
        }

        async fn stop_daemon(&self) -> Result<(), RemoteError> {
            self.stops.fetch_add(1, Ordering::SeqCst);
            self.answer(Answer::NoDaemon);
            Ok(())
        }

        async fn open_tunnel(
            &self,
            ports: Reservations,
            port: u16,
        ) -> Result<FakeTunnel, RemoteError> {
            assert_eq!(
                ports.daemon.port(),
                port,
                "the forward must use the reserved port"
            );
            for (forward, reservation) in &ports.forwards {
                assert_eq!(
                    reservation.port(),
                    forward.local,
                    "a forward must use its reserved port"
                );
            }
            // The ports are free from here on, as if `ssh` held them.
            let forwards = ports.release();
            self.opened.lock().expect("opened").push(forwards);
            let (end, ended) = watch::channel(None);
            self.tunnels.lock().expect("tunnels").push(end);
            Ok(FakeTunnel {
                ended,
                host: Arc::clone(self),
            })
        }

        async fn issue_bootstrap(&self) -> Result<String, RemoteError> {
            if *self.refuse_codes.lock().expect("refuse") {
                return Err(RemoteError::Bootstrap(Box::new(RemoteError::Ssh {
                    message: "issue-bootstrap failed".into(),
                    hint: None,
                })));
            }
            let n = self.codes.fetch_add(1, Ordering::SeqCst);
            Ok(format!("{n:032x}"))
        }
    }

    impl FakeHost {
        fn new(answer: Answer) -> Arc<Self> {
            Arc::new(Self {
                version: "0.1.0",
                lacks_server: false,
                answer: Mutex::new(answer),
                codes: AtomicUsize::new(0),
                refuse_codes: Mutex::new(false),
                tunnels: Mutex::new(Vec::new()),
                closed: AtomicUsize::new(0),
                hints: Mutex::new(Vec::new()),
                forwards: Vec::new(),
                opened: Mutex::new(Vec::new()),
                server: Mutex::new("0.1.0".into()),
                stops: AtomicUsize::new(0),
            })
        }

        /// A host the client forwards `forwards` of.
        fn forwarding(answer: Answer, forwards: Vec<PortForward>) -> Arc<Self> {
            Arc::new(Self {
                forwards,
                ..Arc::into_inner(Self::new(answer)).expect("one owner")
            })
        }

        /// The forwards of the tunnel opened last.
        fn last_forwards(&self) -> Vec<Forward> {
            self.opened
                .lock()
                .expect("opened")
                .last()
                .expect("a tunnel")
                .clone()
        }

        fn answer(&self, answer: Answer) {
            *self.answer.lock().expect("answer") = answer;
        }

        /// The current tunnel's ssh process dies.
        fn drop_tunnel(&self) {
            let tunnels = self.tunnels.lock().expect("tunnels");
            let _ = tunnels
                .last()
                .expect("an open tunnel")
                .send(Some("ssh exited with 255".into()));
        }
    }

    /// A daemon on a loopback port that is free right now.
    fn daemon(pid: u32) -> RunningDaemon {
        let port = PortReservation::acquire().expect("free port").port();
        RunningDaemon {
            pid,
            port,
            version: "0.1.0".into(),
            protocol: 1,
            started_at: format!("started-{pid}"),
            keeps_sessions: false,
            started: false,
        }
    }

    struct Running {
        events: mpsc::UnboundedReceiver<Event>,
        stop: Option<oneshot::Sender<()>>,
        done: tokio::task::JoinHandle<Result<(), RemoteError>>,
    }

    async fn run(host: &Arc<FakeHost>) -> (Running, String) {
        run_for(host, GIVE_UP_AFTER).await
    }

    async fn run_for(host: &Arc<FakeHost>, give_up_after: Duration) -> (Running, String) {
        let connected = connect(host, |_| {}).await.expect("connect");
        let url = connected.url.clone();
        let (events_tx, events) = mpsc::unbounded_channel();
        let (stop, stopped) = oneshot::channel::<()>();
        let remote = Arc::clone(host);
        let done = tokio::spawn(async move {
            let shutdown = async {
                let _ = stopped.await;
            };
            supervise(&remote, connected, give_up_after, shutdown, move |event| {
                let _ = events_tx.send(event);
            })
            .await
        });
        let running = Running {
            events,
            stop: Some(stop),
            done,
        };
        (running, url)
    }

    impl Running {
        async fn next(&mut self) -> Event {
            self.events.recv().await.expect("an event")
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_first_url_carries_a_code_and_the_host() {
        let first = daemon(1);
        let host = FakeHost::new(Answer::Daemon(first.clone()));
        let (_running, url) = run(&host).await;
        assert_eq!(
            url,
            format!(
                "http://127.0.0.1:{}/?bootstrap={:032x}&host=user%40vm",
                first.port, 0
            )
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_missing_daemon_is_started_on_a_port_free_here() {
        let host = FakeHost::new(Answer::NoDaemon);
        let connected = connect(&host, |_| {}).await.expect("connect");
        assert!(connected.daemon.started);
        let hint = host.hints.lock().expect("hints")[0];
        assert_eq!(
            connected.daemon.port, hint,
            "the daemon got the free local port"
        );
        assert!(
            connected
                .url
                .starts_with(&format!("http://127.0.0.1:{hint}/")),
            "{}",
            connected.url
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_running_daemon_on_another_port_is_used_as_it_is() {
        let running = daemon(1);
        let host = FakeHost::new(Answer::Daemon(running.clone()));
        let connected = connect(&host, |_| {}).await.expect("connect");
        assert!(!connected.daemon.started);
        assert_eq!(connected.daemon.port, running.port);
    }

    #[tokio::test(start_paused = true)]
    async fn a_daemon_gone_after_a_reboot_is_started_again_on_the_previous_port() {
        let first = daemon(1);
        let host = FakeHost::new(Answer::Daemon(first.clone()));
        let (mut running, first_url) = run(&host).await;

        host.answer(Answer::NoDaemon);
        host.drop_tunnel();
        assert!(matches!(running.next().await, Event::Lost { .. }));
        match running.next().await {
            Event::NewUrl { url } => {
                assert_ne!(url, first_url);
                assert!(
                    url.starts_with(&format!("http://127.0.0.1:{}/", first.port)),
                    "{url}"
                );
            }
            other => panic!("expected a new URL, got {other:?}"),
        }
        assert_eq!(host.hints.lock().expect("hints").last(), Some(&first.port));
    }

    #[tokio::test(start_paused = true)]
    async fn a_remote_that_cannot_mint_a_code_closes_the_tunnel() {
        let host = FakeHost::new(Answer::Daemon(daemon(1)));
        *host.refuse_codes.lock().expect("refuse") = true;
        let error = connect(&host, |_| {}).await.err().expect("no code");
        assert!(matches!(error, RemoteError::Bootstrap(_)), "{error:?}");
        assert!(error
            .to_string()
            .starts_with("Failed to obtain a session from the remote host: "));
        assert_eq!(
            host.closed.load(Ordering::SeqCst),
            1,
            "the tunnel must be closed"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_dropped_tunnel_to_the_same_daemon_is_restored_without_a_new_url() {
        let host = FakeHost::new(Answer::Daemon(daemon(1)));
        let (mut running, _) = run(&host).await;

        host.drop_tunnel();
        assert!(matches!(running.next().await, Event::Lost { .. }));
        assert!(matches!(running.next().await, Event::Restored));
        assert_eq!(
            host.codes.load(Ordering::SeqCst),
            1,
            "no new code for the same daemon"
        );
        assert_eq!(host.tunnels.lock().expect("tunnels").len(), 2);

        // And again: the restored tunnel is supervised too.
        host.drop_tunnel();
        assert!(matches!(running.next().await, Event::Lost { .. }));
        assert!(matches!(running.next().await, Event::Restored));
    }

    #[tokio::test(start_paused = true)]
    async fn a_restarted_daemon_gets_a_new_url() {
        let first = daemon(1);
        let host = FakeHost::new(Answer::Daemon(first.clone()));
        let (mut running, first_url) = run(&host).await;

        host.answer(Answer::Daemon(RunningDaemon {
            pid: 2,
            started_at: "started-2".into(),
            ..first.clone()
        }));
        host.drop_tunnel();
        assert!(matches!(running.next().await, Event::Lost { .. }));
        match running.next().await {
            Event::NewUrl { url } => {
                assert_ne!(url, first_url);
                assert!(
                    url.starts_with(&format!("http://127.0.0.1:{}/?bootstrap=", first.port)),
                    "{url}"
                );
            }
            other => panic!("expected a new URL, got {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_daemon_on_another_port_gets_a_new_url_on_that_port() {
        let first = daemon(1);
        let host = FakeHost::new(Answer::Daemon(first.clone()));
        let (mut running, _) = run(&host).await;

        let moved = RunningDaemon {
            port: daemon(1).port,
            ..first.clone()
        };
        host.answer(Answer::Daemon(moved.clone()));
        host.drop_tunnel();
        assert!(matches!(running.next().await, Event::Lost { .. }));
        match running.next().await {
            Event::NewUrl { url } => assert!(
                url.starts_with(&format!("http://127.0.0.1:{}/", moved.port)),
                "{url}"
            ),
            other => panic!("expected a new URL, got {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn an_unreachable_host_is_retried_with_backoff_and_given_up_after_ten_minutes() {
        let first = daemon(1);
        let host = FakeHost::new(Answer::Daemon(first.clone()));
        let (mut running, _) = run(&host).await;
        let started = Instant::now();

        host.answer(Answer::Unreachable);
        host.drop_tunnel();
        assert!(matches!(running.next().await, Event::Lost { .. }));

        let mut waits = Vec::new();
        while let Some(event) = running.events.recv().await {
            match event {
                Event::RetryFailed { retry_in, .. } => {
                    waits.push(retry_in.as_secs());
                    // The port stays ours while nothing forwards it (SR-13).
                    assert!(
                        std::net::TcpListener::bind(("127.0.0.1", first.port)).is_err(),
                        "the local port must be held while reconnecting"
                    );
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(&waits[..6], [1, 2, 4, 8, 10, 10]);
        let result = running.done.await.expect("join");
        assert!(
            matches!(result, Err(RemoteError::GaveUp { .. })),
            "{result:?}"
        );
        let elapsed = started.elapsed();
        assert!(
            elapsed > GIVE_UP_AFTER - Duration::from_secs(11) && elapsed <= GIVE_UP_AFTER,
            "gave up after {elapsed:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_host_that_comes_back_is_reconnected() {
        let host = FakeHost::new(Answer::Daemon(daemon(1)));
        let (mut running, _) = run(&host).await;
        let answer = host.answer.lock().expect("answer").clone();

        host.answer(Answer::Unreachable);
        host.drop_tunnel();
        assert!(matches!(running.next().await, Event::Lost { .. }));
        assert!(matches!(running.next().await, Event::RetryFailed { .. }));
        host.answer(answer);
        assert!(matches!(running.next().await, Event::Restored));
    }

    #[tokio::test(start_paused = true)]
    async fn ctrl_c_while_waiting_ends_the_supervisor_cleanly() {
        let host = FakeHost::new(Answer::Daemon(daemon(1)));
        let (mut running, _) = run(&host).await;

        host.answer(Answer::Unreachable);
        host.drop_tunnel();
        assert!(matches!(running.next().await, Event::Lost { .. }));
        assert!(matches!(running.next().await, Event::RetryFailed { .. }));
        let _ = running.stop.take().expect("stop").send(());
        let result = running.done.await.expect("join");
        assert!(result.is_ok(), "{result:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn ctrl_c_with_the_tunnel_up_closes_it() {
        let host = FakeHost::new(Answer::Daemon(daemon(1)));
        let (mut running, _) = run(&host).await;
        let _ = running.stop.take().expect("stop").send(());
        let result = running.done.await.expect("join");
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(host.closed.load(Ordering::SeqCst), 1);
    }

    /// The give-up time is the caller's: a desktop window may wait less.
    #[tokio::test(start_paused = true)]
    async fn the_supervisor_gives_up_after_the_time_it_was_given() {
        let host = FakeHost::new(Answer::Daemon(daemon(1)));
        let give_up_after = Duration::from_secs(30);
        let (mut running, _) = run_for(&host, give_up_after).await;
        let started = Instant::now();

        host.answer(Answer::Unreachable);
        host.drop_tunnel();
        while running.events.recv().await.is_some() {}
        match running.done.await.expect("join") {
            Err(error @ RemoteError::GaveUp { .. }) => assert!(
                error.to_string().starts_with("giving up after 30 seconds"),
                "{error}"
            ),
            other => panic!("expected to give up, got {other:?}"),
        }
        assert!(started.elapsed() <= give_up_after);
    }

    /// The steps a client shows while it connects, in order: install, ask for
    /// the daemon, its start, a version notice, then the tunnel.
    #[tokio::test(start_paused = true)]
    async fn the_steps_of_a_connect_arrive_in_order() {
        let host = Arc::new(FakeHost {
            version: "0.2.0",
            lacks_server: true,
            ..Arc::into_inner(FakeHost::new(Answer::NoDaemon)).expect("one owner")
        });
        let mut reported = Vec::new();
        let connected = connect(&host, |progress| reported.push(progress))
            .await
            .expect("connect");
        let port = connected.daemon.port;
        let steps: Vec<_> = reported
            .iter()
            .map(|progress| match progress {
                Progress::Step { step, .. } => Some(*step),
                Progress::Notice { .. } => None,
            })
            .collect();
        assert_eq!(
            steps,
            [
                Some(Step::Install),
                Some(Step::Daemon),
                Some(Step::Started),
                None,
                Some(Step::Tunnel)
            ]
        );
        let messages: Vec<_> = reported
            .into_iter()
            .map(|progress| match progress {
                Progress::Step { message, .. } | Progress::Notice { message } => message,
            })
            .collect();
        assert_eq!(
            messages,
            [
                "Installing Splitty 0.2.0 on user@vm (x86_64)...".to_owned(),
                "Asking user@vm for its Splitty daemon...".to_owned(),
                format!("Started a Splitty daemon on user@vm (port {port})."),
                "The daemon on user@vm runs Splitty 0.1.0, this client is 0.2.0. Updating it \
                 ends its sessions: to update, stop it there (`daemon stop`) and connect again."
                    .to_owned(),
                format!("Opening the SSH tunnel 127.0.0.1:{port} -> user@vm:{port}..."),
            ]
        );
    }

    /// `cli`, Remote Connect Tunnel, "Older daemon that keeps its sessions":
    /// stopped, and the client's own version started, which adopts them.
    #[tokio::test(start_paused = true)]
    async fn an_older_daemon_that_keeps_its_sessions_is_updated() {
        let host = Arc::new(FakeHost {
            version: "0.2.0",
            server: Mutex::new("0.2.0".into()),
            ..Arc::into_inner(FakeHost::new(Answer::Daemon(RunningDaemon {
                keeps_sessions: true,
                ..daemon(1)
            })))
            .expect("one owner")
        });
        let mut reported = Vec::new();
        let connected = connect(&host, |progress| reported.push(progress))
            .await
            .expect("connect");
        assert_eq!(host.stops.load(Ordering::SeqCst), 1);
        assert_eq!(connected.daemon.version, "0.2.0");
        assert!(connected.daemon.started);
        let messages: Vec<_> = reported
            .iter()
            .map(|progress| match progress {
                Progress::Step { message, .. } => message.clone(),
                Progress::Notice { message } => panic!("unexpected notice {message}"),
            })
            .collect();
        assert!(
            messages.contains(
                &"Updating Splitty 0.1.0 on user@vm to 0.2.0; its sessions keep running..."
                    .to_owned()
            ),
            "{messages:?}"
        );
    }

    /// `cli`, Remote Connect Tunnel, "Newer daemon, or one whose sessions
    /// would end", and an older one without this client's server on the host.
    #[tokio::test(start_paused = true)]
    async fn other_daemons_are_used_as_they_are() {
        for (daemon_version, keeps, server, notice) in [
            ("0.3.0", true, "0.2.0", "it is used as it is"),
            ("0.1.0", false, "0.2.0", "Updating it ends its sessions"),
            ("0.1.0", true, "0.1.0", "`splitty daemon restart` there"),
        ] {
            let host = Arc::new(FakeHost {
                version: "0.2.0",
                server: Mutex::new(server.into()),
                ..Arc::into_inner(FakeHost::new(Answer::Daemon(RunningDaemon {
                    version: daemon_version.into(),
                    keeps_sessions: keeps,
                    ..daemon(1)
                })))
                .expect("one owner")
            });
            let mut notices = Vec::new();
            let connected = connect(&host, |progress| {
                if let Progress::Notice { message } = progress {
                    notices.push(message);
                }
            })
            .await
            .expect("connect");
            assert_eq!(
                host.stops.load(Ordering::SeqCst),
                0,
                "{daemon_version} stays"
            );
            assert_eq!(connected.daemon.version, daemon_version);
            assert!(
                notices.iter().any(|message| message.contains(notice)),
                "{daemon_version}: {notices:?}"
            );
        }
    }

    #[test]
    fn versions_compare_by_their_numbers() {
        assert!(older("0.1.9", "0.1.10"));
        assert!(older("0.1.17", "0.2.0"));
        assert!(older("v1.2.3", "1.3"));
        assert!(!older("0.2.0", "0.2.0"));
        assert!(!older("0.3.0", "0.2.9"));
        assert!(!older("unknown", "0.2.0"), "never older on a doubt");
        assert!(!older("0.1.0", "dev"));
        assert!(older("0.1.0-rc1", "0.1.1"));
    }

    /// A host that has the server and its daemon reports only what happens.
    #[tokio::test(start_paused = true)]
    async fn a_ready_host_reports_only_the_daemon_and_the_tunnel() {
        let host = FakeHost::new(Answer::Daemon(daemon(1)));
        let mut steps = Vec::new();
        connect(&host, |progress| steps.push(progress))
            .await
            .expect("connect");
        let steps: Vec<_> = steps
            .into_iter()
            .map(|progress| match progress {
                Progress::Step { step, .. } => step,
                Progress::Notice { message } => panic!("unexpected notice {message}"),
            })
            .collect();
        assert_eq!(steps, [Step::Daemon, Step::Tunnel]);
    }

    /// A loopback port free right now.
    fn free_port() -> u16 {
        PortReservation::acquire().expect("free port").port()
    }

    fn is_held(port: u16) -> bool {
        std::net::TcpListener::bind(("127.0.0.1", port)).is_err()
    }

    /// `cli`, Remote Connect Port Forwards (Local port busy).
    #[tokio::test(start_paused = true)]
    async fn forwards_take_the_asked_port_or_any_free_one() {
        let free = free_port();
        let taken = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let busy = taken.local_addr().expect("address").port();
        let host = FakeHost::forwarding(
            Answer::Daemon(daemon(1)),
            vec![
                PortForward {
                    local: free,
                    remote: 3000,
                },
                PortForward {
                    local: busy,
                    remote: 5173,
                },
            ],
        );
        let connected = connect(&host, |_| {}).await.expect("connect");
        let &[first, second] = connected.forwards.as_slice() else {
            panic!("two forwards: {:?}", connected.forwards);
        };
        assert_eq!(
            first,
            Forward {
                remote: 3000,
                local: free,
                asked: free
            }
        );
        assert_eq!((second.remote, second.asked), (5173, busy));
        assert_ne!(second.local, busy, "the busy port is left alone");
        assert_eq!(
            host.last_forwards(),
            connected.forwards,
            "the tunnel holds the forwards it reports"
        );
    }

    /// `cli`, Remote Connect Port Forwards (Forwards survive a reconnect), and
    /// `access-control`, Forwarded Ports Bound To Loopback: held while the host
    /// is away, the same ports again afterwards, also for a restarted daemon.
    #[tokio::test(start_paused = true)]
    async fn forwards_keep_their_local_ports_through_reconnects() {
        let first = daemon(1);
        let host = FakeHost::forwarding(
            Answer::Daemon(first.clone()),
            vec![
                PortForward {
                    local: free_port(),
                    remote: 3000,
                },
                PortForward {
                    local: free_port(),
                    remote: 5173,
                },
            ],
        );
        let (mut running, _) = run(&host).await;
        let forwards = host.last_forwards();
        assert_eq!(forwards.len(), 2);

        host.answer(Answer::Unreachable);
        host.drop_tunnel();
        assert!(matches!(running.next().await, Event::Lost { .. }));
        assert!(matches!(running.next().await, Event::RetryFailed { .. }));
        for forward in &forwards {
            assert!(
                is_held(forward.local),
                "port {} must be held while reconnecting",
                forward.local
            );
        }
        host.answer(Answer::Daemon(first.clone()));
        assert!(matches!(running.next().await, Event::Restored));
        assert_eq!(host.last_forwards(), forwards);

        host.answer(Answer::Daemon(RunningDaemon {
            pid: 2,
            started_at: "started-2".into(),
            ..first
        }));
        host.drop_tunnel();
        assert!(matches!(running.next().await, Event::Lost { .. }));
        assert!(matches!(running.next().await, Event::NewUrl { .. }));
        assert_eq!(host.last_forwards(), forwards);
        assert_eq!(host.opened.lock().expect("opened").len(), 3);
    }

    /// A forward's port taken while the tunnel was down is waited for, not
    /// replaced: the pages open on it would lose their server.
    #[tokio::test(start_paused = true)]
    async fn a_forward_port_taken_meanwhile_is_waited_for() {
        let first = daemon(1);
        let host = FakeHost::forwarding(
            Answer::Daemon(first.clone()),
            vec![PortForward {
                local: free_port(),
                remote: 3000,
            }],
        );
        let (mut running, _) = run(&host).await;
        let local = host.last_forwards()[0].local;

        // The scripted tunnel leaves its ports free, so a process can take one,
        // as it could between a dying `ssh` and the supervisor's reservation.
        let taken = std::net::TcpListener::bind(("127.0.0.1", local)).expect("take the port");
        host.drop_tunnel();
        assert!(matches!(running.next().await, Event::Lost { .. }));
        match running.next().await {
            Event::RetryFailed {
                error:
                    RemoteError::ForwardPortUnavailable {
                        local: port,
                        remote: 3000,
                        ..
                    },
                ..
            } => assert_eq!(port, local),
            other => panic!("expected the forward's port to be reported, got {other:?}"),
        }
        assert!(is_held(first.port), "the daemon's port stays held");
        drop(taken);
        assert!(matches!(running.next().await, Event::Restored));
        assert_eq!(host.last_forwards()[0].local, local);
    }

    #[test]
    fn the_backoff_grows_to_ten_seconds() {
        let waits: Vec<u64> = (0..7).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(waits, [1, 2, 4, 8, 10, 10, 10]);
        assert_eq!(backoff(200), Duration::from_secs(10));
    }
}
