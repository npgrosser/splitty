#!/usr/bin/env python3
"""Exercise native remote Screens in one owned X11 window and two private hosts.

Run --setup-only first to capture a window for geometry calibration. Later
phases require a screenshot-calibrated geometry JSON of named [x, y] points.
The backend fixture, window and display are owned by this run. Screenshots
record visible states; private CLI reads and shell markers prove backend effects.
"""

import argparse
from collections import deque
import csv
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shlex
import signal
import socket
import stat
import subprocess
import time
import tomllib
import traceback

HERE = Path(__file__).resolve().parent


def helper(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


fixture_module = helper("remote_screen_fixture", "gpui-remote-screen-fixture.py")
port_fixture_module = helper("outbound_port_fixture", "gpui-outbound-port-fixture.py")
performance = helper("performance", "gpui-performance.py")
pane_drag = helper("pane_drag", "gpui-pane-drag.py")

PHASES = ("setup", "remote-create", "connect", "open", "reorder", "ports", "host-management", "settings-remote", "terminal", "two-hosts", "recovery", "detach", "full")
POINTS = {
    "remote-create": ("hosts.toolbar", "hosts.connect_a", "hosts.open_a",
                      "hosts.toolbar_remote", "ports.host_b_connect", "ports.host_b_open",
                      "drawer.local_row", "drawer.a_row", "drawer.b_row",
                      "drawer.b_new", "drawer.b_created_row", "drawer.b_created_close",
                      "hosts.open_b_created"),
    "connect": ("hosts.toolbar", "hosts.connect_a"),
    "open": ("hosts.open_a",),
    "reorder": ("hosts.toolbar_remote", "hosts.open_a_second", "drawer.a_first",
                "drawer.a_second", "drawer.a_first_before"),
    "ports": ("hosts.toolbar_remote", "ports.host_b_connect", "ports.host_b_open",
              "ports.add", "ports.target", "ports.remote", "ports.local", "ports.label",
              "ports.remember", "ports.forward", "ports.open", "ports.save",
              "ports.dialog_title_region",
              "ports.drawer_b_region",
              "ports.edit.dialog_title_region", "ports.edit.local", "ports.edit.label",
              "ports.edit.save", "ports.edit.resume_alt",
              "ports.b_more",
              "ports.b_menu_stop", "ports.b_menu_edit", "ports.b_menu_forget",
              "ports.b_menu_pause_saved",
              "ports.b_primary", "ports.b_row_status_region",
              "ports.b_menu_edit_paused", "ports.b_menu_forget_paused",
              "ports.a_primary", "ports.a_more", "ports.a_menu_stop",
              "ports.terminal_link", "ports.link_output_region", "ports.scroll_area", "terminal.body",
              "drawer.a_row", "drawer.b_row"),
    "host-management": ("hosts.toolbar_remote", "hosts.connect_entry", "hosts.dialog_target",
                        "hosts.dialog_alias", "hosts.dialog_submit", "hosts.dialog_title_region", "hosts.open_b",
                        "hosts.b_more", "hosts.b_edit", "hosts.b_alias", "hosts.b_alias_save",
                        "hosts.b_remove", "drawer.b_row"),
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


def pause_private_bridge(owned, target, record):
    """Pause only the fixture's identified bridge, retaining a pidfd for CONT."""
    if target not in owned.hosts or record.get("target") != target or not record.get("accepted"):
        raise RuntimeError("bridge is outside this private fixture")
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
        argv = [part.decode() for part in (proc / "cmdline").read_bytes().split(b"\0") if part]
        environment = (proc / "environ").read_bytes().split(b"\0")
        if (not pid_live(pid) or fixture_module.process_start(pid) != start or
                (proc / "exe").resolve(strict=True) != expected_exe or
                argv != expected_argv or expected_config not in environment):
            raise RuntimeError("private bridge identity changed; refusing pause")
        paused = {"pidfd": pidfd, "pid": pid, "start": start, "target": target}
        signal.pidfd_send_signal(pidfd, signal.SIGSTOP)
        return paused
    except BaseException:
        os.close(pidfd)
        raise


def resume_private_bridge(paused):
    """Release an identified bridge; retain its fd if CONT fails for cleanup."""
    pidfd = paused.get("pidfd")
    if pidfd is None:
        return
    if (pid_live(paused["pid"]) and
            fixture_module.process_start(paused["pid"]) == paused["start"]):
        signal.pidfd_send_signal(pidfd, signal.SIGCONT)
    os.close(pidfd)
    paused["pidfd"] = None


def run_window(args, geometry, result):
    if args.phase == "remote-create":
        fixture_tmp = args.output / "t"
        fixture_tmp.mkdir(mode=0o700, exist_ok=False)
        os.environ["SPLITTY_TEST_TMPDIR"] = str(fixture_tmp)
    fixture = (port_fixture_module.OutboundPortFixture if args.phase == "ports"
               else fixture_module.RemoteScreenFixture)(args.server_binary)
    owned = None
    xvfb = app = display = window = None
    xvfb_start = app_start = None
    app_children = {}
    normal_close = False
    paused_bridge = None
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
        if args.phase == "host-management":
            # B remains an owned, reachable fixture host, but is not saved in
            # the local directory. Entering it must use the real Connect form.
            a = fixture_module.TARGETS[0]
            for filename, field in (("recent-hosts.json", "hosts"),
                                    ("remote-host-order.json", "order")):
                path = owned.local_config / filename
                document = json.loads(path.read_text())
                document[field] = [row for row in document[field]
                                   if (row["target"] if field == "hosts" else row) == a]
                path.write_text(json.dumps(document) + "\n")
        if args.phase == "ports":
            result["portFixtureSha256"] = fixture_module.sha256(HERE / "gpui-outbound-port-fixture.py")
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
        if args.phase == "ports":
            spy_dir = args.output / "opener-spy"
            spy_dir.mkdir(mode=0o700)
            opener_log = args.output / "opener.jsonl"
            opener_log.touch(mode=0o600)
            spy = spy_dir / "spy.py"
            spy.write_text(
                "#!/usr/bin/python3\n"
                "import json,os,sys,urllib.parse,urllib.request\n"
                "from pathlib import Path\n"
                "url=os.environ.get('OPEN_RS_TARGET') or sys.argv[-1]\n"
                "record={'url':url,'argv':sys.argv[1:]}\n"
                "parsed=urllib.parse.urlsplit(url)\n"
                "allowed=json.loads(Path(os.environ['SPLITTY_OWNED_FORWARD_PORTS']).read_text())\n"
                "if parsed.scheme not in ('http','https') or parsed.hostname!='localhost' or parsed.port not in allowed:\n"
                " record['error']='URL outside owned forwarding allowlist'\n"
                "else:\n"
                " try:\n"
                "  opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))\n"
                "  with opener.open(url,timeout=4) as response:\n"
                "   record['status']=response.status;record['body']=response.read(8192).decode()\n"
                " except Exception as error:record['error']=repr(error)\n"
                "with Path(os.environ['SPLITTY_PORT_OPENER_LOG']).open('a') as out:out.write(json.dumps(record)+'\\n')\n"
                "sys.exit(0)\n")
            spy.chmod(0o700)
            for launcher in ("xdg-open", "gio", "gnome-open", "kde-open", "powershell.exe"):
                (spy_dir / launcher).symlink_to(spy.name)
            allowed_ports = args.output / "allowed-forward-ports.json"
            allowed_ports.write_text("[]\n")
            env["PATH"] = str(spy_dir) + os.pathsep + env.get("PATH", "/usr/bin:/bin")
            env["BROWSER"] = str(spy)
            env["SPLITTY_PORT_OPENER_LOG"] = str(opener_log)
            env["SPLITTY_OWNED_FORWARD_PORTS"] = str(allowed_ports)
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

        def region(name):
            value = geometry[name]
            if (not isinstance(value, list) or len(value) != 4 or
                    not all(type(component) is int and component >= 0 for component in value) or
                    value[2] == 0 or value[3] == 0):
                raise ValueError(f"geometry {name} must be [x, y, width, height]")
            return tuple(value)

        def wait_region_stable(name, description):
            previous = None
            repeats = 0
            def stable():
                nonlocal previous, repeats
                current = display.pixels(window, region(name))
                repeats = repeats + 1 if current == previous else 0
                previous = current
                return repeats >= 3
            wait_until(stable, description, seconds=5)

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

        def fill_text(value):
            x("key", "--clearmodifiers", "ctrl+a")
            x("key", "--clearmodifiers", "BackSpace")
            if value:
                x("type", "--clearmodifiers", "--delay", 25, value)
            display.sync()

        def scroll_hosts(down):
            x("mousemove", "--window", window, *point("ports.scroll_area"))
            for _ in range(3):
                x("click", 5 if down else 4)
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

        def local_metadata(filename, field):
            # The embedded control socket intentionally exposes only pane APIs.
            # These are the local hub's authoritative, atomically replaced
            # metadata files; Screenshots show the corresponding UI projection.
            path = owned.local_config / filename
            if not path.exists():
                return {}
            value = json.loads(path.read_text())
            assert value.get("version") == 1 and isinstance(value.get(field), list), value
            return {entry["target"]: entry for entry in value[field]}

        def local_recent():
            return local_metadata("recent-hosts.json", "hosts")

        def local_catalog():
            return local_metadata("remote-catalog.json", "hosts")

        def local_preferences():
            return local_metadata("remote-host-preferences.json", "hosts")

        def local_order():
            path = owned.local_config / "remote-host-order.json"
            value = json.loads(path.read_text())
            assert value.get("version") == 1 and isinstance(value.get("order"), list), value
            return value["order"]

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

        def forward_calls(target=None):
            return [call for call in ssh_calls() if call.get("kind") == "forward"
                    and call.get("accepted") and (target is None or call.get("target") == target)]

        def active_forward(target, local):
            return next((call for call in reversed(forward_calls(target))
                         if call.get("local") == local and pid_live(call["pid"])
                         and fixture_module.process_start(call["pid"]) == call["start"]), None)

        def saved_forward(target, remote):
            path = owned.local_config / "remote-forwards.json"
            if not path.exists():
                return None
            document = json.loads(path.read_text())
            assert document.get("version") == 1 and isinstance(document.get("forwards"), list)
            return next((row for row in document["forwards"]
                         if row["target"] == target and row["remote"] == remote), None)

        def opened():
            if args.phase != "ports":
                return []
            return [json.loads(line) for line in opener_log.read_text().splitlines()]

        def choose_local_port():
            with socket.socket() as probe:
                probe.bind(("127.0.0.1", 0))
                return probe.getsockname()[1]

        def forwarded_http(local, target, path="/app?source=owned"):
            import urllib.request
            opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
            with opener.open(f"http://127.0.0.1:{local}{path}", timeout=4) as response:
                body = json.loads(response.read())
            assert body["fixture"] == "splitty-ports-v1" and body["target"] == target
            assert body["path"] == path and body["pid"] == owned.http[target]["pid"]
            return body

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

                if args.phase == "remote-create":
                    click("hosts.open_a")
                    marker(a, "create-open-a", {
                        "pid": owned.hosts[a]["shell_pid"],
                        "pane": owned.hosts[a]["pane"],
                        "session": owned.hosts[a]["session"],
                    })
                    click("hosts.toolbar_remote")
                    click("ports.host_b_connect")
                    wait_until(lambda: host_bridge_started(b),
                               "Connect B did not start private bridge", seconds=25)
                    time.sleep(1.2)
                    capture("create-host-b-ready")
                    click("ports.host_b_open")
                    time.sleep(.4)
                    marker(b, "create-open-b", {
                        "pid": owned.hosts[b]["shell_pid"],
                        "pane": owned.hosts[b]["pane"],
                        "session": owned.hosts[b]["session"],
                    })
                    click("drawer.local_row")
                    capture("create-local-selected-b-ready")
                    if args.stop_after_create_setup:
                        result["checks"].append("remote-create-geometry-setup-only")
                    else:
                        before_a = inventory(a)
                        before_b = inventory(b)
                        before_b_ids = set(before_b)
                        before_local = set(local_state().get("screens", {}))
                        names_b = {screen["name"] for screen in screens(b)}
                        default_name = next(
                            f"Screen {number}" for number in range(1, 1000)
                            if f"Screen {number}" not in names_b
                        )
                        bridge = bridge_records(b)[-1]
                        paused_bridge = pause_private_bridge(owned, b, bridge)
                        result["pausedBridge"] = {
                            key: paused_bridge[key] for key in ("pid", "start", "target")
                        }
                        click("drawer.b_new")
                        click("drawer.a_row")
                        marker(a, "a-before-b-confirmation", result["terminalMarkers"]["create-open-a"])
                        assert inventory(b) == before_b, "B created while its owned bridge was paused"
                        capture("create-a-selected-before-b-confirmation")
                        resume_private_bridge(paused_bridge)
                        paused_bridge = None
                        result["pausedBridgeReleasedAfterNavigation"] = True

                        def created_b():
                            catalog = screens(b)
                            additions = [row for row in catalog if row["id"] not in before_b_ids]
                            return additions[0] if len(additions) == 1 else None

                        created = wait_until(created_b,
                                             "B did not create exactly one canonical Screen", seconds=25)
                        assert created["name"] == default_name, (created, default_name)
                        assert inventory(a) == before_a
                        assert set(local_state().get("screens", {})) == before_local
                        new_screen = created["id"]
                        new_panes = panes(b, new_screen)
                        assert len(new_panes) == 1, new_panes
                        new_pane, new_details = next(iter(new_panes.items()))
                        new_session = new_details["sessionId"]
                        assert new_session not in set(before_b[owned.hosts[b]["screen"]].values())

                        def assert_b_catalog_exact():
                            current = inventory(b)
                            assert set(current) == before_b_ids | {new_screen}, current
                            assert all(current[screen] == sessions
                                       for screen, sessions in before_b.items()), current
                            assert current[new_screen] == {new_pane: new_session}, current

                        assert_b_catalog_exact()
                        marker(a, "a-after-b-confirmation", result["terminalMarkers"]["create-open-a"])
                        capture("create-b-confirmed-a-still-selected")
                        result["createdScreen"] = {
                            "target": b, "screen": new_screen, "name": created["name"],
                            "pane": new_pane, "session": new_session,
                        }
                        result["checks"].append(
                            "b-only-canonical-screen-created-while-a-selection-retained")

                        def created_marker(label):
                            path = owned.root / f"ui-{label}.tsv"
                            type_text("printf '%s\\t%s\\t%s\\n' \"$$\" \"$SPLITTY_PANE_ID\" "
                                      f"\"$SPLITTY_SESSION_ID\" > {shlex.quote(str(path))}")
                            value = wait_until(lambda: path.read_text().strip()
                                               if path.exists() else None,
                                               f"new B Screen marker {label} did not arrive")
                            pid_text, pane, session = value.split("\t")
                            pid = int(pid_text)
                            assert (pane, session) == (new_pane, new_session)
                            assert pid in owned._live_shell_markers(b)
                            return {"pid": pid, "pane": pane, "session": session}

                        click("drawer.b_created_row")
                        opened = created_marker("b-created-opened")
                        capture("create-b-new-screen-opened")
                        created_title = x("getwindowname", window).strip()
                        assert created["name"] in created_title, created_title
                        result["createdWindowTitle"] = created_title
                        # The close action is revealed on hover. Let that
                        # presentation render before sending the actual click.
                        x("mousemove", "--window", window,
                          *point("drawer.b_created_close"))
                        display.sync()
                        time.sleep(.35)
                        capture("create-b-detach-hover-ready")
                        x("click", 1)
                        display.sync()
                        detached_title = wait_until(
                            lambda: (title if title != created_title else None)
                            if (title := x("getwindowname", window).strip()) else None,
                            "B Screen did not detach from the shown window", seconds=8)
                        assert created["name"] not in detached_title, detached_title
                        result["detachedWindowTitle"] = detached_title
                        assert_b_catalog_exact()
                        assert pid_live(opened["pid"])
                        marker(b, "b-after-created-detach",
                               result["terminalMarkers"]["create-open-b"])
                        capture("create-b-new-screen-detached")
                        click("hosts.toolbar_remote")
                        click("hosts.open_b_created")
                        reopened_title = wait_until(
                            lambda: (title if created["name"] in title else None)
                            if (title := x("getwindowname", window).strip()) else None,
                            "B Screen did not reopen into the shown window", seconds=8)
                        result["reopenedWindowTitle"] = reopened_title
                        reopened = created_marker("b-created-reopened")
                        assert reopened == opened
                        assert_b_catalog_exact()
                        assert inventory(a) == before_a
                        capture("create-b-new-screen-reopened-same-session")
                        result["checks"].append("b-created-screen-detach-reopen-reuses-session")

                if PHASES.index(args.phase) >= PHASES.index("open"):
                    click("hosts.open_a")
                    wait_until(lambda: not local_state() is None, "local service lost during Open A")
                    marker(a, "open-a", {"pid": owned.hosts[a]["shell_pid"],
                                         "pane": owned.hosts[a]["pane"],
                                         "session": owned.hosts[a]["session"]})
                    capture("remote-a-open")
                    result["checks"].append("open-existing-a-terminal-input-matches-private-session")

                if args.phase == "reorder":
                    def drawer_word_order(label):
                        crop = args.output / f"{label}-drawer.png"
                        pane_drag.save_png(crop, display.pixels(window, (0, 0, 280, 760)), 280, 760)
                        ocr_env = os.environ.copy()
                        if args.tesseract_lib:
                            ocr_env["LD_LIBRARY_PATH"] = str(args.tesseract_lib) + (
                                ":" + ocr_env["LD_LIBRARY_PATH"] if ocr_env.get("LD_LIBRARY_PATH") else "")
                        if args.tessdata:
                            ocr_env["TESSDATA_PREFIX"] = str(args.tessdata)
                        words = subprocess.check_output(
                            [str(args.tesseract), str(crop), "stdout", "--psm", "6", "tsv"],
                            env=ocr_env, text=True, timeout=12, stderr=subprocess.DEVNULL)
                        rows = list(csv.DictReader(io.StringIO(words), delimiter="\t"))
                        def top(term):
                            return min(int(row["top"]) for row in rows
                                       if term in row["text"].lower())
                        positions = {term: top(term) for term in ("alpha", "remote")}
                        result.setdefault("drawerOcr", {})[label] = positions
                        result["screenshots"].append(str(crop))
                        return positions

                    host_order_before = local_order()
                    owned._run([str(owned.binary), "new-screen", "--name", "Remote second"],
                               owned._host_env(a))
                    host_order = wait_until(
                        lambda: (items if len(items := screens(a)) == 2 else None),
                        "private host A second Screen did not appear")
                    second = next(item for item in host_order
                                  if item["id"] != owned.hosts[a]["screen"])
                    click("hosts.toolbar_remote")
                    capture("host-a-two-screen-catalog")
                    click("hosts.open_a_second")
                    capture("host-a-two-open-references")
                    before_visual = drawer_word_order("before-remote-reorder")
                    assert before_visual["alpha"] < before_visual["remote"], before_visual
                    click("drawer.a_first")
                    time.sleep(.7)
                    capture("first-remote-selected-before-reorder")
                    before = marker(a, "before-remote-reorder",
                                    {"pid": owned.hosts[a]["shell_pid"],
                                     "pane": owned.hosts[a]["pane"],
                                     "session": owned.hosts[a]["session"]})
                    start = point("drawer.a_second")
                    finish = point("drawer.a_first_before")
                    x("mousemove", "--window", window, *start)
                    x("mousedown", 1)
                    x("mousemove", "--window", window, start[0] + 10, start[1])
                    x("mousemove", "--window", window, *finish)
                    display.sync()
                    time.sleep(.12)
                    capture("remote-reference-insertion")
                    x("mouseup", 1)
                    display.sync()
                    time.sleep(.4)
                    capture("remote-reference-reordered")
                    after_visual = drawer_word_order("after-remote-reorder")
                    assert after_visual["remote"] < after_visual["alpha"], after_visual
                    assert [item["id"] for item in screens(a)] == [item["id"] for item in host_order], (
                        "window reference drag changed host Screen order")
                    assert local_order() == host_order_before, "Screen drag changed host group order"
                    assert marker(a, "after-remote-reorder", before) == before
                    result["remoteReordered"] = {"moved": second["id"],
                                                 "anchor": owned.hosts[a]["screen"],
                                                 "hostOrderUnchanged": True,
                                                 "localHostOrderUnchanged": True}
                    result["checks"].append("remote-window-reference-reorder-preserves-host-order-and-terminal")

                if args.phase == "ports":
                    remote = owned.http_port
                    assert saved_forward(a, remote) is None and saved_forward(b, remote) is None
                    assert not forward_calls(), "passive detected ports started SSH forwarding"
                    click("hosts.toolbar_remote")
                    click("ports.host_b_connect")
                    wait_until(lambda: host_bridge_started(b), "Connect B did not start private bridge", 25)
                    wait_until(lambda: b in local_recent(),
                               "connected B was not reconciled into local Hosts", 25)
                    drawer_b_before = display.pixels(window, region("ports.drawer_b_region"))
                    click("ports.host_b_open")
                    wait_until(lambda: display.pixels(window, region("ports.drawer_b_region"))
                               != drawer_b_before, "Open B did not add its Drawer route", 25)
                    click("drawer.b_row")
                    before_b = marker(b, "ports-open-b", {
                        "pid": owned.hosts[b]["shell_pid"], "pane": owned.hosts[b]["pane"],
                        "session": owned.hosts[b]["session"]})
                    click("hosts.toolbar_remote")
                    scroll_hosts(True)
                    capture("ports-detected-private-http")
                    assert not forward_calls(), "detected port observation invoked forward SSH"
                    result["checks"].append("passive-two-host-detection-with-no-forward-ssh")

                    def form(target, local, label="", remember=False):
                        click("ports.add")
                        wait_region_stable("ports.dialog_title_region", "Add port dialog did not settle")
                        click("ports.target")
                        fill_text(target)
                        click("ports.remote")
                        fill_text(str(remote))
                        click("ports.local")
                        fill_text(str(local))
                        click("ports.label")
                        fill_text(label)
                        if remember:
                            click("ports.remember")

                    def stop_b():
                        click("ports.b_more")
                        click("ports.b_menu_stop")

                    def stop_a():
                        click("ports.a_more")
                        capture("ports-a-menu-live")
                        click("ports.a_menu_stop")

                    local_b = choose_local_port()
                    hidden_dialog = display.pixels(window, region("ports.dialog_title_region"))
                    form(b, local_b, "Private Beta app", remember=True)
                    capture("ports-b-add-form")
                    click("ports.forward")
                    wait_until(lambda: active_forward(b, local_b), "B forward did not bind", 25)
                    wait_until(lambda: saved_forward(b, remote), "remembered B mapping not saved")
                    saved = saved_forward(b, remote)
                    assert saved == {"target": b, "remote": remote, "local": local_b,
                                     "label": "Private Beta app", "enabled": True}, saved
                    forwarded_http(local_b, b, "/app?source=B")
                    wait_until(lambda: display.pixels(window, region("ports.dialog_title_region"))
                               == hidden_dialog, "B Forward form did not close")
                    capture("ports-b-forwarding-remembered")
                    result["checks"].append("manual-b-forward-binds-owned-loopback-and-remembers-mapping")

                    initial_forward = active_forward(b, local_b)
                    click("ports.b_more")
                    capture("ports-b-menu-saved-and-live")
                    click("ports.b_menu_edit")
                    wait_region_stable("ports.edit.dialog_title_region", "Edit port dialog did not settle")
                    capture("ports-b-edit-form")
                    click("ports.edit.label")
                    fill_text("Private Beta renamed")
                    click("ports.edit.save")
                    wait_until(lambda: (saved_forward(b, remote) or {}).get("label")
                               == "Private Beta renamed", "B label edit not saved")
                    assert active_forward(b, local_b) == initial_forward, "label edit restarted forwarding"
                    capture("ports-b-label-edited")
                    click("ports.b_more")
                    click("ports.b_menu_edit")
                    wait_region_stable("ports.edit.dialog_title_region", "B clear-label editor did not settle")
                    click("ports.edit.label")
                    fill_text("")
                    key("Return")
                    wait_until(lambda: (saved_forward(b, remote) or {}).get("label") == "",
                               "B label clear not saved by Enter")
                    assert active_forward(b, local_b) == initial_forward
                    capture("ports-b-label-cleared")
                    result["checks"].append("edit-save-and-enter-clear-label-without-restarting-forward")

                    click("ports.b_more")
                    click("ports.b_menu_forget")
                    wait_until(lambda: saved_forward(b, remote) is None,
                               "Forget left B mapping in durable preferences")
                    assert active_forward(b, local_b) == initial_forward
                    forwarded_http(local_b, b, "/still-live")
                    capture("ports-b-forgotten-but-live")
                    stop_b()
                    wait_until(lambda: active_forward(b, local_b) is None,
                               "Stop did not end B forward")
                    assert saved_forward(b, remote) is None
                    capture("ports-b-stopped-after-forget")
                    result["checks"].append("forget-and-stop-are-independent-live-and-saved-operations")

                    scroll_hosts(False)
                    click("ports.a_primary")
                    forward_a = wait_until(lambda: forward_calls(a)[-1] if forward_calls(a) else None,
                                           "detected A Forward did not start", 25)
                    local_a = forward_a["local"]
                    wait_until(lambda: active_forward(a, local_a), "A forward did not become live")
                    assert saved_forward(a, remote) is None
                    forwarded_http(local_a, a, "/app?source=A")
                    capture("ports-a-detected-forward")
                    stop_a()
                    wait_until(lambda: active_forward(a, local_a) is None, "A Stop did not end forward")
                    result["checks"].append("detected-a-forward-keeps-host-source-and-is-not-remembered")

                    scroll_hosts(True)

                    rival = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
                    rival.bind(("127.0.0.1", 0))
                    rival.listen(1)
                    busy = rival.getsockname()[1]
                    try:
                        calls_before_save = ssh_calls()
                        detected_status = display.pixels(window, region("ports.b_row_status_region"))
                        form(b, busy)
                        click("ports.save")
                        wait_until(lambda: (saved_forward(b, remote) or {}).get("local") == busy,
                                   "Save without starting did not persist busy B port")
                        saved_only = saved_forward(b, remote)
                        assert saved_only["enabled"] is True and saved_only["label"] == ""
                        assert ssh_calls() == calls_before_save, "Save without starting invoked SSH"
                        assert active_forward(b, busy) is None
                        wait_until(lambda: display.pixels(window, region("ports.b_row_status_region"))
                                   != detected_status, "saved-only B row did not render", 25)
                        wait_region_stable("ports.b_row_status_region", "saved-only B row did not settle")
                        capture("ports-b-saved-without-starting")
                        saved_status = display.pixels(window, region("ports.b_row_status_region"))
                        click("ports.b_more")
                        capture("ports-b-menu-saved-only")
                        click("ports.b_menu_pause_saved")
                        wait_until(lambda: (saved_forward(b, remote) or {}).get("enabled") is False,
                                   "Pause saved port did not persist paused B mapping")
                        saved_busy = saved_forward(b, remote)
                        assert saved_busy["local"] == busy and saved_busy["label"] == ""
                        assert ssh_calls() == calls_before_save, "Pause saved port invoked SSH"
                        assert active_forward(b, busy) is None
                        wait_until(lambda: display.pixels(window, region("ports.b_row_status_region"))
                                   != saved_status, "paused B row did not render", 25)
                        wait_region_stable("ports.b_row_status_region", "paused B row did not settle")
                        capture("ports-b-busy-saved-paused")
                        calls_before_busy = ssh_calls()
                        before_status = display.pixels(window, region("ports.b_row_status_region"))
                        click("ports.b_primary")
                        wait_until(lambda: display.pixels(window, region("ports.b_row_status_region"))
                                   != before_status, "busy Resume did not update B row", 25)
                        wait_region_stable("ports.b_row_status_region", "busy B row did not settle")
                        assert rival.getsockname()[1] == busy and rival.fileno() >= 0
                        assert ssh_calls() == calls_before_busy, "busy Resume invoked SSH"
                        assert saved_forward(b, remote) == saved_busy
                        capture("ports-busy-local-error")

                        scroll_hosts(True)
                        click("ports.b_more")
                        capture("ports-b-menu-paused-after-conflict")
                        click("ports.b_menu_edit_paused")
                        wait_region_stable("ports.edit.dialog_title_region",
                                           "failed B Edit dialog did not settle")
                        capture("ports-b-edit-alternate-form")
                        alternate = choose_local_port()
                        click("ports.edit.local")
                        fill_text(str(alternate))
                        click("ports.edit.resume_alt")
                        wait_until(lambda: active_forward(b, alternate),
                                   "Resume with local port did not bind alternate", 25)
                        updated = saved_forward(b, remote)
                        assert updated and updated["local"] == alternate and updated["enabled"]
                        forwarded_http(alternate, b, "/alternate")
                        assert rival.getsockname()[1] == busy and rival.fileno() >= 0
                        capture("ports-b-alternate-local-forward")
                    finally:
                        rival.close()
                    result["checks"].append("busy-owned-local-port-refused-without-touching-rival")
                    result["checks"].append("explicit-edit-resume-with-alternate-port-restores-b-forwarding")
                    stop_b()
                    wait_until(lambda: active_forward(b, alternate) is None,
                               "alternate B forward did not stop")
                    wait_until(lambda: (saved_forward(b, remote) or {}).get("enabled") is False,
                               "Stop did not pause remembered B mapping")
                    click("ports.b_more")
                    click("ports.b_menu_forget_paused")
                    wait_until(lambda: saved_forward(b, remote) is None,
                               "Forget did not clear paused alternate B mapping")

                    # The remote terminal supplies a real link. The owned SSH
                    # shim holds readiness so selection can change before Open
                    # settles; the opener must retain B's immutable source.
                    click("drawer.b_row")
                    link = f"http://localhost:{remote}/app?x=1#frag"
                    link_before = display.pixels(window, region("ports.link_output_region"))
                    type_text(f"printf '%s\\n' {shlex.quote(link)}")
                    wait_until(lambda: display.pixels(window, region("ports.link_output_region"))
                               != link_before, "B terminal did not print development link")
                    capture("ports-b-remote-terminal-link")
                    gate = owned.root / "forward-gate.json"
                    release = owned.root / "forward-gate-release"
                    entered = owned.root / "forward-gate-entered.json"
                    gate.write_text(json.dumps({"target": b, "remote": remote}) + "\n")
                    release.unlink(missing_ok=True)
                    entered.unlink(missing_ok=True)
                    assert not any(active_forward(b, call["local"]) for call in forward_calls(b)), (
                        "B mapping was live before gated terminal Open")
                    before_open = opened()
                    x("mousemove", "--window", window, *point("ports.terminal_link"))
                    x("keydown", "ctrl")
                    try:
                        x("click", 1)
                    finally:
                        x("keyup", "ctrl")
                    gate_record = wait_until(lambda: json.loads(entered.read_text())
                                             if entered.exists() else None,
                                             "terminal link did not start gated B forward", 25)
                    assert gate_record["target"] == b and gate_record["remote"] == remote
                    click("drawer.a_row")
                    allowed_ports.write_text(json.dumps([gate_record["local"]]) + "\n")
                    release.touch()
                    expected_url = f"http://localhost:{gate_record['local']}/app?x=1#frag"
                    event = wait_until(lambda: opened()[-1] if len(opened()) > len(before_open)
                                       else None, "gated terminal Open did not reach owned opener", 25)
                    assert len(opened()) == len(before_open) + 1 and event["url"] == expected_url, event
                    body = json.loads(event["body"])
                    assert event["status"] == 200 and body["target"] == b
                    assert body["path"] == "/app?x=1" and body["pid"] == owned.http[b]["pid"]
                    capture("ports-b-link-open-after-selecting-a")
                    result["terminalOpen"] = {"source": b, "original": link,
                                              "rewritten": expected_url, "opener": event}
                    result["checks"].append("gated-terminal-link-retains-b-source-and-opens-owned-http")

                    click("hosts.toolbar_remote")
                    scroll_hosts(True)
                    stop_b()
                    wait_until(lambda: active_forward(b, gate_record["local"]) is None,
                               "B terminal-link forward did not stop")
                    gate.unlink(missing_ok=True)
                    release.unlink(missing_ok=True)
                    entered.unlink(missing_ok=True)
                    assert saved_forward(b, remote) is None
                    form_open_local = choose_local_port()
                    allowed_ports.write_text(json.dumps([form_open_local]) + "\n")
                    form(b, form_open_local)
                    click("ports.open")
                    # The form's Open is also tested through its own UI path.
                    # A later candidate may require a row-menu Stop calibration.
                    reopened = wait_until(lambda: forward_calls(b)[-1]
                                          if forward_calls(b) and forward_calls(b)[-1]["pid"]
                                          != gate_record["pid"] else None,
                                          "B form Open did not start a fresh forward", 25)
                    wait_until(lambda: len(opened()) == len(before_open) + 2,
                               "B form Open did not dispatch owned opener", 25)
                    from urllib.parse import urlsplit
                    form_url = urlsplit(opened()[-1]["url"])
                    assert (form_url.scheme, form_url.hostname, form_url.port,
                            form_url.path) in (("http", "localhost", reopened["local"], ""),
                                               ("http", "localhost", reopened["local"], "/"))
                    assert json.loads(opened()[-1]["body"])["target"] == b
                    capture("ports-b-form-open")
                    stop_b()
                    wait_until(lambda: active_forward(b, reopened["local"]) is None,
                               "B form-open forward did not stop")
                    gate.write_text(json.dumps({"target": b, "remote": remote}) + "\n")
                    release.unlink(missing_ok=True)
                    entered.unlink(missing_ok=True)
                    stale_local = choose_local_port()
                    stale_opens = opened()
                    form(b, stale_local)
                    click("ports.open")
                    stale = wait_until(lambda: json.loads(entered.read_text())
                                       if entered.exists() else None,
                                       "dismissal Open did not enter owned readiness gate", 25)
                    assert stale["local"] == stale_local and stale["target"] == b
                    key("Escape")
                    wait_until(lambda: display.pixels(window, region("ports.dialog_title_region"))
                               == hidden_dialog, "Escape did not dismiss pending Open dialog")
                    release.touch()
                    wait_until(lambda: active_forward(b, stale_local),
                               "dismissed Open did not settle its owned forward", 20)
                    assert opened() == stale_opens, "dismissed Open reached the browser spy"
                    capture("ports-stale-open-dismissed")
                    result["checks"].append("dismissed-pending-open-never-dispatches-opener")
                    stop_b()
                    wait_until(lambda: active_forward(b, stale_local) is None,
                               "dismissed Open forward did not stop")
                    gate.unlink(missing_ok=True)
                    release.unlink(missing_ok=True)
                    entered.unlink(missing_ok=True)
                    click("drawer.b_row")
                    marker(b, "ports-after-forward-work", before_b)
                    result["checks"].append("form-open-dispatch-and-remote-b-pty-survive-port-work")
                    assert not any(active_forward(call["target"], call["local"])
                                   for call in forward_calls()), "owned forward remained live"
                    assert saved_forward(a, remote) is None and saved_forward(b, remote) is None
                    result["ports"] = {
                        "remotePort": remote,
                        "http": owned.http,
                        "forwardCalls": forward_calls(),
                        "openerEvents": opened(),
                        "savedMappingsAfter": [],
                    }

                if args.phase == "host-management":
                    assert b not in local_recent() and b not in local_order() and b not in local_catalog(), (
                        "unsaved B appeared in initial local host metadata")
                    assert b not in local_preferences(), "unsaved B had an initial preference"
                    before_invalid = ssh_calls()
                    click("hosts.toolbar_remote")
                    capture("host-management-b-absent")
                    dialog_hidden_pixels = display.pixels(window, region("hosts.dialog_title_region"))
                    click("hosts.connect_entry")
                    assert display.pixels(window, region("hosts.dialog_title_region")) != dialog_hidden_pixels, (
                        "Connect dialog did not become visible")
                    wait_region_stable("hosts.dialog_title_region", "Connect dialog did not settle")
                    capture("host-management-connect-dialog")
                    click("hosts.dialog_target")
                    fill_text("bad target!")
                    key("Return")
                    capture("host-management-invalid-target")
                    assert ssh_calls() == before_invalid, "invalid target invoked SSH"
                    assert b not in local_recent() and b not in local_preferences(), (
                        "invalid target changed local host metadata")
                    key("Escape")
                    wait_until(lambda: display.pixels(window, region("hosts.dialog_title_region"))
                               == dialog_hidden_pixels,
                               "Escape did not dismiss invalid Connect dialog")
                    capture("host-management-escape-dismissed")
                    key("Return")
                    wait_until(lambda: display.pixels(window, region("hosts.dialog_title_region"))
                               != dialog_hidden_pixels,
                               "Return did not reopen Connect after Escape focus return")
                    wait_region_stable("hosts.dialog_title_region", "reopened Connect dialog did not settle")
                    capture("host-management-focus-return-reopened")
                    result["checks"].append(
                        "invalid-target-enter-no-ssh-escape-dismisses-and-return-reopens")

                    click("hosts.dialog_target")
                    fill_text(b)
                    click("hosts.dialog_submit")
                    wait_until(lambda: host_bridge_started(b),
                               "entered private B target did not start native bridge", seconds=25)
                    wait_until(lambda: b in local_recent(),
                               "entered B target was not recorded after ready connection")
                    wait_until(lambda: display.pixels(window, region("hosts.dialog_title_region"))
                               == dialog_hidden_pixels,
                               "Connect dialog did not close after confirmed B preparation")
                    capture("host-management-entered-b-connected")
                    click("hosts.open_b")
                    before_b = marker(b, "host-management-open-b",
                                      {"pid": owned.hosts[b]["shell_pid"],
                                       "pane": owned.hosts[b]["pane"],
                                       "session": owned.hosts[b]["session"]})
                    routes_before = {target: inventory(target) for target in fixture_module.TARGETS}
                    shells_before = {target: sorted(owned._live_shell_markers(target))
                                     for target in fixture_module.TARGETS}
                    remote_config_before = {target: (owned.hosts[target]["config"] / "config.toml").read_bytes()
                                            for target in fixture_module.TARGETS}
                    before_metadata_ssh = ssh_calls()
                    result["checks"].append("entered-absent-owned-target-opens-original-remote-session")

                    click("hosts.toolbar_remote")
                    click("hosts.b_more")
                    click("hosts.b_edit")
                    assert display.pixels(window, region("hosts.dialog_title_region")) != dialog_hidden_pixels, (
                        "B alias editor did not open")
                    wait_region_stable("hosts.dialog_title_region", "B alias editor did not settle")
                    capture("host-management-alias-editor")
                    click("hosts.b_alias")
                    fill_text("Private Beta")
                    key("Return")
                    wait_until(lambda: local_preferences().get(b, {}).get("alias") == "Private Beta"
                               and b in local_recent(),
                               "B alias did not reconcile in authoritative local metadata")
                    wait_until(lambda: display.pixels(window, region("hosts.dialog_title_region"))
                               == dialog_hidden_pixels, "B alias editor did not close after Save")
                    capture("host-management-alias-saved")
                    assert ssh_calls() == before_metadata_ssh, "alias edit invoked SSH"
                    click("hosts.b_more")
                    click("hosts.b_edit")
                    assert display.pixels(window, region("hosts.dialog_title_region")) != dialog_hidden_pixels, (
                        "B clear editor did not open")
                    wait_region_stable("hosts.dialog_title_region", "B clear editor did not settle")
                    click("hosts.b_alias")
                    fill_text("")
                    click("hosts.b_alias_save")
                    wait_until(lambda: local_preferences().get(b, {}).get("alias") is None
                               and b in local_preferences()
                               and b in local_recent(),
                               "B alias clear did not reconcile in authoritative local metadata")
                    wait_until(lambda: display.pixels(window, region("hosts.dialog_title_region"))
                               == dialog_hidden_pixels, "B alias editor did not close after clear")
                    capture("host-management-alias-cleared")
                    assert ssh_calls() == before_metadata_ssh, "alias clear invoked SSH"
                    result["checks"].append("display-name-edit-and-clear-are-passive-local-metadata")

                    click("hosts.b_more")
                    click("hosts.b_remove")
                    wait_until(lambda: local_preferences().get(b, {}).get("hidden") is True
                               and b not in local_recent() and b not in local_order()
                               and b not in local_catalog(),
                               "Remove did not hide B in authoritative local metadata")
                    forgotten_preference = local_preferences()[b]
                    capture("host-management-b-forgotten-with-open-work")
                    assert ssh_calls() == before_metadata_ssh, "Remove invoked SSH"
                    assert {target: inventory(target) for target in fixture_module.TARGETS} == routes_before
                    assert {target: sorted(owned._live_shell_markers(target))
                            for target in fixture_module.TARGETS} == shells_before
                    click("drawer.b_row")
                    marker(b, "host-management-after-remove", before_b)
                    capture("host-management-b-terminal-after-remove")
                    result["checks"].append("remove-hides-metadata-and-preserves-open-remote-pty")

                    click("hosts.toolbar_remote")
                    click("hosts.connect_entry")
                    assert display.pixels(window, region("hosts.dialog_title_region")) != dialog_hidden_pixels, (
                        "B re-add dialog did not open")
                    wait_region_stable("hosts.dialog_title_region", "B re-add dialog did not settle")
                    click("hosts.dialog_target")
                    fill_text(b)
                    click("hosts.dialog_alias")
                    fill_text("Restored Beta")
                    click("hosts.dialog_submit")
                    wait_until(lambda: local_preferences().get(b, {}).get("alias") == "Restored Beta"
                               and local_preferences()[b].get("hidden") is not True,
                               "explicit B re-add did not persist its display name")
                    wait_until(lambda: display.pixels(window, region("hosts.dialog_title_region"))
                               == dialog_hidden_pixels,
                               "Connect dialog did not close after confirmed B re-add")
                    capture("host-management-b-readded")
                    click("drawer.b_row")
                    marker(b, "host-management-after-readd", before_b)
                    assert {target: inventory(target) for target in fixture_module.TARGETS} == routes_before
                    assert {target: sorted(owned._live_shell_markers(target))
                            for target in fixture_module.TARGETS} == shells_before
                    assert all((owned.hosts[target]["config"] / "config.toml").read_bytes()
                               == remote_config_before[target] for target in fixture_module.TARGETS)
                    result["hostManagement"] = {
                        "target": b, "aliasEdit": "Private Beta",
                        "forgottenPreference": forgotten_preference,
                        "readdedPreference": local_preferences().get(b),
                        "remoteRoutes": routes_before, "remoteShellPids": shells_before,
                        "remoteConfigSha256": {target: hashlib.sha256(data).hexdigest()
                                               for target, data in remote_config_before.items()},
                    }
                    result["checks"].append("explicit-readd-preserves-remote-session-and-target-identity")

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

        if paused_bridge is not None:
            try:
                resume_private_bridge(paused_bridge)
                result["pausedBridgeReleasedInCleanup"] = True
            except Exception as error:
                cleanup_errors.append(f"paused bridge: {error}")
            paused_bridge = None

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
    parser.add_argument("--stop-after-create-setup", action="store_true",
                        help="capture connected A/B Drawer geometry, then close normally")
    parser.add_argument("--phase", choices=PHASES, default="full")
    parser.add_argument("--display", default=":98")
    parser.add_argument("--xvfb", default="Xvfb")
    parser.add_argument("--xvfb-lib", type=Path)
    parser.add_argument("--xdotool", default="xdotool")
    parser.add_argument("--tesseract", type=Path)
    parser.add_argument("--tesseract-lib", type=Path)
    parser.add_argument("--tessdata", type=Path)
    args = parser.parse_args()
    if args.setup_only:
        args.phase = "setup"
    if args.stop_after_create_setup and args.phase != "remote-create":
        parser.error("--stop-after-create-setup requires --phase remote-create")
    geometry = json.loads(args.geometry.read_text()) if args.geometry else {}
    if args.phase != "setup" and not args.geometry:
        parser.error("interaction phases require screenshot-calibrated --geometry JSON")
    if args.phase == "reorder" and not args.tesseract:
        parser.error("reorder phase requires --tesseract for visible Drawer order")
    required = set()
    for phase in PHASES[1:PHASES.index(args.phase) + 1]:
        if phase in ("reorder", "ports", "host-management", "settings-remote") and args.phase != phase:
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
        result["traceback"] = traceback.format_exc()
    (args.output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"passed": result["passed"], "phase": args.phase,
                      "checks": result["checks"], "error": result.get("error"),
                      "result": str(args.output / "result.json")}, indent=2))
    if not result["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
