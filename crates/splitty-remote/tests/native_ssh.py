#!/usr/bin/env python3
"""Fake OpenSSH executable: real child pipes and pinned host commands."""
import json
import shlex
import sys
from pathlib import Path

directory = Path(__file__).parent
mode = (directory / "mode").read_text()
state_file = directory / "state"
state = json.loads(state_file.read_text()) if state_file.exists() else {}
command = sys.argv[-1]
with (directory / "calls").open("a") as log:
    log.write(json.dumps(sys.argv[1:]) + "\n")


def save():
    state_file.write_text(json.dumps(state))


if "splitty-probe" in command:
    print("Linux\nx86_64\n/home/it's me\n" + "a" * 64)
elif "remote-handshake" in command:
    state["handshakes"] = state.get("handshakes", 0) + 1
    save()
    if mode == "missing" and state["handshakes"] == 1:
        print("pinned binary disappeared", file=sys.stderr)
        sys.exit(127)
    if mode == "stale_handshake" and state["handshakes"] == 1:
        print("daemon changed during screen discovery", file=sys.stderr)
        sys.exit(75)
    daemon = {
        "running": True, "pid": 47, "instance": state.get("instance", "a" * 32),
        "transportVersion": 1, "version": state.get("version", "0.1.29" if mode == "update" else "0.1.30"),
        "protocol": 1, "startedAt": "1", "keepsSessions": True, "started": False,
    }
    print(json.dumps({"handshake": 2, "server": {"version": "0.1.30", "protocol": 1,
        "path": "/home/it's me/.local/share/splitty/remote/0.1.30/splitty", "transportVersions": [1]},
        "daemon": daemon, "screens": []}))
elif "daemon stop" in command:
    words = shlex.split(command)
    assert words[-2:] == ["--if-instance", state.get("instance", "a" * 32)]
    state["instance"], state["version"] = "b" * 32, "0.1.30"
    save()
    print("stopped checked instance")
elif "remote-bridge" in command:
    assert shlex.split(command)[-3:] == ["remote-bridge", "--transport", "native-v1"]
    if mode == "auth_bridge":
        print("Permission denied (publickey).", file=sys.stderr)
        sys.exit(255)
    if mode == "stale":
        state["instance"] = "b" * 32
        save()
    ready = {"generation": 1, "instance": state.get("instance", "a" * 32), "muxProtocol": 1,
        "purpose": {"kind": "direct"}, "limits": {"frameBytes": 16777216, "apiBytes": 1048576,
        "pendingRequests": 64, "connectionBytes": 33554432}}
    sys.argv = ["native_bridge", json.dumps(ready), '{"type":"mux.welcome"}']
    exec((directory / "native_bridge.py").read_text())
else:
    raise AssertionError("unexpected host command: " + command)
