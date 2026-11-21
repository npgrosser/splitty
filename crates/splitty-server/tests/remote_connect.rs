//! The remote screen hub against a real OpenSSH server.
//!
//! Smoke tests exercise the binary's owner backend, installed host daemon,
//! real SSH forwarding, reconnects, askpass and session preservation. Scripted
//! in-process connectors cannot establish these process and network outcomes.
//!
//! The SSH lab gives each test a host of its own without root:
//! - an unprivileged user and network namespace, so the host has its own
//!   loopback independently of the local hub;
//! - `sshd -i`, one per connection, started inside that namespace by the
//!   client's `ProxyCommand` through `nsenter`, with throwaway keys;
//! - a `ForceCommand` that gives the host its own `HOME` and XDG directories and
//!   runs the command through `sh -c`, as a login shell would;
//! - a wrapper `ssh` first in `PATH` that adds `-F <lab config>`, so the hub
//!   runs the plain `ssh` it always runs. The lab config sets `GatewayPorts yes`
//!   (and so does the host's `sshd_config`), and names two hosts that fail: one
//!   nothing listens for, and one whose host key is unknown;
//! - a configuration directory of the client's own, so the hub records its
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
use std::net::{TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Output, Stdio};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use nix::sys::signal::{kill, killpg, Signal};
use nix::unistd::Pid;
use tempfile::TempDir;

