#!/usr/bin/env python3
"""Private two-host backend fixture for a native remote-Screen window harness.

Import RemoteScreenFixture as a context manager. Its ui_env() belongs to the
window process; its host daemons and shells remain alive until the context
exits, so a window harness can check detach and normal close before cleanup.
No display server or UI process is started here.
"""

import argparse
from contextlib import AbstractContextManager
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tarfile
import tempfile
import time


SCRUB = (
    "SPLITTY_SOCK", "SPLITTY_AGENT_SOCK", "SPLITTY_ADDR", "SPLITTY_TOKEN",
    "SPLITTY_SERVER", "SPLITTY_PANE_ID", "SPLITTY_SCREEN_ID",
    "SPLITTY_SESSION_ID", "SPLITTY_CONFIG_DIR", "SPLITTY_REMOTE_PAYLOAD",
    "ENV", "BASH_ENV", "ZDOTDIR", "CDPATH",
)
TARGETS = ("screen-fixture-a.invalid", "screen-fixture-b.invalid")
SHARED_SCREEN_ID = "fixture-shared-screen"
SHARED_PANE_ID = "fixture-shared-pane"
SSH_OPTIONS = (
    "-o", "ExitOnForwardFailure=yes", "-o", "BatchMode=yes",
    "-o", "ConnectTimeout=10", "-o", "ServerAliveInterval=15",
    "-o", "ServerAliveCountMax=3", "-T", "-a", "-x",
    "-o", "ClearAllForwardings=yes", "-o", "ControlMaster=no",
    "-o", "ControlPath=none", "-o", "ControlPersist=no",
    "-o", "ForkAfterAuthentication=no", "-o", "StdinNull=no",
)
PROBE = ('printf "%s\\n" "$(uname -s)" "$(uname -m)" "$HOME"; '
         'd="$HOME/.local/share/splitty/remote/$1"; '
         'if [ -x "$d/splitty" ] && [ -f "$d/.payload-sha256" ]; '
         'then cat "$d/.payload-sha256"; fi')


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def alive(pid):
    try:
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0] != "Z"
    except FileNotFoundError:
        return False


def process_start(pid):
    try:
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19]
    except FileNotFoundError:
        return None


class PrivateControl(http.client.HTTPConnection):
    def __init__(self, path):
        super().__init__("localhost", timeout=5)
        self.path = path

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(5)
        self.sock.connect(str(self.path))


