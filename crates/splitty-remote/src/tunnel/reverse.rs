//! A port of this machine shared with a host (a reverse forward), each by an
//! `ssh` of its own ([`ReverseTunnel`], the remote screen hub's reverse
//! forwards).
//!
//! The `ssh` makes the host's `sshd` listen on an owner-only Unix socket in
//! the relay directory of the host's server and carries each connection to it
//! to `localhost:<local>` here. The same `ssh` runs `splitty remote-relay
//! serve` there, which listens on the host's loopback for the user's own
//! programs only and relays them to the socket. `sshd`'s `GatewayPorts` does
//! not apply to a Unix socket, so nothing listens beyond loopback, and only
//! `localhost` of this machine is ever reached.
//!
//! The relay lives as long as its standard input: this side writes a
//! heartbeat to it every [`HEARTBEAT`], and the relay ends after 45 seconds
//! without one, so a connection lost without a word does not hold the host's
//! port for long.

use std::io;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Child;
use tokio::task::JoinHandle;

use crate::install::Server;
use crate::ssh::{classify, keep_end, kept_end, CommandLine, Ssh, SshError};

/// The line the relay prints once it listens.
pub const RELAY_READY: &str = "SPLITTY_RELAY_READY_V1";

/// The start of the line the relay prints when it cannot listen.
pub const RELAY_FAILED: &str = "SPLITTY_RELAY_FAILED_V1";

/// How often the relay hears from this side.
pub const HEARTBEAT: Duration = Duration::from_secs(10);

/// How long `ssh` and the relay get to report.
const SETUP_TIMEOUT: Duration = Duration::from_secs(20);

/// The most of the relay's output read for its report.
const REPORT_LIMIT: u64 = 4096;

/// The longest socket path this side accepts: a Unix socket address holds
/// about 108 bytes, less on some systems.
pub const MAX_SOCKET_PATH: usize = 100;

/// Why a [`ReverseTunnel`] could not be opened.
#[derive(Debug, thiserror::Error)]
pub enum ReverseTunnelError {
    /// The user's SSH configuration forwards ports for the host; `ssh`
    /// would open them too.
    #[error(
        "the SSH configuration for {host} forwards ports itself ({}), which the reverse \
         forward's ssh would open as well",
        .forwards.join("; ")
    )]
    ConfiguredForwards { host: String, forwards: Vec<String> },

    /// The host's port is in use there.
    #[error("{detail}")]
    HostPortConflict { port: u16, detail: String },

    /// The host's relay could not serve the port, with its code.
    #[error("the relay on {host} failed ({code}): {detail}")]
    Relay {
        host: String,
        code: String,
        detail: String,
    },

    /// `ssh -G` or the reverse forward's `ssh` failed.
    #[error(transparent)]
    Ssh(SshError),

    /// `ssh` ran, but the relay did not report.
    #[error("sharing local port {local} as port {port} of {host} failed: {detail}")]
    NotEstablished {
        host: String,
        port: u16,
        local: u16,
        detail: String,
    },
}

/// What the relay on the host reported.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Report {
    Ready,
    Failed { code: String, detail: String },
}

/// Read the relay's report: the first [`RELAY_READY`] or [`RELAY_FAILED`]
/// line within [`REPORT_LIMIT`] bytes. A login shell may print other lines
/// first.
async fn read_report(stdout: impl tokio::io::AsyncRead + Unpin) -> io::Result<Report> {
    let mut reader = BufReader::new(stdout.take(REPORT_LIMIT));
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            return Err(io::Error::other("ssh ended before the relay reported"));
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line == RELAY_READY {
            return Ok(Report::Ready);
        }
        if let Some(rest) = line
            .strip_prefix(RELAY_FAILED)
            .and_then(|rest| rest.strip_prefix(' '))
        {
            let (code, detail) = rest.split_once(' ').unwrap_or((rest, ""));
            return Ok(Report::Failed {
                code: code.to_owned(),
                detail: detail.to_owned(),
            });
        }
    }
}

/// The relay directory `remote-relay prepare` printed: its one JSON line,
/// checked to make a socket path that `ssh -R` and a Unix socket take.
pub fn parse_prepared(stdout: &[u8]) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    struct Prepared {
        version: u32,
        dir: String,
    }
    let text = std::str::from_utf8(stdout).map_err(|_| "the relay's answer is not UTF-8")?;
    let line = text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .ok_or("the relay printed nothing")?;
    let prepared: Prepared =
        serde_json::from_str(line).map_err(|e| format!("the relay's answer is invalid: {e}"))?;
    if prepared.version != 1 {
        return Err(format!(
            "the relay's answer has version {}, not 1",
            prepared.version
        ));
    }
    let dir = prepared.dir;
    // `ssh -R` splits its argument at colons; a socket address is short.
    if !dir.starts_with('/')
        || dir.contains(':')
        || dir.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(format!(
            "the relay directory {dir:?} cannot hold the socket"
        ));
    }
    if socket_path(&dir, "0000000000000000.sock").len() > MAX_SOCKET_PATH {
        return Err(format!(
            "the relay directory {dir} is too long for a Unix socket path"
        ));
    }
    Ok(dir)
}

