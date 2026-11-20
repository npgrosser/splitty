"""Machines to reach over SSH, such as `ssh devbox`, for the scenes of remote
screens.

Each host is an OpenSSH server in an unprivileged user, mount, network and UTS
namespace of its own, as the SSH lab of the server's smoke tests builds it:

- a holder process keeps the namespace: its own loopback (so the host's
  daemon binds its ports apart from the app's), the host's name, and a
  root of bind mounts with the system's trees and the lab directory;
- `sshd -i`, one per connection, started in that namespace by the client's
  `ProxyCommand` through `nsenter`, with throwaway keys and a `ForceCommand`
  that gives the host a HOME of its own, `/home/demo` in its namespace;
- a wrapper `ssh` first in the app's `PATH` that adds `-F ~/.ssh/config`
  (OpenSSH reads the account's home, not `HOME`), so the app runs the plain
  `ssh` it always runs, and `~/.ssh/config` names the hosts for the Hosts list;
- the host's HOME is a copy of the demo home (stand-in programs, project)
  with a prompt that names the host, and the app's `splitty` in its `PATH`.

Nothing outlives `stop()`: the host's daemon and its session keepers stay
in the host's network namespace after the holder and are found there.

Needs `sshd` (from `PATH`, or an unpacked `openssh-server` under
`--sshd-root`), `ssh`, `ssh-keygen`, `unshare`, `nsenter`, `ip`, `chroot`,
`hostname` and unprivileged user namespaces.
"""

from __future__ import annotations

import getpass
import os
import shutil
import signal
import subprocess
import tempfile
import time
from pathlib import Path

# In the environment of the host's processes: which lab they belong to.
MARK = "DEMO_HOST_LAB"

# The holder of the namespace: loopback up, the host's name, a root of bind
# mounts (the mount helper of the system would drop the namespace's
# capabilities, so Python calls mount(2) itself), then wait.
HOLDER = r"""
set -eu
lab=$1
package=$2
rootfs="$lab/rootfs"
ip link set lo up
hostname "$3"
python3 - "$rootfs" "$lab" "$package" <<'MOUNTS'
import ctypes, os, sys
rootfs, lab, package = sys.argv[1:]
libc = ctypes.CDLL(None, use_errno=True)
libc.mount.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_ulong, ctypes.c_void_p]
def bind(source, recursive=False, at=None):
    destination = rootfs + (at or source)
    os.makedirs(destination, exist_ok=True)
    if libc.mount(os.fsencode(source), os.fsencode(destination), None, 4096 | (16384 if recursive else 0), None):
        raise OSError(ctypes.get_errno(), "bind", source)
bind(lab)
if package != "/": bind(package)
for system in ["/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc", "/dev", "/proc", "/run"]:
    if os.path.exists(system): bind(system, True)
# The host's home where a reader expects it; the machine's own homes stay out.
bind(lab + "/home", at="/home/demo")
MOUNTS
exec chroot "$rootfs" /bin/sh -c 'cd /; echo ready; exec sleep infinity'
"""


def executable(path: Path, text: str):
    path.write_text(text)
    path.chmod(0o755)


class Host:
    def __init__(self, lab: Path, holder: subprocess.Popen, home: Path):
        self.lab, self.holder, self.home = lab, holder, home
        self.network = os.readlink(f"/proc/{holder.pid}/ns/net")

    def members(self) -> list[int]:
        """The processes of the host: everything in its network namespace,
        which outlives the holder while the host's daemon runs, anything
        started with the lab's mark, and the app's `ssh` to it (a port
        forward outlives the app)."""
        marker = f"\0{MARK}={self.lab}\0".encode()
        client = f"\0HOME={self.home}\0".encode()
        found = []
        for entry in os.listdir("/proc"):
            if not entry.isdigit() or int(entry) == os.getpid():
                continue
            try:
                inside = os.readlink(f"/proc/{entry}/ns/net") == self.network
                environ = b"\0" + Path(f"/proc/{entry}/environ").read_bytes()
                program = Path(f"/proc/{entry}/cmdline").read_bytes().split(b"\0")[0]
            except OSError:
                continue
            if inside or marker in environ or (client in environ and program.endswith(b"ssh")):
                found.append(int(entry))
        return found

    def stop(self):
        for attempt in range(3):
            members = self.members()
            if not members:
                break
            for pid in members:
                try:
                    os.kill(pid, signal.SIGTERM if attempt == 0 else signal.SIGKILL)
                except OSError:
                    pass
            time.sleep(0.6)
        if self.holder.poll() is None:
            self.holder.kill()
            self.holder.wait()
        shutil.rmtree(self.lab, ignore_errors=True)