class RemoteScreenFixture(AbstractContextManager):
    def __init__(self, binary=Path("target/release/splitty")):
        self.source_binary = Path(binary).resolve(strict=True)
        self.root = None
        self.hosts = {}
        self.started = []
        self.stopped = set()
        self.closed = False

    def __enter__(self):
        if sys.platform != "linux" or os.uname().machine != "x86_64":
            raise RuntimeError("this fixture requires Linux x86_64")
        self.root = Path(tempfile.mkdtemp(prefix="snr-", dir="/tmp"))
        self.root.chmod(0o700)
        try:
            self._prepare()
            return self
        except BaseException as error:
            try:
                self.close()
            except Exception as cleanup_error:
                error.add_note(f"fixture cleanup also failed: {cleanup_error}")
            raise

    def _prepare(self):
        self.binary = self.root / "splitty"
        shutil.copy2(self.source_binary, self.binary)
        self.binary.chmod(0o500)
        self.binary_sha256 = sha256(self.binary)
        version_line = self._run([str(self.binary), "--version"], self._base_env()).stdout.strip()
        match = re.fullmatch(r"splitty ([A-Za-z0-9][A-Za-z0-9.+-]{0,63})", version_line)
        if not match:
            raise RuntimeError(f"unexpected server version: {version_line!r}")
        self.version = match.group(1)
        (self.root / "version").write_text(self.version + "\n")
        (self.root / "binary.sha256").write_text(self.binary_sha256 + "\n")
        self.payload = self.root / "payload.tar.gz"
        with tarfile.open(self.payload, "w:gz") as archive:
            info = tarfile.TarInfo("splitty")
            info.size = self.binary.stat().st_size
            info.mode = 0o755
            with self.binary.open("rb") as source:
                archive.addfile(info, source)
        self.payload_sha256 = sha256(self.payload)
        self.local_config = self.root / "local-config"
        self._config(self.local_config, self.root / "local-shells", "fixture-local")
        for target, name in zip(TARGETS, ("Alpha work", "Beta work")):
            slug = "a" if target == TARGETS[0] else "b"
            host_root = self.root / slug
            installed = host_root / ".local" / "share" / "splitty" / "remote" / self.version
            installed.mkdir(parents=True, mode=0o700)
            shutil.copy2(self.binary, installed / "splitty")
            (installed / "splitty").chmod(0o500)
            (installed / ".payload-sha256").write_text(self.payload_sha256 + "\n")
            config = host_root / "config"
            markers = host_root / "shells"
            self._config(config, markers, f"fixture-{slug}")
            self.hosts[target] = {"root": host_root, "config": config,
                                  "installed": installed / "splitty", "markers": markers,
                                  "name": name}
        (self.local_config / "recent-hosts.json").write_text(json.dumps({
            "version": 1, "hosts": [
                {"target": target, "lastConnected": 1760000000 - index}
                for index, target in enumerate(TARGETS)]}) + "\n")
        (self.local_config / "remote-host-order.json").write_text(json.dumps({
            "version": 1, "order": list(TARGETS)}) + "\n")
        self._ssh_program()
        for target in TARGETS:
            host = self.hosts[target]
            self.started.append(target)
            self._run([str(self.binary), "daemon", "start"], self._host_env(target))
            host["daemon"] = self._discovered_daemon(target)
            reply = self._create_shared_screen(target)
            if reply.get("screenId") != SHARED_SCREEN_ID or reply.get("created", [{}])[0].get("paneId") != SHARED_PANE_ID:
                raise RuntimeError(f"private create_screen returned unexpected IDs on {target}: {reply}")
            screens = self._json_cli(target, "list-screens", "--json")
            screen = next(item for item in screens if item.get("name") == host["name"])
            if screen["id"] != SHARED_SCREEN_ID:
                raise RuntimeError(f"private Screen ID changed on {target}")
            host["screen"] = screen["id"]
            panes = self._json_cli(target, "list-panes", "--screen", screen["id"], "--json")
            if len(panes) != 1:
                raise RuntimeError(f"unexpected pane count on {target}: {len(panes)}")
            pane, details = next(iter(panes.items()))
            if pane != SHARED_PANE_ID:
                raise RuntimeError(f"private pane ID changed on {target}")
            host["pane"] = pane
            host["session"] = details["sessionId"]
            self._wait_for_marker(target)
        self.assert_sessions_alive()

    def _config(self, config, markers, profile_id):
        config.mkdir(mode=0o700)
        markers.mkdir(mode=0o700)
        wrapper = markers / "sh"
        wrapper.write_text(
            "#!/bin/sh\n"
            f"cd '{markers}' || exit 1\n"
            "printf '%s\\t%s\\t%s\\n' \"$$\" \"$SPLITTY_PANE_ID\" "
            "\"$SPLITTY_SESSION_ID\" > \"shell-$$.tsv\"\n"
            "exec /bin/sh \"$@\"\n")
        wrapper.chmod(0o700)
        (config / "config.toml").write_text(
            '[appearance]\ntheme_mode = "fixed"\ntheme = "splitty-dark"\n\n'
            f'[shells]\ndefault_profile_id = "{profile_id}"\n\n'
            f'[[shells.profiles]]\nid = "{profile_id}"\nname = "Fixture shell"\n'
            f'command = {json.dumps(str(wrapper))}\n'
            'enabled = true\ntarget = "local"\nsource = "custom"\n')

    def _ssh_program(self):
        bin_dir = self.root / "bin"
        bin_dir.mkdir(mode=0o700)
        ssh = bin_dir / "ssh"
        # The helper is re-executed with a private fixture root. It never
        # invokes a login shell, host resolver, installer or real ssh binary.
        ssh.write_text(f"#!{sys.executable}\n"
                       "import runpy, sys\n"
                       f"sys.argv = [{str(Path(__file__).resolve())!r}, '--fake-ssh', {str(self.root)!r}, *sys.argv[1:]]\n"
                       f"runpy.run_path({str(Path(__file__).resolve())!r}, run_name='__main__')\n")
        ssh.chmod(0o700)
        self.ssh = ssh

    def _base_env(self):
        env = os.environ.copy()
        for key in SCRUB:
            env.pop(key, None)
        env["PATH"] = str(self.root / "bin") + ":/usr/bin:/bin"
        return env

    def _host_env(self, target):
        env = self._base_env()
        env["SPLITTY_CONFIG_DIR"] = str(self.hosts[target]["config"])
        return env

    def ui_env(self):
        env = self._base_env()
        env["SPLITTY_CONFIG_DIR"] = str(self.local_config)
        env["SPLITTY_REMOTE_PAYLOAD"] = str(self.payload)
        return env

    def _run(self, argv, env, timeout=15):
        result = subprocess.run(argv, env=env, cwd=self.root, capture_output=True,
                                text=True, timeout=timeout)
        if result.returncode:
            raise RuntimeError(f"{argv[:3]} exited {result.returncode}: {result.stderr[-1000:]}")
        return result

    def _json_cli(self, target, *arguments):
        return json.loads(self._run([str(self.binary), *arguments], self._host_env(target)).stdout)

    def _create_shared_screen(self, target):
        host = self.hosts[target]
        control = host["config"] / "run" / "daemon.sock"
        if not stat.S_ISSOCK(control.lstat().st_mode):
            raise RuntimeError(f"private control socket absent on {target}")
        profile = "fixture-a" if target == TARGETS[0] else "fixture-b"
        body = json.dumps({"screenId": "", "op": {
            "type": "create_screen", "screenId": SHARED_SCREEN_ID,
            "newPaneId": SHARED_PANE_ID, "name": host["name"],
            "shellProfileId": profile,
        }})
        connection = PrivateControl(control)
        try:
            connection.request("POST", "/api/mux/op", body=body,
                               headers={"Content-Type": "application/json"})
            response = connection.getresponse()
            payload = response.read(65537)
            if len(payload) > 65536 or response.status != 200:
                raise RuntimeError(f"private create_screen failed on {target}: HTTP {response.status}: {payload[:500]!r}")
            value = json.loads(payload)
            if value.get("ok") is not True:
                raise RuntimeError(f"private create_screen refused on {target}: {value}")
            return value
        finally:
            connection.close()

    def _discovered_daemon(self, target):
        info = json.loads((self.hosts[target]["config"] / "daemon.json").read_text())
        instance, pid = info.get("instance"), info.get("pid")
        if not isinstance(instance, str) or not re.fullmatch(r"[0-9a-f]{32}", instance):
            raise RuntimeError(f"invalid private daemon instance on {target}")
        if type(pid) is not int or pid <= 0 or not alive(pid):
            raise RuntimeError(f"invalid private daemon pid on {target}")
        if Path(f"/proc/{pid}/exe").resolve(strict=True) != self.binary:
            raise RuntimeError(f"private daemon executable differs on {target}")
        return {"instance": instance, "pid": pid, "start": process_start(pid)}

    def _live_shell_markers(self, target):
        host = self.hosts[target]
        found = {}
        for marker in host["markers"].glob("shell-*.tsv"):
            try:
                pid_text, pane, session = marker.read_text().strip().split("\t")
                pid = int(pid_text)
            except (ValueError, OSError) as error:
                raise RuntimeError(f"invalid private shell marker {marker}: {error}") from error
            if marker.name != f"shell-{pid}.tsv" or not pane or not session:
                raise RuntimeError(f"invalid private shell marker {marker}")
            if not alive(pid):
                continue
            environment = Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
            expected = (
                f'SPLITTY_CONFIG_DIR={host["config"]}'.encode(),
                f"SPLITTY_PANE_ID={pane}".encode(),
                f"SPLITTY_SESSION_ID={session}".encode(),
            )
            if not all(entry in environment for entry in expected):
                # A marker can outlive its shell; this PID now belongs elsewhere.
                continue
            found[pid] = process_start(pid)
        return found

    def _wait_for_marker(self, target):
        host = self.hosts[target]
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            for marker in host["markers"].glob("shell-*.tsv"):
                try:
                    pid, pane, session = marker.read_text().strip().split("\t")
                except ValueError:
                    continue
                if pane == host["pane"] and session == host["session"] and alive(int(pid)):
                    host["shell_pid"] = int(pid)
                    host["shell_start"] = process_start(int(pid))
                    return
            time.sleep(.08)
        raise TimeoutError(f"owned shell did not start on {target}")

    def assert_sessions_alive(self):
        for target, host in self.hosts.items():
            if "shell_pid" in host:
                assert alive(host["shell_pid"]), f"{target} shell exited"
                screens = self._json_cli(target, "list-screens", "--json")
                assert any(s["id"] == host["screen"] for s in screens), target

    def manifest(self):
        return {"root": str(self.root), "binarySha256": self.binary_sha256,
                "payloadSha256": self.payload_sha256, "version": self.version,
                "sharedScreenId": SHARED_SCREEN_ID, "sharedPaneId": SHARED_PANE_ID,
                "localConfig": str(self.local_config), "ssh": str(self.ssh),
                "hosts": {target: {key: str(value) if isinstance(value, Path) else value
                                   for key, value in host.items()}
                          for target, host in self.hosts.items()}}

    def close(self):
        if self.closed or self.root is None:
            return
        errors = []
        calls = self.root / "ssh-calls.jsonl"
        if calls.exists():
            try:
                items = [json.loads(line) for line in calls.read_text().splitlines()]
            except Exception as error:
                items = []
                errors.append(f"private SSH log unreadable: {error}")
            for item in items:
                if item.get("kind") != "bridge" or not item.get("accepted"):
                    continue
                pid, start, target = item.get("pid"), item.get("start"), item.get("target")
                if target not in self.hosts or type(pid) is not int or not isinstance(start, str):
                    errors.append(f"invalid private bridge record: {item}")
                    continue
                if not alive(pid) or process_start(pid) != start:
                    continue
                try:
                    expected = self.hosts[target]["installed"]
                    if Path(f"/proc/{pid}/exe").resolve(strict=True) != expected:
                        raise RuntimeError(f"bridge pid {pid} executable changed")
                    os.kill(pid, signal.SIGTERM)
                    deadline = time.monotonic() + 3
                    while alive(pid) and process_start(pid) == start and time.monotonic() < deadline:
                        time.sleep(.05)
                    if alive(pid) and process_start(pid) == start:
                        os.kill(pid, signal.SIGKILL)
                        deadline = time.monotonic() + 3
                        while alive(pid) and process_start(pid) == start and time.monotonic() < deadline:
                            time.sleep(.05)
                    if alive(pid) and process_start(pid) == start:
                        raise RuntimeError(f"owned bridge pid {pid} remained after SIGKILL")
                except Exception as error:
                    errors.append(str(error))
        for target in reversed(self.started):
            if target in self.stopped:
                continue
            host = self.hosts[target]
            info_path = host["config"] / "daemon.json"
            try:
                if "daemon" not in host:
                    host["daemon"] = self._discovered_daemon(target)
                daemon = host["daemon"]
                if info_path.exists():
                    info = json.loads(info_path.read_text())
                    if info.get("instance") != daemon["instance"] or info.get("pid") != daemon["pid"]:
                        raise RuntimeError(f"private daemon identity changed on {target}; refusing cleanup")
                elif alive(daemon["pid"]) and process_start(daemon["pid"]) == daemon["start"]:
                    raise RuntimeError(f"private daemon discovery disappeared while pid {daemon['pid']} lives")
                shells = self._live_shell_markers(target)
                self._run([str(self.binary), "daemon", "stop", "--end-sessions"],
                          self._host_env(target), timeout=20)
                deadline = time.monotonic() + 8
                while time.monotonic() < deadline and (
                    (alive(daemon["pid"]) and process_start(daemon["pid"]) == daemon["start"])
                    or any(alive(pid) and process_start(pid) == start for pid, start in shells.items())
                ):
                    time.sleep(.1)
                if alive(daemon["pid"]) and process_start(daemon["pid"]) == daemon["start"]:
                    raise RuntimeError(f"owned daemon remained on {target}")
                if any(alive(pid) and process_start(pid) == start for pid, start in shells.items()):
                    raise RuntimeError(f"owned shell remained on {target}")
                if self._live_shell_markers(target):
                    raise RuntimeError(f"new private shell remained on {target}")
                self.stopped.add(target)
            except Exception as error:
                errors.append(str(error))
        if not errors:
            shutil.rmtree(self.root)
            self.started.clear()
            self.closed = True
        if errors:
            raise RuntimeError("; ".join(errors))

    def __exit__(self, exc_type, exc, tb):
        try:
            self.close()
        except Exception as cleanup_error:
            if exc is None:
                raise
            exc.add_note(f"fixture cleanup also failed: {cleanup_error}")
        return False


