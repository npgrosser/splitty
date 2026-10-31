//! Ports of the host forwarded next to the daemon's (`splitty connect
//! --forward`), or each by an `ssh` of its own ([`PortTunnel`], the remote
//! screen hub's forwards).
//!
//! Each forward is bound to `127.0.0.1` here and reaches `localhost:<port>` on
//! the host, a name `sshd` resolves there, so a dev server that listens only on
//! IPv6 is reached as well. It is announced as `http://localhost:<port>`: the
//! session cookie of a Splitty server belongs to the host `127.0.0.1`, whatever
//! its port, so a browser never sends it to a forwarded server opened as
//! `localhost`.

use std::io;
use std::net::{Ipv6Addr, TcpListener};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

use tokio::process::Child;
use tokio::task::JoinHandle;

use super::{wait_for_forward, PortReservation};
use crate::ssh::{classify, keep_end, kept_end, Ssh, SshError};

/// A forward as the user asks for it: `PORT`, or `LOCAL:REMOTE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortForward {
    /// The local port asked for: `LOCAL`, or the host's port.
    pub local: u16,
    /// The host's port.
    pub remote: u16,
}

/// A forward a tunnel holds: the host's `remote` port at `local` here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Forward {
    pub remote: u16,
    pub local: u16,
    /// The local port asked for; `local` differs when that one was not free.
    pub asked: u16,
}

impl Forward {
    /// The URL the forward is announced as.
    pub fn url(&self) -> String {
        format!("http://localhost:{}", self.local)
    }
}

/// Why a `--forward` list was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ForwardError {
    #[error(
        "invalid --forward entry '{entry}': expected PORT or LOCAL:REMOTE, with ports from 1 \
         to 65535"
    )]
    Invalid { entry: String },

    #[error("--forward names port {port} of the host twice")]
    RemoteTwice { port: u16 },

    #[error("--forward names local port {port} twice")]
    LocalTwice { port: u16 },
}

/// The forwards of every `--forward` list, in order. A list is comma-separated
/// and each entry is `PORT` or `LOCAL:REMOTE`; a host port or a local port
/// named twice, across all lists, is rejected.
pub fn parse_forwards<S: AsRef<str>>(lists: &[S]) -> Result<Vec<PortForward>, ForwardError> {
    let mut forwards: Vec<PortForward> = Vec::new();
    for entry in lists.iter().flat_map(|list| list.as_ref().split(',')) {
        let entry = entry.trim();
        let forward = parse_entry(entry).ok_or_else(|| ForwardError::Invalid {
            entry: entry.to_owned(),
        })?;
        if forwards.iter().any(|known| known.remote == forward.remote) {
            return Err(ForwardError::RemoteTwice {
                port: forward.remote,
            });
        }
        if forwards.iter().any(|known| known.local == forward.local) {
            return Err(ForwardError::LocalTwice {
                port: forward.local,
            });
        }
        forwards.push(forward);
    }
    Ok(forwards)
}

fn parse_entry(entry: &str) -> Option<PortForward> {
    match entry.split_once(':') {
        Some((local, remote)) => Some(PortForward {
            local: parse_port(local)?,
            remote: parse_port(remote)?,
        }),
        None => parse_port(entry).map(|port| PortForward {
            local: port,
            remote: port,
        }),
    }
}

/// A port from 1 to 65535 in decimal digits; `u16::from_str` would also take
/// a sign.
fn parse_port(text: &str) -> Option<u16> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok().filter(|port| *port != 0)
}

/// Reserve the local port of `forward`: the one asked for when it is free here,
/// any free loopback port otherwise.
///
/// Free means free on `::1` too, although the forward binds only `127.0.0.1`: a
/// browser tries `localhost` on `::1` first, so a local server listening there
/// (as Node's dev servers do by default) would answer the announced URL in
/// place of the forward.
pub fn reserve(forward: PortForward) -> io::Result<(Forward, PortReservation)> {
    let reservation = match reserve_exact(forward.local) {
        Ok(reservation) => reservation,
        Err(_) => reserve_free(PortReservation::acquire, |held| {
            held.check_forward_address()
        })?,
    };
    Ok((
        Forward {
            remote: forward.remote,
            local: reservation.port(),
            asked: forward.local,
        },
        reservation,
    ))
}

