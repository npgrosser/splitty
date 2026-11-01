//! `splitty connect` and the remote screen hub against a real OpenSSH server.
//!
//! Why a smoke test: only OpenSSH shows what the forward arguments,
//! `GatewayPorts`, `ExitOnForwardFailure` and a killed connection really do,
//! and only the binary shows what `connect` prints. The supervisor's rules
//! (same daemon, restarted daemon, backoff, giving up) are covered in-process
//! in `splitty_remote::tunnel::supervisor`. Likewise only a real SSH
//! connection and a real host daemon show the hub's relay end to end, and
//! only real daemons that `update=1` stops and starts on the host; its
//! messages, error codes, relay rules and when it updates are covered
//! in-process with a scripted host (`splitty_server::http`'s tests). The
//! hub's port forwards are covered in-process the same way; only real `ssh`
//! shows that their arguments forward on `127.0.0.1` alone, that the
//! configuration's forwards are not cleared at their expense, and that a
//! killed one comes back. Reverse forwards likewise: only real `ssh -R` and
//! `sshd` show that the remote end is an owner-only Unix socket, that the
//! host's `GatewayPorts yes` opens nothing beyond loopback, and that the
//! host's relay reaches a local service and ends with its connection.
//!
//! The SSH lab gives each test a host of its own without root:
//! - an unprivileged user and network namespace, so the host has its own
//!   loopback and its daemon's port is free on this side;
//! - `sshd -i`, one per connection, started inside that namespace by the
//!   client's `ProxyCommand` through `nsenter`, with throwaway keys;
//! - a `ForceCommand` that gives the host its own `HOME` and XDG directories and
//!   runs the command through `sh -c`, as a login shell would;
//! - a wrapper `ssh` first in `PATH` that adds `-F <lab config>`, so `connect`
//!   runs the plain `ssh` it always runs. The lab config sets `GatewayPorts yes`
//!   (and so does the host's `sshd_config`), and names two hosts that fail: one
//!   nothing listens for, and one whose host key is unknown;
//! - a configuration directory of the client's own, so `connect` records its
//!   recent hosts there and never in the user's;
//! - servers the test starts on the host's loopback with `nsenter`, which only
//!   a forward reaches from this side.
//!
//! It needs `sshd` (`/usr/sbin/sshd`, or an unpacked `openssh-server` package
//! under `SPLITTY_TEST_SSHD_ROOT`), `ssh`, `ssh-keygen`, `unshare`, `nsenter`
//! and `ip`, and unprivileged user namespaces. Without them the tests are
//! skipped at runtime with a message, never `#[ignore]`d.

#![cfg(target_os = "linux")]

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv6Addr, TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use nix::sys::signal::{kill, killpg, Signal};
use nix::unistd::Pid;
use tempfile::TempDir;

const HOST: &str = "splittyhost";
/// A host nothing listens for.
const NOWHERE: &str = "splittynowhere";
/// The lab's host under a name whose host key is unknown.
const NEW_KEY: &str = "splittynewkey";
const WAIT: Duration = Duration::from_secs(30);

/// Find an executable in `PATH`.
fn which(name: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}

fn write_executable(path: &Path, text: &str) {
    fs::write(path, text).expect("write script");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod");
}

