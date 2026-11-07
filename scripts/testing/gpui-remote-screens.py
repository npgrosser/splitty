#!/usr/bin/env python3
"""Exercise native remote Screens in one owned X11 window and two private hosts.

Run --setup-only first to capture a window for geometry calibration. Later
phases require a screenshot-calibrated geometry JSON of named [x, y] points.
The backend fixture, window and display are owned by this run. Screenshots
record visible states; private CLI reads and shell markers prove backend effects.
"""

import argparse
from collections import deque
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shlex
import signal
import stat
import subprocess
import time
import tomllib

HERE = Path(__file__).resolve().parent


def helper(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


fixture_module = helper("remote_screen_fixture", "gpui-remote-screen-fixture.py")
performance = helper("performance", "gpui-performance.py")
pane_drag = helper("pane_drag", "gpui-pane-drag.py")

PHASES = ("setup", "connect", "open", "settings-remote", "terminal", "two-hosts", "recovery", "detach", "full")
POINTS = {
    "connect": ("hosts.toolbar", "hosts.connect_a"),
    "open": ("hosts.open_a",),
    "settings-remote": ("drawer.settings", "settings.theme.select", "settings.theme.option",
                        "terminal.body"),
    "terminal": ("drawer.local_row", "drawer.a_row"),
    "two-hosts": ("hosts.toolbar_remote", "hosts.connect_b", "hosts.open_b", "drawer.b_row"),
    "recovery": ("drawer.a_row", "drawer.b_row", "drawer.local_row"),
    "detach": ("drawer.a_row", "drawer.a_close", "hosts.toolbar_remote", "hosts.connect_a", "hosts.open_a"),
    "full": ("hosts.toolbar_remote", "hosts.open_disposable_b", "drawer.disposable_b_row",
             "drawer.disposable_b_more", "terminate.menu_item", "terminate.cancel", "terminate.confirm"),
}


def wait_until(predicate, description, seconds=15):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.08)
    raise TimeoutError(description)


def pid_live(pid):
    return fixture_module.alive(pid)