/// Reserve exactly this IPv4 loopback port, checking IPv6 loopback too.
/// The temporary IPv6 probe is closed immediately: SSH serves only IPv4,
/// and a dummy IPv6 listener would intercept browsers' localhost requests.
pub fn reserve_exact(port: u16) -> io::Result<PortReservation> {
    let held = PortReservation::acquire_port(port)?;
    held.check_forward_address()?;
    Ok(held)
}

fn reserve_free(
    mut acquire: impl FnMut() -> io::Result<PortReservation>,
    mut check: impl FnMut(&PortReservation) -> io::Result<()>,
) -> io::Result<PortReservation> {
    // Retry occupied candidates only; unknown and permission errors fail
    // closed, instead of spinning or announcing an unchecked address.
    for _ in 0..128 {
        let held = acquire()?;
        match check(&held) {
            Ok(()) => return Ok(held),
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AddrInUse,
        "no free localhost port found",
    ))
}

/// Verify that localhost cannot resolve to a competing IPv6 listener.
/// Repeat at handoff and after SSH verification, since SSH setup can await.
pub fn check_forward_address(port: u16) -> io::Result<()> {
    ipv6_result(TcpListener::bind((Ipv6Addr::LOCALHOST, port)).map(drop))
}

fn ipv6_result(result: io::Result<()>) -> io::Result<()> {
    match result {
        // AddrNotAvailable means this machine has no ::1 interface. The
        // platform EAFNOSUPPORT values cover a disabled IPv6 address family;
        // Rust does not map that errno consistently to ErrorKind::Unsupported.
        Err(error)
            if error.kind() == io::ErrorKind::AddrNotAvailable
                || error.kind() == io::ErrorKind::Unsupported
                || unsupported_family(&error) =>
        {
            Ok(())
        }
        other => other,
    }
}

fn unsupported_family(error: &io::Error) -> bool {
    #[cfg(target_os = "linux")]
    const FAMILY_UNSUPPORTED: i32 = 97; // EAFNOSUPPORT
    #[cfg(target_os = "macos")]
    const FAMILY_UNSUPPORTED: i32 = 47; // EAFNOSUPPORT
    #[cfg(windows)]
    const FAMILY_UNSUPPORTED: i32 = 10047; // WSAEAFNOSUPPORT
    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    return error.raw_os_error() == Some(FAMILY_UNSUPPORTED);
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = error;
        false
    }
}

// A fixed, portable shell builtin: no host, label, path, environment expansion
// or credential can become executable text. OpenSSH uses sh / Windows cmd.
const READY_MARKER: &str = "SPLITTY_FORWARD_READY_V1";
const READY_COMMAND: &str = "LocalCommand=echo SPLITTY_FORWARD_READY_V1";
const SETUP_TIMEOUT: Duration = Duration::from_secs(15);

async fn read_ready(stdout: impl tokio::io::AsyncRead + Unpin, marker: &str) -> io::Result<()> {
    let mut reader = BufReader::new(stdout.take(4096));
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            return Err(io::Error::other(
                "SSH ended without confirming its local forward",
            ));
        }
        if line.trim_end_matches(['\r', '\n']) == marker {
            return Ok(());
        }
    }
}

/// OpenSSH ssh_session2 runs ssh_init_forwarding before LocalCommand, and
/// ExitOnForwardFailure exits before that command when -L cannot bind. A live
/// authenticating child plus someone else's TCP listener is not readiness.
async fn wait_for_ready(child: &mut Child, port: u16, marker: &str) -> io::Result<()> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing SSH readiness pipe"))?;
    tokio::time::timeout(SETUP_TIMEOUT, async {
        read_ready(stdout, marker).await?;
        wait_for_forward(child, port).await
    })
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "SSH did not confirm its local forward in time",
        )
    })?
}