fn describe(out: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A remote host for one test.
struct Lab {
    /// Short, in `/tmp`: the host's control socket path must fit `sun_path`.
    dir: TempDir,
    /// Holds the host's namespace; its pid is the `nsenter` target.
    holder: Child,
}

impl Lab {
    /// The lab, or why it cannot run here.
    fn start() -> Result<Self, String> {
        let root =
            PathBuf::from(env::var_os("SPLITTY_TEST_SSHD_ROOT").unwrap_or_else(|| "/".into()));
        let sshd = root.join("usr/sbin/sshd");
        if !sshd.is_file() {
            return Err(format!(
                "no sshd at {} (install openssh-server, or unpack it and set SPLITTY_TEST_SSHD_ROOT)",
                sshd.display()
            ));
        }
        for tool in ["ssh", "ssh-keygen", "unshare", "nsenter", "ip"] {
            if which(tool).is_none() {
                return Err(format!("{tool} is not installed"));
            }
        }

        let dir = tempfile::Builder::new()
            .prefix("sl")
            .tempdir_in("/tmp")
            .expect("lab directory");
        let d = dir.path();
        for sub in ["home", "run", "host-bin", "client-bin", "client-config"] {
            fs::create_dir(d.join(sub)).expect("lab subdirectory");
        }
        fs::set_permissions(d.join("run"), fs::Permissions::from_mode(0o700)).expect("chmod run");
        for key in ["hostkey", "clientkey"] {
            let keygen = Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(d.join(key))
                .output()
                .expect("ssh-keygen");
            assert!(keygen.status.success(), "ssh-keygen\n{}", describe(&keygen));
        }
        fs::copy(d.join("clientkey.pub"), d.join("authorized_keys")).expect("authorized_keys");

        let d_str = d.display();
        write_executable(
            &d.join("host-env.sh"),
            &format!(
                "#!/bin/sh\n\
                 export HOME={d_str}/home XDG_CONFIG_HOME={d_str}/home/.config \
                 XDG_DATA_HOME={d_str}/home/.local/share XDG_RUNTIME_DIR={d_str}/run\n\
                 export PATH={d_str}/host-bin:/usr/bin:/bin\n\
                 cd \"$HOME\" || exit 1\n\
                 if [ -n \"$SSH_ORIGINAL_COMMAND\" ]; then exec /bin/sh -c \"$SSH_ORIGINAL_COMMAND\"; fi\n\
                 exec /bin/sh -l\n"
            ),
        );

        let mut sshd_config = format!(
            "HostKey {d_str}/hostkey\nAuthorizedKeysFile {d_str}/authorized_keys\nUsePAM no\n\
             StrictModes no\nPidFile none\nPasswordAuthentication no\n\
             KbdInteractiveAuthentication no\nPubkeyAuthentication yes\nAllowTcpForwarding yes\n\
             GatewayPorts yes\nForceCommand {d_str}/host-env.sh\nLogLevel ERROR\n"
        );
        let mut libraries = Vec::new();
        if root != Path::new("/") {
            // An unpacked package: its helpers and libraries are not where
            // sshd was built to look for them.
            for (option, helper) in [
                ("SshdSessionPath", "usr/lib/openssh/sshd-session"),
                ("SshdAuthPath", "usr/lib/openssh/sshd-auth"),
            ] {
                if root.join(helper).is_file() {
                    sshd_config.push_str(&format!("{option} {}\n", root.join(helper).display()));
                }
            }
            for lib in ["usr/lib/x86_64-linux-gnu", "usr/lib/aarch64-linux-gnu"] {
                if root.join(lib).is_dir() {
                    libraries.push(root.join(lib).display().to_string());
                }
            }
        }
        fs::write(d.join("sshd_config"), sshd_config).expect("sshd_config");

        let holder_pid_file = d.join("ns.pid");
        write_executable(
            &d.join("proxy.sh"),
            &format!(
                "#!/bin/sh\n\
                 if [ \"$SPLITTY_TEST_FORWARD\" = 1 ] && [ -f {d_str}/gate/armed ]; then\n\
                   echo $$ > {d_str}/gate/entered\n\
                   remaining=200\n\
                   while [ ! -f {d_str}/gate/release ]; do\n\
                     [ \"$remaining\" -gt 0 ] || exit 124\n\
                     remaining=$((remaining - 1))\n\
                     sleep 0.05\n\
                   done\n\
                   : > {d_str}/gate/passed\n\
                 fi\n\
                 exec env LD_LIBRARY_PATH={libs} nsenter --target \"$(cat {pid})\" \
                 --user --net --preserve-credentials {sshd} -i -f {d_str}/sshd_config\n",
                libs = libraries.join(":"),
                pid = holder_pid_file.display(),
                sshd = sshd.display(),
            ),
        );
        let host_key = fs::read_to_string(d.join("hostkey.pub")).expect("host key");
        let host_key: Vec<&str> = host_key.split_whitespace().take(2).collect();
        fs::write(
            d.join("known_hosts"),
            format!("{HOST} {}\n", host_key.join(" ")),
        )
        .expect("known_hosts");
        let user =
            String::from_utf8_lossy(&Command::new("id").arg("-un").output().expect("id").stdout)
                .trim()
                .to_owned();
        fs::write(
            d.join("ssh_config"),
            format!(
                "Host {HOST}\n  User {user}\n  IdentityFile {d_str}/clientkey\n  IdentitiesOnly yes\n  \
                 UserKnownHostsFile {d_str}/known_hosts\n  StrictHostKeyChecking yes\n  \
                 GatewayPorts yes\n  ProxyCommand {d_str}/proxy.sh\n\
                 Host {NOWHERE}\n  HostName 127.0.0.1\n  Port 1\n  ConnectTimeout 5\n\
                 Host {NEW_KEY}\n  User {user}\n  IdentityFile {d_str}/clientkey\n  IdentitiesOnly yes\n  \
                 UserKnownHostsFile {d_str}/no_known_hosts\n  StrictHostKeyChecking yes\n  \
                 ProxyCommand {d_str}/proxy.sh\n"
            ),
        )
        .expect("ssh_config");
        write_executable(
            &d.join("client-bin/ssh"),
            &format!(
                "#!/bin/sh\n\
                 export LC_ALL=C\n\
                 role=connection\n\
                 unset SPLITTY_TEST_FORWARD\n\
                 for arg in \"$@\"; do\n\
                   case \"$arg\" in\n\
                     -G) role=config ;;\n\
                     -L) role=forward; export SPLITTY_TEST_FORWARD=1 ;;\n\
                   esac\n\
                 done\n\
                 echo \"$role\" >> {d_str}/ssh-invocations\n\
                 if [ \"$role\" = forward ] && [ -f {d_str}/gate/armed ]; then\n\
                   set -- -vv -E {d_str}/gate/ssh.log \"$@\"\n\
                 fi\n\
                 exec {} -F {d_str}/ssh_config \"$@\"\n",
                which("ssh").expect("ssh").display()
            ),
        );
        // `connect --no-open` must not open anything; these would record it.
        for opener in [
            "xdg-open",
            "gio",
            "gnome-open",
            "kde-open",
            "wslview",
            "open",
        ] {
            write_executable(
                &d.join("client-bin").join(opener),
                &format!("#!/bin/sh\necho \"$@\" >> {d_str}/opened\n"),
            );
        }

        let mut holder = Command::new("unshare")
            .args([
                "--user",
                "--map-current-user",
                "--keep-caps",
                "--net",
                "sh",
                "-c",
            ])
            .arg("ip link set lo up && echo ready && exec sleep infinity")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("unshare: {e}"))?;
        let mut line = String::new();
        let ready = BufReader::new(holder.stdout.take().expect("stdout"))
            .read_line(&mut line)
            .is_ok()
            && line.trim() == "ready";
        if !ready {
            let mut stderr = String::new();
            let _ = holder
                .stderr
                .take()
                .expect("stderr")
                .read_to_string(&mut stderr);
            let _ = holder.kill();
            let _ = holder.wait();
            return Err(format!(
                "unprivileged namespaces are not available: {}",
                stderr.trim()
            ));
        }
        fs::write(&holder_pid_file, holder.id().to_string()).expect("ns.pid");

        let lab = Self { dir, holder };
        let probe = lab.on_host(&["true"]);
        assert!(
            probe.status.success(),
            "the SSH lab cannot log in\n{}",
            describe(&probe)
        );
        Ok(lab)
    }

    fn path(&self) -> OsString {
        let mut dirs = vec![self.dir.path().join("client-bin")];
        dirs.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
        env::join_paths(dirs).expect("PATH")
    }

    /// Run a command on the host.
    fn on_host(&self, args: &[&str]) -> Output {
        Command::new("ssh")
            .env("PATH", self.path())
            .args(["-o", "BatchMode=yes", HOST, "--"])
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("ssh")
    }

    /// Put the `splitty` under test into the host's `PATH`.
    fn install_splitty(&self) {
        std::os::unix::fs::symlink(
            env!("CARGO_BIN_EXE_splitty"),
            self.dir.path().join("host-bin/splitty"),
        )
        .expect("symlink splitty");
    }

    /// Start the host's daemon on `port` (0: any) and return its port.
    fn start_daemon(&self, port: u16) -> u16 {
        let addr = format!("127.0.0.1:{port}");
        let start = self.on_host(&["splitty", "daemon", "start", "--addr", &addr]);
        assert!(
            start.status.success(),
            "daemon start on the host\n{}",
            describe(&start)
        );
        String::from_utf8_lossy(&start.stdout)
            .lines()
            .find_map(|line| line.strip_prefix("SPLITTY_PORT="))
            .and_then(|port| port.trim().parse().ok())
            .unwrap_or_else(|| panic!("no SPLITTY_PORT\n{}", describe(&start)))
    }

    /// A server on the host's loopback at `port`: `splitty serve`, whose
    /// `/health` answers `ok`. Only a forward reaches it from this side.
    fn serve_on_host(&self, port: u16) -> HostServer {
        let directory = tempfile::Builder::new()
            .prefix("serve-")
            .tempdir_in(self.dir.path())
            .expect("host service directory");
        let home = directory.path();
        fs::create_dir_all(home.join("run")).expect("server home");
        fs::set_permissions(home.join("run"), fs::Permissions::from_mode(0o700))
            .expect("chmod run");
        let mut child = Command::new("nsenter")
            .arg("--target")
            .arg(self.holder.id().to_string())
            .args(["--user", "--net", "--preserve-credentials"])
            .arg(env!("CARGO_BIN_EXE_splitty"))
            .args(["serve", "--oneshot-token", "--addr"])
            .arg(format!("127.0.0.1:{port}"))
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_RUNTIME_DIR", home.join("run"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("nsenter");
        let lines = read_lines(child.stdout.take().expect("stdout"));
        let mut server = HostServer {
            child,
            port,
            _directory: directory,
        };
        let deadline = Instant::now() + WAIT;
        loop {
            match lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(line) => {
                    if let Some(port) = line.strip_prefix("SPLITTY_PORT=") {
                        server.port = port.trim().parse().expect("host service port");
                        return server;
                    }
                }
                Err(_) => panic!("the host's server on port {port} did not start"),
            }
        }
    }

    /// A dev service outside pane discovery, in the existing host namespace.
    /// Python stdlib only: HTTP, application auth, and a bounded RFC6455 exchange
    /// carrying an HMR-style update (not a test of Vite's module replacement).
    /// Optional TLS needs Python ssl and OpenSSL >= 1.1.1 (`req -addext`). Missing
    /// OpenSSL is reported as TLS UNRUN; a present but broken tool fails the test.
    fn marker_on_host(&self, python: &Path, marker: &str, openssl: Option<&Path>) -> HostServer {
        let directory = tempfile::Builder::new()
            .prefix("browser-service-")
            .tempdir_in(self.dir.path())
            .expect("browser service directory");
        if let Some(openssl) = openssl {
            let generated = Command::new(openssl)
                .args([
                    "req",
                    "-x509",
                    "-newkey",
                    "rsa:2048",
                    "-nodes",
                    "-days",
                    "1",
                    "-subj",
                    "/CN=localhost",
                    "-addext",
                    "subjectAltName=DNS:localhost",
                    "-keyout",
                    "key.pem",
                    "-out",
                    "cert.pem",
                ])
                .current_dir(directory.path())
                .output()
                .expect("TLS fixture: openssl req");
            assert!(
                generated.status.success(),
                "TLS fixture: certificate generation failed"
            );
        }
        let mut child = Command::new("nsenter")
            .arg("--target")
            .arg(self.holder.id().to_string())
            .args(["--user", "--net", "--preserve-credentials"])
            .arg(python)
            .args(["-I", "-u", "-c"])
            .arg(
                r#"
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import base64, hashlib, json, struct, sys
marker = sys.argv[1]
body = marker.encode('ascii')
class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    def setup(self):
        self.request.settimeout(5)
        super().setup()
    def clean(self):
        return ('splitty-session' not in self.headers.get('Cookie', '')
                and self.headers.get('Authorization', '') in ('', 'Bearer app-' + marker)
                and self.headers.get('Host', '').startswith('localhost:'))
    def reply(self, status, data):
        self.send_response(status)
        self.send_header('Content-Type', 'text/plain; charset=utf-8')
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_POST(self):
        if not self.clean():
            self.send_error(403)
            return
        if self.path != '/api/echo?mode=raw%2Fbytes':
            self.send_error(404)
            return
        if self.headers.get('Authorization') != 'Bearer app-' + marker:
            self.reply(401, b'app-auth-required')
            return
        length = int(self.headers.get('Content-Length', '0'))
        if not 0 < length <= 4096:
            self.send_error(400)
            return
        self.reply(200, body + b'\n' + self.rfile.read(length))
    def frame(self, opcode, data):
        head = bytes([0x80 | opcode])
        head += bytes([len(data)]) if len(data) < 126 else b'\x7e' + struct.pack('!H', len(data))
        self.wfile.write(head + data)
        self.wfile.flush()
    def receive(self):
        head = self.rfile.read(2)
        assert len(head) == 2 and head[0] & 0xf0 == 0x80 and head[1] & 0x80
        length = head[1] & 0x7f
        if length == 126:
            length = struct.unpack('!H', self.rfile.read(2))[0]
        else:
            assert length < 126
        assert length <= 4096
        mask = self.rfile.read(4)
        data = self.rfile.read(length)
        assert len(mask) == 4 and len(data) == length
        return head[0] & 0x0f, bytes(value ^ mask[i % 4] for i, value in enumerate(data))
    def do_GET(self):
        if not self.clean():
            self.send_error(403)
            return
        if self.path == '/':
            self.reply(200, body)
            return
        if self.path == '/favicon.ico':
            self.reply(204, b'')
            return
        if (self.path != '/hmr?mode=raw%2Fupdate'
                or self.headers.get('Upgrade', '').lower() != 'websocket'
                or self.headers.get('Sec-WebSocket-Version') != '13'
                or self.headers.get('Sec-WebSocket-Protocol') != 'splitty-hmr'
                or self.headers.get('Origin') != 'http://' + self.headers.get('Host', '')):
            self.send_error(400)
            return
        key = self.headers['Sec-WebSocket-Key']
        accept = base64.b64encode(hashlib.sha1((key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()).decode()
        self.send_response(101)
        self.send_header('Upgrade', 'websocket')
        self.send_header('Connection', 'Upgrade')
        self.send_header('Sec-WebSocket-Accept', accept)
        self.send_header('Sec-WebSocket-Protocol', 'splitty-hmr')
        self.end_headers()
        self.frame(1, json.dumps({'type': 'connected', 'marker': marker}).encode())
        assert self.receive() == (1, b'change:src/App.tsx')
        self.frame(1, json.dumps({'type': 'update', 'path': '/src/App.tsx', 'marker': marker}).encode())
        opcode, payload = self.receive()
        assert opcode == 2 and payload == bytes(range(256))
        self.frame(2, payload)
        opcode, payload = self.receive()
        assert opcode == 8 and payload == struct.pack('!H', 1000) + b'checked'
        self.frame(8, payload)
        self.close_connection = True
    def log_message(self, *args):
        pass
server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
if sys.argv[2] == 'tls':
    import ssl
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain('cert.pem', 'key.pem')
    server.socket = context.wrap_socket(server.socket, server_side=True)
print('SPLITTY_PORT=' + str(server.server_port), flush=True)
server.serve_forever()
"#,
            )
            .arg(marker)
            .arg(if openssl.is_some() { "tls" } else { "plain" })
            .current_dir(directory.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("browser BLOCKED: cannot spawn namespace marker service");
        let lines = read_lines(child.stdout.take().expect("marker service stdout"));
        let mut service = HostServer {
            child,
            port: 0,
            _directory: directory,
        };
        service.port = lines
            .recv_timeout(WAIT)
            .expect("browser BLOCKED: marker service startup deadline")
            .strip_prefix("SPLITTY_PORT=")
            .and_then(|port| port.parse().ok())
            .expect("browser BLOCKED: marker service port");
        assert!(
            service.port >= 1024 && service.port != 44877,
            "browser BLOCKED: reserved service port"
        );
        service
    }

    fn stop_daemon(&self) {
        let stop = self.on_host(&["splitty", "daemon", "stop"]);
        assert!(
            stop.status.success(),
            "daemon stop on the host\n{}",
            describe(&stop)
        );
    }

    /// `splitty connect` with the lab's `ssh`, its output line by line.
    fn connect(&self, extra: &[&str]) -> Connect {
        self.connect_with(extra, &[])
    }

    fn connect_with(&self, extra: &[&str], env: &[(&str, &Path)]) -> Connect {
        let mut child = self
            .client(&["connect", HOST])
            .args(extra)
            .envs(env.iter().map(|(key, value)| (*key, *value)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("splitty connect");
        let lines = read_lines(child.stdout.take().expect("stdout"));
        Connect { child, lines }
    }

    /// Where the lab's host keeps the installed server of this version.
    fn installed_server(&self) -> PathBuf {
        self.dir.path().join(format!(
            "home/.local/share/splitty/remote/{}/splitty",
            env!("CARGO_PKG_VERSION")
        ))
    }

    /// A server archive of the `splitty` under test, as the release builds it.
    fn payload(&self) -> PathBuf {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let bytes = splitty_remote::install::pack(
            Path::new(env!("CARGO_BIN_EXE_splitty")),
            Some(&repository.join("shell-integration")),
        )
        .expect("pack the server");
        let path = self.dir.path().join("payload.tar.gz");
        fs::write(&path, bytes).expect("write payload");
        path
    }

    fn opened(&self) -> Option<String> {
        fs::read_to_string(self.dir.path().join("opened")).ok()
    }

    /// The `splitty` under test on the client, with the lab's `ssh` and its
    /// own configuration directory.
    fn client(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_splitty"));
        command
            .args(args)
            // A developer's own payload must not leak into the test.
            .env_remove("SPLITTY_REMOTE_PAYLOAD")
            .env("XDG_CONFIG_HOME", self.dir.path().join("client-config"))
            .env("PATH", self.path());
        command
    }

    /// The client's recent hosts' file.
    fn recent_hosts(&self) -> PathBuf {
        self.dir
            .path()
            .join("client-config/splitty/recent-hosts.json")
    }

    /// All invocations, including short-lived failures that /proc could miss.
    /// Record roles only, never command arguments or credentials.
    fn ssh_invocations(&self) -> String {
        fs::read_to_string(self.dir.path().join("ssh-invocations")).unwrap_or_default()
    }

    fn saved_forwards(&self) -> serde_json::Value {
        serde_json::from_slice(
            &fs::read(
                self.dir
                    .path()
                    .join("client-config/splitty/remote-forwards.json"),
            )
            .expect("saved forwards file"),
        )
        .expect("saved forwards JSON")
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let info = self.dir.path().join("home/.config/splitty/daemon.json");
        if let Some(pid) = fs::read_to_string(info)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|info| info["pid"].as_i64())
            .and_then(|pid| i32::try_from(pid).ok())
            .filter(|pid| *pid > 0)
        {
            let _ = kill(Pid::from_raw(pid), Signal::SIGKILL);
        }
        let _ = self.holder.kill();
        let _ = self.holder.wait();
    }
}

/// A server the test started on the host.
struct HostServer {
    child: Child,
    port: u16,
    _directory: TempDir,
}

impl Drop for HostServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A port nothing listens on here, on `127.0.0.1` or `::1`, which a forward
/// therefore takes as asked.
fn free_local_port() -> u16 {
    loop {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().expect("address").port();
        let ipv6 = TcpListener::bind((Ipv6Addr::LOCALHOST, port));
        if !matches!(ipv6, Err(error) if error.kind() == io::ErrorKind::AddrInUse) {
            return port;
        }
    }
}

/// The lab, or `None` after printing why the test is skipped.
fn lab() -> Option<Lab> {
    match Lab::start() {
        Ok(lab) => Some(lab),
        Err(reason) => {
            eprintln!("skipping: the SSH lab cannot run here: {reason}");
            None
        }
    }
}

/// The `ssh` processes `parent` started that have not been reaped.
fn ssh_children_of(parent: u32) -> Vec<i32> {
    let parent = parent.to_string();
    fs::read_dir("/proc")
        .expect("/proc")
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<i32>().ok())
        .filter(|pid| {
            fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
                // pid (comm) state ppid …
                let after = stat.rsplit_once(") ").map(|(_, rest)| rest).unwrap_or("");
                stat.contains("(ssh)") && after.split_whitespace().nth(1) == Some(parent.as_str())
            })
        })
        .collect()
}

fn read_lines(stdout: ChildStdout) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

/// A running `splitty connect`.
struct Connect {
    child: Child,
    lines: mpsc::Receiver<String>,
}

impl Connect {
    /// The lines up to and including the first one that starts with `prefix`.
    fn until(&self, prefix: &str) -> Vec<String> {
        let mut seen = Vec::new();
        loop {
            match self.lines.recv_timeout(WAIT) {
                Ok(line) => {
                    let done = line.starts_with(prefix);
                    seen.push(line);
                    if done {
                        return seen;
                    }
                }
                Err(_) => panic!(
                    "no line starting with {prefix:?}; stdout so far:\n{}",
                    seen.join("\n")
                ),
            }
        }
    }

    /// The next line.
    fn next(&self) -> String {
        self.lines
            .recv_timeout(WAIT)
            .unwrap_or_else(|_| panic!("no further line"))
    }