def run_window(args, geometry, result):
    fixture = fixture_module.RemoteScreenFixture(args.server_binary)
    owned = None
    xvfb = app = display = window = None
    xvfb_start = app_start = None
    app_children = {}
    normal_close = False
    try:
        owned = fixture.__enter__()
        result["fixture"] = owned.manifest()
        host_a, host_b = (owned.hosts[target] for target in fixture_module.TARGETS)
        assert host_a["screen"] == host_b["screen"] == fixture_module.SHARED_SCREEN_ID
        assert host_a["pane"] == host_b["pane"] == fixture_module.SHARED_PANE_ID
        assert host_a["session"] != host_b["session"]
        result["sharedRemoteIds"] = {"screen": host_a["screen"], "pane": host_a["pane"],
                                     "separateSessions": [host_a["session"], host_b["session"]]}
        with (owned.local_config / "config.toml").open("a") as local_config:
            local_config.write("\n[drawer]\npinned = true\nsidebar_width = 280\n")
        if args.phase == "full":
            b = fixture_module.TARGETS[1]
            owned._run([str(owned.binary), "new-screen", "--name", "Disposable B"],
                       owned._host_env(b))
            disposable = next(screen for screen in owned._json_cli(b, "list-screens", "--json")
                              if screen.get("name") == "Disposable B")
            result["disposableScreen"] = disposable["id"]
        env = owned.ui_env()
        env.pop("WAYLAND_DISPLAY", None)
        env["DISPLAY"] = args.display
        env["GPUI_BACKEND"] = "x11"
        xvfb_env = env.copy()
        if args.xvfb_lib:
            xvfb_env["LD_LIBRARY_PATH"] = str(args.xvfb_lib.resolve()) + (
                ":" + env["LD_LIBRARY_PATH"] if env.get("LD_LIBRARY_PATH") else "")

        def x(*arguments):
            return subprocess.check_output([str(args.xdotool), *map(str, arguments)],
                                           env=env, text=True, timeout=12)

        def point(name):
            value = geometry[name]
            if (not isinstance(value, list) or len(value) != 2 or
                    not all(type(component) is int for component in value)):
                raise ValueError(f"geometry {name} must be [x, y]")
            return value

        def click(name):
            x("mousemove", "--window", window, *point(name))
            x("click", 1)
            display.sync()
            time.sleep(.25)

        def key(value):
            x("key", "--clearmodifiers", value)
            display.sync()
            time.sleep(.2)

        def type_text(value):
            x("type", "--clearmodifiers", "--delay", 25, value)
            display.type_key("Return")
            display.sync()

        def capture(label):
            path = args.output / f"{label}.png"
            pane_drag.save_png(path, display.pixels(window, (0, 0, 1200, 760)))
            result["screenshots"].append(str(path))

        def local_state():
            run = owned.local_config / "run"
            sockets = list(run.glob("spike-*.sock")) if run.exists() else []
            if len(sockets) != 1 or not stat.S_ISSOCK(sockets[0].lstat().st_mode):
                return None
            try:
                body = subprocess.check_output(
                    ["curl", "--fail", "--silent", "--show-error", "--max-time", "5",
                     "--unix-socket", str(sockets[0]), "http://localhost/api/mux/state"],
                    env=env, text=True, timeout=7)
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
                return None
            return json.loads(body)

        def screens(target):
            return owned._json_cli(target, "list-screens", "--json")

        def panes(target, screen):
            return owned._json_cli(target, "list-panes", "--screen", screen, "--json")

        def inventory(target):
            return {screen["id"]: {pane: details["sessionId"] for pane, details in
                                    panes(target, screen["id"]).items()}
                    for screen in screens(target)}

        def ssh_calls():
            path = owned.root / "ssh-calls.jsonl"
            return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

        def host_bridge_started(target):
            calls = ssh_calls()
            return any(call.get("target") == target and call.get("kind") == "bridge" and
                       call.get("accepted") for call in calls)

        def bridge_records(target):
            return [call for call in ssh_calls() if call.get("target") == target and
                    call.get("kind") == "bridge" and call.get("accepted")]

        def drop_owned_bridge(target, record):
            # The fixture's accepted SSH record identifies the exec'd private
            # bridge. A pidfd keeps the signal tied to that process after the
            # start time, executable, command and private config are checked.
            if target not in owned.hosts or record not in bridge_records(target):
                raise RuntimeError("bridge record is outside this private fixture")
            pid, start = record.get("pid"), record.get("start")
            if type(pid) is not int or not isinstance(start, str):
                raise RuntimeError("private bridge record has no process identity")
            host = owned.hosts[target]
            expected_exe = host["installed"].resolve(strict=True)
            expected_argv = [str(expected_exe), "remote-bridge", "--transport", "native-v1"]
            expected_config = f'SPLITTY_CONFIG_DIR={host["config"]}'.encode()
            pidfd = os.pidfd_open(pid)
            try:
                proc = Path(f"/proc/{pid}")
                argv = [part.decode() for part in (proc / "cmdline").read_bytes().split(b"\0")
                        if part]
                environment = (proc / "environ").read_bytes().split(b"\0")
                if (not pid_live(pid) or fixture_module.process_start(pid) != start or
                        (proc / "exe").resolve(strict=True) != expected_exe or
                        argv != expected_argv or expected_config not in environment):
                    raise RuntimeError("private bridge identity changed; refusing signal")
                signal.pidfd_send_signal(pidfd, signal.SIGTERM)
            finally:
                os.close(pidfd)
            wait_until(lambda: not pid_live(pid) or fixture_module.process_start(pid) != start,
                       "private A bridge did not stop after SIGTERM", seconds=8)
            return {"pid": pid, "start": start, "exe": str(expected_exe)}

        def marker(target, label, expected=None):
            host = owned.hosts[target]
            path = owned.root / f"ui-{label}.tsv"
            # Input follows focus restored by Open, Tab, Split or Screen select.
            type_text("printf '%s\\t%s\\t%s\\n' \"$$\" \"$SPLITTY_PANE_ID\" "
                      f"\"$SPLITTY_SESSION_ID\" > {shlex.quote(str(path))}")
            value = wait_until(lambda: path.read_text().strip() if path.exists() else None,
                               f"terminal marker {label} did not arrive")
            pid, pane, session = value.split("\t")
            profile_id = "fixture-a" if target == fixture_module.TARGETS[0] else "fixture-b"
            assert f'default_profile_id = "{profile_id}"' in (host["config"] / "config.toml").read_text()
            observed = {"pid": int(pid), "pane": pane, "session": session,
                        "host": target, "configuredProfileId": profile_id,
                        "profileWrapper": str(host["markers"] / "sh")}
            assert pid_live(observed["pid"]), observed
            assert pane in panes(target, host["screen"]), observed
            assert panes(target, host["screen"])[pane]["sessionId"] == session, observed
            assert observed["pid"] in owned._live_shell_markers(target), observed
            if expected:
                assert all(observed[key] == value for key, value in expected.items()), (observed, expected)
            result["terminalMarkers"][label] = observed
            return observed

        def assert_host_unchanged(target, baseline):
            now = screens(target)
            assert now == baseline, f"host Screens changed without an explicit mutation: {target}"

        display_number = args.display.lstrip(":")
        if not display_number.isdigit() or Path(f"/tmp/.X11-unix/X{display_number}").exists() or Path(f"/tmp/.X{display_number}-lock").exists():
            raise RuntimeError(f"private DISPLAY {args.display} is unavailable")
        with (args.output / "xvfb.log").open("w") as xvfb_log, (args.output / "app.log").open("w") as app_log:
            xvfb = subprocess.Popen([str(args.xvfb), args.display, "-screen", "0", "1280x900x24",
                                     "-nolisten", "unix", "-nolisten", "tcp", "-listen", "local",
                                     "-noreset", "-ac", "-xkbdir", "/usr/share/X11/xkb"],
                                    env=xvfb_env, stdout=xvfb_log, stderr=xvfb_log,
                                    start_new_session=True)
            xvfb_start = fixture_module.process_start(xvfb.pid)

            def display_ready():
                if xvfb.poll() is not None:
                    raise RuntimeError(f"private Xvfb exited {xvfb.returncode}: "
                                       f"{(args.output / 'xvfb.log').read_text()[-2000:]}")
                try:
                    return subprocess.run([str(args.xdotool), "getdisplaygeometry"], env=env,
                                          capture_output=True, timeout=2).returncode == 0
                except subprocess.TimeoutExpired:
                    return False

            wait_until(display_ready, "private Xvfb did not start", seconds=20)
            previous_display = os.environ.get("DISPLAY")
            os.environ["DISPLAY"] = args.display
            try:
                display = performance.Display()
            finally:
                if previous_display is None:
                    os.environ.pop("DISPLAY", None)
                else:
                    os.environ["DISPLAY"] = previous_display
            app = subprocess.Popen([str(args.binary)], env=env, stdout=app_log, stderr=app_log,
                                   start_new_session=True)
            app_start = fixture_module.process_start(app.pid)

            def find_window():
                if app.poll() is not None:
                    raise RuntimeError(f"native app exited before window: {app.returncode}")
                found = subprocess.run([str(args.xdotool), "search", "--onlyvisible", "--pid", str(app.pid)],
                                       env=env, capture_output=True, text=True, timeout=3)
                for candidate in found.stdout.splitlines():
                    values = dict(line.split("=", 1) for line in x("getwindowgeometry", "--shell", candidate).splitlines()
                                  if "=" in line)
                    if values.get("WIDTH") == "1200" and values.get("HEIGHT") == "760":
                        return int(candidate)

            window = wait_until(find_window, "no native 1200x760 window", seconds=30)
            x("windowfocus", window)
            state = wait_until(local_state, "private local service did not start", seconds=20)
            result["localInitial"] = {"activeScreenId": state.get("activeScreenId"),
                                      "screenIds": list(state.get("screens", {}))}
            def local_shells():
                return [int(path.stem.removeprefix("shell-"))
                        for path in (owned.root / "local-shells").glob("shell-*.tsv")
                        if path.stem.removeprefix("shell-").isdigit() and
                        pid_live(int(path.stem.removeprefix("shell-")))]
            result["localShellPids"] = wait_until(local_shells, "private local shell did not start", seconds=30)
            time.sleep(.4)
            baseline = {target: screens(target) for target in fixture_module.TARGETS}
            capture("setup")
            result["checks"].append("isolated-native-window-and-two-private-host-sessions")
            if args.phase == "setup" and "window.confirm_close" not in geometry:
                x("mousemove", "--window", window, 600, 350)
                x("click", 1)
                type_text("exit")
                wait_until(lambda: not any(pid_live(pid) for pid in result["localShellPids"]),
                           "setup local shell did not exit", seconds=8)

            if args.phase != "setup":
                click("hosts.toolbar")
                capture("hosts-passive")
                assert not ssh_calls(), "passive Hosts visit invoked SSH"
                for target in fixture_module.TARGETS:
                    assert_host_unchanged(target, baseline[target])
                result["checks"].append("passive-hosts-no-ssh")

                a, b = fixture_module.TARGETS
                click("hosts.connect_a")
                wait_until(lambda: host_bridge_started(a), "Connect A did not start native bridge", seconds=25)
                time.sleep(1)
                capture("host-a-after-bridge-start")
                assert_host_unchanged(a, baseline[a])
                assert_host_unchanged(b, baseline[b])
                result["checks"].append("deliberate-connect-a-preserves-existing-screens")

                if PHASES.index(args.phase) >= PHASES.index("open"):
                    click("hosts.open_a")
                    wait_until(lambda: not local_state() is None, "local service lost during Open A")
                    marker(a, "open-a", {"pid": owned.hosts[a]["shell_pid"],
                                         "pane": owned.hosts[a]["pane"],
                                         "session": owned.hosts[a]["session"]})
                    capture("remote-a-open")
                    result["checks"].append("open-existing-a-terminal-input-matches-private-session")

                if args.phase == "settings-remote":
                    local_path = owned.local_config / "config.toml"
                    remote_paths = {target: owned.hosts[target]["config"] / "config.toml"
                                    for target in fixture_module.TARGETS}
                    local_before = local_path.read_bytes()
                    remote_before = {target: path.read_bytes() for target, path in remote_paths.items()}
                    remote_routes = {target: inventory(target) for target in fixture_module.TARGETS}
                    remote_shells = {target: sorted(owned._live_shell_markers(target))
                                     for target in fixture_module.TARGETS}
                    local_routes = local_state()["screens"]
                    before_marker = result["terminalMarkers"]["open-a"]
                    bridge_count = len(bridge_records(a))
                    assert tomllib.loads(local_before.decode())["appearance"]["theme"] == "splitty-dark"
                    click("drawer.settings")
                    capture("settings-on-remote-a")
                    assert local_path.read_bytes() == local_before, "opening Settings wrote local config"
                    assert all(path.read_bytes() == remote_before[target]
                               for target, path in remote_paths.items()), "opening Settings wrote remote config"
                    click("settings.theme.select")
                    capture("settings-theme-choices")
                    click("settings.theme.option")

                    def local_theme_saved():
                        try:
                            return tomllib.loads(local_path.read_text())["appearance"]["theme"] == "splitty-light"
                        except (OSError, KeyError, tomllib.TOMLDecodeError):
                            return False

                    wait_until(local_theme_saved, "local theme did not persist from remote-selected Settings")
                    capture("settings-local-theme-saved")
                    assert local_path.read_bytes() != local_before, "Settings did not write local config"
                    assert all(path.read_bytes() == remote_before[target]
                               for target, path in remote_paths.items()), "Settings wrote a remote config"
                    assert {target: inventory(target) for target in fixture_module.TARGETS} == remote_routes
                    assert {target: sorted(owned._live_shell_markers(target))
                            for target in fixture_module.TARGETS} == remote_shells
                    assert local_state()["screens"] == local_routes, "Settings recreated a local route"
                    assert len(bridge_records(a)) == bridge_count, "Settings reopened the remote bridge"
                    settings_pixels = display.pixels(window, (475, 105, 575, 85))
                    key("Escape")
                    wait_until(lambda: display.pixels(window, (475, 105, 575, 85)) != settings_pixels,
                               "Settings did not return to the remote workspace")
                    capture("remote-a-after-settings-close")
                    click("terminal.body")
                    marker(a, "after-local-settings", before_marker)
                    capture("remote-a-after-local-settings")
                    assert {target: inventory(target) for target in fixture_module.TARGETS} == remote_routes
                    assert {target: sorted(owned._live_shell_markers(target))
                            for target in fixture_module.TARGETS} == remote_shells
                    assert local_state()["screens"] == local_routes
                    assert all(path.read_bytes() == remote_before[target]
                               for target, path in remote_paths.items())
                    result["settingsRemote"] = {
                        "localThemeBefore": "splitty-dark", "localThemeAfter": "splitty-light",
                        "remoteConfigSha256": {target: hashlib.sha256(data).hexdigest()
                                                for target, data in remote_before.items()},
                        "remoteRoutes": remote_routes, "remoteShellPids": remote_shells,
                        "bridgeCount": bridge_count,
                    }
                    result["checks"].append(
                        "remote-selected-settings-write-only-local-config-and-retain-remote-session")

                if PHASES.index(args.phase) >= PHASES.index("terminal"):
                    before_a = panes(a, owned.hosts[a]["screen"])
                    before_b = panes(b, owned.hosts[b]["screen"])
                    key("alt+t")
                    wait_until(lambda: len(panes(a, owned.hosts[a]["screen"])) == len(before_a) + 1,
                               "new tab did not land on A")
                    tab = marker(a, "new-tab-a")
                    assert tab["session"] != owned.hosts[a]["session"]
                    assert panes(b, owned.hosts[b]["screen"]) == before_b
                    capture("remote-a-new-tab")
                    key("alt+d")
                    wait_until(lambda: len(panes(a, owned.hosts[a]["screen"])) == len(before_a) + 2,
                               "split did not land on A")
                    split = marker(a, "split-a")
                    assert split["session"] != tab["session"]
                    assert panes(b, owned.hosts[b]["screen"]) == before_b
                    capture("remote-a-split")
                    click("drawer.local_row")
                    hidden = owned.root / "hidden-a.marker"
                    line = f"printf HIDDEN_A; printf hidden > {shlex.quote(str(hidden))}"
                    owned._run([str(owned.binary), "send-keys", "--screen", owned.hosts[a]["screen"],
                                "--pane", owned.hosts[a]["pane"], line + "\\n"], owned._host_env(a))
                    wait_until(lambda: hidden.exists(), "A terminal did not run while hidden")
                    capture("local-with-a-hidden")
                    click("drawer.a_row")
                    capture("remote-a-after-hidden-output")
                    result["checks"].append("a-tab-split-and-hidden-shell-output-remain-source-local")

                if PHASES.index(args.phase) >= PHASES.index("two-hosts"):
                    click("hosts.toolbar_remote")
                    click("hosts.connect_b")
                    wait_until(lambda: host_bridge_started(b), "Connect B did not start native bridge", seconds=25)
                    time.sleep(1)
                    capture("host-b-after-bridge-start")
                    click("hosts.open_b")
                    marker(b, "open-b", {"pid": owned.hosts[b]["shell_pid"],
                                         "pane": owned.hosts[b]["pane"],
                                         "session": owned.hosts[b]["session"]})
                    opened_a = result["terminalMarkers"]["open-a"]
                    opened_b = result["terminalMarkers"]["open-b"]
                    assert opened_a["pane"] == opened_b["pane"] == fixture_module.SHARED_PANE_ID
                    assert opened_a["pid"] != opened_b["pid"] and opened_a["session"] != opened_b["session"]
                    capture("remote-b-open")
                    click("drawer.a_row")
                    capture("switch-back-a")
                    click("drawer.b_row")
                    capture("switch-back-b")
                    result["checks"].append("two-hosts-share-raw-screen-and-pane-ids-with-distinct-live-sessions")

                if PHASES.index(args.phase) >= PHASES.index("recovery"):
                    click("drawer.a_row")
                    before_a = inventory(a)
                    before_b = inventory(b)
                    before_local = set(local_state().get("screens", {}))
                    before_a_shells = owned._live_shell_markers(a)
                    before_b_shells = owned._live_shell_markers(b)
                    before_marker = marker(a, "before-drop-a")
                    once = owned.root / "a-input-once.txt"
                    type_text(f"printf once >> {shlex.quote(str(once))}")
                    wait_until(lambda: once.read_text() == "once" if once.exists() else False,
                               "A did not execute pre-drop input exactly once")
                    previous_bridges = bridge_records(a)
                    assert previous_bridges, "A has no owned ready bridge to drop"
                    result["droppedBridge"] = drop_owned_bridge(a, previous_bridges[-1])
                    capture("a-after-bridge-drop")

                    click("drawer.local_row")
                    local_marker = owned.root / "local-while-a-reconnecting.txt"
                    type_text(f"printf LOCAL_OK > {shlex.quote(str(local_marker))}")
                    wait_until(lambda: local_marker.read_text() == "LOCAL_OK"
                               if local_marker.exists() else False,
                               "local terminal was not usable after A bridge loss")
                    click("drawer.b_row")
                    marker(b, "b-while-a-reconnecting", result["terminalMarkers"]["open-b"])
                    assert inventory(b) == before_b
                    assert set(local_state().get("screens", {})) == before_local

                    def replacement_bridge():
                        current = bridge_records(a)
                        return current[-1] if len(current) > len(previous_bridges) else None

                    replacement = wait_until(replacement_bridge,
                                             "A did not reconnect without a host action", seconds=30)
                    result["reconnectedBridge"] = {
                        "pid": replacement["pid"], "start": replacement["start"]}
                    assert (replacement["pid"], replacement["start"]) != (
                        result["droppedBridge"]["pid"], result["droppedBridge"]["start"])
                    click("drawer.a_row")
                    marker(a, "after-recovery-a", before_marker)
                    time.sleep(.5)
                    assert once.read_text() == "once", "pre-drop input replayed after reconnect"
                    assert inventory(a) == before_a, "recovery duplicated or changed A tab/split"
                    assert inventory(b) == before_b, "recovery changed B"
                    assert owned._live_shell_markers(a) == before_a_shells
                    assert owned._live_shell_markers(b) == before_b_shells
                    capture("a-recovered")
                    result["checks"].append(
                        "owned-a-bridge-drop-recovers-same-sessions-without-duplicate-input-or-layout")

                if PHASES.index(args.phase) >= PHASES.index("detach"):
                    click("drawer.a_row")
                    a_before_detach = panes(a, owned.hosts[a]["screen"])
                    a_bridges = sum(call.get("target") == a and call.get("kind") == "bridge" and
                                    call.get("accepted") for call in ssh_calls())
                    click("drawer.a_close")
                    time.sleep(.5)
                    assert panes(a, owned.hosts[a]["screen"]) == a_before_detach
                    assert pid_live(owned.hosts[a]["shell_pid"])
                    capture("a-detached")
                    click("hosts.toolbar_remote")
                    wait_until(lambda: not any(call.get("target") == a and call.get("kind") == "bridge" and
                                               call.get("accepted") and pid_live(call["pid"])
                                               for call in ssh_calls()),
                               "A bridge did not release after final Detach", seconds=20)
                    click("hosts.connect_a")
                    wait_until(lambda: sum(call.get("target") == a and call.get("kind") == "bridge" and
                                           call.get("accepted") for call in ssh_calls()) > a_bridges,
                               "reconnect A did not start a new bridge", seconds=25)
                    time.sleep(1)
                    capture("a-reconnected-catalog")
                    click("hosts.open_a")
                    marker(a, "a-reopened")
                    assert panes(a, owned.hosts[a]["screen"]) == a_before_detach
                    capture("a-reopened")
                    result["checks"].append("default-detach-keeps-a-pty-and-reopen-reuses-session")

                if args.phase == "full":
                    disposable = result["disposableScreen"]
                    click("hosts.toolbar_remote")
                    click("hosts.open_disposable_b")
                    wait_until(lambda: any(s["id"] == disposable for s in screens(b)),
                               "disposable B Screen absent")
                    ended_sessions = {details["sessionId"] for details in panes(b, disposable).values()}
                    result["disposableSessions"] = sorted(ended_sessions)

                    def disposable_shells_alive():
                        for shell_marker in owned.hosts[b]["markers"].glob("shell-*.tsv"):
                            pid, _, session = shell_marker.read_text().strip().split("\t")
                            if session in ended_sessions and pid_live(int(pid)):
                                return True
                        return False

                    click("drawer.disposable_b_row")
                    click("drawer.disposable_b_more")
                    capture("terminate-b-menu")
                    click("terminate.menu_item")
                    capture("terminate-b-prompt-cancel")
                    click("terminate.cancel")
                    assert any(s["id"] == disposable for s in screens(b))
                    click("drawer.disposable_b_more")
                    click("terminate.menu_item")
                    capture("terminate-b-prompt-confirm")
                    click("terminate.confirm")
                    wait_until(lambda: not any(s["id"] == disposable for s in screens(b)),
                               "confirmed B termination did not close disposable Screen")
                    wait_until(lambda: not disposable_shells_alive(),
                               "confirmed B termination left its private PTY shell", seconds=10)
                    capture("disposable-b-terminated")
                    result["checks"].append("explicit-b-termination-cancel-and-confirm")

            result["remoteBeforeWindowClose"] = {target: inventory(target) for target in fixture_module.TARGETS}
            result["remoteShellsBeforeWindowClose"] = {target: sorted(owned._live_shell_markers(target))
                                                       for target in fixture_module.TARGETS}
            app_children = {pid: fixture_module.process_start(pid)
                            for pid in set(performance.process_stats(app.pid)) - {app.pid}}
            display.close_window(window)
            try:
                app.wait(timeout=1.5)
            except subprocess.TimeoutExpired:
                present = subprocess.run(
                    [str(args.xdotool), "getwindowgeometry", "--shell", str(window)],
                    env=env, capture_output=True, timeout=3).returncode == 0
                if present and "window.confirm_close" in geometry:
                    capture("window-close-prompt")
                    click("window.confirm_close")
            app.wait(timeout=15)
            assert app.returncode == 0, f"native window exited {app.returncode}"
            normal_close = True
            owned.assert_sessions_alive()
            result["remoteAfterWindowClose"] = {target: inventory(target) for target in fixture_module.TARGETS}
            assert result["remoteAfterWindowClose"] == result["remoteBeforeWindowClose"], "remote sessions changed on normal window close"
            result["remoteShellsAfterWindowClose"] = {target: sorted(owned._live_shell_markers(target))
                                                      for target in fixture_module.TARGETS}
            assert result["remoteShellsAfterWindowClose"] == result["remoteShellsBeforeWindowClose"], "remote shells changed on window close"
            for pid, start in app_children.items():
                assert not pid_live(pid) or fixture_module.process_start(pid) != start, f"app child {pid} survived close"
            result["checks"].append("normal-window-close-preserves-private-host-sessions")
            result["normalWindowClose"] = True
    except Exception as error:
        result["interactionError"] = repr(error)
        if owned is not None:
            try:
                calls = owned.root / "ssh-calls.jsonl"
                if calls.exists():
                    with calls.open() as source:
                        tail = deque(source, maxlen=64)
                    result["sshCalls"] = [
                        {"target": item.get("target"), "kind": str(item.get("kind", ""))[:300],
                         "accepted": item.get("accepted")}
                        for item in (json.loads(line) for line in tail)
                    ]
            except Exception as log_error:
                result["sshCallsError"] = repr(log_error)
        if display is not None and window is not None and app is not None and app.poll() is None:
            try:
                path = args.output / "failure.png"
                pane_drag.save_png(path, display.pixels(window, (0, 0, 1200, 760)))
                result["screenshots"].append(str(path))
            except Exception as capture_error:
                result["captureError"] = repr(capture_error)
        raise
    finally:
        cleanup_errors = []

        def stop_owned(process, started, label):
            if process is None:
                return
            try:
                if process.poll() is None:
                    if fixture_module.process_start(process.pid) != started:
                        raise RuntimeError(f"owned {label} pid identity changed")
                    os.killpg(process.pid, signal.SIGTERM)
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        if fixture_module.process_start(process.pid) != started:
                            raise RuntimeError(f"owned {label} pid identity changed before KILL")
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait(timeout=5)
                else:
                    process.wait(timeout=1)
            except Exception as error:
                cleanup_errors.append(f"{label}: {error}")

        stop_owned(app, app_start, "app")
        if display is not None:
            try:
                display.close()
            except Exception as error:
                cleanup_errors.append(f"display: {error}")
        stop_owned(xvfb, xvfb_start, "Xvfb")
        if owned is not None:
            try:
                owned.close()
                result["fixtureCleanup"] = "complete"
            except Exception as error:
                result["fixtureCleanup"] = repr(error)
                cleanup_errors.append(f"fixture: {error}")
        if cleanup_errors:
            result["cleanupErrors"] = cleanup_errors
            if "interactionError" not in result:
                raise RuntimeError("; ".join(cleanup_errors))
        result["normalWindowClose"] = normal_close


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path, help="candidate splitty-gpui executable")
    parser.add_argument("--server-binary", type=Path, default=Path("target/release/splitty"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--geometry", type=Path)
    parser.add_argument("--setup-only", action="store_true")
    parser.add_argument("--phase", choices=PHASES, default="full")
    parser.add_argument("--display", default=":98")
    parser.add_argument("--xvfb", default="Xvfb")
    parser.add_argument("--xvfb-lib", type=Path)
    parser.add_argument("--xdotool", default="xdotool")
    args = parser.parse_args()
    if args.setup_only:
        args.phase = "setup"
    geometry = json.loads(args.geometry.read_text()) if args.geometry else {}
    if args.phase != "setup" and not args.geometry:
        parser.error("interaction phases require screenshot-calibrated --geometry JSON")
    required = set()
    for phase in PHASES[1:PHASES.index(args.phase) + 1]:
        if phase == "settings-remote" and args.phase != phase:
            continue
        required.update(POINTS.get(phase, ()))
    required.add("window.confirm_close") if args.phase != "setup" else None
    absent = sorted(required - geometry.keys())
    if absent:
        parser.error("geometry misses points: " + ", ".join(absent))
    args.binary = args.binary.resolve(strict=True)
    args.server_binary = args.server_binary.resolve(strict=True)
    if not os.access(args.binary, os.X_OK):
        parser.error("candidate GPUI binary is not executable")
    args.output.mkdir(parents=True, exist_ok=False)
    app_stat = args.binary.stat()
    server_stat = args.server_binary.stat()
    result = {"phase": args.phase, "passed": False,
              "appBinary": str(args.binary),
              "appSha256": fixture_module.sha256(args.binary),
              "appSource": {"size": app_stat.st_size, "mtimeNs": app_stat.st_mtime_ns,
                            "device": app_stat.st_dev, "inode": app_stat.st_ino},
              "serverBinary": str(args.server_binary),
              "serverSha256": fixture_module.sha256(args.server_binary),
              "serverSource": {"size": server_stat.st_size, "mtimeNs": server_stat.st_mtime_ns,
                               "device": server_stat.st_dev, "inode": server_stat.st_ino},
              "harnessSha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "geometrySha256": hashlib.sha256(args.geometry.read_bytes()).hexdigest() if args.geometry else None,
              "checks": [], "screenshots": [], "terminalMarkers": {}, "normalWindowClose": False}
    try:
        run_window(args, geometry, result)
        assert fixture_module.sha256(args.binary) == result["appSha256"], "candidate binary changed during run"
        assert fixture_module.sha256(args.server_binary) == result["serverSha256"], "server binary changed during run"
        result["passed"] = (result.get("normalWindowClose") and result.get("fixtureCleanup") == "complete"
                            and not result.get("cleanupErrors"))
    except Exception as error:
        result["error"] = repr(error)
    (args.output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"passed": result["passed"], "phase": args.phase,
                      "checks": result["checks"], "error": result.get("error"),
                      "result": str(args.output / "result.json")}, indent=2))
    if not result["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