#[path = "support/listeners.rs"]
mod process_inventory;

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
    /// Pins the exact private namespace even after its initial holder exits.
    namespace: fs::File,
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
        for tool in [
            "ssh",
            "ssh-keygen",
            "unshare",
            "nsenter",
            "ip",
            "python3",
            "chroot",
        ] {
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
                 --user --net --mount --root --wd --preserve-credentials {sshd} -i -f {d_str}/sshd_config\n",
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
        let mut holder = Command::new("unshare")
            .args([
                "--user",
                "--map-current-user",
                "--keep-caps",
                "--mount",
                "--net",
                "sh",
                "-c",
            ])
            .arg(r#"
set -eu
lab=$1
package=$2
rootfs="$lab/rootfs"
ip link set lo up
# Call mount directly: a setuid mount helper drops the ambient capabilities
# of this unprivileged user namespace before it can bind its fixture root.
python3 - "$rootfs" "$lab" "$package" <<'PY_MOUNT'
import ctypes, os, sys
rootfs, lab, package = sys.argv[1:]
libc = ctypes.CDLL(None, use_errno=True)
libc.mount.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_ulong, ctypes.c_void_p]
def bind(source, recursive=False):
    destination = rootfs + source
    os.makedirs(destination, exist_ok=True)
    if libc.mount(os.fsencode(source), os.fsencode(destination), None, 4096 | (16384 if recursive else 0), None):
        raise OSError(ctypes.get_errno(), "bind fixture root", source)
os.makedirs(rootfs + "/tmp", exist_ok=True)
# Bind the lab before populating its rootfs, avoiding a recursive fixture.
bind(lab)
if package != "/": bind(package)
for system in ["/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc", "/dev", "/proc", "/home", "/run"]:
    # System trees can contain locked nested mounts (e.g. WSL libraries).
    # Recursive bind preserves those restrictions in an unprivileged namespace.
    if os.path.exists(system): bind(system, True)
PY_MOUNT
exec chroot "$rootfs" /bin/sh -c 'cd /; echo ready; exec sleep infinity'
"#).arg("splitty-ssh-lab").arg(d).arg(&root)
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

        let namespace = fs::File::open(format!("/proc/{}/ns/net", holder.id()))
            .expect("pin fixture network namespace");
        let lab = Self {
            dir,
            holder,
            namespace,
        };
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

    /// The host's daemon has a private socket and no TCP port.
    fn start_daemon(&self) {
        let start = self.on_host(&["splitty", "daemon", "start"]);
        assert!(
            start.status.success(),
            "daemon start on the host\n{}",
            describe(&start)
        );
        assert!(
            String::from_utf8_lossy(&start.stdout).contains("SPLITTY_CLIENT_SOCKET="),
            "{}",
            describe(&start)
        );
    }

    /// An ordinary HTTP service on the host's loopback, whose `/health`
    /// answers `ok`. It is independent of Splitty's terminal-service gateway
    /// and its exact Host/Origin policy. Only a forward reaches this namespace.
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
            .args([
                "--user",
                "--net",
                "--mount",
                "--root",
                "--wd",
                "--preserve-credentials",
            ])
            .args([
                "python3",
                "-u",
                "-c",
                r#"
import http.server, sys
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header('Content-Length', '2')
        self.send_header('Connection', 'close')
        self.end_headers()
        self.wfile.write(b'ok')
    def log_message(self, *args): pass
server = http.server.HTTPServer(('127.0.0.1', int(sys.argv[1])), Handler)
print('SPLITTY_PORT=' + str(server.server_port), flush=True)
server.serve_forever()
"#,
            ])
            .arg(port.to_string())
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

    /// The `splitty` under test on the client, with the lab's `ssh` and its
    /// own configuration directory.
    fn client(&self, args: &[&str]) -> Command {
        let mut command = self.client_command(env!("CARGO_BIN_EXE_splitty"));
        command.args(args);
        command
    }

    /// `program` on the client, in the environment of [`Lab::client`].
    fn client_command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(program);
        command
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

/// Resolve a saved PID only as a lookup hint. The pidfd is the signal target;
/// namespace membership and a live-handle check establish fixture ownership.
fn kill_fixture_namespace_member(pid: u32, namespace: &fs::File) -> io::Result<()> {
    use std::os::{fd::FromRawFd, unix::fs::MetadataExt};
    // SAFETY: pidfd_open takes a PID and zero flags; no pointers are supplied.
    let raw = unsafe { nix::libc::syscall(nix::libc::SYS_pidfd_open, pid, 0) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    let raw = i32::try_from(raw).expect("kernel returned a process descriptor");
    // SAFETY: successful pidfd_open returns a new descriptor owned by this call.
    let handle = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw) };
    let owner = namespace.metadata()?;
    let member = fs::metadata(format!("/proc/{pid}/ns/net"))?;
    if (owner.dev(), owner.ino()) != (member.dev(), member.ino()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "process is outside the fixture network namespace",
        ));
    }
    for signal in [0, nix::libc::SIGKILL] {
        // Verify that the opened generation is still live after the namespace
        // lookup. If its PID was reused during that lookup, this exact pidfd
        // reports exit and can never signal the replacement process.
        // SAFETY: the owned pidfd is valid, siginfo is absent and flags are zero.
        if unsafe {
            nix::libc::syscall(
                nix::libc::SYS_pidfd_send_signal,
                handle.as_raw_fd(),
                signal,
                std::ptr::null::<nix::libc::siginfo_t>(),
                0,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

impl Drop for Lab {
    fn drop(&mut self) {
        // Daemons and keepers detach from their SSH parent, but cannot leave
        // the private fixture network namespace accidentally. Pin that kernel
        // namespace and use pidfds for every signal, including after its holder
        // exits. Re-scan to collect descendants forked during the first pass.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let mut signaled = false;
            for entry in fs::read_dir("/proc")
                .expect("fixture process inventory")
                .flatten()
            {
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.parse::<u32>().ok())
                else {
                    continue;
                };
                if kill_fixture_namespace_member(pid, &self.namespace).is_ok() {
                    signaled = true;
                }
            }
            if !signaled || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.holder.kill();
        let _ = self.holder.wait();
    }
}

#[test]
fn fixture_cleanup_refuses_processes_outside_its_network_namespace() {
    let Some(mut lab) = lab() else { return };
    struct SeparateFixture(Child);
    impl Drop for SeparateFixture {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut unrelated = SeparateFixture(Command::new("sleep").arg("60").spawn().unwrap());
    assert_eq!(
        kill_fixture_namespace_member(unrelated.0.id(), &lab.namespace)
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    assert!(unrelated.0.try_wait().unwrap().is_none());
    let launched = lab.on_host(&[
        "sh",
        "-c",
        "'setsid sleep 60 </dev/null >/dev/null 2>&1 & echo $!'",
    ]);
    assert!(launched.status.success(), "{}", describe(&launched));
    let orphan: u32 = String::from_utf8(launched.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // SAFETY: pidfd_open receives only a fixture-reported PID and zero flags.
    let raw = unsafe { nix::libc::syscall(nix::libc::SYS_pidfd_open, orphan, 0) };
    assert!(raw >= 0, "{}", io::Error::last_os_error());
    use std::os::fd::FromRawFd;
    // SAFETY: this successful pidfd_open returned a new owned descriptor.
    let orphan_handle = unsafe { std::os::fd::OwnedFd::from_raw_fd(i32::try_from(raw).unwrap()) };
    kill_fixture_namespace_member(lab.holder.id(), &lab.namespace).unwrap();
    assert!(!lab.holder.wait().unwrap().success());
    drop(lab);
    let mut descriptor = nix::libc::pollfd {
        fd: orphan_handle.as_raw_fd(),
        events: nix::libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd is writable for the duration of this call.
    assert_eq!(
        unsafe { nix::libc::poll(&mut descriptor, 1, 5000) },
        1,
        "detached fixture process must exit even after the namespace holder"
    );
    assert!(unrelated.0.try_wait().unwrap().is_none());
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
    use std::sync::atomic::{AtomicU32, Ordering};
    // Keep fixture ports unique after probe/handoff and outside the kernel's
    // automatic range, where another parallel fixture could reuse a freed
    // number before the SSH-bind or final-release assertion observes it.
    static NEXT: AtomicU32 = AtomicU32::new(25000);
    #[cfg(target_os = "linux")]
    let automatic: Vec<u16> = std::fs::read_to_string("/proc/sys/net/ipv4/ip_local_port_range")
        .unwrap()
        .split_whitespace()
        .map(|number| number.parse().unwrap())
        .collect();
    loop {
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        assert!(number < 30000, "SSH fixture port pool exhausted");
        let port = u16::try_from(number).expect("bounded fixture port");
        #[cfg(target_os = "linux")]
        if (automatic[0]..=automatic[1]).contains(&port) {
            continue;
        }
        let Ok(_listener) = std::net::TcpListener::bind(("127.0.0.1", port)) else {
            continue;
        };
        let ipv6 = std::net::TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, port));
        if !matches!(ipv6, Err(error) if error.kind() == std::io::ErrorKind::AddrInUse) {
            return port;
        }
    }
}

/// The lab, or `None` after printing why the test is skipped.
fn lab() -> Option<Lab> {
    match Lab::start() {
        Ok(lab) => Some(lab),
        Err(reason) => {
            assert_ne!(
                std::env::var("SPLITTY_TEST_REQUIRE_SSH_LAB").as_deref(),
                Ok("1"),
                "required SSH lab could not start: {reason}"
            );
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
fn the_bridge_carries_native_frames_through_real_ssh() {
    let Some(lab) = lab() else { return };
    lab.install_splitty();
    lab.start_daemon();
    let info: serde_json::Value = serde_json::from_slice(
        &fs::read(lab.dir.path().join("home/.config/splitty/daemon.json")).unwrap(),
    )
    .unwrap();
    let mut command = Command::new("ssh");
    command.env("PATH", lab.path()).args([
        "-T",
        "-a",
        "-x",
        "-o",
        "ClearAllForwardings=yes",
        "-o",
        "BatchMode=yes",
        HOST,
        "--",
        "splitty",
        "remote-bridge",
        "--transport",
        "native-v1",
    ]);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        use splitty_transport::{admission, negotiation, wire};
        let mut bridge = splitty_remote::bridge::NativeBridge::spawn(
            tokio::process::Command::from(command),
            admission::Assignment::new(admission::Backend::new(), admission::Domain::new()),
            negotiation::Expected {
                instance: info["instance"].as_str(),
                purpose: &wire::Purpose::Direct,
                mux_protocol: 1,
                preparation: WAIT,
            },
            |_| {},
        )
        .await
        .unwrap();
        let frame = bridge.recv().await.unwrap();
        let welcome: serde_json::Value = frame.frame.decode().unwrap();
        assert_eq!(welcome["type"], "mux.welcome");
        for secret in ["token", "ticket", "bootstrap"] {
            assert!(!welcome.to_string().contains(secret));
        }
        drop(frame);
        let (protocol, ended) = bridge.finish().await;
        protocol.unwrap();
        // Finishing closes both directions at once. The host's bridge ends
        // by itself either way: cleanly when it reads the end of its input
        // first, with a failure of its own when it first finds its output
        // closed while the daemon still had a frame for the client. An
        // `ssh` that failed by itself would report 255.
        let status = ended.status.unwrap();
        assert!(
            !ended.forced && matches!(status.code(), Some(0 | 1)),
            "{status}, forced: {}, stderr: {}",
            ended.forced,
            ended.stderr
        );
    });
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

/// A local hub process, whose `ssh` children the tests watch.
trait HubProcess {
    fn pid(&self) -> u32;

    /// The `ssh` processes the hub runs now.
    fn ssh(&self) -> Vec<i32> {
        ssh_children_of(self.pid())
    }
}

impl HubProcess for HubServer {
    fn pid(&self) -> u32 {
        self.pid
    }
}

/// Wait until `hub` runs no `ssh` any more.
fn wait_without_ssh(hub: &impl HubProcess) {
    let deadline = std::time::Instant::now() + WAIT;
    while !hub.ssh().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "the hub still runs ssh: {:?}",
            hub.ssh()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The remote screen hub as the desktop app runs it: an app-owned
/// `splitty desktop-backend --transport native-v1 --backend embedded --owner`
/// over inherited pipes, with the lab's `ssh`, a home and runtime directory
/// of its own and its standard error in a file. Host streams are the owner's
/// host-open intents; API calls go over a direct stream of the owner, which
/// holds the local hub's authority.
///
/// Why smoke tests: these need real `ssh`, a real host namespace and the
/// binary's own askpass and install. An in-process hub uses scripted
/// connectors (`splitty_server::hub`'s tests).
struct HubServer {
    owner: Arc<splitty_remote::owner::Owner>,
    /// The backend process, the leader of its own process group.
    pid: u32,
    stderr: PathBuf,
}

impl HubServer {
    /// Start it with `payload` as the server it installs on hosts.
    async fn start(lab: &Lab, payload: &Path) -> Self {
        Self::start_with(lab, Some(payload)).await
    }

    /// Start it with `payload`, if any, as the server it installs on hosts.
    async fn start_with(lab: &Lab, payload: Option<&Path>) -> Self {
        let d = lab.dir.path();
        let home = d.join("client-home");
        let run = d.join("client-run");
        fs::create_dir_all(home.join(".ssh")).expect("client home");
        fs::create_dir_all(&run).expect("client runtime directory");
        fs::set_permissions(&run, fs::Permissions::from_mode(0o700)).expect("chmod");
        let stderr = d.join("hub.stderr");
        // The bridge keeps only the end of a backend's standard error; the
        // shell appends all of it to the file and then becomes the backend.
        let mut command = lab.client_command("sh");
        command
            .args(["-c", r#"file=$1; shift; exec "$@" 2>>"$file""#, "sh"])
            .arg(&stderr)
            .arg(env!("CARGO_BIN_EXE_splitty"))
            .args([
                "desktop-backend",
                "--transport",
                "native-v1",
                "--backend",
                "embedded",
                "--owner",
            ])
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
            .process_group(0);
        if let Some(payload) = payload {
            command.env("SPLITTY_REMOTE_PAYLOAD", payload);
        }
        let owner = splitty_remote::owner::Owner::spawn(tokio::process::Command::from(command), 1)
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "the hub did not start: {error}\n{}",
                    fs::read_to_string(&stderr).unwrap_or_default()
                )
            });
        let pid = owner.id().expect("the backend's pid");
        Self { owner, pid, stderr }
    }

    /// Open a host stream for `target` as the app's main window does.
    async fn open(&self, target: &str) -> HubSocket {
        self.open_with(target, false).await
    }

    /// [`HubServer::open`], asking the hub to update the host's daemon when
    /// `update` is set.
    async fn open_with(&self, target: &str, update: bool) -> HubSocket {
        use splitty_server::hub::Step;

        let target = splitty_remote::ssh::SshTarget::parse(target).expect("an SSH target");
        // Each step once, in order: the transport repeats its latest report
        // as a pulse and reports phases that are no step.
        let mut progress = Vec::new();
        let mut last: Option<Step> = None;
        let opened = tokio::time::timeout(
            Duration::from_secs(120),
            self.owner.open_context(
                Some(target),
                update,
                splitty_transport::queue::Context::default(),
                |report| {
                    let Some(step) = Step::from_name(&report.phase) else {
                        return;
                    };
                    if last.is_some_and(|last| step <= last) {
                        return;
                    }
                    last = Some(step);
                    progress.push(serde_json::json!({
                        "type": "remote.progress", "step": step.name(), "message": report.message,
                    }));
                },
            ),
        )
        .await
        .expect("the host stream opens or fails before its deadline");
        match opened {
            Ok(connection) => HubSocket {
                progress,
                failure: None,
                connection: Some(connection),
            },
            Err(error) => {
                let closed = splitty_transport::negotiation::preparation_failure(&error)
                    .unwrap_or_else(|| {
                        panic!("the host stream failed without a diagnostic: {error}")
                    });
                let failure = serde_json::from_str(&closed.message)
                    .unwrap_or_else(|e| panic!("not a remote.error ({e}): {}", closed.message));
                HubSocket {
                    progress,
                    failure: Some(failure),
                    connection: None,
                }
            }
        }
    }

    /// A request to the hub's local API, named as the HTTP API names it, over
    /// a direct stream of the owner: the status and the JSON body.
    async fn api(&self, method: &str, path: &str, body: Option<&str>) -> (u16, serde_json::Value) {
        owner_api(&self.owner, method, path, body).await
    }

    /// [`HubServer::api`] as a task of its own, on a stream of its own.
    fn api_task(
        &self,
        method: &'static str,
        path: &'static str,
        body: Option<String>,
    ) -> tokio::task::JoinHandle<(u16, serde_json::Value)> {
        let owner = self.owner.clone();
        tokio::spawn(async move { owner_api(&owner, method, path, body.as_deref()).await })
    }

    /// `/api/remote/forwards`.
    async fn forwards(&self, method: &str, body: Option<&str>) -> (u16, serde_json::Value) {
        self.api(method, "/api/remote/forwards", body).await
    }

    /// `/api/remote/forward-preferences`.
    async fn preferences(&self, method: &str, body: Option<&str>) -> (u16, serde_json::Value) {
        self.api(method, "/api/remote/forward-preferences", body)
            .await
    }

    /// `/api/remote/reverse-forwards`.
    async fn reverses(&self, method: &str, body: Option<&str>) -> (u16, serde_json::Value) {
        self.api(method, "/api/remote/reverse-forwards", body).await
    }

    /// Wait until the hub runs no `ssh` any more.
    fn wait_without_ssh(&self) {
        wait_without_ssh(self);
    }

    /// Whether the backend process has ended (it may not be reaped yet).
    fn ended(&self) -> bool {
        fs::read_to_string(format!("/proc/{}/stat", self.pid)).map_or(true, |stat| {
            stat.rsplit_once(") ")
                .is_some_and(|(_, fields)| fields.starts_with('Z') || fields.starts_with('X'))
        })
    }
}

/// A request to the local API of `owner`'s service, named as the HTTP API
/// names it, over a direct stream of its own: the status and the JSON body.
async fn owner_api(
    owner: &splitty_remote::owner::Owner,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, serde_json::Value) {
    let description = splitty_core::native_api::OPERATIONS
        .iter()
        .find(|operation| operation.method == method && operation.path == path)
        .unwrap_or_else(|| panic!("no operation for {method} {path}"));
    let mut args = serde_json::Map::new();
    if let Some(body) = body {
        args.insert(
            "body".into(),
            serde_json::from_str(body).expect("a JSON request body"),
        );
    }
    let mut connection = owner
        .open_context(
            None,
            false,
            splitty_transport::queue::Context::default(),
            |_| {},
        )
        .await
        .expect("an API stream of the owner");
    let reply = connection
        .request(
            description.name.into(),
            args,
            Some(description.deadline.duration().unwrap_or(WAIT)),
        )
        .await
        .unwrap_or_else(|error| panic!("{method} {path}: {error}"));
    let (status, body) = (reply.status, reply.body.clone());
    drop(reply);
    connection.finish().await.expect("the API stream ends");
    let json = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("not JSON ({e}): {body}"))
    };
    (status, json)
}

impl Drop for HubServer {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "splitty desktop-backend stderr:\n{}",
                fs::read_to_string(&self.stderr).unwrap_or_default()
            );
        }
        // Only this backend's group, created at spawn; never process-name
        // cleanup. The owner reaps the backend; until then its unreaped pid
        // keeps the group's number from being reused.
        if let Ok(pid) = i32::try_from(self.pid) {
            let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.ended() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// A message of a host stream.
#[derive(Debug)]
enum Message {
    Text(String),
    Binary(Vec<u8>),
}

/// A host stream of the hub, or why it could not be opened.
struct HubSocket {
    /// The `remote.progress` reports of its preparation.
    progress: Vec<serde_json::Value>,
    /// The `remote.error` that ended the preparation.
    failure: Option<serde_json::Value>,
    connection: Option<splitty_remote::owner::Connection>,
}

impl HubSocket {
    fn connection(&mut self) -> &mut splitty_remote::owner::Connection {
        self.connection.as_mut().expect("an open host stream")
    }

    /// The next message, or `None` once the stream has ended.
    async fn try_next(&mut self) -> Option<Message> {
        use splitty_transport::wire::Kind;
        loop {
            let message = tokio::time::timeout(WAIT, self.connection().recv())
                .await
                .expect("a message in time")?;
            let bytes = message.frame.application().expect("an application frame").1;
            match message.frame.kind {
                Kind::Text => {
                    return Some(Message::Text(
                        String::from_utf8(bytes.to_vec()).expect("UTF-8 text"),
                    ))
                }
                Kind::Binary => return Some(Message::Binary(bytes.to_vec())),
                // Replies to requests made on this stream.
                _ => continue,
            }
        }
    }

    async fn next(&mut self) -> Message {
        self.try_next().await.expect("the stream is open")
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

    /// The hub's progress messages and the message that ends them: the
    /// stream's first message, `remote.ready`, or the `remote.error` that
    /// refused it.
    async fn progress(&mut self) -> (Vec<serde_json::Value>, serde_json::Value) {
        let progress = std::mem::take(&mut self.progress);
        match self.failure.take() {
            Some(failure) => (progress, failure),
            None => (progress, self.next_json().await),
        }
    }

    async fn send(&mut self, text: &str) {
        self.connection()
            .sender()
            .data(splitty_transport::wire::Kind::Text, text.as_bytes())
            .expect("send");
    }

    /// A request over the stream to the host's daemon: the status and the
    /// JSON body. Messages that arrive meanwhile are skipped.
    async fn request(&mut self, operation: &str) -> (u16, serde_json::Value) {
        let reply = self
            .connection()
            .request(operation.into(), serde_json::Map::new(), Some(WAIT))
            .await
            .unwrap_or_else(|error| panic!("{operation}: {error}"));
        (
            reply.status,
            serde_json::from_str(&reply.body).unwrap_or(serde_json::Value::Null),
        )
    }

    async fn close(&mut self) {
        if let Some(connection) = self.connection.take() {
            connection.finish().await.expect("close");
        }
    }

    /// The stream ends without the client closing it, skipping what the host
    /// still sends: the hub lost the host.
    async fn lost(&mut self) {
        while self.try_next().await.is_some() {}
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
    let hub = HubServer::start(&lab, &payload).await;

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

    // cli, On a host over SSH: the remote command's pick list, terminal mode
    // and private daemon attachment, through real SSH. The lab's sshd cannot
    // allocate a terminal inside its user namespace, so `script` gives the
    // command one on the host.
    {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        async fn until(stdout: &mut tokio::process::ChildStdout, marker: &[u8]) -> Vec<u8> {
            tokio::time::timeout(WAIT, async {
                let mut output = Vec::new();
                loop {
                    let mut bytes = [0; 4096];
                    let count = stdout.read(&mut bytes).await.expect("SSH terminal output");
                    assert!(
                        count > 0,
                        "SSH terminal ended before {:?}: {}",
                        marker,
                        String::from_utf8_lossy(&output)
                    );
                    output.extend_from_slice(&bytes[..count]);
                    assert!(output.len() < 1024 * 1024, "bounded terminal output");
                    if output.windows(marker.len()).any(|part| part == marker) {
                        return output;
                    }
                }
            })
            .await
            .expect("remote attach response")
        }
        socket
            .send(
                &serde_json::json!({
                    "type":"input", "id":session,
                    "data":"printf 'ATTACH_%s\\n' REPLAY_READY\n"
                })
                .to_string(),
            )
            .await;
        tokio::time::timeout(WAIT, async {
            let mut replay = Vec::new();
            loop {
                if let Message::Binary(frame) = socket.next().await {
                    let start = 1 + usize::from(frame[0]);
                    if frame.get(1..start) == Some(session.as_bytes()) {
                        replay.extend_from_slice(&frame[start..]);
                        assert!(replay.len() < 1024 * 1024, "bounded replay");
                        if replay
                            .windows(b"ATTACH_REPLAY_READY".len())
                            .any(|part| part == b"ATTACH_REPLAY_READY")
                        {
                            break;
                        }
                    }
                }
            }
        })
        .await
        .expect("marker in host replay");
        let mut command = lab.client_command("ssh");
        command
            .args(["-o", "BatchMode=yes", HOST, "--"])
            .arg(format!(
                "script -qefc '{} attach' /dev/null",
                lab.installed_server().display()
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(fs::File::create(lab.dir.path().join("attach.stderr")).unwrap());
        let mut terminal = tokio::process::Command::from(command)
            .kill_on_drop(true)
            .spawn()
            .expect("ssh attach");
        let mut input = terminal.stdin.take().unwrap();
        let mut output = terminal.stdout.take().unwrap();
        let list = until(&mut output, b"Attach to (1-1): ").await;
        assert!(
            String::from_utf8_lossy(&list).contains("hub"),
            "{}",
            String::from_utf8_lossy(&list)
        );
        input.write_all(b"1\n").await.unwrap();
        until(&mut output, b"ATTACH_REPLAY_READY").await;
        input
            .write_all(b"printf '%s%s\\n' NATIVE_ ATTACH_OK\n")
            .await
            .unwrap();
        until(&mut output, b"NATIVE_ATTACH_OK").await;
        input.write_all(&[0x1c]).await.unwrap();
        until(&mut output, b"[detached]").await;
        assert!(tokio::time::timeout(WAIT, terminal.wait())
            .await
            .unwrap()
            .unwrap()
            .success());
    }

    // The relaying ssh dies: the host stream ends without the client.
    let bridges = hub.ssh();
    assert_eq!(bridges.len(), 1, "one ssh carries the bridge: {bridges:?}");
    kill(Pid::from_raw(bridges[0]), Signal::SIGKILL).expect("kill the bridge's ssh");
    socket.lost().await;
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
    let (status, hosts) = hub.api("GET", "/api/remote/directory", None).await;
    assert_eq!(status, 200, "{hosts}");
    let hosts = &hosts["hosts"];
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
    // Keeper socket identity alone does not establish a live terminal. A
    // fresh nonce must round-trip through the bridge to the original shell,
    // preserving both its PID and private in-memory state across migration.
    async fn prove_shell(
        socket: &mut HubSocket,
        session: &str,
        secret: &str,
        initialize: bool,
    ) -> u32 {
        let nonce = format!("{:032x}", rand::random::<u128>());
        let initialize = if initialize {
            format!("SPLITTY_MIGRATION_PROOF='{secret}'; ")
        } else {
            String::new()
        };
        let input = format!("{initialize}printf '\nMIGRATION_%s:%s:%s\n' '{nonce}' \"$$\" \"$SPLITTY_MIGRATION_PROOF\"\n");
        socket
            .send(
                &serde_json::json!({"type":"attach","id":session,"cols":80,"rows":24}).to_string(),
            )
            .await;
        socket
            .send(&serde_json::json!({"type":"input","id":session,"data":input}).to_string())
            .await;
        let prefix = format!("MIGRATION_{nonce}:");
        let mut bytes = Vec::new();
        loop {
            if let Message::Binary(frame) = socket.next().await {
                let Some((id, output)) =
                    splitty_server::client_session::codec::decode_binary_frame(&frame)
                else {
                    panic!("invalid terminal output");
                };
                if id != session {
                    continue;
                }
                bytes.extend_from_slice(output);
                assert!(
                    bytes.len() < 64 * 1024,
                    "shell nonce missing from bounded output"
                );
                let text = String::from_utf8_lossy(&bytes);
                for line in text.lines() {
                    if let Some((pid, value)) = line
                        .strip_prefix(&prefix)
                        .and_then(|line| line.split_once(':'))
                    {
                        if value.trim() == secret {
                            return pid.parse().expect("same shell PID");
                        }
                    }
                }
            }
        }
    }
    let Some(lab) = lab() else { return };
    let payload = lab.payload();
    let hub = HubServer::start(&lab, &payload).await;

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
    let session = loop {
        let event = socket.next_json().await;
        if event["type"] == "mux.pane_spawned" {
            break event["sessionId"].as_str().unwrap().to_owned();
        }
    };
    let secret = format!("{:032x}", rand::random::<u128>());
    let shell_pid = prove_shell(&mut socket, &session, &secret, true).await;
    socket.close().await;
    hub.wait_without_ssh();
    let daemon = host_daemon_pid(&lab);
    let keepers = host_keepers(&lab);
    assert_eq!(keepers.len(), 1, "the screen's shell has a keeper");
    let mut socket = hub.open_with(HOST, true).await;
    let (steps, ready) = socket.outcome().await;
    assert_eq!(steps, ["daemon", "bridge"], "nothing to update: {ready}");
    assert_eq!(ready["daemon"]["started"], false, "{ready}");
    assert_eq!(socket.next_json().await["type"], "mux.welcome");
    assert_eq!(
        prove_shell(&mut socket, &session, &secret, false).await,
        shell_pid
    );
    socket.close().await;
    hub.wait_without_ssh();
    assert_eq!(host_daemon_pid(&lab), daemon, "the same daemon runs");

    // The daemon becomes one of another version and protocol.
    let info_path = lab.dir.path().join("home/.config/splitty/daemon.json");
    let original_record = fs::read(&info_path).expect("daemon.json");
    let mut info: serde_json::Value = serde_json::from_slice(&original_record).expect("JSON");
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
    socket.close().await;
    hub.wait_without_ssh();
    assert_eq!(host_keepers(&lab), keepers);
    let old_record = fs::read(&info_path).unwrap();
    // Restore only the test's simulated version metadata to probe the actual
    // unchanged daemon, then restore the old record for explicit update.
    fs::write(&info_path, &original_record).unwrap();
    let mut probe = hub.open(HOST).await;
    assert_eq!(probe.outcome().await.1["type"], "remote.ready");
    assert_eq!(probe.next_json().await["type"], "mux.welcome");
    assert_eq!(
        prove_shell(&mut probe, &session, &secret, false).await,
        shell_pid
    );
    probe.close().await;
    hub.wait_without_ssh();
    assert_eq!(host_daemon_pid(&lab), daemon);
    fs::write(&info_path, &old_record).unwrap();

    // This fixture simulates an older version by editing diagnostic fields;
    // the actual daemon still owns its original publication. Restore exactly
    // that record at the SSH stop boundary, after version-policy selection but
    // before real shutdown. Production cleanup correctly refuses a changed
    // publication. This hook never replaces a different record/instance and
    // records that the expected conditional stop actually crossed SSH.
    fs::write(
        lab.dir.path().join("original-publication.json"),
        &original_record,
    )
    .unwrap();
    fs::write(
        lab.dir.path().join("simulated-publication.json"),
        &old_record,
    )
    .unwrap();
    let restore = lab.dir.path().join("restore-publication.py");
    fs::write(&restore, r#"import json, os, pathlib, shlex
root = pathlib.Path(__file__).parent
marker = root / "publication-restored"
original = (root / "original-publication.json").read_bytes()
instance = json.loads(original)["instance"]
command = shlex.split(os.environ.get("SSH_ORIGINAL_COMMAND", ""))
if not marker.exists() and command[-4:] == ["daemon", "stop", "--if-instance", instance]:
    target = root / "home/.config/splitty/daemon.json"
    assert target.read_bytes() == (root / "simulated-publication.json").read_bytes(), "unexpected publication"
    target.write_bytes(original)
    marker.write_text(instance)
"#).unwrap();
    let force_path = lab.dir.path().join("host-env.sh");
    let original_force = fs::read_to_string(&force_path).unwrap();
    let hook = format!(
        "python3 {} || exit 1\n",
        splitty_remote::ssh::quote(restore.to_str().unwrap())
    );
    let force = original_force.replacen(
        "if [ -n \"$SSH_ORIGINAL_COMMAND\" ];",
        &format!("{hook}if [ -n \"$SSH_ORIGINAL_COMMAND\" ];"),
        1,
    );
    assert_ne!(force, original_force);
    write_executable(&force_path, &force);

    let mut socket = hub.open_with(HOST, true).await;
    let (progress, ready) = socket.progress().await;
    write_executable(&force_path, &original_force);
    assert!(
        lab.dir.path().join("publication-restored").exists(),
        "conditional stop crossed the fixture boundary"
    );
    let steps: Vec<&str> = progress
        .iter()
        .map(|message| message["step"].as_str().expect("a step"))
        .collect();
    assert_eq!(steps, ["daemon", "update", "started", "bridge"], "{ready}");
    assert_eq!(
        progress[1]["message"],
        format!("Updating Splitty 0.0.1-old on {HOST}; its sessions keep running...")
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
    assert_eq!(
        prove_shell(&mut socket, &session, &secret, false).await,
        shell_pid
    );
    assert_ne!(host_daemon_pid(&lab), daemon, "a new daemon runs");
    // Its info file is gone before the process has quite ended.
    let deadline = std::time::Instant::now() + WAIT;
    while kill(Pid::from_raw(daemon), None).is_ok() {
        assert!(std::time::Instant::now() < deadline, "the old daemon ends");
        std::thread::sleep(Duration::from_millis(50));
    }
    socket.close().await;
    hub.wait_without_ssh();

    // A downgraded client may use a newer daemon when the native/mux contract
    // remains compatible. Version metadata simulates that version difference;
    // this is still the actual daemon, native bridge and existing keeper.
    let replacement_pid = host_daemon_pid(&lab);
    let replacement_record = fs::read(&info_path).unwrap();
    struct RestoreDiscovery(PathBuf, Vec<u8>);
    impl Drop for RestoreDiscovery {
        fn drop(&mut self) {
            // Keep fixture teardown usable even if a policy assertion fails.
            fs::write(&self.0, &self.1).expect("restore native fixture discovery");
        }
    }
    let _restore = RestoreDiscovery(info_path.clone(), replacement_record.clone());
    let mut newer: serde_json::Value = serde_json::from_slice(&replacement_record).unwrap();
    newer["version"] = "99.0.0".into();
    let newer_record = serde_json::to_vec(&newer).unwrap();
    fs::write(&info_path, &newer_record).unwrap();
    let mut socket = hub.open(HOST).await;
    let (_, ready) = socket.progress().await;
    assert_eq!(ready["daemon"]["version"], "99.0.0", "{ready}");
    assert_eq!(ready["daemon"]["started"], false, "{ready}");
    assert_eq!(socket.next_json().await["type"], "mux.welcome");
    assert_eq!(
        prove_shell(&mut socket, &session, &secret, false).await,
        shell_pid
    );
    socket.close().await;
    hub.wait_without_ssh();
    assert_eq!(host_daemon_pid(&lab), replacement_pid);
    assert_eq!(fs::read(&info_path).unwrap(), newer_record);
    assert_eq!(host_keepers(&lab), keepers);

    // Even an explicit UI update cannot downgrade a newer incompatible daemon
    // or terminate a daemon that does not promise to keep its sessions.
    for (version, keeps) in [("99.0.0", true), ("0.0.1-old", false)] {
        let mut incompatible: serde_json::Value =
            serde_json::from_slice(&replacement_record).unwrap();
        incompatible["version"] = version.into();
        incompatible["protocol"] = (protocol + 2).into();
        incompatible["keepsSessions"] = keeps.into();
        if !keeps {
            // Native format-2 daemons promise keepers. A non-kept predecessor
            // must use a structurally valid legacy record, not corrupt native
            // metadata that would fail before the migration policy is reached.
            let fields = incompatible.as_object_mut().unwrap();
            for field in ["format", "clientSocket", "instance", "transportVersion"] {
                fields.remove(field);
            }
            fields.insert("port".into(), 1.into());
        }
        let record = serde_json::to_vec(&incompatible).unwrap();
        fs::write(&info_path, &record).unwrap();
        let mut socket = hub.open_with(HOST, true).await;
        let (_, failure) = socket.outcome().await;
        assert_eq!(failure["type"], "remote.error", "{failure}");
        assert_eq!(
            failure["code"],
            if keeps {
                "incompatible"
            } else {
                "migration_unavailable"
            },
            "{failure}"
        );
        socket.close().await;
        hub.wait_without_ssh();
        assert_eq!(host_daemon_pid(&lab), replacement_pid);
        assert_eq!(fs::read(&info_path).unwrap(), record);
        assert_eq!(host_keepers(&lab), keepers);
        // The refusal left the simulated discovery intact. Repair it only
        // for this fixture's native liveness probe of the same service/shell.
        fs::write(&info_path, &replacement_record).unwrap();
        let mut probe = hub.open(HOST).await;
        assert_eq!(probe.outcome().await.1["type"], "remote.ready");
        assert_eq!(probe.next_json().await["type"], "mux.welcome");
        assert_eq!(
            prove_shell(&mut probe, &session, &secret, false).await,
            shell_pid
        );
        probe.close().await;
        hub.wait_without_ssh();
        assert_eq!(host_daemon_pid(&lab), replacement_pid);
        assert_eq!(host_keepers(&lab), keepers);
    }
    fs::write(&info_path, replacement_record).unwrap();
}

/// The client's recent hosts: their targets, none before the first record.
fn recent_targets(lab: &Lab) -> Vec<String> {
    let Ok(bytes) = fs::read(lab.recent_hosts()) else {
        return Vec::new();
    };
    let recent: serde_json::Value = serde_json::from_slice(&bytes).expect("recent hosts JSON");
    assert_eq!(recent["version"], 1, "{recent}");
    let mode = fs::metadata(lab.recent_hosts())
        .expect("mode")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    recent["hosts"]
        .as_array()
        .expect("hosts")
        .iter()
        .map(|host| host["target"].as_str().expect("target").to_owned())
        .collect()
}

/// Wait until the host the hub became ready for is recorded as recent.
async fn wait_for_recent(lab: &Lab) {
    let deadline = Instant::now() + WAIT;
    while recent_targets(lab) != [HOST] {
        assert!(
            Instant::now() < deadline,
            "{HOST} is not recorded as recent"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// remote-screens, Remote Screen Hub Endpoint, for a host without Splitty
/// that this Splitty cannot install on: the hub refuses the host stream with
/// a `remote.error` and a hint, before any daemon, and records nothing as
/// recent (remote-hosts, Recent Hosts).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_hub_reports_a_host_without_splitty() {
    let Some(lab) = lab() else { return };
    let hub = HubServer::start_with(&lab, None).await;
    let mut socket = hub.open(HOST).await;
    let (steps, error) = socket.outcome().await;
    assert!(!steps.iter().any(|step| step == "started"), "{steps:?}");
    assert_eq!(error["type"], "remote.error", "{error}");
    assert_eq!(error["code"], "unsupported_host", "{error}");
    assert!(
        error["message"]
            .as_str()
            .expect("message")
            .contains(&format!("Splitty is not installed on {HOST}")),
        "{error}"
    );
    assert!(
        error["hint"]
            .as_str()
            .expect("a hint")
            .contains(&format!("Install splitty on {HOST}")),
        "{error}"
    );
    assert!(!lab.installed_server().exists(), "nothing was installed");
    hub.wait_without_ssh();
    assert!(
        !lab.recent_hosts().exists(),
        "a failed host stream records nothing"
    );
}

/// remote-screens, Remote Screen Hub Endpoint: a host with Splitty but no
/// daemon gets a private daemon from the hub, reported as `started`; the
/// host stream reaches it, and the host is recorded as recent once ready.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_hub_starts_a_daemon_on_a_host_without_one() {
    let Some(lab) = lab() else { return };
    lab.install_splitty();
    // No server of its own to install: the hub runs the host's splitty.
    let hub = HubServer::start_with(&lab, None).await;
    assert!(!lab.recent_hosts().exists(), "nothing recorded before");
    let mut socket = hub.open(HOST).await;
    let (progress, ready) = socket.progress().await;
    assert_eq!(ready["type"], "remote.ready", "{ready}");
    assert_eq!(ready["daemon"]["started"], true, "{ready}");
    assert!(
        progress.iter().any(|report| report["step"] == "started"
            && report["message"] == format!("Started a private Splitty daemon on {HOST}.")),
        "{progress:?}"
    );
    assert!(
        !progress.iter().any(|report| report["step"] == "install"),
        "the host's own splitty runs: {progress:?}"
    );
    assert!(!lab.installed_server().exists());
    assert_eq!(socket.next_json().await["type"], "mux.welcome");
    let (status, state) = socket.request("mux.state").await;
    assert_eq!(status, 200, "{state}");
    wait_for_recent(&lab).await;
    assert!(kill(Pid::from_raw(host_daemon_pid(&lab)), None).is_ok());
    socket.close().await;
    hub.wait_without_ssh();
}

/// remote-screens, Remote Screen Hub Endpoint, on a host without Splitty:
/// the hub installs its server and starts the daemon; after a reboot (the
/// daemon is gone) the next host stream starts it again, after a rebuilt
/// container (server and daemon are gone) the next one installs the server
/// again, and a later one reuses the installed server and running daemon.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_hub_installs_its_server_and_survives_a_reboot_and_a_rebuild() {
    let Some(lab) = lab() else { return };
    let payload = lab.payload();
    let installed = lab.installed_server();
    let hub = HubServer::start(&lab, &payload).await;

    // A host without Splitty: install, start the daemon, connect.
    let mut socket = hub.open(HOST).await;
    let (progress, ready) = socket.progress().await;
    assert_eq!(ready["type"], "remote.ready", "{ready}");
    let text: Vec<&str> = progress
        .iter()
        .map(|report| report["message"].as_str().expect("message"))
        .collect();
    assert!(
        text.iter().any(|line| line.starts_with(&format!(
            "Installing Splitty {} on {HOST} (",
            env!("CARGO_PKG_VERSION")
        ))),
        "{text:?}"
    );
    assert!(
        text.iter()
            .any(|line| line.starts_with(&format!("Started a private Splitty daemon on {HOST}"))),
        "{text:?}"
    );
    assert!(
        installed.is_file(),
        "server installed at {}",
        installed.display()
    );
    assert!(installed.with_file_name("shell-integration").is_dir());
    assert_eq!(socket.next_json().await["type"], "mux.welcome");
    assert_eq!(socket.request("mux.state").await.0, 200);

    // A reboot: the daemon is gone and the host stream ends. The next one
    // starts the daemon again.
    kill(Pid::from_raw(host_daemon_pid(&lab)), Signal::SIGKILL).expect("kill the daemon");
    socket.lost().await;
    hub.wait_without_ssh();
    let mut socket = hub.open(HOST).await;
    let (steps, ready) = socket.outcome().await;
    assert_eq!(steps, ["daemon", "started", "bridge"], "{ready}");
    assert_eq!(ready["daemon"]["started"], true, "{ready}");
    assert_eq!(socket.next_json().await["type"], "mux.welcome");
    assert_eq!(socket.request("mux.state").await.0, 200);

    // A rebuilt container: server and daemon are gone. The next host stream
    // installs the server again and starts the daemon.
    kill(Pid::from_raw(host_daemon_pid(&lab)), Signal::SIGKILL).expect("kill the daemon");
    socket.lost().await;
    hub.wait_without_ssh();
    fs::remove_dir_all(lab.dir.path().join("home/.local/share/splitty"))
        .expect("remove the server");
    let mut socket = hub.open(HOST).await;
    let (steps, ready) = socket.outcome().await;
    assert_eq!(steps, ["install", "daemon", "started", "bridge"], "{ready}");
    assert!(installed.is_file(), "the server is installed again");
    assert_eq!(socket.next_json().await["type"], "mux.welcome");
    assert_eq!(socket.request("mux.state").await.0, 200);
    socket.close().await;
    hub.wait_without_ssh();

    // Connecting again reuses the installed server and the running daemon.
    let mut again = hub.open(HOST).await;
    let (steps, ready) = again.outcome().await;
    assert_eq!(steps, ["daemon", "bridge"], "{ready}");
    assert_eq!(ready["daemon"]["started"], false, "{ready}");
    again.close().await;
    hub.wait_without_ssh();
}

/// remote-screens, Remote Screen Hub Endpoint (Host connection lost) and
/// Port Forwards Through The Hub, through real `ssh` and a real host daemon:
/// after the bridge's `ssh` and a saved forward's `ssh` die, the next host
/// stream shows the same daemon with the same sessions and the forward is
/// rebuilt on its local port; a new daemon incarnation is reached the same
/// way; closing the stream ends its `ssh`. The host is recorded as recent
/// only once a stream is ready.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_hub_reconnects_and_hands_over_sessions_through_real_ssh() {
    let Some(lab) = lab() else { return };
    lab.install_splitty();
    lab.start_daemon();
    let service = lab.serve_on_host(0);
    let payload = lab.payload();
    let hub = HubServer::start(&lab, &payload).await;
    let local = free_local_port();
    let saved = serde_json::json!({
        "target": HOST, "port": service.port, "local": local, "label": "Reconnected service",
    })
    .to_string();
    assert_eq!(hub.preferences("PUT", Some(&saved)).await.0, 200);
    assert!(!lab.recent_hosts().exists(), "nothing recorded before");

    let mut socket = hub.open(HOST).await;
    let (_, ready) = socket.outcome().await;
    assert_eq!(ready["type"], "remote.ready", "{ready}");
    assert_eq!(ready["daemon"]["started"], false, "{ready}");
    assert_eq!(socket.next_json().await["type"], "mux.welcome");
    wait_for_recent(&lab).await;
    let forward = expected_forward(service.port, local);
    wait_for_active(&hub, &serde_json::json!([forward])).await;
    assert_eq!(health(local), "ok");
    socket
        .send(
            r#"{"type":"mux.op","screenId":"kept","op":{"type":"create_screen","screenId":"kept","name":"kept","newPaneId":"kept-pane"}}"#,
        )
        .await;
    loop {
        let message = socket.next_json().await;
        if message["type"] == "mux.pane_spawned" {
            assert_eq!(message["paneId"], "kept-pane", "{message}");
            break;
        }
    }
    assert_eq!(socket.request("mux.state").await.0, 200);
    let daemon = host_daemon_pid(&lab);

    // The ssh processes die; the same daemon is still there.
    let forwards = forward_ssh(&hub);
    assert_eq!(forwards.len(), 1, "one ssh holds the forward: {forwards:?}");
    let bridges: Vec<_> = hub
        .ssh()
        .into_iter()
        .filter(|pid| !forwards.contains(pid))
        .collect();
    assert_eq!(bridges.len(), 1, "one ssh carries the bridge: {bridges:?}");
    for pid in bridges.iter().chain(&forwards) {
        kill(Pid::from_raw(*pid), Signal::SIGKILL).expect("kill ssh");
    }
    socket.lost().await;
    let mut socket = hub.open(HOST).await;
    let (_, ready) = socket.outcome().await;
    assert_eq!(ready["type"], "remote.ready", "{ready}");
    assert_eq!(ready["daemon"]["started"], false, "{ready}");
    assert_eq!(
        ready["screens"],
        serde_json::json!([{"id": "kept", "name": "kept", "panes": 1}]),
        "the same sessions: {ready}"
    );
    assert_eq!(host_daemon_pid(&lab), daemon, "the same daemon");
    assert_eq!(socket.next_json().await["type"], "mux.welcome");
    let deadline = Instant::now() + WAIT;
    loop {
        let rebuilt = forward_ssh(&hub);
        let (_, list) = hub.forwards("GET", None).await;
        if list == serde_json::json!([forward])
            && !rebuilt.is_empty()
            && !rebuilt.contains(&forwards[0])
        {
            break;
        }
        assert!(Instant::now() < deadline, "not rebuilt: {list}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        listeners(local),
        [format!("0100007F:{local:04X}")],
        "the forward listens on its own port, on 127.0.0.1 only"
    );
    assert_eq!(health(local), "ok");
    assert_eq!(recent_targets(&lab), [HOST]);

    // A new host incarnation is reached by the next host stream.
    kill(Pid::from_raw(daemon), Signal::SIGKILL).expect("kill host daemon");
    socket.lost().await;
    let mut socket = hub.open(HOST).await;
    let (_, ready) = socket.outcome().await;
    assert_eq!(ready["type"], "remote.ready", "{ready}");
    assert_eq!(ready["daemon"]["started"], true, "{ready}");
    assert_ne!(host_daemon_pid(&lab), daemon, "a new daemon runs");
    assert_eq!(socket.next_json().await["type"], "mux.welcome");
    assert_eq!(socket.request("mux.state").await.0, 200);

    // Closing the stream ends its ssh; the forward stays until stopped.
    let held = forward_ssh(&hub);
    socket.close().await;
    let deadline = Instant::now() + WAIT;
    while hub.ssh() != held {
        assert!(
            Instant::now() < deadline,
            "the stream's ssh must end with it: {:?}",
            hub.ssh()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let key = serde_json::json!({"target": HOST, "port": service.port}).to_string();
    assert_eq!(hub.forwards("DELETE", Some(&key)).await.0, 204);
    hub.wait_without_ssh();
}

/// The hub's `ssh` processes that hold a forward, not a bridge.
fn forward_ssh(hub: &impl HubProcess) -> Vec<i32> {
    hub.ssh()
        .into_iter()
        .filter(|pid| {
            fs::read(format!("/proc/{pid}/cmdline"))
                .is_ok_and(|cmdline| cmdline.split(|b| *b == 0).any(|arg| arg == b"-L"))
        })
        .collect()
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

/// Smoke-level D7 regression: a real OpenSSH child has reached ProxyCommand,
/// but sshd has not even started. TCP reachability cannot authorize success.
/// The positive control uses the identical gate and actual host service.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_hub_waits_for_real_ssh_bind_after_the_pre_auth_gate() {
    let Some(lab) = lab() else { return };
    let service = lab.serve_on_host(0);
    let payload = lab.payload();
    let hub = HubServer::start(&lab, &payload).await;
    let local = free_local_port();
    let key = serde_json::json!({"target": HOST, "port": service.port}).to_string();
    let saved = serde_json::json!({
        "target": HOST, "port": service.port, "local": local, "label": "D7 service",
    })
    .to_string();
    assert_eq!(hub.preferences("PUT", Some(&saved)).await.0, 200);
    assert_eq!(hub.forwards("DELETE", Some(&key)).await.0, 204);
    let original = lab.saved_forwards();
    assert_eq!(original["forwards"][0]["enabled"], false);

    // First fail with a rival, then use exactly the same saved mapping without
    // one. Success cannot be explained by a gate that never reaches real SSH.
    for competing in [true, false] {
        let gate = AuthGate::arm(&lab);
        let mut opening = hub.api_task("POST", "/api/remote/forwards", Some(key.clone()));
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
        assert_eq!(
            hub.forwards("GET", None).await,
            (200, serde_json::json!([]))
        );
        assert_eq!(lab.saved_forwards(), original, "no premature resume");
        assert!(!gate.directory.join("passed").exists());
        assert!(
            !gate.ssh_log().contains("Authenticated to "),
            "authentication is gated"
        );

        gate.release();
        let (status, body) = tokio::time::timeout(WAIT, opening)
            .await
            .expect("forward request deadline")
            .expect("request worker");
        assert!(
            gate.directory.join("passed").exists(),
            "real sshd was released"
        );
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
            assert_eq!(status, 409, "{body}");
            assert_eq!(body["code"], "local_port_conflict", "{body}");
            assert_eq!(
                hub.forwards("GET", None).await,
                (200, serde_json::json!([]))
            );
            hub.wait_without_ssh();
            assert_eq!(lab.saved_forwards(), original, "failed resume stays paused");
            let (status, saved) = hub.preferences("GET", None).await;
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
            assert_eq!(status, 200, "{body}");
            assert_eq!(body, expected_forward(service.port, local));
            assert_eq!(
                forward_ssh(&hub),
                ssh,
                "the authenticated child owns the bind"
            );
            assert_eq!(listeners(local), [format!("0100007F:{local:04X}")]);
            assert_eq!(health(local), "ok", "bytes reach the actual remote service");
            let (_, saved) = hub.preferences("GET", None).await;
            assert_eq!(saved["forwards"][0]["enabled"], true);
            assert!(saved["forwards"][0].get("lastError").is_none());
            assert_eq!(saved, lab.saved_forwards());
            assert_eq!(hub.forwards("DELETE", Some(&key)).await.0, 204);
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
        let (status, actual) = hub.forwards("GET", None).await;
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
        assert_eq!(hub.preferences("GET", None).await, (200, saved.clone()));
        assert_eq!(
            hub.forwards("GET", None).await,
            (200, serde_json::json!([]))
        );
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
    let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
    loop {
        if tokio::time::Instant::now() >= deadline {
            return;
        }
        match tokio::time::timeout_at(deadline, socket.try_next()).await {
            Err(_) => return,
            Ok(Some(Message::Text(text))) => {
                let message: serde_json::Value = serde_json::from_str(&text).expect("host JSON");
                assert!(
                    !message["type"]
                        .as_str()
                        .expect("message type")
                        .starts_with("remote."),
                    "unexpected hub frame after remote.ready and restoration: {message}"
                );
            }
            Ok(Some(Message::Binary(_))) => {}
            Ok(None) => panic!(
                "host relay closed: {:?}",
                socket.connection().closed_reason()
            ),
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
    let hub = HubServer::start(&lab, &payload).await;
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
            hub.preferences("PUT", Some(&request.to_string())).await,
            (200, expected.clone())
        );
        entries.push(expected);
    }
    let api_key = serde_json::json!({"target": HOST, "port": api.port}).to_string();
    assert_eq!(hub.forwards("DELETE", Some(&api_key)).await.0, 204);
    entries[1]["enabled"] = false.into();
    let edit = serde_json::json!({"target": HOST, "port": api.port, "label": "Paused API"});
    entries[1]["label"] = "Paused API".into();
    assert_eq!(
        hub.preferences("PUT", Some(&edit.to_string())).await,
        (200, entries[1].clone())
    );
    let saved = saved_document(entries.clone());
    assert_passive(&hub, &lab, &before_start, &saved).await;
    drop(hub);

    let hub = HubServer::start(&lab, &payload).await;
    assert_passive(&hub, &lab, &before_start, &saved).await;
    for port in &locals {
        let _unreserved =
            TcpListener::bind(("127.0.0.1", *port)).expect("passive port is unreserved");
    }
    let automatic = serde_json::json!({"target": HOST, "port": api.port, "automatic": true});
    let (status, error) = hub.forwards("POST", Some(&automatic.to_string())).await;
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
    assert_eq!(hub.preferences("GET", None).await, (200, saved.clone()));
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
    assert_eq!(hub.preferences("GET", None).await, (200, saved));
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
        assert_eq!(hub.preferences("DELETE", Some(&frontend_key)).await.0, 204);
        assert_eq!(forward_ssh(&hub), frontend_ssh);
        assert_eq!(
            hub.forwards("GET", None).await,
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
    assert_eq!(hub.preferences("GET", None).await, (200, forgotten.clone()));
    assert_eq!(lab.saved_forwards(), forgotten);
    socket.close().await;
    drop(hub);
    let before_restart = lab.ssh_invocations();
    let hub = HubServer::start(&lab, &payload).await;
    assert_passive(&hub, &lab, &before_restart, &forgotten).await;
    let mut socket = browse_empty_host(&hub).await;
    assert_no_extra_hub_frames(&mut socket).await;
    assert_eq!(
        hub.forwards("GET", None).await,
        (200, serde_json::json!([]))
    );
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
    assert_eq!(hub.forwards("DELETE", Some(&frontend_key)).await.0, 404);

    // Explicit Resume is the remaining consent that can enable the paused API.
    let (status, resumed) = hub.forwards("POST", Some(&api_key)).await;
    assert_eq!(status, 200, "{resumed}");
    assert_eq!(resumed, expected_forward(api.port, locals[1]));
    assert_eq!(health(locals[1]), "ok");
    entries[0]["enabled"] = true.into();
    assert_eq!(
        hub.preferences("GET", None).await,
        (200, saved_document(entries.clone()))
    );
    assert_eq!(lab.saved_forwards(), saved_document(entries));
    assert_eq!(hub.forwards("DELETE", Some(&api_key)).await.0, 204);
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
/// `ssh` and frees the port; a forward ends with the hub on SIGTERM too (a
/// smoke test: only the real process shows what a signal leaves behind).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_hub_forwards_a_port_a_host_pane_listens_on() {
    let Some(lab) = lab() else { return };
    let payload = lab.payload();
    let hub = HubServer::start(&lab, &payload).await;
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
    let (status, forward) = hub.forwards("POST", Some(&body)).await;
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
        let (_, list) = hub.forwards("GET", None).await;
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

    let (status, _) = hub.forwards("DELETE", Some(&body)).await;
    assert_eq!(status, 204);
    assert_eq!(
        forward_ssh(&hub),
        Vec::<i32>::new(),
        "the forward's ssh ended"
    );
    assert_eq!(listeners(port), Vec::<String>::new());
    let (_, list) = hub.forwards("GET", None).await;
    assert_eq!(list, serde_json::json!([]));

    // A forward does not outlive the hub ended with SIGTERM.
    let (status, again) = hub.forwards("POST", Some(&body)).await;
    assert_eq!(status, 200, "{again}");
    let held = forward_ssh(&hub);
    assert_eq!(held.len(), 1, "one ssh holds the forward: {held:?}");
    socket.close().await;
    kill(Pid::from_raw(hub.pid as i32), Signal::SIGTERM).expect("SIGTERM the hub");
    let deadline = std::time::Instant::now() + WAIT;
    while !hub.ended() {
        assert!(
            std::time::Instant::now() < deadline,
            "the hub did not end on SIGTERM"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    while Path::new(&format!("/proc/{}", held[0])).exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the forward's ssh outlived the hub"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(listeners(port), Vec::<String>::new());
}

// ── Reverse forwards ────────────────────────────────────────────────────────

/// The hub's `ssh` processes that hold a reverse forward.
fn reverse_ssh(hub: &impl HubProcess) -> Vec<i32> {
    hub.ssh()
        .into_iter()
        .filter(|pid| {
            fs::read(format!("/proc/{pid}/cmdline"))
                .is_ok_and(|cmdline| cmdline.split(|b| *b == 0).any(|arg| arg == b"-R"))
        })
        .collect()
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
/// A real local dev service whose thread/listener are observed through cleanup.
struct LocalMarkerService {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    task: Option<std::thread::JoinHandle<()>>,
}
impl Drop for LocalMarkerService {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(task) = self.task.take() {
            task.join().unwrap();
        }
    }
}
fn local_marker_service() -> (LocalMarkerService, u16) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped = stop.clone();
    let task = std::thread::spawn(move || {
        while !stopped.load(std::sync::atomic::Ordering::Acquire) {
            match listener.accept() {
                Ok((mut socket, _)) => {
                    let _ = socket.write_all(b"local-service\n");
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(error) => panic!("local marker listener failed: {error}"),
            }
        }
    });
    (
        LocalMarkerService {
            stop,
            task: Some(task),
        },
        port,
    )
}

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
    let hub = HubServer::start(&lab, &payload).await;
    let (_service, local) = local_marker_service();
    let port = 18431;
    let share = serde_json::json!({"target": HOST, "port": port, "local": local}).to_string();

    let before = lab.ssh_invocations();
    let (status, shared) = hub.reverses("POST", Some(&share)).await;
    if splitty_remote::tunnel::exposure::check_sharing_supported()
        .is_err_and(|error| error.kind() == io::ErrorKind::Unsupported)
    {
        assert_eq!(status, 501, "{shared}");
        assert_eq!(shared["code"], "unsupported_environment");
        assert!(shared["error"].as_str().unwrap().contains("reservation"));
        assert_eq!(
            lab.ssh_invocations(),
            before,
            "unsupported sharing must fail before any SSH preparation"
        );
        assert!(reverse_ssh(&hub).is_empty());
        assert_eq!(
            hub.reverses("GET", None).await,
            (200, serde_json::json!([]))
        );
        assert!(!lab
            .dir
            .path()
            .join("home/.local/share/splitty/run/relay")
            .exists());
        eprintln!(
            "reverse sharing: verified explicit unsupported environment without SSH or exposure"
        );
        return;
    }
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
        let (_, list) = hub.reverses("GET", None).await;
        let pids = reverse_ssh(&hub);
        if list[0]["state"] == "up" && !pids.is_empty() && pids != ssh {
            break;
        }
        assert!(Instant::now() < deadline, "rebuilt: {list}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(from_host(&lab, "127.0.0.1", port), "local-service");

    let key = serde_json::json!({"target": HOST, "port": port}).to_string();
    assert_eq!(hub.reverses("DELETE", Some(&key)).await.0, 204);
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

/// The lab's host under a name whose host key is not yet known and whose key
/// has a passphrase: OpenSSH asks first to trust the host key, then for the
/// passphrase. `StrictHostKeyChecking ask` writes the trusted key.
const PROMPTED: &str = "splittyprompted";
const PASSPHRASE: &str = "lab passphrase";

/// A process's environment variable, if it is readable.
fn environment_of(pid: i32, name: &str) -> Option<String> {
    let environ = fs::read(format!("/proc/{pid}/environ")).ok()?;
    let prefix = format!("{name}=");
    environ
        .split(|byte| *byte == 0)
        .find_map(|entry| entry.strip_prefix(prefix.as_bytes()))
        .map(|value| String::from_utf8_lossy(value).into_owned())
}

/// `pid`'s parent.
fn parent_of(pid: i32) -> Option<i32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// The askpass helpers OpenSSH started for `target`, waiting on a question.
fn askpass_helpers(target: &str) -> Vec<i32> {
    fs::read_dir("/proc")
        .expect("/proc")
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<i32>().ok())
        .filter(|pid| {
            environment_of(*pid, "SPLITTY_BROWSER_ASKPASS").as_deref() == Some("1")
                && environment_of(*pid, "SPLITTY_ASKPASS_TARGET").as_deref() == Some(target)
                && fs::read_to_string(format!("/proc/{}/comm", parent_of(*pid).unwrap_or(0)))
                    .is_ok_and(|comm| comm.trim() == "ssh")
        })
        .collect()
}

impl Lab {
    /// Add [`PROMPTED`] to the lab: a key with [`PASSPHRASE`] the host
    /// accepts, and a known-hosts file of its own that starts empty.
    fn add_prompted_host(&self) {
        let d = self.dir.path();
        let keygen = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", PASSPHRASE, "-f"])
            .arg(d.join("promptkey"))
            .output()
            .expect("ssh-keygen");
        assert!(keygen.status.success(), "ssh-keygen\n{}", describe(&keygen));
        let mut authorized = fs::OpenOptions::new()
            .append(true)
            .open(d.join("authorized_keys"))
            .expect("authorized_keys");
        authorized
            .write_all(&fs::read(d.join("promptkey.pub")).expect("public key"))
            .expect("authorize the key");
        let user =
            String::from_utf8_lossy(&Command::new("id").arg("-un").output().expect("id").stdout)
                .trim()
                .to_owned();
        let d_str = d.display();
        let mut config = fs::OpenOptions::new()
            .append(true)
            .open(d.join("ssh_config"))
            .expect("ssh_config");
        write!(
            config,
            "Host {PROMPTED}\n  User {user}\n  IdentityFile {d_str}/promptkey\n  IdentitiesOnly yes\n  \
             UserKnownHostsFile {d_str}/prompted_known_hosts\n  StrictHostKeyChecking ask\n  \
             ProxyCommand {d_str}/proxy.sh\n"
        )
        .expect("ssh_config");
    }
}

/// remote-hosts, SSH Questions Of The Local Service, as the Windows desktop
/// app runs it: an app-owned `desktop-backend --owner` over inherited pipes,
/// whose host connection's `ssh` runs the backend's own executable as its
/// askpass helper. The app's connection with local hub authority is told with
/// `ssh.prompts.changed`, reads the host-key confirmation and the passphrase
/// with `ssh.prompts.get` and answers them with `ssh.prompts.answer`; the
/// host stream then becomes ready, also after an answer that took longer than
/// a handshake frame may. A wrong passphrase is asked again, and a cancelled
/// question fails the open.
///
/// Why a smoke test: in-process tests publish questions with a stand-in
/// helper inside the test process. Only the real binary in owner mode shows
/// that the backend names itself as `SSH_ASKPASS` without a program from the
/// app, that OpenSSH without a terminal runs that helper process, and that its
/// question reaches the owner's client and its answer reaches OpenSSH.
#[test]
fn an_owned_backend_asks_its_ssh_questions_over_the_owner_connection() {
    let Some(lab) = lab() else { return };
    lab.add_prompted_host();
    let payload = lab.payload();
    let d = lab.dir.path();
    let home = d.join("client-home");
    let run = d.join("client-run");
    fs::create_dir_all(home.join(".ssh")).expect("client home");
    fs::create_dir_all(&run).expect("client runtime directory");
    fs::set_permissions(&run, fs::Permissions::from_mode(0o700)).expect("chmod");
    let mut command = lab.client(&[
        "desktop-backend",
        "--transport",
        "native-v1",
        "--backend",
        "embedded",
        "--owner",
    ]);
    command
        .env("SPLITTY_REMOTE_PAYLOAD", &payload)
        .env("HOME", &home)
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_RUNTIME_DIR", &run)
        // As under `wsl.exe` from the desktop app: no askpass from outside,
        // no display.
        .env_remove("SPLITTY_ASKPASS_PROGRAM")
        .env_remove("SSH_ASKPASS")
        .env_remove("SSH_ASKPASS_REQUIRE")
        .env_remove("DISPLAY")
        .env_remove("SPLITTY_TOKEN")
        .env_remove("SPLITTY_SERVER")
        .env_remove("SPLITTY_SOCK")
        .env_remove("SPLITTY_AGENT_SOCK")
        .env_remove("SPLITTY_PANE_ID")
        .env_remove("SPLITTY_SCREEN_ID")
        .env_remove("SPLITTY_SESSION_ID")
        // A separate, owned group lets teardown also end an in-flight
        // ssh/ProxyCommand, even when an assertion unwinds the test.
        .process_group(0);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        use splitty_remote::owner::{Connection, Owner};
        use splitty_transport::{queue, wire::Kind};
        async fn request(
            connection: &mut Connection,
            operation: &str,
            args: serde_json::Value,
        ) -> (u16, serde_json::Value) {
            let reply = connection
                .request(
                    operation.into(),
                    args.as_object().unwrap().clone(),
                    Some(WAIT),
                )
                .await
                .unwrap();
            let body = serde_json::from_str(&reply.body).unwrap_or(serde_json::Value::Null);
            (reply.status, body)
        }
        async fn next_json(connection: &mut Connection) -> serde_json::Value {
            loop {
                let frame = tokio::time::timeout(WAIT, connection.recv())
                    .await
                    .expect("a frame before the deadline")
                    .expect("an open connection");
                if frame.frame.kind == Kind::Text {
                    return frame.frame.decode().unwrap();
                }
            }
        }
        /// After `ssh.prompts.changed` on `events`, the one pending question.
        async fn question(events: &mut Connection, api: &mut Connection) -> serde_json::Value {
            loop {
                let event = next_json(events).await;
                if event["type"] != "ssh.prompts.changed" {
                    continue;
                }
                let (status, prompts) =
                    request(api, "ssh.prompts.get", serde_json::json!({})).await;
                assert_eq!(status, 200, "{prompts}");
                if let Some(prompt) = prompts.as_array().and_then(|list| list.first()) {
                    assert_eq!(prompts.as_array().unwrap().len(), 1, "{prompts}");
                    assert_eq!(prompt["target"], PROMPTED, "{prompts}");
                    return prompt.clone();
                }
            }
        }
        async fn answer(api: &mut Connection, prompt: &serde_json::Value, answer: Option<&str>) {
            let (status, reply) = request(
                api,
                "ssh.prompts.answer",
                serde_json::json!({"body": {"id": prompt["id"], "answer": answer}}),
            )
            .await;
            assert_eq!(status, 200, "{reply}");
            assert_eq!(reply["accepted"], true, "{reply}");
        }

        let owner = Owner::spawn(tokio::process::Command::from(command), 1)
            .await
            .unwrap();
        let mut events = owner
            .open_context(None, false, queue::Context::default(), |_| {})
            .await
            .unwrap();
        let welcome = next_json(&mut events).await;
        assert_eq!(welcome["type"], "mux.welcome", "{welcome}");
        assert!(
            welcome["capabilities"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("ssh-prompts-v1")),
            "{welcome}"
        );
        let mut api = owner
            .open_context(None, false, queue::Context::default(), |_| {})
            .await
            .unwrap();
        let (status, prompts) = request(&mut api, "ssh.prompts.get", serde_json::json!({})).await;
        assert_eq!((status, prompts), (200, serde_json::json!([])));

        let target = splitty_remote::ssh::SshTarget::parse(PROMPTED).unwrap();
        let opening = {
            let owner = owner.clone();
            let target = target.clone();
            tokio::spawn(async move {
                let mut steps = Vec::new();
                let result = owner
                    .open_context(Some(target), false, queue::Context::default(), |progress| {
                        steps.push(format!("{:?}", *progress));
                    })
                    .await;
                (result, steps)
            })
        };

        let trust = question(&mut events, &mut api).await;
        assert_eq!(trust["confirm"], true, "{trust}");
        // The waiting helper is the backend's own executable, started by an
        // `ssh` of the backend that names it as a forced askpass program.
        let backend = i32::try_from(owner.id().unwrap()).unwrap();
        let executable = fs::canonicalize(env!("CARGO_BIN_EXE_splitty")).unwrap();
        let helpers = askpass_helpers(PROMPTED);
        assert_eq!(helpers.len(), 1, "one waiting helper: {helpers:?}");
        let helper = helpers[0];
        assert_eq!(
            fs::read_link(format!("/proc/{helper}/exe")).unwrap(),
            executable
        );
        let ssh = parent_of(helper).unwrap();
        assert_eq!(parent_of(ssh), Some(backend), "the backend's own ssh");
        assert_eq!(
            environment_of(ssh, "SSH_ASKPASS").map(PathBuf::from),
            Some(executable.clone())
        );
        assert_eq!(
            environment_of(ssh, "SSH_ASKPASS_REQUIRE").as_deref(),
            Some("force")
        );
        assert!(
            environment_of(ssh, "DISPLAY").is_some(),
            "older OpenSSH needs DISPLAY"
        );
        // Reading a question takes longer than any handshake frame may.
        tokio::time::sleep(splitty_transport::wire::HANDSHAKE_TIMEOUT + Duration::from_secs(1))
            .await;
        answer(&mut api, &trust, Some("yes")).await;
        let passphrase = question(&mut events, &mut api).await;
        assert_eq!(passphrase["confirm"], false, "{passphrase}");
        assert!(
            passphrase["prompt"]
                .as_str()
                .unwrap()
                .starts_with("Enter passphrase for key"),
            "{passphrase}"
        );
        answer(&mut api, &passphrase, Some("wrong")).await;
        let again = question(&mut events, &mut api).await;
        assert_ne!(again["id"], passphrase["id"], "{again}");
        answer(&mut api, &again, Some(PASSPHRASE)).await;

        let (host, steps) = tokio::time::timeout(Duration::from_secs(120), opening)
            .await
            .expect("the host opens")
            .unwrap();
        assert!(
            steps.iter().any(|step| step.contains("bridge")),
            "{steps:?}"
        );
        let mut host = host.expect("the host stream is ready");
        let ready = next_json(&mut host).await;
        assert_eq!(ready["type"], "remote.ready", "{ready}");
        assert_eq!(ready["target"], PROMPTED, "{ready}");
        let welcome = next_json(&mut host).await;
        assert_eq!(welcome["type"], "mux.welcome", "{welcome}");
        assert!(lab.installed_server().is_file(), "installed on the host");
        assert!(
            fs::read_to_string(d.join("prompted_known_hosts"))
                .is_ok_and(|known| known.contains(PROMPTED)),
            "the confirmed host key is recorded"
        );
        host.finish().await.unwrap();

        // Settled questions are gone; a cancelled one fails the next open.
        let (_, prompts) = request(&mut api, "ssh.prompts.get", serde_json::json!({})).await;
        assert_eq!(prompts, serde_json::json!([]));
        let opening = {
            let owner = owner.clone();
            tokio::spawn(async move {
                owner
                    .open_context(Some(target), false, queue::Context::default(), |_| {})
                    .await
            })
        };
        let passphrase = question(&mut events, &mut api).await;
        answer(&mut api, &passphrase, None).await;
        let error = tokio::time::timeout(Duration::from_secs(120), opening)
            .await
            .expect("the cancelled open ends")
            .unwrap()
            .err()
            .expect("a cancelled question fails the open");
        assert!(error.to_string().contains(r#""code":"auth""#), "{error}");

        api.finish().await.unwrap();
        events.finish().await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), owner.close_result())
            .await
            .unwrap()
            .unwrap();
    });
}