    fn url(lines: &[String]) -> String {
        lines
            .iter()
            .find_map(|line| line.strip_prefix("SPLITTY_URL="))
            .unwrap_or_else(|| panic!("no SPLITTY_URL in:\n{}", lines.join("\n")))
            .to_owned()
    }

    /// The `ssh` processes this `connect` started.
    fn ssh_children(&self) -> Vec<i32> {
        ssh_children_of(self.child.id())
    }

    fn kill_tunnel(&self) {
        let tunnels = self.ssh_children();
        assert_eq!(
            tunnels.len(),
            1,
            "exactly one ssh should hold the tunnel: {tunnels:?}"
        );
        kill(Pid::from_raw(tunnels[0]), Signal::SIGKILL).expect("kill ssh");
    }

    /// SIGTERM, the exit status and every line printed after the ones read.
    fn terminate(mut self) -> (std::process::ExitStatus, String) {
        kill(Pid::from_raw(self.child.id() as i32), Signal::SIGTERM).expect("SIGTERM");
        let status = self.child.wait().expect("wait");
        // Until the reader has seen the end of the output.
        let mut rest = Vec::new();
        while let Ok(line) = self.lines.recv_timeout(WAIT) {
            rest.push(line);
        }
        (status, rest.join("\n"))
    }
}

impl Drop for Connect {
    /// SIGTERM first, so `connect` ends its tunnel's `ssh`; a SIGKILL alone
    /// would leave that `ssh` running, holding a connection into the lab.
    fn drop(&mut self) {
        let _ = kill(Pid::from_raw(self.child.id() as i32), Signal::SIGTERM);
        for _ in 0..50 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A minimal HTTP/1.1 exchange with the tunnel's local end.
fn http(
    port: u16,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    body: Option<&str>,
) -> (u16, String) {
    let mut stream = TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), WAIT)
        .expect("connect to the tunnel");
    stream.set_read_timeout(Some(WAIT)).expect("timeout");
    stream.set_write_timeout(Some(WAIT)).expect("timeout");
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nConnection: close\r\n"
    );
    if let Some(cookie) = cookie {
        request.push_str(&format!("Cookie: {cookie}\r\n"));
    }
    let body = body.unwrap_or("");
    request.push_str(&format!(
        "Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    ));
    stream.write_all(request.as_bytes()).expect("send request");
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("no HTTP status in {response:?}"));
    (status, response)
}

/// Exchange the URL's bootstrap code for a session cookie.
fn sign_in(url: &str) -> (u16, String) {
    let port: u16 = url
        .strip_prefix("http://127.0.0.1:")
        .and_then(|rest| rest.split('/').next())
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("unexpected URL {url}"));
    let code = url
        .split_once("bootstrap=")
        .and_then(|(_, rest)| rest.split('&').next())
        .expect("bootstrap code");
    let (status, response) = http(
        port,
        "POST",
        "/api/session",
        None,
        Some(&format!("{{\"bootstrap\":\"{code}\"}}")),
    );
    assert_eq!(
        status, 200,
        "bootstrap exchange through the tunnel:\n{response}"
    );
    let cookie = response
        .lines()
        .find_map(|line| {
            line.strip_prefix("set-cookie: ")
                .or_else(|| line.strip_prefix("Set-Cookie: "))
        })
        .and_then(|value| value.split(';').next())
        .unwrap_or_else(|| panic!("no session cookie in:\n{response}"))
        .to_owned();
    (port, cookie)
}

/// Every listener on `port` in this network namespace, as `/proc/net/tcp`
/// addresses (`0100007F` is 127.0.0.1, `00000000` every interface).
fn listeners(port: u16) -> Vec<String> {
    let suffix = format!(":{port:04X}");
    ["/proc/net/tcp", "/proc/net/tcp6"]
        .iter()
        .filter_map(|table| fs::read_to_string(table).ok())
        .flat_map(|text| text.lines().skip(1).map(str::to_owned).collect::<Vec<_>>())
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (local, state) = (fields.get(1)?, fields.get(3)?);
            (local.ends_with(&suffix) && *state == "0A").then(|| local.to_string())
        })
        .collect()
}

#[test]
fn connect_reconnects_and_hands_over_sessions_through_real_ssh() {
    let Some(lab) = lab() else { return };
    lab.install_splitty();
    let port = lab.start_daemon(0);

    let connect = lab.connect(&["--no-open"]);
    let lines = connect.until("SSH tunnel active.");
    let url = Connect::url(&lines);
    assert!(
        url.ends_with(&format!("&host={HOST}")),
        "the URL names the host: {url}"
    );

    // GatewayPorts yes in the client's config, yet only loopback listens.
    assert_eq!(
        listeners(port),
        [format!("0100007F:{port:04X}")],
        "the forward must listen on 127.0.0.1 only"
    );

    let (port, cookie) = sign_in(&url);
    let (status, response) = http(port, "GET", "/api/mux/state", Some(&cookie), None);
    assert_eq!(status, 200, "{response}");

    // The ssh process dies; the same daemon is still there.
    connect.kill_tunnel();
    let lines = connect.until("SSH tunnel restored.");
    assert!(
        !lines.iter().any(|line| line.starts_with("SPLITTY_URL=")),
        "no new URL for the same daemon:\n{}",
        lines.join("\n")
    );
    let (status, response) = http(port, "GET", "/api/mux/state", Some(&cookie), None);
    assert_eq!(
        status, 200,
        "the browser's session works again:\n{response}"
    );

    // The daemon restarts on the same port while the tunnel is down.
    lab.stop_daemon();
    assert_eq!(lab.start_daemon(port), port);
    connect.kill_tunnel();
    let lines = connect.until("SSH tunnel restored.");
    let new_url = Connect::url(&lines);
    assert_ne!(new_url, url, "a restarted daemon needs a new session");
    let (_, new_cookie) = sign_in(&new_url);
    let (status, response) = http(port, "GET", "/api/mux/state", Some(&new_cookie), None);
    assert_eq!(status, 200, "{response}");

    assert_eq!(lab.opened(), None, "--no-open must not open a browser");
    let tunnels = connect.ssh_children();
    let (status, rest) = connect.terminate();
    assert!(
        status.success(),
        "SIGTERM ends connect with status 0: {status}\n{rest}"
    );
    assert!(rest.contains("Closing tunnel..."), "{rest}");
    for pid in tunnels {
        assert!(
            !Path::new(&format!("/proc/{pid}")).exists()
                || fs::read_to_string(format!("/proc/{pid}/stat"))
                    .is_ok_and(|stat| stat.contains(") Z ")),
            "the tunnel's ssh process must end with connect"
        );
    }
}

#[test]
fn a_host_without_splitty_is_reported() {
    let Some(lab) = lab() else { return };
    let out = lab
        .client(&["connect", HOST, "--no-open"])
        .stdin(Stdio::null())
        .output()
        .expect("splitty connect");
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&format!("splitty is not installed on {HOST}")),
        "{}",
        describe(&out)
    );
    assert!(
        !lab.recent_hosts().exists(),
        "a failed connect records nothing"
    );
}

/// Every line of `connect --json`'s standard output, each a JSON object with
/// an `event`.
fn json_events(lines: &[String]) -> Vec<serde_json::Value> {
    lines
        .iter()
        .map(|line| {
            let event: serde_json::Value =
                serde_json::from_str(line).unwrap_or_else(|e| panic!("not JSON ({e}): {line:?}"));
            assert!(event["event"].is_string(), "no event: {line}");
            event
        })
        .collect()
}

/// `cli`, Remote Connect Machine-Readable Output (Connected, Interrupted) and
/// Remote Connect Recent Hosts (Target recorded), with `remote-hosts --json`.
#[test]
fn connect_json_reports_progress_the_session_and_its_end() {
    let Some(lab) = lab() else { return };
    lab.install_splitty();
    let connect = lab.connect(&["--no-open", "--json"]);
    let events = json_events(&connect.until(r#"{"event":"ready""#));

    let steps: Vec<&str> = events
        .iter()
        .filter(|event| event["event"] == "progress")
        .map(|event| event["step"].as_str().expect("a step"))
        .collect();
    assert_eq!(steps, ["daemon", "started", "tunnel"], "{events:?}");
    let ready = events.last().expect("ready");
    let port = ready["port"].as_u64().expect("port");
    let url = ready["url"].as_str().expect("url");
    let code = url
        .strip_prefix(&format!("http://127.0.0.1:{port}/?bootstrap="))
        .and_then(|rest| rest.strip_suffix(&format!("&host={HOST}")))
        .unwrap_or_else(|| panic!("unexpected URL {url}"));
    assert!(
        code.len() == 32 && code.bytes().all(|b| b.is_ascii_hexdigit()),
        "{url}"
    );
    let (port, cookie) = sign_in(url);
    let (status, response) = http(port, "GET", "/api/mux/state", Some(&cookie), None);
    assert_eq!(status, 200, "{response}");

    // Recorded once the URL was obtained, before it was printed.
    let recent: serde_json::Value =
        serde_json::from_slice(&fs::read(lab.recent_hosts()).expect("recent hosts")).expect("JSON");
    assert_eq!(recent["version"], 1, "{recent}");
    assert_eq!(recent["hosts"][0]["target"], HOST, "{recent}");
    let mode = fs::metadata(lab.recent_hosts())
        .expect("mode")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);

    // The host picker's list: the recent host, then `~/.ssh/config`.
    let home = lab.dir.path().join("client-home");
    fs::create_dir_all(home.join(".ssh")).expect(".ssh");
    fs::write(
        home.join(".ssh/config"),
        format!("Host api.devpod {HOST}\n"),
    )
    .expect("config");
    let hosts = lab
        .client(&["remote-hosts", "--json"])
        .env("HOME", &home)
        .output()
        .expect("remote-hosts");
    assert!(hosts.status.success(), "{}", describe(&hosts));
    let hosts: serde_json::Value = serde_json::from_slice(&hosts.stdout).expect("one array");
    assert_eq!(
        hosts,
        serde_json::json!([
            {"target": HOST, "source": "recent", "lastConnected": recent["hosts"][0]["lastConnected"]},
            {"target": "api.devpod", "source": "ssh-config"},
        ])
    );

    let (status, rest) = connect.terminate();
    assert!(
        status.success(),
        "SIGTERM ends with status 0: {status}\n{rest}"
    );
    let rest: Vec<String> = rest.lines().map(str::to_owned).collect();
    let after = json_events(&rest);
    assert_eq!(
        after.last(),
        Some(&serde_json::json!({"event": "closed"})),
        "{rest:?}"
    );
}

/// `cli`, Remote Connect Machine-Readable Output (Unknown host key) and
/// Remote Connect Recent Hosts (Failed connect).
#[test]
fn connect_json_reports_why_it_failed_and_records_nothing() {
    let Some(lab) = lab() else { return };
    lab.install_splitty();
    for target in [NOWHERE, NEW_KEY] {
        let out = lab
            .client(&["connect", target, "--json", "--no-open"])
            .stdin(Stdio::null())
            .output()
            .expect("splitty connect");
        assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
        let lines: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_owned)
            .collect();
        let events = json_events(&lines);
        let failed = events.last().expect("an event");
        assert_eq!(failed["event"], "failed", "{}", describe(&out));
        let message = failed["message"].as_str().expect("message");
        assert!(
            message.starts_with(&format!(
                "Failed to reach the remote host: ssh to {target} failed"
            )),
            "{message}"
        );
        if target == NEW_KEY {
            let hint = failed["hint"].as_str().expect("a hint");
            assert!(
                hint.contains(&format!("Run `ssh {NEW_KEY}` once")),
                "{hint}"
            );
            assert!(hint.contains("fingerprint"), "{hint}");
        } else {
            assert!(failed.get("hint").is_none(), "{failed}");
        }
    }
    assert!(
        !lab.recent_hosts().exists(),
        "a failed connect records nothing"
    );
}

#[test]
fn connect_starts_a_daemon_on_a_host_without_one() {
    let Some(lab) = lab() else { return };
    lab.install_splitty();
    let connect = lab.connect(&["--no-open"]);
    let lines = connect.until("SSH tunnel active.");
    let started = lines
        .iter()
        .find_map(|line| line.strip_prefix(&format!("Started a Splitty daemon on {HOST} (port ")))
        .and_then(|rest| rest.strip_suffix(")."))
        .and_then(|port| port.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("connect says it started the daemon:\n{}", lines.join("\n")));
    let url = Connect::url(&lines);
    assert!(
        url.starts_with(&format!("http://127.0.0.1:{started}/")),
        "{url}"
    );
    let (port, cookie) = sign_in(&url);
    let (status, response) = http(port, "GET", "/api/mux/state", Some(&cookie), None);
    assert_eq!(status, 200, "{response}");
}

