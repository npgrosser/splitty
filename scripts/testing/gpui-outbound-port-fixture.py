#!/usr/bin/env python3
"""Owned HTTP/SSH-forward shim for native Ports window acceptance.

The two remote daemons and PTYs come from gpui-remote-screen-fixture.py. This
shim accepts only their exact forward-only SSH command, binds an owned local
IPv4 loopback listener and relays to an HTTP process launched by each private
PTY shell. It is a fixture transport, not evidence of real OpenSSH behavior.
"""

import importlib.util
import json
import os
from pathlib import Path
import re
import select
import shlex
import signal
import socket
import sys
import threading
import time

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("remote_screen_fixture", HERE / "gpui-remote-screen-fixture.py")
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)

HARDENING = ("-o", "ExitOnForwardFailure=yes", "-o", "BatchMode=yes", "-o",
             "ConnectTimeout=10", "-o", "ServerAliveInterval=15", "-o",
             "ServerAliveCountMax=3")
FORWARD_ONLY = ("-N", "-T", "-a", "-x")
EXTRA = ("-o", "ControlMaster=no", "-o", "ControlPath=none", "-o",
         "ControlPersist=no", "-o", "ForkAfterAuthentication=no", "-o",
         "PermitLocalCommand=yes", "-o", "LocalCommand=echo SPLITTY_FORWARD_READY_V1", "-L")
ADDRESS = {base.TARGETS[0]: "127.0.0.2", base.TARGETS[1]: "127.0.0.3"}


def write_record(root, **record):
    with (root / "ssh-calls.jsonl").open("a") as output:
        output.write(json.dumps(record) + "\n")


def relay(client, address, remote):
    try:
        with client, socket.create_connection((address, remote), timeout=4) as upstream:
            client.setblocking(False)
            upstream.setblocking(False)
            peers = {client: upstream, upstream: client}
            while peers:
                readable, _, _ = select.select(list(peers), [], [], 8)
                if not readable:
                    break
                for source in readable:
                    try:
                        data = source.recv(65536)
                    except OSError:
                        return
                    if not data:
                        return
                    try:
                        peers[source].sendall(data)
                    except OSError:
                        return
    except OSError:
        return


def forward(root, args):
    prefix = HARDENING + FORWARD_ONLY
    if tuple(args[:len(prefix)]) != prefix:
        return None
    tail = args[len(prefix):]
    if len(tail) == 2 and tail[0] == "-G" and tail[1] in ADDRESS:
        target = tail[1]
        write_record(root, target=target, kind="config", accepted=True)
        print(f"hostname {target}", flush=True)
        return 0
    if len(tail) != len(EXTRA) + 2 or tuple(tail[:len(EXTRA)]) != EXTRA:
        write_record(root, target=tail[-1] if tail else None,
                     kind="invalid-forward-shape", accepted=False)
        return 97
    specification, target = tail[-2:]
    match = re.fullmatch(r"127\.0\.0\.1:([0-9]{1,5}):localhost:([0-9]{1,5})", specification)
    config = json.loads((root / "port-fixture.json").read_text())
    if target not in ADDRESS or not match:
        write_record(root, target=target, kind="invalid-forward-key", accepted=False)
        return 97
    local, remote = map(int, match.groups())
    if not 1 <= local <= 65535 or remote != config["remotePort"]:
        write_record(root, target=target, kind="unowned-forward-port", accepted=False)
        return 97
    gate = root / "forward-gate.json"
    if gate.exists() and json.loads(gate.read_text()) == {"target": target, "remote": remote}:
        (root / "forward-gate-entered.json").write_text(json.dumps({
            "target": target, "remote": remote, "local": local,
            "pid": os.getpid(), "start": base.process_start(os.getpid())}) + "\n")
        deadline = time.monotonic() + 12
        while not (root / "forward-gate-release").exists() and time.monotonic() < deadline:
            time.sleep(.04)
        if not (root / "forward-gate-release").exists():
            write_record(root, target=target, kind="forward-gate-timeout", accepted=False)
            return 97
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 0)
        listener.bind(("127.0.0.1", local))
        listener.listen(16)
    except OSError as error:
        print(f"owned forward bind failed: {error}", file=sys.stderr, flush=True)
        write_record(root, target=target, kind="forward-bind-failed", local=local,
                     remote=remote, accepted=False)
        listener.close()
        return 97
    write_record(root, target=target, kind="forward", local=local, remote=remote,
                 pid=os.getpid(), start=base.process_start(os.getpid()), accepted=True)
    print("SPLITTY_FORWARD_READY_V1", flush=True)
    try:
        while True:
            client, _ = listener.accept()
            threading.Thread(target=relay, args=(client, ADDRESS[target], remote), daemon=True).start()
    finally:
        listener.close()


def fake_ssh(root, args):
    root = Path(root).resolve(strict=True)
    handled = forward(root, args)
    if handled is not None:
        return handled
    return base.fake_ssh(root, args)