/// The socket `name` in the relay directory `dir`.
pub fn socket_path(dir: &str, name: &str) -> String {
    format!("{}/{name}", dir.trim_end_matches('/'))
}

/// The command that prepares the host for reverse forwards, by `server`.
pub fn prepare_command(server: &Server) -> CommandLine {
    server.command().word("remote-relay").word("prepare")
}

/// The reverse forward's `ssh` up to its target, independent of the host's
/// configuration: its own process and the one remote forward.
fn relay_command(ssh: &Ssh, socket: &str, local: u16) -> tokio::process::Command {
    let mut command = ssh.command();
    command
        // As for `PortTunnel`: a reused multiplex master could carry the
        // forward past this process's life; it must own its connection.
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
        .arg("-R")
        .arg(format!("{socket}:localhost:{local}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// One local port shared with a host by an `ssh -R` of its own and the
/// host's relay. The `ssh` is killed when this is dropped.
pub struct ReverseTunnel {
    child: Child,
    stderr: Option<JoinHandle<String>>,
    tasks: Vec<JoinHandle<()>>,
}

impl ReverseTunnel {
    /// Share `localhost:<local>` here as `localhost:<port>` on the host,
    /// through the socket `name` in the host's relay directory `dir` and
    /// `server`'s relay. Returns once the relay reported that it listens.
    ///
    /// A host whose configuration forwards ports itself is refused before
    /// `ssh` connects, as for a [`super::PortTunnel`].
    pub async fn open(
        ssh: &Ssh,
        server: &Server,
        dir: &str,
        name: &str,
        local: u16,
        port: u16,
    ) -> Result<Self, ReverseTunnelError> {
        let host = ssh.target().as_str();
        let not_established = |detail: String| ReverseTunnelError::NotEstablished {
            host: host.to_owned(),
            port,
            local,
            detail,
        };
        let configured = tokio::time::timeout(SETUP_TIMEOUT, ssh.configured_forwards())
            .await
            .map_err(|_| not_established("SSH configuration lookup timed out".into()))?
            .map_err(ReverseTunnelError::Ssh)?;
        if !configured.is_empty() {
            return Err(ReverseTunnelError::ConfiguredForwards {
                host: host.to_owned(),
                forwards: configured,
            });
        }

        let socket = socket_path(dir, name);
        let relay = server
            .command()
            .word("remote-relay")
            .word("serve")
            .word("--port")
            .value(&port.to_string())
            .word("--socket")
            .value(&socket);
        let mut child = relay_command(ssh, &socket, local)
            .arg(host)
            .arg("--")
            .arg(relay.as_str())
            .spawn()
            .map_err(|error| ReverseTunnelError::Ssh(SshError::Spawn(error)))?;
        let stderr = child
            .stderr
            .take()
            .map(|stderr| tokio::spawn(keep_end(stderr)));
        let stdout = child.stdout.take();
        let stdin = child.stdin.take();
        let mut tunnel = Self {
            child,
            stderr,
            tasks: Vec::new(),
        };
        let Some(stdout) = stdout else {
            tunnel.close().await;
            return Err(not_established("missing the relay's output".into()));
        };
        let mut reader = BufReader::new(stdout);
        let report = tokio::time::timeout(SETUP_TIMEOUT, read_report(&mut reader)).await;
        match report {
            Ok(Ok(Report::Ready)) if tunnel.child.try_wait().ok().flatten().is_none() => {}
            Ok(Ok(Report::Failed { code, detail })) => {
                tunnel.close().await;
                return Err(if code == "host_port_conflict" {
                    ReverseTunnelError::HostPortConflict { port, detail }
                } else {
                    ReverseTunnelError::Relay {
                        host: host.to_owned(),
                        code,
                        detail,
                    }
                });
            }
            other => {
                let exited = tunnel.child.try_wait().ok().flatten();
                tunnel.close().await;
                let stderr = kept_end(tunnel.stderr.take()).await;
                return Err(match exited {
                    Some(status) if !status.success() => {
                        ReverseTunnelError::Ssh(classify(host, status, &stderr))
                    }
                    _ => not_established(match other {
                        Err(_) => "the relay did not report in time".into(),
                        Ok(Err(error)) => error.to_string(),
                        Ok(Ok(_)) => "ssh ended right after the relay reported".into(),
                    }),
                });
            }
        }
        // Nothing more is expected on standard output; keep it drained so
        // the pipe never fills.
        tunnel.tasks.push(tokio::spawn(async move {
            let _ = tokio::io::copy(&mut reader, &mut tokio::io::sink()).await;
        }));
        if let Some(mut stdin) = stdin {
            tunnel.tasks.push(tokio::spawn(async move {
                loop {
                    tokio::time::sleep(HEARTBEAT).await;
                    if stdin.write_all(b"\n").await.is_err() || stdin.flush().await.is_err() {
                        return;
                    }
                }
            }));
        }
        Ok(tunnel)
    }

    /// Resolves when the reverse forward has ended, with why: `ssh`'s status
    /// and the end of its error output. Cancel-safe until `ssh` has ended.
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

    /// End the reverse forward: kill `ssh` and wait for it. The relay on the
    /// host ends with its input.
    pub async fn close(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
        // Fails only when `ssh` has already been waited for.
        let _ = self.child.kill().await;
    }
}

impl Drop for ReverseTunnel {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_report_is_the_first_ready_or_failed_line() {
        for output in [
            format!("{RELAY_READY}\n"),
            format!("welcome to the host\r\n{RELAY_READY}\r\n"),
        ] {
            assert_eq!(
                read_report(output.as_bytes()).await.expect("report"),
                Report::Ready
            );
        }
        let failed = format!("{RELAY_FAILED} host_port_conflict port 8080 is already in use\n");
        assert_eq!(
            read_report(failed.as_bytes()).await.expect("report"),
            Report::Failed {
                code: "host_port_conflict".into(),
                detail: "port 8080 is already in use".into(),
            }
        );
        for output in [
            String::new(),
            format!("prefix {RELAY_READY}\n"),
            format!("{RELAY_READY} extra\n"),
            format!("{RELAY_FAILED}x code\n"),
            format!("{}\n{RELAY_READY}\n", "x".repeat(4096)),
        ] {
            assert!(read_report(output.as_bytes()).await.is_err(), "{output:?}");
        }
    }

    #[test]
    fn the_prepared_directory_must_make_a_short_plain_socket_path() {
        assert_eq!(
            parse_prepared(
                b"{\"version\":1,\"dir\":\"/home/me/.local/share/splitty/run/relay\"}\n"
            ),
            Ok("/home/me/.local/share/splitty/run/relay".to_owned())
        );
        assert_eq!(
            parse_prepared(b"motd\n{\"version\":1,\"dir\":\"/r\"}\n"),
            Ok("/r".to_owned())
        );
        let long = format!("{{\"version\":1,\"dir\":\"/{}\"}}", "d".repeat(90));
        for answer in [
            "",
            "{}",
            "{\"version\":2,\"dir\":\"/r\"}",
            "{\"version\":1,\"dir\":\"relative\"}",
            "{\"version\":1,\"dir\":\"/with:colon\"}",
            "{\"version\":1,\"dir\":\"/with space\"}",
            "{\"version\":1,\"dir\":\"/with\\nline\"}",
            long.as_str(),
        ] {
            assert!(parse_prepared(answer.as_bytes()).is_err(), "{answer}");
        }
    }

    #[test]
    fn the_relay_commands_name_the_subcommand_and_quote_values() {
        let server = Server::Installed {
            path: "/home/me/.local/share/splitty/remote/1.0.0/splitty".into(),
            config_dir: None,
        };
        assert_eq!(
            prepare_command(&server).as_str(),
            "'/home/me/.local/share/splitty/remote/1.0.0/splitty' remote-relay prepare"
        );
        assert_eq!(socket_path("/r/", "a.sock"), "/r/a.sock");
    }

    #[tokio::test]
    async fn the_reverse_command_owns_its_connection_and_carries_one_remote_forward() {
        let dir = tempfile::tempdir().expect("dir");
        let config = dir.path().join("config");
        std::fs::write(&config, "Host *\n ControlMaster auto\n ControlPath /tmp/unused-splitty-master\n ControlPersist yes\n ForkAfterAuthentication yes\n GatewayPorts yes\n RequestTTY force\n ForwardAgent yes\n ForwardX11 yes\n ExitOnForwardFailure no\n").expect("config");
        let ssh = Ssh::relay_only(crate::ssh::SshTarget::parse("localhost").expect("target"));
        let mut command = relay_command(&ssh, "/r/0123456789abcdef.sock", 11434);
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
            "requesttty false",
            "forwardagent no",
            "forwardx11 no",
            "exitonforwardfailure yes",
            "batchmode yes",
            "remoteforward /r/0123456789abcdef.sock [localhost]:11434",
        ] {
            assert!(
                config.lines().any(|line| line == expected),
                "missing {expected:?} in\n{config}"
            );
        }
        assert_eq!(
            config
                .lines()
                .filter(
                    |line| line.starts_with("remoteforward ") || line.starts_with("localforward ")
                )
                .count(),
            1
        );
    }
}