def fake_ssh(root, args):
    import shlex
    root = Path(root).resolve(strict=True)
    log = root / "ssh-calls.jsonl"
    allowed = {target: root / slug for target, slug in
               ((TARGETS[0], "a"), (TARGETS[1], "b"))}
    accepted = False
    try:
        if tuple(args[:len(SSH_OPTIONS)]) != SSH_OPTIONS:
            raise ValueError("SSH options differ from command-only policy")
        tail = args[len(SSH_OPTIONS):]
        if len(tail) != 3 or tail[0] not in allowed or tail[1] != "--":
            raise ValueError("unknown target or command shape")
        target, _, command = tail
        host_root = allowed[target]
        version = (root / "version").read_text().strip()
        words = shlex.split(command)
        probe = ["sh", "-c", PROBE, "splitty-probe", version]
        installed = host_root / ".local" / "share" / "splitty" / "remote" / version / "splitty"
        if words == probe:
            marker = installed.parent / ".payload-sha256"
            if not installed.is_file() or not os.access(installed, os.X_OK):
                raise ValueError("private installed binary absent")
            if sha256(installed) != (root / "binary.sha256").read_text().strip():
                raise ValueError("private installed binary changed")
            if marker.read_text().strip() != sha256(root / "payload.tar.gz"):
                raise ValueError("private payload marker changed")
            print("Linux\nx86_64\n" + str(host_root) + "\n" + marker.read_text().strip())
            accepted = True
        elif words == [str(installed), "remote-bridge", "--transport", "native-v1"]:
            if sha256(installed) != (root / "binary.sha256").read_text().strip():
                raise ValueError("private installed binary changed")
            env = os.environ.copy()
            for key in SCRUB:
                env.pop(key, None)
            env["SPLITTY_CONFIG_DIR"] = str(host_root / "config")
            accepted = True
            with log.open("a") as output:
                output.write(json.dumps({"target": target, "kind": "bridge", "accepted": True,
                                         "pid": os.getpid(), "start": process_start(os.getpid())}) + "\n")
            os.execve(installed, [str(installed), "remote-bridge", "--transport", "native-v1"], env)
        else:
            raise ValueError("remote command is not probe or native bridge")
        kind = "probe"
    except Exception as error:
        accepted = False
        kind = str(error)
    with log.open("a") as output:
        output.write(json.dumps({"target": args[len(SSH_OPTIONS)] if len(args) > len(SSH_OPTIONS) else None,
                                 "kind": kind, "accepted": accepted}) + "\n")
    return 0 if accepted else 97


