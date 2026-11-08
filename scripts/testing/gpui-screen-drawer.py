#!/usr/bin/env python3
"""Private real-window acceptance for the native Screen Drawer.

Control API calls are reads only. Screen, tab and preference changes use real
window input. A calibrated geometry JSON supplies every feature hit region.
"""

import argparse
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


performance = helper("performance", "gpui-performance.py")
pane_drag = helper("pane_drag", "gpui-pane-drag.py")


def wait_until(predicate, description, seconds=15):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.07)
    raise TimeoutError(description)


def alive(pid):
    try:
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0] != "Z"
    except FileNotFoundError:
        return False


def slots(node):
    if node["type"] == "slot":
        return [node]
    return [slot for child in node["children"] for slot in slots(child)]


def selected(slot):
    return slot.get("activePaneId") or slot["paneIds"][0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--display", default=":97")
    parser.add_argument("--xvfb", default="Xvfb")
    parser.add_argument("--xvfb-lib", type=Path)
    parser.add_argument("--xdotool", default="xdotool")
    parser.add_argument("--geometry", type=Path)
    parser.add_argument("--setup-only", action="store_true")
    parser.add_argument("--reorder-only", action="store_true",
                        help="exercise relative Screen drag with a second owned local client")
    parser.add_argument("--compact-maximize-only", action="store_true",
                        help="capture a pinned Drawer and group maximize at compact window size")
    parser.add_argument("--server-binary", type=Path, default=Path("target/release/splitty"))
    args = parser.parse_args()
    if not args.setup_only and not args.geometry:
        parser.error("full acceptance requires screenshot-calibrated --geometry JSON")
    geometry = json.loads(args.geometry.read_text()) if args.geometry else {}
    binary = args.binary.resolve(strict=True)
    args.output.mkdir(parents=True, exist_ok=False)
    owned = args.output / "owned"
    owned.mkdir(mode=0o700)
    config = args.output / "config"
    config.mkdir(mode=0o700)
    if len(os.fsencode(config / "run" / ("spike-" + "0" * 32 + ".sock"))) > 107:
        parser.error("output path is too long for the owned Linux Unix socket; use a short /tmp path")
    wrapper = args.output / "sh"
    wrapper.write_text(
        "#!/bin/sh\n"
        f"cd {shlex.quote(str(owned.resolve()))} || exit 1\n"
        f"printf '%s\\t%s\\t%s\\n' \"$$\" \"$SPLITTY_PANE_ID\" \"$SPLITTY_SESSION_ID\" > {shlex.quote(str(owned.resolve()))}/shell-$$.tsv\n"
        "exec /bin/sh \"$@\"\n"
    )
    wrapper.chmod(0o700)
    (config / "config.toml").write_text(
        '[appearance]\ntheme_mode = "fixed"\ntheme = "splitty-dark"\n\n'
        '[drawer]\npinned = true\nsidebar_width = 280\n\n'
        '[shells]\ndefault_profile_id = "drawer-test"\n\n'
        '[[shells.profiles]]\nid = "drawer-test"\nname = "Owned shell"\n'
        f'command = {json.dumps(str(wrapper.resolve()))}\n'
        'enabled = true\ntarget = "local"\nsource = "custom"\n'
    )
    env = os.environ.copy()
    env.pop("WAYLAND_DISPLAY", None)
    env["DISPLAY"] = args.display
    env["SPLITTY_CONFIG_DIR"] = str(config.resolve())
    for key in ("SPLITTY_SOCK", "SPLITTY_AGENT_SOCK", "SPLITTY_ADDR", "SPLITTY_TOKEN",
                "SPLITTY_SERVER", "SPLITTY_PANE_ID", "SPLITTY_SCREEN_ID", "SPLITTY_SESSION_ID"):
        env.pop(key, None)
    xvfb_env = env.copy()
    if args.xvfb_lib:
        xvfb_env["LD_LIBRARY_PATH"] = str(args.xvfb_lib) + (
            ":" + env["LD_LIBRARY_PATH"] if env.get("LD_LIBRARY_PATH") else "")
    result = {"binarySha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "harnessSha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "geometrySha256": hashlib.sha256(args.geometry.read_bytes()).hexdigest() if args.geometry else None,
              "checks": [], "states": {}, "screenshots": [], "passed": False}
    xvfb = display = app = window = None
    children = set()

    def x(*arguments):
        return subprocess.check_output([str(args.xdotool), *map(str, arguments)],
                                       env=env, text=True, timeout=12)

    def point(key):
        value = geometry[key]
        assert len(value) == 2 and all(isinstance(n, int) for n in value), (key, value)
        return value

    def click(key):
        x("mousemove", "--window", window, *point(key))
        x("click", 1)
        display.sync()
        time.sleep(.18)

    def wheel(key, button):
        x("mousemove", "--window", window, *point(key))
        x("click", button)
        display.sync()
        time.sleep(.2)

    def key(name):
        display.type_key(name)
        display.sync()
        time.sleep(.12)

    def chord(modifier, name):
        display.key(modifier, True)
        key(name)
        display.key(modifier, False)
        display.sync()

    def type_text(value):
        x("type", "--clearmodifiers", "--delay", 3, value)
        display.sync()

    def capture(label):
        path = args.output / f"{label}.png"
        pane_drag.save_png(path, display.pixels(window, (0, 0, 1200, 760)))
        result["screenshots"].append(str(path))

    def color_pixels(pixels, rgb):
        # XGetImage is BGRX on this owned X11 display. Allow a small renderer
        # tolerance but require a substantial solid terminal background patch.
        red, green, blue = rgb
        return sum(abs(pixels[offset] - blue) <= 10
                   and abs(pixels[offset + 1] - green) <= 10
                   and abs(pixels[offset + 2] - red) <= 10
                   for offset in range(0, len(pixels), 4))

    def api(path):
        def socket():
            candidates = list((config / "run").glob("spike-*.sock"))
            if len(candidates) > 1:
                raise RuntimeError("more than one owned service socket")
            if candidates and stat.S_ISSOCK(candidates[0].lstat().st_mode):
                return candidates[0]
            return None
        owned_socket = wait_until(socket, "owned service socket did not bind", seconds=6)
        data = subprocess.check_output(
            ["curl", "--fail", "--silent", "--show-error", "--max-time", "5",
             "--unix-socket", str(owned_socket), f"http://localhost{path}"],
            text=True, timeout=7)
        return json.loads(data)

    def state(label=None):
        value = api("/api/mux/state")
        if label:
            result["states"][label] = value
        return value

    def read_settings():
        # The owned control socket has no home-settings route. This file is
        # private to the run, and its saved bytes prove explicit persistence.
        return tomllib.loads((config / "config.toml").read_text())

    def shells():
        found = {}
        for marker in owned.glob("shell-*.tsv"):
            pid, pane, session = marker.read_text().strip().split("\t")
            found[int(pid)] = {"pane": pane, "session": session}
        return found

    def assert_routes(value):
        routes = {pane: item.get("sessionId") for screen in value["screens"].values()
                  for pane, item in screen["panes"].items()}
        for pid, shell in shells().items():
            if alive(pid):
                assert routes.get(shell["pane"]) == shell["session"], (pid, shell, routes)

    def launch(log):
        nonlocal app, window
        app = subprocess.Popen([str(binary)], env=env, stdout=log, stderr=log,
                               start_new_session=True)

        def find_window():
            if app.poll() is not None:
                raise RuntimeError(f"app exited before window: {app.returncode}")
            found = subprocess.run([str(args.xdotool), "search", "--onlyvisible", "--pid", str(app.pid)],
                                   env=env, capture_output=True, text=True, timeout=3)
            for candidate in found.stdout.splitlines():
                values = dict(line.split("=", 1) for line in x("getwindowgeometry", "--shell", candidate).splitlines()
                              if "=" in line)
                if values.get("WIDTH") == "1200" and values.get("HEIGHT") == "760":
                    return int(candidate)
        window = wait_until(find_window, "no 1200x760 native window", seconds=30)
        x("windowfocus", window)
        wait_until(lambda: any(alive(pid) for pid in shells()), "owned /bin/sh did not start", seconds=30)

        def active_sessions_running():
            current = state()
            screen_id = current["activeScreenId"]
            sessions = current["screens"][screen_id]["sessions"]
            return bool(sessions) and all(session["status"] == "running" for session in sessions.values())

        wait_until(active_sessions_running, "active Screen sessions did not reach running", seconds=20)
        time.sleep(.3)

    def close_app():
        nonlocal app, window
        if app and app.poll() is None and display and window:
            display.close_window(window)
            time.sleep(.4)
            if app.poll() is None:
                exists = subprocess.run(
                    [str(args.xdotool), "getwindowgeometry", "--shell", str(window)],
                    env=env, capture_output=True, timeout=3).returncode == 0
                if exists:
                    try:
                        click("window.confirm_close")
                    except subprocess.CalledProcessError:
                        # WM close may complete between the existence check and click.
                        if app.poll() is None:
                            raise
            app.wait(timeout=12)
            assert app.returncode == 0, f"window exit {app.returncode}"
        app = None
        window = None

    with (args.output / "xvfb.log").open("w") as xvfb_log, (args.output / "app.log").open("w") as app_log:
        try:
            number = args.display.lstrip(":")
            if Path(f"/tmp/.X11-unix/X{number}").exists() or Path(f"/tmp/.X{number}-lock").exists():
                raise RuntimeError(f"DISPLAY {args.display} is already in use")
            xvfb = subprocess.Popen([str(args.xvfb), args.display, "-screen", "0", "1280x900x24",
                                     "-nolisten", "unix", "-nolisten", "tcp", "-listen", "local",
                                     "-noreset", "-ac", "-xkbdir", "/usr/share/X11/xkb"],
                                    env=xvfb_env, stdout=xvfb_log, stderr=xvfb_log,
                                    start_new_session=True)
            def ready():
                if xvfb.poll() is not None:
                    log = (args.output / "xvfb.log").read_text(errors="replace")[-2000:]
                    raise RuntimeError(f"Xvfb exited early ({xvfb.returncode}): {log}")
                try:
                    return subprocess.run([str(args.xdotool), "getdisplaygeometry"], env=env,
                                          capture_output=True, timeout=2).returncode == 0
                except subprocess.TimeoutExpired:
                    return False

            wait_until(ready, "private Xvfb did not become ready", seconds=20)
            old_display = os.environ.get("DISPLAY")
            os.environ["DISPLAY"] = args.display
            try:
                display = performance.Display()
            finally:
                if old_display is None:
                    os.environ.pop("DISPLAY", None)
                else:
                    os.environ["DISPLAY"] = old_display
            launch(app_log)
            initial = state("initial")
            initial_id = initial["activeScreenId"]
            initial_pid = next(pid for pid in shells() if alive(pid))
            assert_routes(initial)
            capture("initial-drawer")
            result["checks"].append("isolated-owned-service-shell-and-saved-drawer")
            if args.setup_only:
                if "terminal.initial" in geometry:
                    click("terminal.initial")
                type_text("exit")
                key("Return")
                wait_until(lambda: not alive(initial_pid), "setup shell did not exit normally")
                close_app()
            elif args.compact_maximize_only:
                x("windowsize", "--sync", window, 560, 420)
                display.sync()
                time.sleep(.25)
                def compact_capture(label):
                    path = args.output / f"{label}.png"
                    pane_drag.save_png(path, display.pixels(window, (0, 0, 560, 420)),
                                       width=560, height=420)
                    result["screenshots"].append(str(path))
                compact_capture("compact-pinned")
                x("mousemove", "--window", window, 430, 230)
                x("click", 1)
                chord("Alt_L", "m")
                time.sleep(.3)
                compact_capture("compact-maximized")
                marker = owned / "compact-terminal.marker"
                type_text(f"printf '%s\\n' \"$$:$SPLITTY_PANE_ID:$SPLITTY_SESSION_ID\" > {shlex.quote(str(marker))}")
                key("Return")
                value = wait_until(lambda: marker.read_text().strip() if marker.exists() else None,
                                   "compact terminal did not receive keyboard input")
                shell = shells()[initial_pid]
                assert value == f"{initial_pid}:{shell['pane']}:{shell['session']}", value
                chord("Alt_L", "m")
                time.sleep(.3)
                compact_capture("compact-restored")
                result["checks"].append("compact-pinned-drawer-maximize-restore-and-terminal-input")
                close_app()
            elif args.reorder_only:
                server = args.server_binary.resolve(strict=True)
                sockets = list((config / "run").glob("spike-*.sock"))
                assert len(sockets) == 1 and stat.S_ISSOCK(sockets[0].lstat().st_mode)
                cli_env = env.copy()
                cli_env["SPLITTY_SOCK"] = str(sockets[0])
                for name in ("Bravo", "Charlie", "Delta"):
                    subprocess.check_output([str(server), "new-screen", "--name", name],
                                            env=cli_env, text=True, timeout=15)
                ordered = wait_until(
                    lambda: (s if len((s := state())["screenOrder"]) == 4 else None),
                    "second client Screens did not reach the window")
                wait_until(
                    lambda: all(screen["sessions"] and all(session["status"] == "running"
                        for session in screen["sessions"].values())
                        for screen in state()["screens"].values()),
                    "second client Screen sessions did not settle")
                ids = {screen["name"]: id for id, screen in ordered["screens"].items()}
                assert set(ids) >= {"Bravo", "Charlie", "Delta"}, ids
                assert ordered["screenOrder"] == [initial_id, ids["Bravo"], ids["Charlie"], ids["Delta"]]
                click("drawer.row_initial")
                wait_until(lambda: state()["activeScreenId"] == initial_id,
                           "initial Screen was not selected before placement")
                baseline = state("before-reorder")
                before_sessions = {id: screen["sessions"] for id, screen in baseline["screens"].items()}
                capture("before-reorder")

                def drag(from_key, to_key, cancel=False):
                    start, finish = point(from_key), point(to_key)
                    x("mousemove", "--window", window, *start)
                    x("mousedown", 1)
                    x("mousemove", "--window", window, start[0] + 10, start[1])
                    x("mousemove", "--window", window, *finish)
                    display.sync()
                    time.sleep(.12)
                    capture("cancel-target" if cancel else "relative-insertion-target")
                    if cancel:
                        key("Escape")
                    x("mouseup", 1)
                    display.sync()

                drag("drawer.row_charlie", "drawer.before_initial")
                after = wait_until(
                    lambda: (s if (s := state())["screenOrder"] ==
                             [ids["Charlie"], initial_id, ids["Bravo"], ids["Delta"]] else None),
                    "relative placement did not preserve concurrent Delta")
                assert after["activeScreenId"] == initial_id, "placement navigated away"
                assert {id: screen["sessions"] for id, screen in after["screens"].items()} == before_sessions
                assert alive(initial_pid), "placement replaced the original shell"
                assert_routes(after)
                time.sleep(.7)
                capture("after-reorder")
                result["checks"].append("relative-drag-retains-concurrent-screen-selection-and-sessions")

                drag("drawer.row_bravo_after", "drawer.before_charlie_after", cancel=True)
                assert state()["screenOrder"] == after["screenOrder"], "Escape changed Screen order"
                capture("after-reorder-cancel")
                result["checks"].append("escape-cancels-drawer-placement")

                marker = owned / "reorder-terminal-output.marker"
                click("terminal.initial")
                type_text(f"printf '%s\\n' \"$$:$SPLITTY_PANE_ID:$SPLITTY_SESSION_ID\" > {shlex.quote(str(marker))}")
                key("Return")
                output = wait_until(lambda: marker.read_text().strip() if marker.exists() else None,
                                    "original terminal did not receive output after placement")
                original_shell = shells()[initial_pid]
                assert output == f"{initial_pid}:{original_shell['pane']}:{original_shell['session']}"
                assert state()["activeScreenId"] == initial_id
                result["checks"].append("terminal-input-and-output-keep-original-session-after-reorder")
                close_app()
            else:
                click("drawer.create")
                created = wait_until(lambda: (s if len((s := state())["screenOrder"]) == 2 else None),
                                     "Drawer create did not add a Screen")
                second_id = next(id for id in created["screenOrder"] if id != initial_id)
                wait_until(lambda: next((pid for pid, shell in shells().items() if alive(pid)
                                        and shell["pane"] in created["screens"][second_id]["panes"]), None),
                           "created Screen has no owned shell")
                wait_until(lambda: (sessions if (sessions := state()["screens"][second_id]["sessions"])
                                    and all(session["status"] == "running" for session in sessions.values()) else None),
                           "created Screen sessions did not reach running")
                time.sleep(.2)
                click("drawer.rename")
                chord("Control_L", "a")
                type_text("Drawer renamed")
                before_rename_wheel = state()
                wheel("drawer.header", 4)
                assert state() == before_rename_wheel, "rename editor allowed a workspace mutation"
                key("Return")
                wait_until(lambda: state()["screens"][second_id]["name"] == "Drawer renamed",
                           "Drawer rename did not persist")
                click("drawer.row_initial")
                wait_until(lambda: state()["activeScreenId"] == initial_id, "Drawer row did not switch")
                capture("created-renamed-and-switched")
                result["checks"].append("drawer-create-rename-switch-through-window")

                # Emit from the original shell after its Screen is hidden.
                # The file proves the PTY wrote; changed terminal pixels after
                # returning prove the preserved view received that output.
                click("group.left_body")
                hidden_marker = owned / "hidden-screen-output.marker"
                hidden_text = "ROUTED_HIDDEN_SCREEN_OUTPUT"
                marker_rgb = (17, 201, 83)
                type_text("(sleep 2; printf '\\033[48;2;17;201;83m          \\033[0m\\n'; "
                          f"printf '%s\\n' {hidden_text} > {shlex.quote(str(hidden_marker))}) &")
                key("Return")
                output_region = geometry["terminal.output_region"]
                assert len(output_region) == 4 and all(isinstance(value, int) for value in output_region)
                before_hidden = color_pixels(display.pixels(window, tuple(output_region)), marker_rgb)
                assert before_hidden < 20, f"marker color was already present: {before_hidden} pixels"
                click("drawer.row_new")
                wait_until(lambda: state()["activeScreenId"] == second_id,
                           "original Screen did not hide before delayed output")
                assert not hidden_marker.exists(), "output emitted before its Screen was hidden"
                wait_until(lambda: hidden_marker.exists(), "hidden Screen shell did not emit output")
                assert state()["activeScreenId"] == second_id, "output did not arrive while original Screen was hidden"
                assert hidden_marker.read_text().strip() == hidden_text
                click("drawer.row_initial")
                wait_until(lambda: state()["activeScreenId"] == initial_id,
                           "original Screen did not reappear after hidden output")
                painted = wait_until(
                    lambda: (count if (count := color_pixels(
                        display.pixels(window, tuple(output_region)), marker_rgb)) >= 200 else None),
                    "hidden Screen truecolor output did not paint after return")
                result["hiddenScreenMarkerPixels"] = {"before": before_hidden, "after": painted}
                assert alive(initial_pid), "hidden Screen output replaced its shell"
                assert_routes(state("after-hidden-screen-output"))
                capture("hidden-screen-output-delivered")
                result["checks"].append("hidden-screen-output-retains-session-and-paints-after-return")

                for count in range(2, 6):
                    chord("Alt_L", "t")
                    wait_until(lambda: len(state()["screens"][initial_id]["panes"]) == count,
                               f"tab {count} did not launch")
                chord("Alt_L", "d")
                wait_until(lambda: len(slots(state()["screens"][initial_id]["root"])) == 2,
                           "second group did not launch")
                root = state()["screens"][initial_id]["root"]
                left, right = slots(root)
                assert len(left["paneIds"]) == 5 and len(right["paneIds"]) == 1
                left_before = selected(left)
                right_before = selected(right)
                click("group.right_body")
                wheel("tab.left_header", 4)
                wait_until(lambda: selected(slots(state()["screens"][initial_id]["root"])[0]) != left_before,
                           "hovered unfocused tab group did not navigate")
                after_tab = slots(state()["screens"][initial_id]["root"])
                assert selected(after_tab[1]) == right_before
                tab_selection = [selected(slot) for slot in after_tab]
                tab_region = geometry["tab.capture_region"]
                assert len(tab_region) == 4 and all(isinstance(value, int) for value in tab_region), tab_region
                x("mousemove", "--window", window, *point("tab.horizontal_overflow"))
                display.sync()
                time.sleep(.3)
                capture("overflow-before-horizontal")
                tab_before_horizontal = display.pixels(window, tuple(tab_region))
                wheel("tab.horizontal_overflow", 6)
                tab_after_horizontal = display.pixels(window, tuple(tab_region))
                assert tab_after_horizontal != tab_before_horizontal, (
                    "horizontal wheel did not reveal different tab-strip pixels; calibrate tab.capture_region")
                assert [selected(slot) for slot in slots(state()["screens"][initial_id]["root"])] == tab_selection, (
                    "horizontal tab overflow changed tab selection")
                capture("horizontal-tab-overflow")

                # X11 maps Shift+vertical wheel to horizontal scrolling.
                x("mousemove", "--window", window, *point("tab.horizontal_overflow"))
                display.sync()
                time.sleep(.3)
                tab_before_shift = display.pixels(window, tuple(tab_region))
                display.key("Shift_L", True)
                display.sync()
                try:
                    wheel("tab.horizontal_overflow", 5)
                finally:
                    display.key("Shift_L", False)
                    display.sync()
                tab_after_shift = display.pixels(window, tuple(tab_region))
                assert tab_after_shift != tab_before_shift, (
                    "Shift+vertical wheel did not reveal different tab-strip pixels; calibrate tab.capture_region")
                assert [selected(slot) for slot in slots(state()["screens"][initial_id]["root"])] == tab_selection, (
                    "Shift+vertical tab overflow changed tab selection")
                capture("shift-wheel-tab-overflow")
                wheel("group.left_body", 5)
                terminal_wheel = state()
                assert terminal_wheel["activeScreenId"] == initial_id
                assert [selected(slot) for slot in slots(terminal_wheel["screens"][initial_id]["root"])] == [
                    selected(slot) for slot in after_tab], "terminal body wheel changed tab selection"
                capture("hovered-tab-wheel")
                result["checks"].append("hovered-unfocused-tab-wheel-horizontal-shift-overflow-and-terminal-wheel-isolation")

                wheel("drawer.header", 5)
                wait_until(lambda: state()["activeScreenId"] == second_id,
                           "header wheel did not select next Screen")
                wheel("drawer.list", 5)
                assert state()["activeScreenId"] == second_id, "list wheel changed selected Screen"
                wheel("drawer.header", 4)
                wait_until(lambda: state()["activeScreenId"] == initial_id,
                           "header wheel did not select prior Screen")
                capture("screen-wheel")
                result["checks"].append("header-wheel-switches-and-list-wheel-scrolls")

                target = selected(slots(state()["screens"][initial_id]["root"])[0])
                target_session = state()["screens"][initial_id]["panes"][target]["sessionId"]
                target_pid = next(pid for pid, shell in shells().items()
                                  if alive(pid) and shell == {"pane": target, "session": target_session})
                marker = owned / "typed-terminal.marker"
                click("group.left_body")
                type_text(f"printf '%s\\n' \"$$:$SPLITTY_PANE_ID:$SPLITTY_SESSION_ID\" > {shlex.quote(str(marker))}")
                key("Return")
                value = wait_until(lambda: marker.read_text().strip() if marker.exists() else None,
                                   "typed command did not reach selected terminal")
                assert value == f"{target_pid}:{target}:{target_session}", value
                assert_routes(state("after-terminal-input"))
                result["checks"].append("terminal-input-routed-to-intended-live-session")

                click("drawer.row_new")
                wait_until(lambda: state()["activeScreenId"] == second_id, "new Screen did not reopen")
                click("group.second_screen_body")
                type_text("sleep 60")
                key("Return")
                wait_until(lambda: any(session["busy"] for session in
                                       state()["screens"][second_id]["sessions"].values()),
                           "second Screen did not enter busy state before close assessment")
                click("drawer.close_new")
                capture("screen-close-prompt")
                assert second_id in state()["screens"], "busy Screen closed without assessment"
                wheel("drawer.header", 4)
                assert state()["activeScreenId"] == second_id, "close prompt allowed Screen wheel navigation"
                assert second_id in state()["screens"], "close prompt changed captured Screen"
                click("prompt.keep")
                assert second_id in state()["screens"], "cancelled Screen close removed Screen"
                click("drawer.close_new")
                click("prompt.confirm")
                wait_until(lambda: second_id not in state()["screens"], "confirmed Screen close did not remove Screen")
                assert alive(initial_pid), "close affected original Screen shell"
                result["checks"].append("drawer-assessed-close-cancel-confirm-and-wheel-priority")

                click("group.left_body")
                before_toggle_state = state()
                focused_pane = selected(slots(before_toggle_state["screens"][initial_id]["root"])[0])
                focused_session = before_toggle_state["screens"][initial_id]["panes"][focused_pane]["sessionId"]
                focused_pid = next(pid for pid, shell in shells().items()
                                   if alive(pid) and shell == {"pane": focused_pane, "session": focused_session})
                drawer_region = tuple(geometry["drawer.capture_region"])
                before = display.pixels(window, drawer_region)
                toggle_started = time.monotonic()
                click("drawer.toggle")
                wait_until(lambda: read_settings()["drawer"]["pinned"] is False,
                           "visibility update was not saved")
                assert read_settings()["drawer"]["sidebar_width"] == 280
                after = wait_until(lambda: (pixels if (pixels := display.pixels(window, drawer_region)) != before
                                            else None), "saved Drawer toggle did not redraw")
                result["drawerVisibilityMs"] = round((time.monotonic() - toggle_started) * 1000)
                focus_marker = owned / "after-toggle.marker"
                type_text(f"printf '%s\\n' \"$$:$SPLITTY_PANE_ID:$SPLITTY_SESSION_ID\" > {shlex.quote(str(focus_marker))}")
                key("Return")
                focused_value = wait_until(lambda: focus_marker.read_text().strip() if focus_marker.exists() else None,
                                           "Drawer toggle did not restore terminal keyboard focus")
                assert focused_value == f"{focused_pid}:{focused_pane}:{focused_session}", focused_value
                capture("drawer-collapsed-saved")
                result["checks"].append("explicit-visibility-update-persisted-and-terminal-focus-restored")
                children |= set(performance.process_stats(app.pid)) - {app.pid}
                close_app()
                assert not any(alive(pid) for pid in children), "first window left owned children"
                saved_bytes = (config / "config.toml").read_bytes()
                launch(app_log)
                assert read_settings()["drawer"]["pinned"] is False
                assert read_settings()["drawer"]["sidebar_width"] == 280
                assert (config / "config.toml").read_bytes() == saved_bytes, (
                    "loading saved Drawer preferences wrote settings")
                restarted = display.pixels(window, tuple(geometry["drawer.capture_region"]))
                assert restarted != before, "restarted Drawer did not restore saved visibility"
                capture("drawer-restored-after-restart")
                result["checks"].append("restart-restores-saved-visibility-and-width")
                children |= set(performance.process_stats(app.pid)) - {app.pid}
                close_app()
            result["passed"] = True
        except Exception as error:
            result["error"] = repr(error)
            if display and window:
                try:
                    capture("failure")
                except Exception as capture_error:
                    result["captureError"] = repr(capture_error)
        finally:
            if app:
                children |= set(performance.process_stats(app.pid)) - {app.pid}
                if app.poll() is None:
                    try:
                        close_app()
                    except Exception as close_error:
                        result["closeError"] = repr(close_error)
                        if app and app.poll() is None:
                            os.killpg(app.pid, signal.SIGTERM)
                            try:
                                app.wait(timeout=3)
                            except subprocess.TimeoutExpired:
                                os.killpg(app.pid, signal.SIGKILL)
                                app.wait(timeout=3)
            if display:
                display.close()
            if xvfb:
                try:
                    os.killpg(xvfb.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    xvfb.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(xvfb.pid, signal.SIGKILL)
            result["survivingChildren"] = [pid for pid in children if alive(pid)]
            result["passed"] = result["passed"] and not result["survivingChildren"]
            (args.output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps({"passed": result["passed"], "checks": result["checks"],
                              "error": result.get("error"), "result": str(args.output / "result.json")}, indent=2))
    if not result["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
