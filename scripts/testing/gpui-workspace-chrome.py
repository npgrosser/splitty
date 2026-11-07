#!/usr/bin/env python3
"""Real-window acceptance of native group chrome on an owned X11 display.

All mutations go through pointer or keyboard input. The owned control socket is
used only to read mux state. Coordinates come from a separately calibrated,
hashed geometry file; --setup-only captures the image used for calibration.
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

HERE = Path(__file__).resolve().parent


def helper(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


performance = helper("performance", "gpui-performance.py")
pane_drag = helper("pane_drag", "gpui-pane-drag.py")
drawer = helper("drawer", "gpui-screen-drawer.py")


def wait_until(predicate, description, seconds=15):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.08)
    raise TimeoutError(description)


def alive(pid):
    try:
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0] != "Z"
    except FileNotFoundError:
        return False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--display", default=":98")
    parser.add_argument("--xvfb", default="Xvfb")
    parser.add_argument("--xvfb-lib", type=Path)
    parser.add_argument("--xdotool", default="xdotool")
    parser.add_argument("--geometry", type=Path)
    parser.add_argument("--setup-only", action="store_true")
    parser.add_argument("--setup-split", action="store_true")
    parser.add_argument("--setup-groups", action="store_true")
    parser.add_argument("--setup-profile", action="store_true")
    parser.add_argument("--setup-menu", action="store_true")
    parser.add_argument("--setup-maximize", action="store_true")
    parser.add_argument("--setup-float", action="store_true")
    parser.add_argument("--setup-dock", action="store_true")
    parser.add_argument("--setup-high-menu", action="store_true")
    parser.add_argument("--high-zoom-check", action="store_true")
    parser.add_argument("--resize-check", action="store_true",
                        help="drag a divider through several confirmed revisions after the chrome checks")
    parser.add_argument("--zoom", type=float, default=1.0)
    args = parser.parse_args()
    if not 0.5 <= args.zoom <= 3.0:
        parser.error("--zoom must be between 0.5 and 3.0")
    if not args.setup_only and not args.geometry:
        parser.error("full acceptance requires screenshot-calibrated --geometry JSON")
    binary = args.binary.resolve(strict=True)
    geometry = json.loads(args.geometry.read_text()) if args.geometry else {}
    args.output.mkdir(parents=True, exist_ok=False)
    owned = args.output / "owned"
    owned.mkdir(mode=0o700)
    config = args.output / "config"
    config.mkdir(mode=0o700)
    if len(os.fsencode(config / "run" / ("spike-" + "0" * 32 + ".sock"))) > 107:
        parser.error("shorten --output for the owned Unix socket")
    wrapper = args.output / "sh"
    wrapper.write_text(
        "#!/bin/sh\n"
        f"cd {shlex.quote(str(owned.resolve()))} || exit 1\n"
        f"printf '%s\\t%s\\t%s\\n' \"$$\" \"$SPLITTY_PANE_ID\" \"$SPLITTY_SESSION_ID\" > {shlex.quote(str(owned.resolve()))}/shell-$$.tsv\n"
        "exec /bin/sh \"$@\"\n"
    )
    wrapper.chmod(0o700)
    (config / "config.toml").write_text(
        '[appearance]\ntheme_mode = "fixed"\ntheme = "splitty-dark"\n'
        f'zoom = {args.zoom}\nzoom_scope = "window"\n\n'
        '[drawer]\npinned = false\nsidebar_width = 280\n\n'
        '[shells]\ndefault_profile_id = "chrome-a"\n\n'
        '[[shells.profiles]]\nid = "chrome-a"\nname = "Owned A"\n'
        f'command = {json.dumps(str(wrapper.resolve()))}\n'
        'enabled = true\ntarget = "local"\nsource = "custom"\n\n'
        '[[shells.profiles]]\nid = "chrome-b"\nname = "Owned B"\n'
        f'command = {json.dumps(str(wrapper.resolve()))}\n'
        'enabled = true\ntarget = "local"\nsource = "custom"\n'
    )
    env = os.environ.copy()
    env.pop("WAYLAND_DISPLAY", None)
    env["DISPLAY"] = args.display
    env["GPUI_BACKEND"] = "x11"
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
              "checks": [], "screenshots": [], "states": {}, "passed": False}
    xvfb = display = app = window = None
    children = set()

    def x(*arguments):
        return subprocess.check_output([str(args.xdotool), *map(str, arguments)],
                                       env=env, text=True, timeout=12)

    def point(name):
        value = geometry[name]
        assert len(value) == 2 and all(isinstance(n, int) for n in value), (name, value)
        return value

    def click(name):
        x("mousemove", "--window", window, *point(name))
        x("click", 1)
        display.sync()
        time.sleep(.2)

    def key(name):
        display.type_key(name)
        display.sync()
        time.sleep(.12)

    def chord(modifier, name):
        display.key(modifier, True)
        try:
            key(name)
        finally:
            display.key(modifier, False)
            display.sync()

    def type_text(value):
        x("type", "--clearmodifiers", "--delay", 3, value)
        display.sync()

    def wheel(name, button):
        x("mousemove", "--window", window, *point(name))
        display.wheel(button)
        display.sync()
        time.sleep(.2)

    def capture(name):
        path = args.output / f"{name}.png"
        pane_drag.save_png(path, display.pixels(window, (0, 0, 1200, 760)))
        result["screenshots"].append(str(path))

    def state(name=None):
        def socket():
            candidates = list((config / "run").glob("spike-*.sock"))
            assert len(candidates) <= 1
            return candidates[0] if candidates and stat.S_ISSOCK(candidates[0].lstat().st_mode) else None
        owned_socket = wait_until(socket, "owned socket did not bind", seconds=6)
        body = subprocess.check_output(
            ["curl", "--fail", "--silent", "--show-error", "--max-time", "5",
             "--unix-socket", str(owned_socket), "http://localhost/api/mux/state"],
            text=True, timeout=7)
        value = json.loads(body)
        if name:
            result["states"][name] = value
        return value

    def shells():
        found = {}
        for marker in owned.glob("shell-*.tsv"):
            pid, pane, session = marker.read_text().strip().split("\t")
            found[int(pid)] = {"pane": pane, "session": session}
        return found

    def routes(value):
        return {pane: item.get("sessionId") for screen in value["screens"].values()
                for pane, item in screen["panes"].items()}

    def assert_routes(value):
        current = routes(value)
        for pid, shell in shells().items():
            if alive(pid):
                assert current.get(shell["pane"]) == shell["session"], (pid, shell, current)

    def close_app():
        nonlocal app, window
        if app and app.poll() is None and display and window:
            display.close_window(window)
            time.sleep(.4)
            if app.poll() is None:
                exists = subprocess.run([str(args.xdotool), "getwindowgeometry", "--shell", str(window)],
                                        env=env, capture_output=True, timeout=3).returncode == 0
                if exists and "window.confirm_close" in geometry:
                    click("window.confirm_close")
            app.wait(timeout=12)
            assert app.returncode == 0, f"window exit {app.returncode}"
            result["normalWindowClose"] = True
            result["exitCode"] = app.returncode
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
                    raise RuntimeError(f"Xvfb exited: {(args.output / 'xvfb.log').read_text()[-2000:]}")
                try:
                    return subprocess.run([str(args.xdotool), "getdisplaygeometry"], env=env,
                                          capture_output=True, timeout=2).returncode == 0
                except subprocess.TimeoutExpired:
                    return False
            wait_until(ready, "private Xvfb did not start", seconds=20)
            old_display = os.environ.get("DISPLAY")
            os.environ["DISPLAY"] = args.display
            try:
                display = performance.Display()
            finally:
                if old_display is None:
                    os.environ.pop("DISPLAY", None)
                else:
                    os.environ["DISPLAY"] = old_display
            app = subprocess.Popen([str(binary)], env=env, stdout=app_log, stderr=app_log,
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
            window = wait_until(find_window, "no native 1200x760 window", seconds=30)
            x("windowfocus", window)
            wait_until(lambda: any(alive(pid) for pid in shells()), "owned shell did not start", seconds=30)
            initial = state("initial")
            assert_routes(initial)
            capture("setup")
            result["checks"].append("isolated-owned-window-and-shell")
            if args.setup_split or args.setup_high_menu or args.high_zoom_check:
                screen = initial["activeScreenId"]
                click("group.initial.split_right")
                wait_until(lambda: len(drawer.slots(state()["screens"][screen]["root"])) == 2,
                           "initial split did not add group")
                capture("split")
                if args.setup_high_menu:
                    click("group.left.more")
                    capture("high-zoom-menu")
                    key("Escape")
                elif args.high_zoom_check:
                    two = state()
                    right = drawer.slots(two["screens"][screen]["root"])[1]
                    right_pane = drawer.selected(right)
                    right_session = two["screens"][screen]["panes"][right_pane]["sessionId"]
                    click("group.right.body")
                    assert display.pixels(window, tuple(geometry["icon.plus_region"])) != display.pixels(
                        window, tuple(geometry["icon.empty_region"])), "Kit plus icon did not render"
                    click("group.left.more")
                    capture("high-zoom-menu")
                    click("menu.high.split_below")
                    three = wait_until(lambda: (s if len(drawer.slots((s := state())["screens"][screen]["root"])) == 3 else None),
                                       "high zoom compact Split below did not add group")
                    assert three["screens"][screen]["panes"][right_pane]["sessionId"] == right_session
                    assert_routes(three)
                    capture("high-zoom-split-below")
                    result["states"]["high-zoom-split-below"] = three
                    result["checks"].append("high-zoom-compact-menu-icon-and-split-preserve-other-session")
            elif args.setup_only:
                initial_pid = next(pid for pid in shells() if alive(pid))
                type_text("exit")
                key("Return")
                wait_until(lambda: not alive(initial_pid), "setup shell did not exit")
            else:
                screen = initial["activeScreenId"]
                click("group.initial.split_right")
                split = wait_until(lambda: (s if len(drawer.slots((s := state())["screens"][screen]["root"])) == 2 else None),
                                   "initial split did not add group")
                left, right = drawer.slots(split["screens"][screen]["root"])
                right_pane = drawer.selected(right)
                right_session = split["screens"][screen]["panes"][right_pane]["sessionId"]
                click("group.right.body")
                click("group.left.new_tab")
                after_tab = wait_until(lambda: (s if len(drawer.slots((s := state())["screens"][screen]["root"])[0]["paneIds"]) == 2 else None),
                                       "unfocused left new-tab did not launch")
                assert drawer.selected(drawer.slots(after_tab["screens"][screen]["root"])[1]) == right_pane
                assert after_tab["screens"][screen]["panes"][right_pane]["sessionId"] == right_session
                assert_routes(after_tab)
                result["checks"].append("unfocused-group-new-tab-preserves-right-session")
                click("group.right.body")
                click("group.left.split_right")
                after_split = wait_until(lambda: (s if len(drawer.slots((s := state())["screens"][screen]["root"])) == 3 else None),
                                         "unfocused left split did not launch")
                assert after_split["screens"][screen]["panes"][right_pane]["sessionId"] == right_session
                assert_routes(after_split)
                capture("three-groups")
                result["states"]["three-groups"] = after_split
                result["checks"].append("unfocused-group-split-preserves-right-session")
                if args.setup_profile:
                    click("group.left.body")
                    click("group.right.profile")
                    capture("profile-picker")
                    key("Escape")
                elif args.setup_menu:
                    click("group.right.more")
                    capture("compact-menu")
                    key("Escape")
                elif args.setup_maximize:
                    click("group.right.more")
                    click("menu.maximize")
                    result["states"]["maximized"] = state()
                    capture("maximized")
                elif not args.setup_groups:
                    click("group.left.body")
                    click("group.right.profile")
                    capture("profile-picker")
                    key("Down")
                    key("Return")
                    profile = wait_until(
                        lambda: (s if any(item["shellProfileId"] == "chrome-b" for item in
                                          s["screens"][screen]["sessions"].values()) else None)
                        if (s := state()) else None,
                        "profile picker did not launch Owned B")
                    profile_slots = drawer.slots(profile["screens"][screen]["root"])
                    assert len(profile_slots[2]["paneIds"]) == 2
                    assert profile["screens"][screen]["panes"][right_pane]["sessionId"] == right_session
                    assert_routes(profile)
                    capture("profile-created-tab")
                    result["checks"].append("keyboard-profile-picker-targets-unfocused-group")

                    selected_right = drawer.selected(profile_slots[2])
                    selected_session = profile["screens"][screen]["panes"][selected_right]["sessionId"]
                    selected_pid = next(pid for pid, shell in shells().items()
                                        if alive(pid) and shell == {"pane": selected_right, "session": selected_session})
                    divider = tuple(geometry["group.divider_region"])
                    assert len(divider) == 4 and all(isinstance(value, int) for value in divider)
                    before_max = display.pixels(window, divider)
                    click("group.right.more")
                    left_selected = drawer.selected(profile_slots[0])
                    if not args.setup_dock:
                        wheel("group.left.header", 4)
                        assert drawer.selected(drawer.slots(state()["screens"][screen]["root"])[0]) == left_selected, (
                            "wheel behind compact menu navigated another tab")
                    click("menu.maximize")
                    wait_until(lambda: display.pixels(window, divider) != before_max,
                               "compact maximize did not change rendered layout")
                    capture("maximized")
                    marker = owned / "after-maximize.marker"
                    type_text(f"printf '%s\\n' \"$$:$SPLITTY_PANE_ID:$SPLITTY_SESSION_ID\" > {shlex.quote(str(marker))}")
                    key("Return")
                    value = wait_until(lambda: marker.read_text().strip() if marker.exists() else None,
                                       "maximized menu did not return terminal focus")
                    assert value == f"{selected_pid}:{selected_right}:{selected_session}", value
                    click("group.right.more")
                    click("menu.maximize")
                    wait_until(lambda: display.pixels(window, divider) == before_max,
                               "compact restore did not restore visible groups")
                    restore_marker = owned / "after-restore.marker"
                    type_text(f"printf '%s\\n' \"$$:$SPLITTY_PANE_ID:$SPLITTY_SESSION_ID\" > {shlex.quote(str(restore_marker))}")
                    key("Return")
                    value = wait_until(lambda: restore_marker.read_text().strip() if restore_marker.exists() else None,
                                       "restore menu did not return terminal focus")
                    assert value == f"{selected_pid}:{selected_right}:{selected_session}", value
                    capture("restored")
                    result["checks"].append("compact-menu-wheel-block-maximize-restore-and-keyboard-focus")

                    click("group.right.more")
                    click("menu.float")
                    floated = wait_until(lambda: (s if len((s := state())["screens"][screen]["floating"]) == 1 else None),
                                         "compact float did not move selected tab")
                    assert selected_right in floated["screens"][screen]["panes"]
                    assert floated["screens"][screen]["panes"][selected_right]["sessionId"] == selected_session
                    assert_routes(floated)
                    result["states"]["floated"] = floated
                    capture("floated")
                    result["checks"].append("compact-menu-float-retains-session")
                    if not args.setup_float:
                        click("floating.dock")
                        docked = wait_until(lambda: (s if not (s := state())["screens"][screen]["floating"] else None),
                                            "floating group did not dock")
                        assert docked["screens"][screen]["panes"][selected_right]["sessionId"] == selected_session
                        assert_routes(docked)
                        result["states"]["docked"] = docked
                        capture("docked")
                        result["checks"].append("dock-retains-selected-session")
                        if not args.setup_dock:
                            dock_marker = owned / "after-dock.marker"
                            type_text(f"printf '%s\\n' \"$$:$SPLITTY_PANE_ID:$SPLITTY_SESSION_ID\" > {shlex.quote(str(dock_marker))}")
                            key("Return")
                            value = wait_until(lambda: dock_marker.read_text().strip() if dock_marker.exists() else None,
                                               "dock did not return terminal focus")
                            assert value == f"{selected_pid}:{selected_right}:{selected_session}", value
                            before_close = state()
                            click("group.left.selected_close")
                            closed = wait_until(lambda: (s if selected_right not in (s := state())["screens"][screen]["panes"] else None),
                                                "selected tab close did not remove its pane")
                            wait_until(lambda: not alive(selected_pid), "closed tab left its shell alive")
                            assert closed["screens"][screen]["panes"][right_pane]["sessionId"] == right_session
                            for pane, session in routes(before_close).items():
                                if pane != selected_right:
                                    assert routes(closed).get(pane) == session, (pane, session)
                            assert_routes(closed)
                            capture("tab-closed")
                            result["checks"].append("tab-close-only-ends-selected-session")
                            result["states"]["final"] = closed
                            if args.resize_check:
                                before_resize = closed
                                root = closed["screens"][screen]["root"]
                                assert root["type"] == "split" and root["axis"] == "horizontal", root
                                original_routes = routes(closed)
                                split_id = root["id"]
                                divider_x, divider_y, divider_width, divider_height = divider
                                start_x = divider_x + divider_width // 2
                                drag_y = divider_y + divider_height // 2
                                x("mousemove", "--window", window, start_x, drag_y, "mousedown", 1)
                                try:
                                    for step, offset in enumerate((40, 80, 60), start=1):
                                        previous_sizes = closed["screens"][screen]["root"]["sizes"]
                                        previous_revision = closed["screens"][screen]["revision"]
                                        x("mousemove", "--window", window, start_x + offset, drag_y)
                                        display.sync()
                                        closed = wait_until(
                                            lambda: (s if (s := state())["screens"][screen]["root"]["sizes"] != previous_sizes else None),
                                            f"divider step {step} did not change canonical sizes")
                                        current_screen = closed["screens"][screen]
                                        assert current_screen["revision"] > previous_revision
                                        assert current_screen["root"]["id"] == split_id
                                        assert routes(closed) == original_routes
                                        assert_routes(closed)
                                        result["states"][f"divider-step-{step}"] = closed
                                        # Let the published revision reach the retained gesture before
                                        # moving again while the same mouse press remains held.
                                        time.sleep(.25)
                                finally:
                                    x("mouseup", 1)
                                    display.sync()
                                assert routes(state()) == routes(before_resize)
                                capture("divider-resized")
                                result["checks"].append("continuous-divider-drag-survives-confirmed-revisions-and-retains-sessions")
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
                            app.wait(timeout=5)
            if display:
                display.close()
            if xvfb:
                os.killpg(xvfb.pid, signal.SIGTERM)
                xvfb.wait(timeout=5)
            result["survivingChildren"] = [pid for pid in children if alive(pid)]
            result["passed"] = (result["passed"] and not result["survivingChildren"]
                                and result.get("normalWindowClose", False)
                                and result.get("exitCode") == 0 and not result.get("closeError"))
            (args.output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps({"passed": result["passed"], "checks": result["checks"],
                              "error": result.get("error"), "result": str(args.output / "result.json")}, indent=2))
    if not result["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