def main():
    if len(sys.argv) >= 3 and sys.argv[1] == "--fake-ssh":
        raise SystemExit(fake_ssh(sys.argv[2], sys.argv[3:]))
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/splitty"))
    parser.add_argument("--self-check", action="store_true")
    args = parser.parse_args()
    with RemoteScreenFixture(args.binary) as fixture:
        if args.self_check:
            a, b = (fixture.hosts[target] for target in TARGETS)
            assert a["screen"] == b["screen"] == SHARED_SCREEN_ID
            assert a["pane"] == b["pane"] == SHARED_PANE_ID
            assert a["session"] != b["session"] and a["shell_pid"] != b["shell_pid"]
            target = TARGETS[0]
            command = "sh -c " + __import__("shlex").quote(PROBE) + " splitty-probe " + __import__("shlex").quote(fixture.version)
            result = fixture._run([str(fixture.ssh), *SSH_OPTIONS, target, "--", command], fixture.ui_env())
            assert result.stdout.splitlines() == ["Linux", "x86_64", str(fixture.hosts[target]["root"]), fixture.payload_sha256]
            bad = subprocess.run([str(fixture.ssh), *SSH_OPTIONS, "unknown.invalid", "--", command],
                                 env=fixture.ui_env(), capture_output=True, timeout=5)
            assert bad.returncode == 97
            install = subprocess.run([str(fixture.ssh), *SSH_OPTIONS, target, "--", "sh -c 'echo install'"],
                                     env=fixture.ui_env(), capture_output=True, timeout=5)
            assert install.returncode == 97
            bridge = fixture.hosts[target]["installed"]
            ended = subprocess.run([str(fixture.ssh), *SSH_OPTIONS, target, "--",
                                    f"'{bridge}' remote-bridge --transport native-v1"],
                                   env=fixture.ui_env(), input=b"", capture_output=True, timeout=8)
            assert ended.returncode != 97, ended.stderr[-1000:]
            bridge.chmod(0o400)
            try:
                refused = subprocess.run([str(fixture.ssh), *SSH_OPTIONS, target, "--",
                                          f"'{bridge}' remote-bridge --transport native-v1"],
                                         env=fixture.ui_env(), input=b"", capture_output=True, timeout=8)
                assert refused.returncode == 97, refused.stderr[-1000:]
            finally:
                bridge.chmod(0o500)
            calls = [json.loads(line) for line in (fixture.root / "ssh-calls.jsonl").read_text().splitlines()]
            assert [item["accepted"] for item in calls] == [True, False, False, True, True, False], calls
            fixture.assert_sessions_alive()
            host = fixture.hosts[target]
            fixture._run([str(fixture.binary), "split", "--screen", host["screen"],
                          "--pane", host["pane"]], fixture._host_env(target))
            deadline = time.monotonic() + 8
            while len(fixture._live_shell_markers(target)) < 2 and time.monotonic() < deadline:
                time.sleep(.08)
            assert len(fixture._live_shell_markers(target)) >= 2, "split shell marker absent"
        result = fixture.manifest()
        if args.self_check:
            fixture.close()
            fixture.close()
            assert fixture.closed and not fixture.root.exists()
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