class Hosts:
    """The hosts of a scene, stopped together."""

    def __init__(self, hosts: list[Host]):
        self.hosts = hosts

    def stop(self):
        for host in self.hosts:
            host.stop()


def start(root: Path, home: Path, environment: dict, args, names=("devbox",)) -> Hosts:
    """Start a host per name; the demo home's SSH configuration names them."""
    started = []
    try:
        for name in names:
            started.append(start_one(root, home, environment, args, name))
    except BaseException:
        Hosts(started).stop()
        raise
    return Hosts(started)


def start_one(root: Path, home: Path, environment: dict, args, name: str) -> Host:
    sshd_root = Path(getattr(args, "sshd_root", None) or os.environ.get("SPLITTY_DEMO_SSHD_ROOT")
                     or "/")
    if sshd_root == Path("/"):
        sshd = Path(shutil.which("sshd") or "/usr/sbin/sshd")
    else:
        sshd = sshd_root / "usr/sbin/sshd"
    if not sshd.is_file():
        raise RuntimeError(f"no sshd at {sshd} (install openssh-server or pass --sshd-root)")
    server = Path(getattr(args, "server", None) or Path(args.app).parent / "splitty").resolve()
    if not server.is_file():
        raise RuntimeError(f"no splitty server at {server} (pass --server)")
    for tool in ("ssh", "ssh-keygen", "unshare", "nsenter", "ip", "chroot", "hostname"):
        if not shutil.which(tool):
            raise RuntimeError(f"{tool} is not installed")

    # Short: the host's socket paths must fit a sockaddr_un.
    lab = Path(tempfile.mkdtemp(prefix="sd", dir="/tmp"))
    holder = None
    try:
        for sub in ("run", "bin"):
            (lab / sub).mkdir(mode=0o700)
        # A copy: the host sees none of this machine's homes.
        shutil.copy2(server, lab / "bin/splitty")
        host_home = lab / "home"
        shutil.copytree(home, host_home, symlinks=True)
        (host_home / ".bashrc").write_text(
            f"PS1='\\[\\e[1;32m\\]demo@{name}\\[\\e[0m\\] \\[\\e[1;34m\\]\\w\\[\\e[0m\\] \\$ '\n"
            "alias ls='ls --color=auto'\n"
            "export PATH=\"$HOME/bin:$PATH\"\n"
            "[ \"$PWD\" = \"$HOME\" ] && cd ~/project 2>/dev/null\n")
        (host_home / ".profile").write_text("[ -n \"$BASH\" ] && [ -f ~/.bashrc ] && . ~/.bashrc\n")

        ssh_dir = home / ".ssh"
        ssh_dir.mkdir(mode=0o700, exist_ok=True)
        # One key of the client for every host, one host key each.
        for key in (lab / "hostkey", ssh_dir / "id_ed25519"):
            if not key.exists():
                subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "demo",
                                "-f", str(key)], check=True, stdout=subprocess.DEVNULL)
        shutil.copy(ssh_dir / "id_ed25519.pub", lab / "authorized_keys")
        host_key = " ".join((lab / "hostkey.pub").read_text().split()[:2])
        with open(ssh_dir / "known_hosts", "a") as known:
            known.write(f"{name} {host_key}\n")

        executable(lab / "host-env.sh",
                   "#!/bin/sh\n"
                   "export HOME=/home/demo XDG_CONFIG_HOME=/home/demo/.config "
                   f"XDG_DATA_HOME=/home/demo/.local/share XDG_RUNTIME_DIR={lab}/run\n"
                   f"export USER=demo LOGNAME=demo SHELL=/bin/bash LANG=C.UTF-8 {MARK}={lab}\n"
                   f"export PATH={lab}/bin:/home/demo/bin:/usr/bin:/bin\n"
                   "cd \"$HOME\" || exit 1\n"
                   "if [ -n \"$SSH_ORIGINAL_COMMAND\" ]; then exec /bin/sh -c \"$SSH_ORIGINAL_COMMAND\"; fi\n"
                   "exec /bin/bash -l\n")
        config = (f"HostKey {lab}/hostkey\nAuthorizedKeysFile {lab}/authorized_keys\nUsePAM no\n"
                  "StrictModes no\nPidFile none\nPasswordAuthentication no\n"
                  "KbdInteractiveAuthentication no\nPubkeyAuthentication yes\n"
                  "AllowTcpForwarding yes\nGatewayPorts yes\n"
                  f"ForceCommand {lab}/host-env.sh\nLogLevel ERROR\n")
        libraries = []
        if sshd_root != Path("/"):
            # An unpacked package: its helpers and libraries are not where
            # sshd was built to look for them.
            for option, helper in (("SshdSessionPath", "usr/lib/openssh/sshd-session"),
                                   ("SshdAuthPath", "usr/lib/openssh/sshd-auth")):
                if (sshd_root / helper).is_file():
                    config += f"{option} {sshd_root / helper}\n"
            libraries = [str(sshd_root / lib) for lib in
                         ("usr/lib/x86_64-linux-gnu", "usr/lib/aarch64-linux-gnu")
                         if (sshd_root / lib).is_dir()]
        (lab / "sshd_config").write_text(config)
        executable(lab / "proxy.sh",
                   "#!/bin/sh\n"
                   f"exec env LD_LIBRARY_PATH={':'.join(libraries)} nsenter --target "
                   f"\"$(cat {lab}/ns.pid)\" --user --net --uts --mount --root --wd "
                   f"--preserve-credentials {sshd} -i -f {lab}/sshd_config\n")
        with open(ssh_dir / "config", "a") as client:
            client.write(
                f"Host {name}\n  User {getpass.getuser()}\n"
                f"  IdentityFile {ssh_dir}/id_ed25519\n  IdentitiesOnly yes\n"
                f"  UserKnownHostsFile {ssh_dir}/known_hosts\n  StrictHostKeyChecking yes\n"
                f"  ProxyCommand {lab}/proxy.sh\n  LogLevel ERROR\n")
        # OpenSSH reads ~/.ssh of the account, not of HOME: a wrapper first
        # in the app's PATH names the demo home's configuration.
        wrapper = root / "ssh-bin"
        if not wrapper.exists():
            wrapper.mkdir()
            executable(wrapper / "ssh",
                       f"#!/bin/sh\nexec {shutil.which('ssh')} -F \"$HOME/.ssh/config\" \"$@\"\n")
            environment["PATH"] = f"{wrapper}:{environment['PATH']}"

        holder = subprocess.Popen(
            ["unshare", "--user", "--map-current-user", "--keep-caps", "--mount", "--net",
             "--uts", "sh", "-c", HOLDER, "splitty-demo-host", str(lab), str(sshd_root), name],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        line = holder.stdout.readline().decode().strip()
        if line != "ready":
            holder.kill()
            raise RuntimeError(f"the host's namespace did not start: {holder.stderr.read().decode()}")
        (lab / "ns.pid").write_text(str(holder.pid))
        # The host's daemon runs before the scene, as on a machine in use.
        started = subprocess.run(
            [str(wrapper / "ssh"), "-o", "BatchMode=yes", name, "--", "splitty", "daemon", "start"],
            env=environment, capture_output=True, text=True, timeout=60)
        if started.returncode != 0:
            raise RuntimeError(f"the host does not answer: {started.stderr.strip()}")
        return Host(lab, holder, home)
    except BaseException:
        if holder is None:
            shutil.rmtree(lab, ignore_errors=True)
        else:
            Host(lab, holder, home).stop()
        raise