/// The host's daemon, as its daemon.json names it.
/// The inodes of the host's keeper sockets: a keeper that is replaced binds a
/// new socket, even under the same name.
fn host_keepers(lab: &Lab) -> Vec<u64> {
    use std::os::unix::fs::MetadataExt;
    let dir = lab.dir.path().join("home/.local/share/splitty/run/keepers");
    let mut inodes: Vec<u64> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "sock"))
                .filter_map(|entry| entry.metadata().ok().map(|meta| meta.ino()))
                .collect()
        })
        .unwrap_or_default();
    inodes.sort_unstable();
    inodes
}

fn host_daemon_pid(lab: &Lab) -> i32 {
    let info = fs::read_to_string(lab.dir.path().join("home/.config/splitty/daemon.json"))
        .expect("the host's daemon.json");
    let info: serde_json::Value = serde_json::from_str(&info).expect("daemon.json");
    info["pid"]
        .as_i64()
        .and_then(|pid| i32::try_from(pid).ok())
        .expect("pid")
}

#[test]
fn connect_installs_its_server_and_survives_a_reboot_and_a_rebuild() {
    let Some(lab) = lab() else { return };
    let payload = lab.payload();
    let installed = lab.installed_server();

    // A host without Splitty: install, start the daemon, connect.
    let connect = lab.connect_with(&["--no-open"], &[("SPLITTY_REMOTE_PAYLOAD", &payload)]);
    let lines = connect.until("SSH tunnel active.");
    let text = lines.join("\n");
    assert!(
        text.contains(&format!(
            "Installing Splitty {} on {HOST} (",
            env!("CARGO_PKG_VERSION")
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!("Started a Splitty daemon on {HOST}")),
        "{text}"
    );
    assert!(
        installed.is_file(),
        "server installed at {}",
        installed.display()
    );
    assert!(installed.with_file_name("shell-integration").is_dir());
    let url = Connect::url(&lines);
    let (port, _) = sign_in(&url);

    // A reboot: the daemon is gone and the tunnel drops. connect starts the
    // daemon again on the same port and hands over a new session.
    kill(Pid::from_raw(host_daemon_pid(&lab)), Signal::SIGKILL).expect("kill the daemon");
    connect.kill_tunnel();
    let lines = connect.until("SSH tunnel restored.");
    let after_reboot = Connect::url(&lines);
    assert_ne!(after_reboot, url);
    let (same_port, cookie) = sign_in(&after_reboot);
    assert_eq!(
        same_port, port,
        "the daemon keeps its port, so the origin stays"
    );
    let (status, response) = http(port, "GET", "/api/mux/state", Some(&cookie), None);
    assert_eq!(status, 200, "{response}");

    // A rebuilt container: server and daemon are gone. connect installs the
    // server again and starts the daemon.
    kill(Pid::from_raw(host_daemon_pid(&lab)), Signal::SIGKILL).expect("kill the daemon");
    fs::remove_dir_all(lab.dir.path().join("home/.local/share/splitty"))
        .expect("remove the server");
    connect.kill_tunnel();
    let lines = connect.until("SSH tunnel restored.");
    assert!(installed.is_file(), "the server is installed again");
    let (_, cookie) = sign_in(&Connect::url(&lines));
    let (status, response) = http(port, "GET", "/api/mux/state", Some(&cookie), None);
    assert_eq!(status, 200, "{response}");
    let (status, rest) = connect.terminate();
    assert!(status.success(), "{status}\n{rest}");

    // Connecting again reuses the installed server and the running daemon.
    let again = lab.connect_with(&["--no-open"], &[("SPLITTY_REMOTE_PAYLOAD", &payload)]);
    let lines = again.until("SSH tunnel active.");
    assert!(
        !lines
            .iter()
            .any(|l| l.starts_with("Installing") || l.starts_with("Started")),
        "{}",
        lines.join("\n")
    );
}

#[test]
fn the_bridge_carries_the_daemons_websocket_through_ssh() {
    let Some(lab) = lab() else { return };
    lab.install_splitty();
    lab.start_daemon(0);
    let mut bridge = Command::new("ssh")
        .env("PATH", lab.path())
        .args([
            "-o",
            "BatchMode=yes",
            HOST,
            "--",
            "splitty",
            "remote-bridge",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("ssh remote-bridge");
    let mut stdout = bridge.stdout.take().expect("stdout");
    let mut header = [0u8; 5];
    stdout.read_exact(&mut header).expect("a frame through ssh");
    assert_eq!(header[0], 1, "a text frame");
    let length = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
    let mut payload = vec![0u8; length];
    stdout
        .read_exact(&mut payload)
        .expect("the frame's payload");
    let welcome: serde_json::Value = serde_json::from_slice(&payload).expect("JSON");
    assert_eq!(welcome["type"], "mux.welcome", "{welcome}");
    for secret in ["token", "ticket", "bootstrap"] {
        assert!(
            !welcome.to_string().contains(secret),
            "{secret} in {welcome}"
        );
    }
    drop(bridge.stdin.take());
    let status = bridge.wait().expect("wait");
    assert!(status.success(), "the bridge ends with its input: {status}");
}

/// `/health` of a host's server, through a forward's local `port`.
fn health(port: u16) -> String {
    let (status, response) = http(port, "GET", "/health", None, None);
    assert_eq!(status, 200, "through local port {port}:\n{response}");
    response
        .rsplit("\r\n\r\n")
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// `cli`, Remote Connect Port Forwards (Forward a dev server, Local port busy,
/// Forwards survive a reconnect), and `access-control`, Forwarded Ports Bound
/// To Loopback (GatewayPorts in the SSH configuration): servers that listen
/// only on the host's loopback, reached through the announced local ports.
#[test]
fn connect_forwards_ports_of_the_host_through_a_reconnect() {
    let Some(lab) = lab() else { return };
    lab.install_splitty();
    let free = lab.serve_on_host(free_local_port());
    let busy = lab.serve_on_host(free_local_port());
    // A local process holds the second server's port here.
    let _local = TcpListener::bind(("127.0.0.1", busy.port)).expect("hold the port here");
    let list = format!("{},{}", free.port, busy.port);

    let connect = lab.connect(&["--no-open", "--json", "--forward", &list]);
    connect.until(r#"{"event":"ready""#);
    // One event per forward, right after `ready`.
    let forwards = json_events(&[connect.next(), connect.next()]);
    assert_eq!(
        forwards[0],
        serde_json::json!({
            "event": "forward",
            "remote": free.port,
            "local": free.port,
            "url": format!("http://localhost:{}", free.port),
        })
    );
    assert_eq!(forwards[1]["event"], "forward", "{forwards:?}");
    assert_eq!(forwards[1]["remote"], busy.port, "{forwards:?}");
    let moved = forwards[1]["local"]
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .expect("a local port");
    assert_ne!(moved, busy.port, "the busy port is left alone");
    assert_eq!(forwards[1]["url"], format!("http://localhost:{moved}"));

    // GatewayPorts yes in the client's config, yet only 127.0.0.1 listens.
    assert_eq!(
        listeners(free.port),
        [format!("0100007F:{:04X}", free.port)],
        "a forward must listen on 127.0.0.1 only"
    );
    assert_eq!(health(free.port), "ok");
    assert_eq!(health(moved), "ok");

    // The ssh process dies; the forwards come back on the same ports.
    connect.kill_tunnel();
    let lines = connect.until(r#"{"event":"restored""#);
    assert!(
        !lines
            .iter()
            .any(|line| line.contains(r#""event":"forward""#)),
        "the forwards are not announced again:\n{}",
        lines.join("\n")
    );
    assert_eq!(health(free.port), "ok");
    assert_eq!(health(moved), "ok");
    let (status, rest) = connect.terminate();
    assert!(status.success(), "{status}\n{rest}");

    // The text output: a line per forward between the URL and the tunnel's
    // line, the moved one with a note.
    let text = lab.connect(&["--no-open", "--forward", &list]);
    let lines = text.until("SSH tunnel active.");
    let url = lines
        .iter()
        .position(|line| line.starts_with("SPLITTY_URL="))
        .expect("the URL line");
    assert_eq!(
        lines[url + 1],
        format!(
            "Forwarding http://localhost:{port} -> {HOST}:{port}",
            port = free.port
        ),
        "{lines:?}"
    );
    let note = format!(
        " -> {HOST}:{port} (local port {port} is not free)",
        port = busy.port
    );
    assert!(
        lines[url + 2].starts_with("Forwarding http://localhost:")
            && lines[url + 2].ends_with(&note),
        "{lines:?}"
    );
    assert_eq!(lines.len(), url + 4, "{lines:?}");
    // remote-forwards-persistence, design D2: `connect` never remembers its
    // forwards; saved mappings belong to the hub.
    assert!(
        !lab.dir
            .path()
            .join("client-config/splitty/remote-forwards.json")
            .exists(),
        "connect saved its forwards"
    );
}

/// `cli`, Remote Connect Port Forwards (Invalid entry) and Remote Connect
/// Machine-Readable Output (Rejected forward list): a list `connect` cannot use
/// ends it with status 2 before any `ssh` runs. The parsing is covered by unit
/// tests; only the binary shows its exit status and output. It needs no lab,
/// only an `ssh` that records being run.
#[test]
fn connect_rejects_an_invalid_forward_before_ssh_runs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = dir.path().join("bin");
    fs::create_dir(&bin).expect("bin");
    let ran = dir.path().join("ssh-ran");
    write_executable(
        &bin.join("ssh"),
        &format!("#!/bin/sh\necho \"$@\" >> {}\nexit 255\n", ran.display()),
    );
    let mut dirs = vec![bin];
    dirs.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
    let path = env::join_paths(dirs).expect("PATH");

    for list in ["70000", "0", "3000:", "3000,3000", "8080:3000,8080:5173"] {
        for json in [false, true] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_splitty"));
            command
                .args(["connect", HOST, "--no-open", "--forward", list])
                .env("PATH", &path)
                .env("XDG_CONFIG_HOME", dir.path().join("config"))
                .stdin(Stdio::null());
            if json {
                command.arg("--json");
            }
            let out = command.output().expect("splitty connect");
            assert_eq!(out.status.code(), Some(2), "{list}\n{}", describe(&out));
            let message = if json {
                let line = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                let event: serde_json::Value = serde_json::from_str(&line)
                    .unwrap_or_else(|e| panic!("one JSON line ({e}): {line:?}"));
                assert_eq!(event["event"], "failed", "{event}");
                event["message"].as_str().expect("message").to_owned()
            } else {
                String::from_utf8_lossy(&out.stderr).into_owned()
            };
            assert!(message.contains("--forward"), "{list}: {message}");
        }
    }
    assert!(
        !ran.exists(),
        "ssh must not run: {}",
        fs::read_to_string(&ran).unwrap_or_default()
    );
}

/// A local `splitty serve` acting as the remote screen hub: the lab's `ssh`,
/// a home and runtime directory of its own, and a browser session.
struct HubServer {
    child: Child,
    port: u16,
    cookie: String,
    stderr: PathBuf,
    /// Keeps reading the server's output, which it must be able to write.
    _lines: mpsc::Receiver<String>,
}

impl HubServer {
    /// Start it with `payload` as the server it installs on hosts, and sign
    /// in with its bootstrap code.
    fn start(lab: &Lab, payload: &Path) -> Self {
        let d = lab.dir.path();
        let home = d.join("client-home");
        let run = d.join("client-run");
        fs::create_dir_all(home.join(".ssh")).expect("client home");
        fs::create_dir_all(&run).expect("client runtime directory");
        fs::set_permissions(&run, fs::Permissions::from_mode(0o700)).expect("chmod");
        let stderr = d.join("serve.stderr");
        let mut child = lab
            .client(&["serve", "--addr", "127.0.0.1:0"])
            .env("SPLITTY_REMOTE_PAYLOAD", payload)
            .env("HOME", &home)
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("XDG_RUNTIME_DIR", &run)
            .env_remove("SPLITTY_TOKEN")
            .env_remove("SPLITTY_SERVER")
            .env_remove("SPLITTY_SOCK")
            .env_remove("SPLITTY_AGENT_SOCK")
            .env_remove("SPLITTY_PANE_ID")
            .env_remove("SPLITTY_SCREEN_ID")
            .env_remove("SPLITTY_SESSION_ID")
            // A separate, owned group lets teardown also end an in-flight
            // ssh/ProxyCommand, even when an assertion unwinds the test.
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(fs::File::create(&stderr).expect("stderr file")))
            .spawn()
            .expect("splitty serve");
        let lines = read_lines(child.stdout.take().expect("stdout"));
        let mut server = Self {
            child,
            port: 0,
            cookie: String::new(),
            stderr,
            _lines: lines,
        };
        let (mut port, mut code) = (None, None);
        let deadline = Instant::now() + WAIT;
        while port.is_none() || code.is_none() {
            let line = server
                ._lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("serve printed no port and bootstrap code before its deadline");
            if let Some(value) = line.strip_prefix("SPLITTY_PORT=") {
                port = value.trim().parse::<u16>().ok();
            } else if let Some(value) = line.strip_prefix("SPLITTY_BOOTSTRAP=") {
                code = Some(value.trim().to_owned());
            }
        }
        let (port, code) = (port.expect("port"), code.expect("code"));
        let (_, cookie) = sign_in(&format!("http://127.0.0.1:{port}/?bootstrap={code}"));
        server.port = port;
        server.cookie = cookie;
        server
    }

    /// Open `/ws/remote` for `target` as the UI does: a fresh ticket, the
    /// server's own origin.
    async fn open(&self, target: &str) -> HubSocket {
        self.open_with(target, "").await
    }

    /// [`HubServer::open`] with more of the query, such as `&update=1`.
    async fn open_with(&self, target: &str, query: &str) -> HubSocket {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;

        let (status, response) = http(
            self.port,
            "POST",
            "/api/ws-ticket",
            Some(&self.cookie),
            Some("{}"),
        );
        assert_eq!(status, 200, "{response}");
        let ticket: serde_json::Value =
            serde_json::from_str(response.split_once("\r\n\r\n").expect("body").1).expect("JSON");
        let ticket = ticket["ticket"].as_str().expect("ticket");
        let mut request = format!(
            "ws://127.0.0.1:{}/ws/remote?ticket={ticket}&target={target}{query}",
            self.port
        )
        .into_client_request()
        .expect("request");
        let origin = format!("http://127.0.0.1:{}", self.port)
            .parse()
            .expect("origin");
        request.headers_mut().insert("origin", origin);
        let (socket, _) = tokio::time::timeout(WAIT, tokio_tungstenite::connect_async(request))
            .await
            .expect("WebSocket upgrade before its deadline")
            .expect("upgrade /ws/remote");
        HubSocket(socket)
    }

    /// The `ssh` processes the hub runs now.
    fn ssh(&self) -> Vec<i32> {
        ssh_children_of(self.child.id())
    }

    /// Wait until the hub runs no `ssh` any more.
    fn wait_without_ssh(&self) {
        let deadline = std::time::Instant::now() + WAIT;
        while !self.ssh().is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "the hub still runs ssh: {:?}",
                self.ssh()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for HubServer {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "splitty serve stderr:\n{}",
                fs::read_to_string(&self.stderr).unwrap_or_default()
            );
        }
        // Only this server's group, created at spawn; never process-name
        // cleanup. The direct child is reaped before its pid can be reused.
        if let Ok(pid) = i32::try_from(self.child.id()) {
            let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

type Message = tokio_tungstenite::tungstenite::Message;

struct HubSocket(
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
);

impl HubSocket {
    async fn next(&mut self) -> Message {
        use futures::StreamExt;

        tokio::time::timeout(WAIT, self.0.next())
            .await
            .expect("a message in time")
            .expect("the socket is open")
            .expect("a message")
    }

    async fn next_json(&mut self) -> serde_json::Value {
        match self.next().await {
            Message::Text(text) => serde_json::from_str(text.as_str()).expect("JSON"),
            other => panic!("not a text message: {other:?}"),
        }
    }

    /// The hub's progress steps and the message that ends them.
    async fn outcome(&mut self) -> (Vec<String>, serde_json::Value) {
        let (progress, outcome) = self.progress().await;
        let steps = progress
            .iter()
            .map(|message| message["step"].as_str().expect("a step").to_owned())
            .collect();
        (steps, outcome)
    }

    /// The hub's progress messages and the message that ends them.
    async fn progress(&mut self) -> (Vec<serde_json::Value>, serde_json::Value) {
        let mut progress = Vec::new();
        loop {
            let message = self.next_json().await;
            if message["type"] != "remote.progress" {
                return (progress, message);
            }
            progress.push(message);
        }
    }

    async fn send(&mut self, text: &str) {
        use futures::SinkExt;

        self.0.send(Message::text(text)).await.expect("send");
    }

    async fn close(&mut self) {
        self.0.close(None).await.expect("close");
    }

    /// The close code, skipping what the host still sends.
    async fn close_code(&mut self) -> u16 {
        loop {
            if let Message::Close(frame) = self.next().await {
                return frame.map_or(1005, |frame| u16::from(frame.code));
            }
        }
    }
}

/// remote-screens, Remote Screen Hub Endpoint, through the real binary, a
/// real SSH connection and a real host daemon: a host without Splitty is
/// installed and started, the relay carries the mux and session protocols, a
/// killed `ssh` closes the socket with 4001, closing the socket ends the `ssh`
/// and leaves the daemon running, and hosts that cannot be reached are told
/// apart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_hub_relays_a_host_through_real_ssh() {
    let Some(lab) = lab() else { return };
    let payload = lab.payload();
    let hub = HubServer::start(&lab, &payload);

    // A host without Splitty.
    let mut socket = hub.open(HOST).await;
    let (steps, ready) = socket.outcome().await;
    assert_eq!(steps, ["install", "daemon", "started", "bridge"], "{ready}");
    assert_eq!(ready["type"], "remote.ready", "{ready}");
    assert_eq!(ready["target"], HOST, "{ready}");
    assert_eq!(ready["daemon"]["started"], true, "{ready}");
    assert_eq!(
        ready["daemon"]["version"],
        env!("CARGO_PKG_VERSION"),
        "{ready}"
    );
    assert_eq!(ready["screens"], serde_json::json!([]), "{ready}");
    assert!(lab.installed_server().is_file(), "installed on the host");
    let welcome = socket.next_json().await;
    assert_eq!(welcome["type"], "mux.welcome", "{welcome}");

    // A screen on the host, and its shell's output through the relay.
    socket
        .send(
            r#"{"type":"mux.op","screenId":"hub-screen","op":{"type":"create_screen","screenId":"hub-screen","name":"hub","newPaneId":"hub-pane"}}"#,
        )
        .await;
    let session = loop {
        let message = socket.next_json().await;
        if message["type"] == "mux.pane_spawned" {
            assert_eq!(message["paneId"], "hub-pane", "{message}");
            break message["sessionId"].as_str().expect("session").to_owned();
        }
    };
    socket
        .send(&format!(
            r#"{{"type":"attach","id":"{session}","cols":80,"rows":24}}"#
        ))
        .await;
    loop {
        if let Message::Binary(frame) = socket.next().await {
            // [id length][id][output]
            let length = usize::from(frame[0]);
            if frame.get(1..1 + length) == Some(session.as_bytes()) && frame.len() > 1 + length {
                break;
            }
        }
    }

    // The relaying ssh dies: the socket closes with 4001.
    let bridges = hub.ssh();
    assert_eq!(bridges.len(), 1, "one ssh carries the bridge: {bridges:?}");
    kill(Pid::from_raw(bridges[0]), Signal::SIGKILL).expect("kill the bridge's ssh");
    assert_eq!(socket.close_code().await, 4001);
    hub.wait_without_ssh();

    // Again: the server is installed and the daemon runs, with the screen.
    let daemon = host_daemon_pid(&lab);
    let mut socket = hub.open(HOST).await;
    let (steps, ready) = socket.outcome().await;
    assert_eq!(steps, ["daemon", "bridge"], "{ready}");
    assert_eq!(ready["daemon"]["started"], false, "{ready}");
    assert_eq!(
        ready["screens"],
        serde_json::json!([{"id": "hub-screen", "name": "hub", "panes": 1}]),
        "{ready}"
    );
    assert_eq!(socket.next_json().await["type"], "mux.welcome");

    // The client closes the socket: the ssh ends, the daemon keeps running.
    assert_eq!(hub.ssh().len(), 1, "one ssh per socket");
    socket.close().await;
    hub.wait_without_ssh();
    assert!(
        kill(Pid::from_raw(daemon), None).is_ok(),
        "the host's daemon keeps running"
    );

    // Hosts the hub cannot reach.
    for (target, code) in [(NOWHERE, "unreachable"), (NEW_KEY, "host_key")] {
        let mut socket = hub.open(target).await;
        let (steps, error) = socket.outcome().await;
        assert!(steps.is_empty(), "{steps:?}");
        assert_eq!(error["type"], "remote.error", "{error}");
        assert_eq!(error["code"], code, "{error}");
        if target == NEW_KEY {
            let hint = error["hint"].as_str().expect("a hint");
            assert!(
                hint.contains(&format!("Run `ssh {NEW_KEY}` once")),
                "{hint}"
            );
        }
    }

    // Only the host that became ready is recorded, and the picker lists it.
    let (status, response) = http(
        hub.port,
        "GET",
        "/api/remote/hosts",
        Some(&hub.cookie),
        None,
    );
    assert_eq!(status, 200, "{response}");
    let hosts: serde_json::Value =
        serde_json::from_str(response.split_once("\r\n\r\n").expect("body").1).expect("JSON");
    assert_eq!(hosts[0]["target"], HOST, "{hosts}");
    assert_eq!(hosts[0]["source"], "recent", "{hosts}");
    assert_eq!(hosts.as_array().map(Vec::len), Some(1), "{hosts}");
}

/// remote-screens, Host Update (Update an incompatible host), through the
/// real binary, SSH and host daemons: `update=1` leaves a daemon of the hub's
/// version alone; a daemon of another version outside the protocol window
/// keeps running without it, and with it is stopped and replaced by one of
/// the hub's version, which the socket relays.
///
/// The test builds no second Splitty, so the old daemon is this version with
/// another version and protocol in its `daemon.json`: the record the
/// handshake reports and `daemon stop` reads, which is all the hub sees of a
/// daemon's version.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_hub_updates_a_host_daemon_of_another_version() {
    let Some(lab) = lab() else { return };
    let payload = lab.payload();
    let hub = HubServer::start(&lab, &payload);

    // A host without Splitty, and then its daemon of the hub's version with
    // a screen.
    let mut socket = hub.open(HOST).await;
    let (steps, ready) = socket.outcome().await;
    assert_eq!(steps, ["install", "daemon", "started", "bridge"], "{ready}");
    socket
        .send(
            r#"{"type":"mux.op","screenId":"kept","op":{"type":"create_screen","screenId":"kept","name":"kept","newPaneId":"kept-pane"}}"#,
        )
        .await;
    while socket.next_json().await["type"] != "mux.pane_spawned" {}
    socket.close().await;
    hub.wait_without_ssh();
    let daemon = host_daemon_pid(&lab);
    let keepers = host_keepers(&lab);
    assert_eq!(keepers.len(), 1, "the screen's shell has a keeper");
    let mut socket = hub.open_with(HOST, "&update=1").await;
    let (steps, ready) = socket.outcome().await;
    assert_eq!(steps, ["daemon", "bridge"], "nothing to update: {ready}");
    assert_eq!(ready["daemon"]["started"], false, "{ready}");
    assert_eq!(socket.next_json().await["type"], "mux.welcome");
    socket.close().await;
    hub.wait_without_ssh();
    assert_eq!(host_daemon_pid(&lab), daemon, "the same daemon runs");

    // The daemon becomes one of another version and protocol.
    let info_path = lab.dir.path().join("home/.config/splitty/daemon.json");
    let mut info: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&info_path).expect("daemon.json")).expect("JSON");
    let protocol = info["protocol"].as_u64().expect("protocol");
    info["version"] = "0.0.1-old".into();
    info["protocol"] = (protocol + 2).into();
    fs::write(&info_path, info.to_string()).expect("write daemon.json");

    let mut socket = hub.open(HOST).await;
    let (steps, error) = socket.outcome().await;
    assert_eq!(steps, ["daemon"], "{error}");
    assert_eq!(error["code"], "incompatible", "{error}");
    assert_eq!(error["version"], "0.0.1-old", "{error}");
    assert_eq!(error["keepsSessions"], true, "{error}");
    assert!(
        kill(Pid::from_raw(daemon), None).is_ok(),
        "without update=1 the daemon keeps running"
    );

    let mut socket = hub.open_with(HOST, "&update=1").await;
    let (progress, ready) = socket.progress().await;
    let steps: Vec<&str> = progress
        .iter()
        .map(|message| message["step"].as_str().expect("a step"))
        .collect();
    assert_eq!(steps, ["daemon", "update", "started", "bridge"], "{ready}");
    assert_eq!(
        progress[1]["message"],
        format!("Stopping Splitty 0.0.1-old on {HOST}...")
    );
    assert_eq!(ready["type"], "remote.ready", "{ready}");
    assert_eq!(
        ready["daemon"]["version"],
        env!("CARGO_PKG_VERSION"),
        "{ready}"
    );
    assert_eq!(ready["daemon"]["protocol"], protocol, "{ready}");
    assert_eq!(ready["daemon"]["started"], true, "{ready}");
    assert_eq!(ready["daemon"]["keepsSessions"], true, "{ready}");
    // The old daemon saved its workspace and its keeper kept the shell; the
    // new one has the screen again, with that shell (`keeper-host-updates`).
    assert_eq!(
        ready["screens"],
        serde_json::json!([{"id": "kept", "name": "kept", "panes": 1}]),
        "{ready}"
    );
    assert_eq!(
        host_keepers(&lab),
        keepers,
        "the new daemon adopted the keeper"
    );
    assert_eq!(socket.next_json().await["type"], "mux.welcome");
    assert_ne!(host_daemon_pid(&lab), daemon, "a new daemon runs");
    // Its info file is gone before the process has quite ended.
    let deadline = std::time::Instant::now() + WAIT;
    while kill(Pid::from_raw(daemon), None).is_ok() {
        assert!(std::time::Instant::now() < deadline, "the old daemon ends");
        std::thread::sleep(Duration::from_millis(50));
    }
    socket.close().await;
    hub.wait_without_ssh();
}

/// The hub's `ssh` processes that hold a forward, not a bridge.
fn forward_ssh(hub: &HubServer) -> Vec<i32> {
    hub.ssh()
        .into_iter()
        .filter(|pid| {
            fs::read(format!("/proc/{pid}/cmdline"))
                .is_ok_and(|cmdline| cmdline.split(|b| *b == 0).any(|arg| arg == b"-L"))
        })
        .collect()
}

/// A request to the hub's `/api/remote/forwards` with its session cookie;
/// the status and the JSON body.
fn forwards(hub: &HubServer, method: &str, body: Option<&str>) -> (u16, serde_json::Value) {
    hub_json(hub, method, "/api/remote/forwards", body)
}

fn preferences(hub: &HubServer, method: &str, body: Option<&str>) -> (u16, serde_json::Value) {
    hub_json(hub, method, "/api/remote/forward-preferences", body)
}

fn hub_json(
    hub: &HubServer,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, serde_json::Value) {
    let (status, response) = http(hub.port, method, path, Some(&hub.cookie), body);
    let body = response.split_once("\r\n\r\n").map_or("", |(_, body)| body);
    let json = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(body).unwrap_or_else(|e| panic!("not JSON ({e}): {response}"))
    };
    (status, json)
}

/// `GET /` of a server through a forward's local `port`.
fn served_page(port: u16) -> u16 {
    http(port, "GET", "/", None, None).0
}

/// Stops only forward ProxyCommands before they exec sshd: even the SSH
/// banner/authentication cannot have completed when `entered` appears.
/// The script has its own ten-second deadline and never writes SSH stdout.
struct AuthGate {
    directory: PathBuf,
}

impl AuthGate {
    fn arm(lab: &Lab) -> Self {
        let directory = lab.dir.path().join("gate");
        fs::create_dir_all(&directory).expect("gate directory");
        for name in ["entered", "release", "passed", "ssh.log"] {
            let _ = fs::remove_file(directory.join(name));
        }
        fs::write(directory.join("armed"), b"").expect("arm gate");
        Self { directory }
    }

    async fn entered(&self, hub: &HubServer) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.directory.join("entered").exists() {
            assert!(
                Instant::now() < deadline,
                "SSH never entered the pre-auth gate"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!self.directory.join("passed").exists(), "gate passed early");
        let children = forward_ssh(hub);
        assert_eq!(children.len(), 1, "one real forward ssh: {children:?}");
        assert!(kill(Pid::from_raw(children[0]), None).is_ok());
    }

    fn release(&self) {
        fs::write(self.directory.join("release"), b"").expect("release auth gate");
    }

    fn ssh_log(&self) -> String {
        fs::read_to_string(self.directory.join("ssh.log")).expect("OpenSSH debug log")
    }
}

impl Drop for AuthGate {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.directory.join("armed"));
        let _ = fs::write(self.directory.join("release"), b"");
    }
}