class OutboundPortFixture(base.RemoteScreenFixture):
    """Private Screen fixture plus exact forward shim and session-owned HTTP."""

    def _ssh_program(self):
        bin_dir = self.root / "bin"
        bin_dir.mkdir(mode=0o700)
        ssh = bin_dir / "ssh"
        ssh.write_text(f"#!{sys.executable}\n"
                       "import runpy, sys\n"
                       f"sys.argv = [{str(Path(__file__).resolve())!r}, '--fake-ssh', {str(self.root)!r}, *sys.argv[1:]]\n"
                       f"runpy.run_path({str(Path(__file__).resolve())!r}, run_name='__main__')\n")
        ssh.chmod(0o700)
        self.ssh = ssh

    def _prepare(self):
        super()._prepare()
        self.http_port = self._free_remote_port()
        (self.root / "port-fixture.json").write_text(json.dumps({"remotePort": self.http_port}) + "\n")
        script = self.root / "owned-http.py"
        script.write_text(
            "import http.server,json,os,sys\n"
            "from pathlib import Path\n"
            "address,port,target,ready=sys.argv[1:]\n"
            "class Handler(http.server.BaseHTTPRequestHandler):\n"
            " def do_GET(self):\n"
            "  body=json.dumps({'fixture':'splitty-ports-v1','target':target,'path':self.path,'pid':os.getpid()}).encode()\n"
            "  self.send_response(200);self.send_header('Content-Type','application/json');"
            "self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)\n"
            " def log_message(self,*args):pass\n"
            "server=http.server.ThreadingHTTPServer((address,int(port)),Handler)\n"
            "Path(ready).write_text(str(os.getpid())+'\\n')\n"
            "server.serve_forever()\n")
        self.http = {}
        for target in base.TARGETS:
            host = self.hosts[target]
            ready = self.root / ("http-a.pid" if target == base.TARGETS[0] else "http-b.pid")
            line = (f"/usr/bin/python3 -u {shlex.quote(str(script))} {ADDRESS[target]} "
                    f"{self.http_port} {shlex.quote(target)} {shlex.quote(str(ready))} "
                    f"> {shlex.quote(str(self.root / (target + '.http.log')))} 2>&1 &")
            self._run([str(self.binary), "send-keys", "--screen", host["screen"],
                       "--pane", host["pane"], line + "\n"], self._host_env(target))
            deadline = time.monotonic() + 8
            while not ready.exists() and time.monotonic() < deadline:
                time.sleep(.08)
            if not ready.exists():
                raise TimeoutError(f"owned HTTP server did not start for {target}")
            pid = int(ready.read_text().strip())
            if not base.alive(pid) or str(script).encode() not in Path(f"/proc/{pid}/cmdline").read_bytes():
                raise RuntimeError(f"owned HTTP identity invalid for {target}")
            self.http[target] = {"pid": pid, "start": base.process_start(pid),
                                 "address": ADDRESS[target], "port": self.http_port}

    @staticmethod
    def _free_remote_port():
        for _ in range(16):
            with socket.socket() as first:
                first.bind((ADDRESS[base.TARGETS[0]], 0))
                port = first.getsockname()[1]
                try:
                    with socket.socket() as second:
                        second.bind((ADDRESS[base.TARGETS[1]], port))
                    return port
                except OSError:
                    continue
        raise RuntimeError("no paired private HTTP port available")

    def manifest(self):
        value = super().manifest()
        value["http"] = self.http
        value["transport"] = "owned exact-shape SSH forwarding shim; real OpenSSH not exercised by this fixture"
        return value

    def close(self):
        if self.closed or self.root is None:
            return
        def stop_owned(pid, started, needle):
            if not (type(pid) is int and base.alive(pid) and base.process_start(pid) == started):
                return
            if needle not in Path(f"/proc/{pid}/cmdline").read_bytes():
                raise RuntimeError(f"owned fixture process {pid} command identity changed")
            pidfd = os.pidfd_open(pid)
            try:
                signal.pidfd_send_signal(pidfd, signal.SIGTERM)
                deadline = time.monotonic() + 5
                while base.alive(pid) and base.process_start(pid) == started and time.monotonic() < deadline:
                    time.sleep(.05)
                if base.alive(pid) and base.process_start(pid) == started:
                    signal.pidfd_send_signal(pidfd, signal.SIGKILL)
            finally:
                os.close(pidfd)
        # The service should stop its forward children on window close. This
        # fallback only touches matching private shim identities.
        calls = self.root / "ssh-calls.jsonl"
        if calls.exists():
            for record in (json.loads(line) for line in calls.read_text().splitlines()):
                if record.get("kind") != "forward" or not record.get("accepted"):
                    continue
                stop_owned(record.get("pid"), record.get("start"), str(self.ssh).encode())
        pending = self.root / "forward-gate-entered.json"
        if pending.exists():
            record = json.loads(pending.read_text())
            stop_owned(record.get("pid"), record.get("start"), str(self.ssh).encode())
        for item in getattr(self, "http", {}).values():
            stop_owned(item["pid"], item["start"], str(self.root / "owned-http.py").encode())
        super().close()


def main():
    if len(sys.argv) >= 3 and sys.argv[1] == "--fake-ssh":
        raise SystemExit(fake_ssh(sys.argv[2], sys.argv[3:]))
    raise SystemExit("this file is an importable fixture, not a standalone test")


if __name__ == "__main__":
    main()