/// Build the owned forward's command independently of host configuration.
fn port_command(ssh: &Ssh, port: u16, remote: u16) -> tokio::process::Command {
    let mut command = ssh.command();
    command
        // A reused multiplex master may bypass LocalCommand entirely;
        // this tunnel must own its SSH process and never background it.
        .args([
            "-o",
            "ControlMaster=no",
            "-o",
            "ControlPath=none",
            "-o",
            "ControlPersist=no",
            "-o",
            "ForkAfterAuthentication=no",
        ])
        .args(["-o", "PermitLocalCommand=yes", "-o"])
        .arg(READY_COMMAND)
        .arg("-L")
        .arg(format!("127.0.0.1:{port}:localhost:{remote}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// Why a [`PortTunnel`] could not be opened.
#[derive(Debug, thiserror::Error)]
pub enum PortTunnelError {
    #[error("local port {local} is unavailable: {source}")]
    LocalPortUnavailable { local: u16, source: io::Error },
    /// The user's SSH configuration forwards ports for the host. `ssh` would
    /// open them too, and they cannot be cleared without clearing the
    /// forward itself.
    #[error(
        "the SSH configuration for {host} forwards ports itself ({}), which the forward's ssh \
         would open as well",
        .forwards.join("; ")
    )]
    ConfiguredForwards { host: String, forwards: Vec<String> },

    /// `ssh -G` or the forward's `ssh` failed, with its status and error
    /// output.
    #[error(transparent)]
    Ssh(SshError),

    /// `ssh` ran, but the local port did not answer for it.
    #[error("the forward of port {remote} on {host} was not established: {detail}")]
    NotEstablished {
        host: String,
        remote: u16,
        detail: String,
    },
}

/// One port of a host forwarded by an `ssh -N` of its own. The `ssh` is
/// killed when this is dropped.
pub struct PortTunnel {
    child: Child,
    stderr: Option<JoinHandle<String>>,
}

impl PortTunnel {
    /// Forward `127.0.0.1:<port>` here, the port `local` holds until `ssh`
    /// binds it, to `localhost:<remote>` on the host, over a connection that
    /// carries nothing else ([`Ssh::forward_only`]). Returns once the forward
    /// is verified: OpenSSH confirms its -L setup through a fixed local
    /// command, stays alive, and the local port answers without an IPv6 rival.
    ///
    /// A host whose configuration forwards ports itself is refused before
    /// `ssh` connects ([`Ssh::configured_forwards`]): every forward's `ssh`
    /// would open those too, binding ports here that a second forward could
    /// not take again, or giving the host a way back to this machine.
    pub async fn open(
        ssh: &Ssh,
        local: PortReservation,
        remote: u16,
    ) -> Result<Self, PortTunnelError> {
        let host = ssh.target().as_str();
        let configured = tokio::time::timeout(SETUP_TIMEOUT, ssh.configured_forwards())
            .await
            .map_err(|_| PortTunnelError::NotEstablished {
                host: host.to_owned(),
                remote,
                detail: "SSH configuration lookup timed out".into(),
            })?
            .map_err(PortTunnelError::Ssh)?;
        if !configured.is_empty() {
            return Err(PortTunnelError::ConfiguredForwards {
                host: host.to_owned(),
                forwards: configured,
            });
        }

        let port = local.port();
        let mut command = port_command(ssh, port, remote);
        command.arg(host);
        // ssh -G can await. Recheck at the handoff, with IPv4 still held.
        local
            .check_forward_address()
            .map_err(|source| PortTunnelError::LocalPortUnavailable {
                local: port,
                source,
            })?;
        // As late as possible (SR-13), see `PortReservation`.
        drop(local);
        let mut child = command
            .spawn()
            .map_err(|error| PortTunnelError::Ssh(SshError::Spawn(error)))?;
        let stderr = child
            .stderr
            .take()
            .map(|stderr| tokio::spawn(keep_end(stderr)));
        let mut tunnel = Self { child, stderr };
        let Err(error) = wait_for_ready(&mut tunnel.child, port, READY_MARKER).await else {
            if let Err(source) = check_forward_address(port) {
                tunnel.close().await;
                return Err(PortTunnelError::LocalPortUnavailable {
                    local: port,
                    source,
                });
            }
            return Ok(tunnel);
        };
        // An `ssh` that failed says why in its status and error output.
        let exited = tunnel.child.try_wait().ok().flatten();
        let _ = tunnel.child.kill().await;
        let stderr = kept_end(tunnel.stderr.take()).await;
        // If another process won the IPv4 handoff (or took IPv6 during login),
        // it stays untouched and the caller gets the same strict conflict.
        if let Err(source) = reserve_exact(port) {
            return Err(PortTunnelError::LocalPortUnavailable {
                local: port,
                source,
            });
        }
        Err(match exited {
            Some(status) if !status.success() => {
                PortTunnelError::Ssh(classify(host, status, &stderr))
            }
            _ => PortTunnelError::NotEstablished {
                host: host.to_owned(),
                remote,
                detail: error.to_string(),
            },
        })
    }

    /// Resolves when the forward has ended, with why: `ssh`'s status and the
    /// end of its error output. Cancel-safe until `ssh` has ended.
    pub async fn closed(&mut self) -> String {
        let status = match self.child.wait().await {
            Ok(status) => format!("ssh exited with {status}"),
            Err(error) => format!("ssh could not be watched: {error}"),
        };
        match kept_end(self.stderr.take()).await {
            stderr if stderr.is_empty() => status,
            stderr => format!("{status}: {stderr}"),
        }
    }

    /// End the forward: kill `ssh` and wait for it, so its local port is
    /// free.
    pub async fn close(&mut self) {
        // Fails only when `ssh` has already been waited for.
        let _ = self.child.kill().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ipv6_listener(port: u16) -> Option<TcpListener> {
        match TcpListener::bind((Ipv6Addr::LOCALHOST, port)) {
            Ok(listener) => Some(listener),
            Err(error) => {
                let reason = error.to_string();
                ipv6_result(Err(error)).expect("IPv6 probe failed for a reason other than absence");
                eprintln!("skipping unavailable IPv6 loopback: {reason}");
                None
            }
        }
    }

    #[tokio::test]
    async fn readiness_requires_the_exact_bounded_local_command_marker() {
        for ending in ["\n", "\r\n"] {
            let output = format!("{READY_MARKER}{ending}");
            read_ready(output.as_bytes(), READY_MARKER)
                .await
                .expect("ready");
        }
        for output in [
            "".to_owned(),
            format!("prefix {READY_MARKER}\n"),
            format!("{READY_MARKER} extra\n"),
            format!("{}\n{READY_MARKER}\n", "x".repeat(4096)),
        ] {
            assert!(read_ready(output.as_bytes(), READY_MARKER).await.is_err());
        }
        assert_eq!(READY_COMMAND, format!("LocalCommand=echo {READY_MARKER}"));
    }

    #[tokio::test]
    async fn owned_port_command_overrides_multiplex_background_and_local_commands() {
        let dir = tempfile::tempdir().expect("dir");
        let config = dir.path().join("config");
        std::fs::write(&config, "Host *\n ControlMaster auto\n ControlPath /tmp/unused-splitty-master\n ControlPersist yes\n ForkAfterAuthentication yes\n LocalCommand echo CONFIG_COMMAND_MUST_NOT_RUN\n PermitLocalCommand no\n GatewayPorts yes\n RequestTTY force\n ForwardAgent yes\n ForwardX11 yes\n ExitOnForwardFailure no\n").expect("config");
        let ssh = Ssh::forward_only(crate::ssh::SshTarget::parse("localhost").expect("target"));
        let mut command = port_command(&ssh, 15173, 5173);
        let output = command
            .arg("-G")
            .arg("-F")
            .arg(config)
            .arg("localhost")
            .output()
            .await
            .expect("OpenSSH config inspection");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let config = String::from_utf8(output.stdout).expect("UTF-8");
        for expected in [
            "controlmaster false",
            "controlpersist no",
            "forkafterauthentication no",
            "permitlocalcommand yes",
            "requesttty false",
            "forwardagent no",
            "forwardx11 no",
            "exitonforwardfailure yes",
            "localcommand echo SPLITTY_FORWARD_READY_V1",
            "localforward [127.0.0.1]:15173 [localhost]:5173",
        ] {
            assert!(
                config.lines().any(|line| line == expected),
                "missing {expected}: {config}"
            );
        }
        assert!(
            !config
                .lines()
                .any(|line| line.starts_with("controlpath ") && line != "controlpath none"),
            "{config}"
        );
        assert!(!config.contains("CONFIG_COMMAND_MUST_NOT_RUN"));
    }

    fn forward(local: u16, remote: u16) -> PortForward {
        PortForward { local, remote }
    }

    #[test]
    fn a_port_forwards_to_the_same_port_and_a_pair_picks_the_local_one() {
        assert_eq!(parse_forwards(&["3000"]), Ok(vec![forward(3000, 3000)]));
        assert_eq!(
            parse_forwards(&["8080:3000"]),
            Ok(vec![forward(8080, 3000)])
        );
        assert_eq!(
            parse_forwards(&["1", "65535"]),
            Ok(vec![forward(1, 1), forward(65535, 65535)])
        );
        assert_eq!(parse_forwards::<&str>(&[]), Ok(vec![]));
    }

    /// Comma lists and repeated flags add up, in order; spaces around an entry
    /// are allowed.
    #[test]
    fn lists_and_repeated_flags_add_up_in_order() {
        assert_eq!(
            parse_forwards(&["3000,5173", "8080:80", " 9000 , 4000:4001 "]),
            Ok(vec![
                forward(3000, 3000),
                forward(5173, 5173),
                forward(8080, 80),
                forward(9000, 9000),
                forward(4000, 4001),
            ])
        );
    }

    #[test]
    fn an_entry_that_does_not_parse_is_rejected() {
        for entry in [
            "",
            "0",
            "70000",
            "65536",
            "-1",
            "+3000",
            "3000x",
            "x",
            "30 00",
            ":3000",
            "3000:",
            "0:3000",
            "3000:0",
            "1:2:3",
            "localhost:3000",
            "3000-3005",
        ] {
            assert_eq!(
                parse_forwards(&[entry]),
                Err(ForwardError::Invalid {
                    entry: entry.trim().to_owned()
                }),
                "{entry:?}"
            );
        }
        // An empty entry in a list, as a trailing comma leaves one.
        assert_eq!(
            parse_forwards(&["3000,"]),
            Err(ForwardError::Invalid {
                entry: String::new()
            })
        );
        assert_eq!(
            ForwardError::Invalid {
                entry: "70000".into()
            }
            .to_string(),
            "invalid --forward entry '70000': expected PORT or LOCAL:REMOTE, with ports from 1 \
             to 65535"
        );
    }

    #[test]
    fn a_port_named_twice_is_rejected() {
        assert_eq!(
            parse_forwards(&["3000,3000"]),
            Err(ForwardError::RemoteTwice { port: 3000 })
        );
        assert_eq!(
            parse_forwards(&["3000", "8080:3000"]),
            Err(ForwardError::RemoteTwice { port: 3000 })
        );
        assert_eq!(
            parse_forwards(&["8080:3000,8080:5173"]),
            Err(ForwardError::LocalTwice { port: 8080 })
        );
        // `3000` takes local port 3000 as well.
        assert_eq!(
            parse_forwards(&["3000", "3000:5173"]),
            Err(ForwardError::LocalTwice { port: 3000 })
        );
        assert_eq!(
            ForwardError::RemoteTwice { port: 3000 }.to_string(),
            "--forward names port 3000 of the host twice"
        );
    }

    /// Never `127.0.0.1`, whose cookies belong to Splitty servers.
    #[test]
    fn a_forward_is_announced_as_localhost() {
        let forward = Forward {
            remote: 3000,
            local: 41234,
            asked: 3000,
        };
        assert_eq!(forward.url(), "http://localhost:41234");
    }

    /// A port free right now.
    fn free_port() -> u16 {
        PortReservation::acquire().expect("free port").port()
    }

    #[test]
    fn a_free_port_is_taken_as_asked_and_held() {
        let port = free_port();
        let (opened, reservation) = reserve(forward(port, 3000)).expect("reserve");
        assert_eq!(
            opened,
            Forward {
                remote: 3000,
                local: port,
                asked: port
            }
        );
        assert_eq!(reservation.port(), port);
        assert!(
            TcpListener::bind(("127.0.0.1", port)).is_err(),
            "the reservation must hold the port"
        );
    }

    #[test]
    fn a_busy_port_is_replaced_by_any_free_one() {
        let busy = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = busy.local_addr().expect("address").port();
        let (opened, reservation) = reserve(forward(port, 3000)).expect("reserve");
        assert_ne!(opened.local, port);
        assert_eq!(opened.asked, port);
        assert_eq!(opened.remote, 3000);
        assert_eq!(reservation.port(), opened.local);
    }

    /// A browser would reach the local server on `[::1]` for `localhost`.
    #[test]
    fn a_port_listened_on_ipv6_loopback_is_busy_too() {
        let Some(busy) = ipv6_listener(0) else { return };
        let port = busy.local_addr().expect("address").port();
        let (opened, _reservation) = reserve(forward(port, 3000)).expect("reserve");
        assert_ne!(opened.local, port, "[::1]:{port} is taken");
    }
    #[test]
    fn exact_ports_refuse_both_loopback_families() {
        let busy = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = busy.local_addr().expect("addr").port();
        assert!(reserve_exact(port).is_err());
        let Some(busy6) = ipv6_listener(0) else {
            return;
        };
        let port = busy6.local_addr().expect("addr").port();
        assert!(reserve_exact(port).is_err());
        drop(busy6);
        let held = reserve_exact(port).expect("exact after release");
        assert_eq!(held.port(), port);
        let rival =
            TcpListener::bind((Ipv6Addr::LOCALHOST, port)).expect("no dummy IPv6 listener remains");
        assert_eq!(
            held.check_forward_address()
                .expect_err("recheck before handoff")
                .kind(),
            io::ErrorKind::AddrInUse
        );
        drop(rival);
        held.check_forward_address()
            .expect("recheck after rival leaves");
    }

    #[test]
    fn ipv6_absence_is_distinct_from_permission_occupancy_and_uncertainty() {
        for kind in [io::ErrorKind::AddrNotAvailable, io::ErrorKind::Unsupported] {
            assert!(ipv6_result(Err(io::Error::from(kind))).is_ok());
        }
        for kind in [
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::AddrInUse,
            io::ErrorKind::Other,
        ] {
            assert_eq!(
                ipv6_result(Err(io::Error::from(kind)))
                    .expect_err("fail closed")
                    .kind(),
                kind
            );
        }
        #[cfg(target_os = "linux")]
        assert!(ipv6_result(Err(io::Error::from_raw_os_error(97))).is_ok());
        #[cfg(target_os = "macos")]
        assert!(ipv6_result(Err(io::Error::from_raw_os_error(47))).is_ok());
        #[cfg(windows)]
        assert!(ipv6_result(Err(io::Error::from_raw_os_error(10047))).is_ok());
    }

    #[test]
    fn every_fallback_candidate_is_checked_and_unknown_failure_stops_selection() {
        let mut checked = Vec::new();
        let held = reserve_free(PortReservation::acquire, |held| {
            checked.push(held.port());
            if checked.len() == 1 {
                Err(io::ErrorKind::AddrInUse.into())
            } else {
                Ok(())
            }
        })
        .expect("second candidate");
        assert_eq!(checked.len(), 2);
        assert_eq!(Some(&held.port()), checked.last());
        let mut calls = 0;
        let result = reserve_free(PortReservation::acquire, |_| {
            calls += 1;
            Err(io::ErrorKind::PermissionDenied.into())
        });
        assert_eq!(
            result.err().expect("failure").kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(calls, 1);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn a_delayed_ssh_and_competing_ipv4_listener_are_not_forward_readiness() {
        use tokio::io::AsyncWriteExt;
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("competing listener");
        let port = listener.local_addr().expect("address").port();
        let mut child = tokio::process::Command::new("sh")
            .args(["-c", "read line; exit 255"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("delayed SSH fixture");
        let mut release = child.stdin.take().expect("gate");
        let ready = wait_for_ready(&mut child, port, "not-emitted-before-binding");
        tokio::pin!(ready);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut ready)
                .await
                .is_err(),
            "alive SSH and rival listener cannot report up"
        );
        release
            .write_all(b"finish authentication and fail binding\n")
            .await
            .expect("release");
        assert!(ready.await.is_err(), "no positive bind marker");
        assert!(
            TcpListener::bind(("127.0.0.1", port)).is_err(),
            "rival listener left alone"
        );
    }
}