/// An owned rival service. It also accepts readiness probes with no request,
/// so the old child-alive/TCP-alive implementation would be fooled here.
struct CompetingListener {
    stop: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CompetingListener {
    fn bind(port: u16) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", port)).expect("win IPv4 handoff");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let (stop, stopped) = mpsc::channel();
        let thread = std::thread::spawn(move || loop {
            if !matches!(stopped.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                break;
            }
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let timeout = Some(Duration::from_millis(200));
                    stream.set_read_timeout(timeout).expect("read deadline");
                    stream.set_write_timeout(timeout).expect("write deadline");
                    let mut request = [0; 4096];
                    if matches!(stream.read(&mut request), Ok(size) if size > 0) {
                        let _ = stream.write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\nConnection: close\r\n\r\nCOMPETING_IPV4_ONLY!",
                        );
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("competing listener: {error}"),
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for CompetingListener {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn expected_forward(remote: u16, local: u16) -> serde_json::Value {
    serde_json::json!({
        "target": HOST, "remote": remote, "local": local,
        "url": format!("http://localhost:{local}"), "state": "up",
    })
}

/// Own only the adapter's process group. Successful exit is reaped by the
/// waiter; timeout, failed stdin handoff and unwinding kill/reap this group.
struct BrowserChild {
    child: Child,
    reaped: bool,
}

impl Drop for BrowserChild {
    fn drop(&mut self) {
        if !self.reaped {
            if let Ok(pid) = i32::try_from(self.child.id()) {
                let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Opt-in smoke adapter for D8. Only real Chrome, the actual UI/API and SSH
/// can establish browser activation and the cross-window navigation boundary.
/// No opt-in means UNRUN; an opted-in unavailable dependency is a test failure.
#[test]
fn the_browser_verifies_open_and_cross_window_stop_through_real_ssh() {
    let (module, chrome) = match (
        env::var_os("SPLITTY_TEST_PLAYWRIGHT_MODULE"),
        env::var_os("SPLITTY_TEST_CHROME"),
    ) {
        (None, None) => {
            eprintln!("browser UNRUN: set both SPLITTY_TEST_PLAYWRIGHT_MODULE and SPLITTY_TEST_CHROME to opt in");
            return;
        }
        (Some(module), Some(chrome)) => (PathBuf::from(module), PathBuf::from(chrome)),
        _ => panic!("browser BLOCKED: both opt-in variables are required"),
    };
    for path in [&module, &chrome] {
        assert!(
            path.is_absolute() && path.is_file(),
            "browser BLOCKED: dependency must be an existing absolute file"
        );
    }
    let node = which("node").expect("browser BLOCKED: node is required");
    let python = which("python3").expect("browser BLOCKED: python3 is required");
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/testing/remote-forwards-browser.mjs");
    assert!(
        script.is_file(),
        "browser BLOCKED: integrate the browser adapter before opting in"
    );
    // Unlike lab(), opted-in prerequisites cannot silently become a green skip.
    let lab = Lab::start().unwrap_or_else(|reason| panic!("browser BLOCKED: {reason}"));
    let marker = format!("splitty-browser-{:032x}", rand::random::<u128>());
    let service = lab.marker_on_host(&python, &marker, None);
    let openssl = which("openssl");
    let api_marker = format!("{marker}-api");
    let api_service = lab.marker_on_host(&python, &api_marker, openssl.as_deref());
    let tls_ca = openssl.as_ref().map(|_| {
        fs::read_to_string(api_service._directory.path().join("cert.pem"))
            .expect("TLS fixture: public certificate")
    });
    if tls_ca.is_none() {
        eprintln!("TLS UNRUN: openssl absent; API transport will use plain HTTP");
    }
    let payload = lab.payload();
    let hub = HubServer::start(&lab, &payload);
    assert_ne!(hub.port, 44877, "browser BLOCKED: reserved hub port");
    assert_eq!(forwards(&hub, "GET", None), (200, serde_json::json!([])));
    assert_eq!(
        preferences(&hub, "GET", None),
        (200, serde_json::json!({"version": 1, "forwards": []}))
    );
    assert!(
        hub.ssh().is_empty(),
        "fresh hub must have no host connection"
    );
    let local = free_local_port();
    assert!(
        local >= 1024 && local != 44877 && local != hub.port,
        "browser BLOCKED: reserved local port"
    );
    // Recheck strictly: permission/unknown IPv6 errors are failures, not an
    // absent family. Release the reservation before the real backend POST.
    let frontend_reservation = splitty_remote::tunnel::reserve_exact(local)
        .expect("browser BLOCKED: local port must be free on both supported families");
    let api_local = free_local_port();
    assert!(api_local >= 1024 && api_local != 44877 && api_local != hub.port);
    drop(splitty_remote::tunnel::reserve_exact(api_local).expect("API local port availability"));
    drop(frontend_reservation);
    let input = serde_json::to_vec(&serde_json::json!({
        "hubUrl": format!("http://127.0.0.1:{}", hub.port),
        "cookie": hub.cookie, "target": HOST, "remote": service.port,
        "local": local, "marker": marker, "playwrightModule": module,
        "executablePath": chrome,
        "apiService": {"remote": api_service.port, "local": api_local,
            "marker": api_marker, "tlsCa": tls_ca},
    }))
    .expect("serialize private browser configuration");
    assert!(
        input.len() <= 16384,
        "browser BLOCKED: configuration exceeds adapter limit"
    );
    let output = lab.dir.path().join("browser-report.json");
    let stderr = lab.dir.path().join("browser.stderr");
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut adapter = BrowserChild {
        child: Command::new(node)
            .arg(script)
            .args(["--lab-only", "--config", "-"])
            .env_remove("DEBUG")
            .env_remove("PWDEBUG")
            .env_remove("DEBUG_FILE")
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::from(
                fs::File::create(&output).expect("private browser report"),
            ))
            .stderr(Stdio::from(
                fs::File::create(stderr).expect("private browser stderr"),
            ))
            .spawn()
            .expect("browser BLOCKED: cannot spawn node adapter"),
        reaped: false,
    };
    let mut stdin = adapter.child.stdin.take().expect("private browser stdin");
    // Include a stalled stdin consumer in the same 120-second bound. Files
    // capture output without pipe backpressure; neither stream is dumped raw.
    nix::fcntl::fcntl(
        stdin.as_raw_fd(),
        nix::fcntl::FcntlArg::F_SETFL(nix::fcntl::OFlag::O_NONBLOCK),
    )
    .expect("nonblocking adapter stdin");
    let mut remaining = input.as_slice();
    while !remaining.is_empty() {
        assert!(
            Instant::now() < deadline,
            "browser FAILED: adapter stdin exceeded 120 seconds"
        );
        match stdin.write(remaining) {
            Ok(0) => panic!("browser FAILED: adapter stdin closed"),
            Ok(written) => remaining = &remaining[written..],
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => panic!("browser FAILED: private stdin handoff failed"),
        }
    }
    drop(stdin);
    let status = loop {
        assert!(
            Instant::now() < deadline,
            "browser FAILED: adapter exceeded 120 seconds"
        );
        if let Some(status) = adapter.child.try_wait().expect("wait for browser adapter") {
            adapter.reaped = true;
            break status;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(
        fs::metadata(&output)
            .expect("browser report metadata")
            .len()
            <= 65536,
        "browser FAILED: oversized report"
    );
    let report: serde_json::Value =
        serde_json::from_reader(fs::File::open(output).expect("browser report"))
            .expect("browser FAILED: adapter did not return JSON");
    // Print only the adapter's documented redacted report, never stderr or
    // configuration. Defensive redaction also covers accidental echoing.
    assert!(
        !report.to_string().contains(&hub.cookie),
        "browser FAILED: report contained a credential"
    );
    eprintln!("browser adapter: {report}");
    assert!(
        status.success(),
        "browser FAILED/BLOCKED: adapter exit {status}"
    );
    assert_eq!(report["status"], "pass", "browser did not pass");
    assert_eq!(
        report["cleanup"]["status"], "pass",
        "browser cleanup did not pass"
    );
    let checks = report["checks"]
        .as_array()
        .expect("browser scenario results");
    assert_eq!(
        checks.len(),
        3,
        "browser and service transport scenarios are required"
    );
    for scenario in [
        "verified-open",
        "cross-window-stale-response",
        "manual-services-transport",
    ] {
        assert!(
            checks.iter().any(|check| check["scenario"] == scenario),
            "browser scenario missing: {scenario}"
        );
    }
    let transport = &checks[2];
    assert_eq!(transport["manualSavedServices"], 2);
    assert_eq!(transport["rawWebSocket"], true);
    assert_eq!(transport["hmrStyleUpdate"], true);
    assert_eq!(transport["apiAuthAndBytes"], true);
    assert_eq!(
        transport["tls"]["status"],
        if openssl.is_some() { "pass" } else { "unrun" }
    );
    assert_eq!(forwards(&hub, "GET", None), (200, serde_json::json!([])));
    assert_eq!(
        preferences(&hub, "GET", None),
        (200, serde_json::json!({"version": 1, "forwards": []}))
    );
    hub.wait_without_ssh();
}

/// Smoke-level D7 regression: a real OpenSSH child has reached ProxyCommand,
/// but sshd has not even started. TCP reachability cannot authorize success.
/// The positive control uses the identical gate and actual host service.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_hub_waits_for_real_ssh_bind_after_the_pre_auth_gate() {
    let Some(lab) = lab() else { return };
    let service = lab.serve_on_host(0);
    let payload = lab.payload();
    let hub = HubServer::start(&lab, &payload);
    let local = free_local_port();
    let key = serde_json::json!({"target": HOST, "port": service.port}).to_string();
    let saved = serde_json::json!({
        "target": HOST, "port": service.port, "local": local, "label": "D7 service",
    })
    .to_string();
    assert_eq!(preferences(&hub, "PUT", Some(&saved)).0, 200);
    assert_eq!(forwards(&hub, "DELETE", Some(&key)).0, 204);
    let original = lab.saved_forwards();
    assert_eq!(original["forwards"][0]["enabled"], false);

    // First fail with a rival, then use exactly the same saved mapping without
    // one. Success cannot be explained by a gate that never reaches real SSH.
    for competing in [true, false] {
        let gate = AuthGate::arm(&lab);
        let port = hub.port;
        let cookie = hub.cookie.clone();
        let body = key.clone();
        let mut opening = tokio::task::spawn_blocking(move || {
            http(
                port,
                "POST",
                "/api/remote/forwards",
                Some(&cookie),
                Some(&body),
            )
        });
        gate.entered(&hub).await;
        let ssh = forward_ssh(&hub);
        let competitor = competing.then(|| CompetingListener::bind(local));
        if competing {
            assert_eq!(health(local), "COMPETING_IPV4_ONLY!");
        } else {
            assert!(listeners(local).is_empty(), "SSH has not bound before auth");
        }
        // An observation window, not the ordering mechanism: the marker and
        // successful rival bind above establish the reservation handoff.
        assert!(
            tokio::time::timeout(Duration::from_millis(300), &mut opening)
                .await
                .is_err(),
            "POST completed before authentication (competing={competing})"
        );
        assert_eq!(forwards(&hub, "GET", None), (200, serde_json::json!([])));
        assert_eq!(lab.saved_forwards(), original, "no premature resume");
        assert!(!gate.directory.join("passed").exists());
        assert!(
            !gate.ssh_log().contains("Authenticated to "),
            "authentication is gated"
        );

        gate.release();
        let (status, response) = tokio::time::timeout(WAIT, opening)
            .await
            .expect("forward request deadline")
            .expect("request worker");
        assert!(
            gate.directory.join("passed").exists(),
            "real sshd was released"
        );
        let body: serde_json::Value =
            serde_json::from_str(response.split_once("\r\n\r\n").expect("response body").1)
                .expect("forward JSON");
        if competing {
            let log = gate.ssh_log();
            assert!(
                log.contains("Authenticated to "),
                "OpenSSH did not authenticate: {log}"
            );
            assert!(
                log.contains(&format!("bind [127.0.0.1]:{local}:"))
                    && log.contains("Address already in use")
                    && log.contains("Could not request local forwarding"),
                "OpenSSH must fail its actual local bind, not just time out: {log}"
            );
            assert_eq!(status, 409, "{response}");
            assert_eq!(body["code"], "local_port_conflict", "{body}");
            assert_eq!(forwards(&hub, "GET", None), (200, serde_json::json!([])));
            hub.wait_without_ssh();
            assert_eq!(lab.saved_forwards(), original, "failed resume stays paused");
            let (status, saved) = preferences(&hub, "GET", None);
            assert_eq!(status, 200);
            assert_eq!(saved["forwards"][0]["enabled"], false);
            assert_eq!(saved["forwards"][0]["local"], local);
            assert_eq!(
                saved["forwards"][0]["lastError"]["code"],
                "local_port_conflict"
            );
            assert_eq!(
                health(local),
                "COMPETING_IPV4_ONLY!",
                "rival survives failure"
            );
        } else {
            assert!(
                gate.ssh_log().contains("Authenticated to "),
                "positive control authenticated"
            );
            assert_eq!(status, 200, "{response}");
            assert_eq!(body, expected_forward(service.port, local));
            assert_eq!(
                forward_ssh(&hub),
                ssh,
                "the authenticated child owns the bind"
            );
            assert_eq!(listeners(local), [format!("0100007F:{local:04X}")]);
            assert_eq!(health(local), "ok", "bytes reach the actual remote service");
            let (_, saved) = preferences(&hub, "GET", None);
            assert_eq!(saved["forwards"][0]["enabled"], true);
            assert!(saved["forwards"][0].get("lastError").is_none());
            assert_eq!(saved, lab.saved_forwards());
            assert_eq!(forwards(&hub, "DELETE", Some(&key)).0, 204);
            hub.wait_without_ssh();
        }
        drop(competitor);
        assert!(listeners(local).is_empty(), "owned listener is released");
    }
}

fn saved_document(mut entries: Vec<serde_json::Value>) -> serde_json::Value {
    entries.sort_by_key(|entry| {
        (
            entry["target"].as_str().expect("target").to_owned(),
            entry["remote"].as_u64().expect("remote"),
        )
    });
    serde_json::json!({"version": 1, "forwards": entries})
}

async fn wait_for_active(hub: &HubServer, expected: &serde_json::Value) {
    let deadline = Instant::now() + WAIT;
    loop {
        let (status, actual) = forwards(hub, "GET", None);
        assert_eq!(status, 200);
        if actual == *expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "expected {expected}, got {actual}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn assert_passive(hub: &HubServer, lab: &Lab, invocations: &str, saved: &serde_json::Value) {
    let deadline = Instant::now() + Duration::from_millis(300);
    loop {
        assert_eq!(preferences(hub, "GET", None), (200, saved.clone()));
        assert_eq!(forwards(hub, "GET", None), (200, serde_json::json!([])));
        assert_eq!(lab.ssh_invocations(), invocations, "passive work ran ssh");
        assert!(hub.ssh().is_empty());
        assert_eq!(lab.saved_forwards(), *saved);
        if Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// No pane or discovery trigger: browsing alone sends remote.ready, then the
/// host's ordinary welcome/empty port list. Extra hub frames fail this check.
async fn browse_empty_host(hub: &HubServer) -> HubSocket {
    tokio::time::timeout(WAIT, async {
        let mut socket = hub.open(HOST).await;
        let (_, ready) = socket.outcome().await;
        assert_eq!(ready["type"], "remote.ready", "{ready}");
        assert_eq!(ready["screens"], serde_json::json!([]));
        assert_eq!(socket.next_json().await["type"], "mux.welcome");
        let ports = socket.next_json().await;
        assert_eq!(ports["type"], "ports.changed", "{ports}");
        assert_eq!(
            ports["ports"],
            serde_json::json!([]),
            "manual services are undiscovered"
        );
        socket
    })
    .await
    .expect("browse host deadline")
}

/// Restore is asynchronous after ready. Observe after it completes as well,
/// allowing ordinary host traffic but rejecting delayed hub-owned frames.
async fn assert_no_extra_hub_frames(socket: &mut HubSocket) {
    use futures::StreamExt;

    let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
    loop {
        if tokio::time::Instant::now() >= deadline {
            return;
        }
        match tokio::time::timeout_at(deadline, socket.0.next()).await {
            Err(_) => return,
            Ok(Some(Ok(Message::Text(text)))) => {
                let message: serde_json::Value = serde_json::from_str(&text).expect("host JSON");
                assert!(
                    !message["type"]
                        .as_str()
                        .expect("message type")
                        .starts_with("remote."),
                    "unexpected hub frame after remote.ready and restoration: {message}"
                );
            }
            Ok(Some(Ok(Message::Close(frame)))) => panic!("host relay closed: {frame:?}"),
            Ok(Some(Ok(_))) => {}
            other => panic!("host relay failed while observing restoration: {other:?}"),
        }
    }
}

/// Smoke-level restart/lazy restore: only a new hub process reading the same
/// XDG store and a real host connection demonstrate these lifecycle rules.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saved_forwards_restore_lazily_after_real_hub_restart_and_forget_stays_live() {
    let Some(lab) = lab() else { return };
    let frontend = lab.serve_on_host(0);
    let api = lab.serve_on_host(0);
    let payload = lab.payload();
    let before_start = lab.ssh_invocations();
    let hub = HubServer::start(&lab, &payload);
    // Keep ports distinct while selecting; drop every reservation before API
    // work, so passive saves cannot hide an attempted bind behind a conflict.
    let reservations: Vec<_> = (0..3)
        .map(|_| TcpListener::bind(("127.0.0.1", 0)).expect("choose local port"))
        .collect();
    let locals: Vec<_> = reservations
        .iter()
        .map(|listener| listener.local_addr().expect("local address").port())
        .collect();
    for port in &locals {
        splitty_remote::tunnel::check_forward_address(*port).expect("IPv6 availability");
    }
    drop(reservations);
    let mut entries = Vec::new();
    for (target, remote, local, label) in [
        (HOST, frontend.port, locals[0], "Frontend"),
        (HOST, api.port, locals[1], "API"),
        (NOWHERE, frontend.port, locals[2], "Other host"),
    ] {
        let request =
            serde_json::json!({"target": target, "port": remote, "local": local, "label": label});
        let expected = serde_json::json!({
            "target": target, "remote": remote, "local": local, "label": label, "enabled": true,
        });
        assert_eq!(
            preferences(&hub, "PUT", Some(&request.to_string())),
            (200, expected.clone())
        );
        entries.push(expected);
    }
    let api_key = serde_json::json!({"target": HOST, "port": api.port}).to_string();
    assert_eq!(forwards(&hub, "DELETE", Some(&api_key)).0, 204);
    entries[1]["enabled"] = false.into();
    let edit = serde_json::json!({"target": HOST, "port": api.port, "label": "Paused API"});
    entries[1]["label"] = "Paused API".into();
    assert_eq!(
        preferences(&hub, "PUT", Some(&edit.to_string())),
        (200, entries[1].clone())
    );
    let saved = saved_document(entries.clone());
    assert_passive(&hub, &lab, &before_start, &saved).await;
    drop(hub);

    let hub = HubServer::start(&lab, &payload);
    assert_passive(&hub, &lab, &before_start, &saved).await;
    for port in &locals {
        let _unreserved =
            TcpListener::bind(("127.0.0.1", *port)).expect("passive port is unreserved");
    }
    let automatic = serde_json::json!({"target": HOST, "port": api.port, "automatic": true});
    let (status, error) = forwards(&hub, "POST", Some(&automatic.to_string()));
    assert_eq!(status, 409, "{error}");
    assert_eq!(error["code"], "forward_paused");
    assert_eq!(
        lab.ssh_invocations(),
        before_start,
        "stale automatic request did no SSH"
    );

    let mut socket = browse_empty_host(&hub).await;
    let frontend_forward = expected_forward(frontend.port, locals[0]);
    wait_for_active(&hub, &serde_json::json!([frontend_forward])).await;
    assert_no_extra_hub_frames(&mut socket).await;
    assert_eq!(health(locals[0]), "ok");
    assert!(
        listeners(locals[1]).is_empty(),
        "paused mapping stays stopped"
    );
    assert!(
        listeners(locals[2]).is_empty(),
        "other host stays disconnected"
    );
    assert_eq!(preferences(&hub, "GET", None), (200, saved.clone()));
    assert_eq!(
        lab.ssh_invocations()
            .lines()
            .filter(|line| *line == "forward")
            .count(),
        1
    );
    let frontend_ssh = forward_ssh(&hub);
    assert_eq!(frontend_ssh.len(), 1);
    socket.close().await;

    // A new real readiness event neither resumes the pause nor duplicates the
    // running mapping. The first forward survives losing its browsing socket.
    let mut socket = browse_empty_host(&hub).await;
    wait_for_active(&hub, &serde_json::json!([frontend_forward])).await;
    assert_no_extra_hub_frames(&mut socket).await;
    assert_eq!(forward_ssh(&hub), frontend_ssh);
    assert_eq!(preferences(&hub, "GET", None), (200, saved));
    assert_eq!(
        lab.ssh_invocations()
            .lines()
            .filter(|line| *line == "forward")
            .count(),
        1
    );

    // Forget changes metadata only, including on repeated calls.
    let frontend_key = serde_json::json!({"target": HOST, "port": frontend.port}).to_string();
    for _ in 0..2 {
        assert_eq!(preferences(&hub, "DELETE", Some(&frontend_key)).0, 204);
        assert_eq!(forward_ssh(&hub), frontend_ssh);
        assert_eq!(
            forwards(&hub, "GET", None),
            (200, serde_json::json!([frontend_forward]))
        );
        assert_eq!(
            health(locals[0]),
            "ok",
            "Forget leaves actual transport alive"
        );
    }
    entries.remove(0);
    let forgotten = saved_document(entries.clone());
    assert_eq!(preferences(&hub, "GET", None), (200, forgotten.clone()));
    assert_eq!(lab.saved_forwards(), forgotten);
    socket.close().await;
    drop(hub);
    let before_restart = lab.ssh_invocations();
    let hub = HubServer::start(&lab, &payload);
    assert_passive(&hub, &lab, &before_restart, &forgotten).await;
    let mut socket = browse_empty_host(&hub).await;
    assert_no_extra_hub_frames(&mut socket).await;
    assert_eq!(forwards(&hub, "GET", None), (200, serde_json::json!([])));
    assert!(
        forward_ssh(&hub).is_empty(),
        "neither forgotten nor paused restores"
    );
    assert_eq!(
        lab.ssh_invocations()
            .lines()
            .filter(|line| *line == "forward")
            .count(),
        1
    );
    assert_eq!(forwards(&hub, "DELETE", Some(&frontend_key)).0, 404);

    // Explicit Resume is the remaining consent that can enable the paused API.
    let (status, resumed) = forwards(&hub, "POST", Some(&api_key));
    assert_eq!(status, 200, "{resumed}");
    assert_eq!(resumed, expected_forward(api.port, locals[1]));
    assert_eq!(health(locals[1]), "ok");
    entries[0]["enabled"] = true.into();
    assert_eq!(
        preferences(&hub, "GET", None),
        (200, saved_document(entries.clone()))
    );
    assert_eq!(lab.saved_forwards(), saved_document(entries));
    assert_eq!(forwards(&hub, "DELETE", Some(&api_key)).0, 204);
    assert!(forward_ssh(&hub).is_empty(), "Stop reaps the forward");
    assert!(listeners(locals[1]).is_empty());
    socket.close().await;
    hub.wait_without_ssh();
}

/// mux-protocol, Listening Ports, and remote-screens, Port Forwards Through
/// The Hub (Forward a dev server, Forward rebuilt), with access-control, Hub
/// Port Forwards (GatewayPorts in the SSH configuration), through the real
/// binary, real `ssh` and a real host daemon: a server a host pane starts
/// is reported through the relay with its session; the hub forwards its port
/// on `127.0.0.1` alone although the lab's configuration sets `GatewayPorts
/// yes`, at `http://localhost:<port>`; a killed forward `ssh` is rebuilt on
/// the same port; the server's end is reported; `DELETE` ends the forward's
/// `ssh` and frees the port; a forward ends with `serve` on SIGTERM too (a
/// smoke test: only the real process shows what a signal leaves behind).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_hub_forwards_a_port_a_host_pane_listens_on() {
    let Some(lab) = lab() else { return };
    let payload = lab.payload();
    let mut hub = HubServer::start(&lab, &payload);
    let port = free_local_port();

    let mut socket = hub.open(HOST).await;
    let (_, ready) = socket.outcome().await;
    assert_eq!(ready["type"], "remote.ready", "{ready}");
    assert_eq!(socket.next_json().await["type"], "mux.welcome");
    let greeting = socket.next_json().await;
    assert_eq!(greeting["type"], "ports.changed", "{greeting}");
    assert_eq!(greeting["ports"], serde_json::json!([]), "{greeting}");

    // A dev server in a pane of the host.
    socket
        .send(
            r#"{"type":"mux.op","screenId":"ports","op":{"type":"create_screen","screenId":"ports","name":"ports","newPaneId":"ports-pane"}}"#,
        )
        .await;
    let session = loop {
        let message = socket.next_json().await;
        if message["type"] == "mux.pane_spawned" {
            break message["sessionId"].as_str().expect("session").to_owned();
        }
    };
    let command = format!("python3 -m http.server --bind 127.0.0.1 {port}\r");
    socket
        .send(&serde_json::json!({"type": "input", "id": session, "data": command}).to_string())
        .await;
    let listed = loop {
        let message = socket.next_json().await;
        if message["type"] != "ports.changed" {
            continue;
        }
        if let Some(listed) = message["ports"]
            .as_array()
            .and_then(|ports| ports.iter().find(|listed| listed["port"] == port))
        {
            break listed.clone();
        }
    };
    assert_eq!(listed["sessionId"], session, "{listed}");
    assert!(
        listed["process"]
            .as_str()
            .is_some_and(|process| process.starts_with("python")),
        "{listed}"
    );

    // Forwarded to the same port here, on 127.0.0.1 only.
    let body = format!(r#"{{"target":"{HOST}","port":{port}}}"#);
    let (status, forward) = forwards(&hub, "POST", Some(&body));
    assert_eq!(status, 200, "{forward}");
    assert_eq!(
        forward,
        serde_json::json!({
            "target": HOST, "remote": port, "local": port,
            "url": format!("http://localhost:{port}"), "state": "up",
        })
    );
    assert_eq!(
        listeners(port),
        [format!("0100007F:{port:04X}")],
        "a forward must listen on 127.0.0.1 only"
    );
    assert_eq!(served_page(port), 200);

    // Its ssh dies; the forward comes back on the same port.
    let killed = forward_ssh(&hub);
    assert_eq!(killed.len(), 1, "one ssh holds the forward: {killed:?}");
    kill(Pid::from_raw(killed[0]), Signal::SIGKILL).expect("kill the forward's ssh");
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        let rebuilt = forward_ssh(&hub);
        let (_, list) = forwards(&hub, "GET", None);
        if list[0]["state"] == "up" && !rebuilt.is_empty() && !rebuilt.contains(&killed[0]) {
            assert_eq!(list, serde_json::json!([forward]), "the same forward");
            break;
        }
        assert!(std::time::Instant::now() < deadline, "not rebuilt: {list}");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(served_page(port), 200);

    // The server ends: the host reports it.
    socket
        .send(&serde_json::json!({"type": "input", "id": session, "data": "\u{3}"}).to_string())
        .await;
    loop {
        let message = socket.next_json().await;
        if message["type"] == "ports.changed" {
            assert_eq!(message["ports"], serde_json::json!([]), "{message}");
            break;
        }
    }

    let (status, _) = forwards(&hub, "DELETE", Some(&body));
    assert_eq!(status, 204);
    assert_eq!(
        forward_ssh(&hub),
        Vec::<i32>::new(),
        "the forward's ssh ended"
    );
    assert_eq!(listeners(port), Vec::<String>::new());
    let (_, list) = forwards(&hub, "GET", None);
    assert_eq!(list, serde_json::json!([]));

    // A forward does not outlive `serve` ended with SIGTERM.
    let (status, again) = forwards(&hub, "POST", Some(&body));
    assert_eq!(status, 200, "{again}");
    let held = forward_ssh(&hub);
    assert_eq!(held.len(), 1, "one ssh holds the forward: {held:?}");
    socket.close().await;
    kill(Pid::from_raw(hub.child.id() as i32), Signal::SIGTERM).expect("SIGTERM serve");
    let deadline = std::time::Instant::now() + WAIT;
    while hub.child.try_wait().expect("wait for serve").is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "serve did not end on SIGTERM"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    while Path::new(&format!("/proc/{}", held[0])).exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the forward's ssh outlived serve"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(listeners(port), Vec::<String>::new());
}

// ── Reverse forwards ────────────────────────────────────────────────────────

/// The hub's `ssh` processes that hold a reverse forward.
fn reverse_ssh(hub: &HubServer) -> Vec<i32> {
    hub.ssh()
        .into_iter()
        .filter(|pid| {
            fs::read(format!("/proc/{pid}/cmdline"))
                .is_ok_and(|cmdline| cmdline.split(|b| *b == 0).any(|arg| arg == b"-R"))
        })
        .collect()
}

fn reverses(hub: &HubServer, method: &str, body: Option<&str>) -> (u16, serde_json::Value) {
    hub_json(hub, method, "/api/remote/reverse-forwards", body)
}

/// What a program on the host gets from `address:port` there: the first
/// line, or why it got none.
fn from_host(lab: &Lab, address: &str, port: u16) -> String {
    // `ssh` joins the words into one line for the host's shell: the script
    // is one single-quoted word, without single quotes of its own.
    let script = format!(
        "'import socket\n\
         try:\n s = socket.create_connection((\"{address}\", {port}), timeout=10)\n \
         print(s.makefile().readline().strip() or \"closed\")\n\
         except OSError as e:\n print(\"failed:\", e)\n'"
    );
    let out = lab.on_host(&["python3", "-c", &script]);
    assert!(out.status.success(), "{}", describe(&out));
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// The host's listeners on `port`, as its `/proc/net/tcp` and `tcp6` list
/// them.
fn host_listeners(lab: &Lab, port: u16) -> Vec<String> {
    let out = lab.on_host(&["cat", "/proc/net/tcp", "/proc/net/tcp6"]);
    let suffix = format!(":{port:04X}");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (local, state) = (fields.get(1)?, fields.get(3)?);
            (local.ends_with(&suffix) && *state == "0A").then(|| local.to_string())
        })
        .collect()
}

/// access-control, Hub Reverse Forwards; remote-screens, Reverse Forwards
/// Through The Hub: a local service shared with a host through real `ssh -R`
/// and the host's relay, on loopback only although the host's `sshd` says
/// `GatewayPorts yes`, rebuilt when its `ssh` dies, and gone after Stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shared_port_reaches_a_local_service_from_the_host_through_real_ssh() {
    let Some(lab) = lab() else { return };
    if which("python3").is_none() {
        eprintln!("skipping: no python3 to connect from the host");
        return;
    }
    let payload = lab.payload();
    let hub = HubServer::start(&lab, &payload);
    let service = TcpListener::bind(("127.0.0.1", 0)).expect("local service");
    let local = service.local_addr().expect("address").port();
    std::thread::spawn(move || {
        for mut stream in service.incoming().flatten() {
            let _ = stream.write_all(b"local-service\n");
        }
    });
    let port = 18431;
    let share = serde_json::json!({"target": HOST, "port": port, "local": local}).to_string();

    let (status, shared) = reverses(&hub, "POST", Some(&share));
    assert_eq!(status, 200, "{shared}");
    assert_eq!(
        shared,
        serde_json::json!({"target": HOST, "remote": port, "local": local, "state": "up"})
    );
    assert_eq!(from_host(&lab, "127.0.0.1", port), "local-service");
    let listening = host_listeners(&lab, port);
    assert!(!listening.is_empty(), "the relay listens");
    for address in &listening {
        assert!(
            address.starts_with("0100007F:")
                || address.starts_with("00000000000000000000000001000000:"),
            "only loopback, whatever GatewayPorts says: {listening:?}"
        );
    }
    if listening.len() == 2 {
        assert_eq!(from_host(&lab, "::1", port), "local-service");
    }
    let relay_dir = lab.dir.path().join("home/.local/share/splitty/run/relay");
    let mode = fs::metadata(&relay_dir)
        .expect("relay directory")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);
    let sockets: Vec<_> = fs::read_dir(&relay_dir)
        .expect("relay directory")
        .flatten()
        .map(|entry| entry.path())
        .collect();
    assert_eq!(sockets.len(), 1, "{sockets:?}");
    let socket_mode = fs::symlink_metadata(&sockets[0])
        .expect("socket")
        .permissions()
        .mode();
    assert_eq!(socket_mode & 0o077, 0, "owner-only socket");

    // A killed `ssh` ends the relay with its session, and the hub shares the
    // port again.
    let ssh = reverse_ssh(&hub);
    assert_eq!(ssh.len(), 1, "one ssh per shared port");
    kill(Pid::from_raw(ssh[0]), Signal::SIGKILL).expect("kill ssh");
    let deadline = Instant::now() + WAIT;
    loop {
        let (_, list) = reverses(&hub, "GET", None);
        let pids = reverse_ssh(&hub);
        if list[0]["state"] == "up" && !pids.is_empty() && pids != ssh {
            break;
        }
        assert!(Instant::now() < deadline, "rebuilt: {list}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(from_host(&lab, "127.0.0.1", port), "local-service");

    let key = serde_json::json!({"target": HOST, "port": port}).to_string();
    assert_eq!(reverses(&hub, "DELETE", Some(&key)).0, 204);
    let deadline = Instant::now() + WAIT;
    while !host_listeners(&lab, port).is_empty() {
        assert!(Instant::now() < deadline, "the relay ends after Stop");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(from_host(&lab, "127.0.0.1", port).starts_with("failed:"));
    let deadline = Instant::now() + WAIT;
    while fs::read_dir(&relay_dir)
        .expect("relay directory")
        .next()
        .is_some()
    {
        assert!(Instant::now() < deadline, "the relay removes its socket");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    hub.wait_without_ssh();
}
