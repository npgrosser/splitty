#!/usr/bin/env python3
"""Real-window acceptance for native tab and Screen Drawer fidelity.

Run --setup-only first, then supply screenshot-calibrated point/region geometry
as JSON. All mutations use the owned X11 window. The mux API is read only.
Visual assertions are saved PNGs for review across themes, not exact colors.
"""

import argparse
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shlex
import signal
import stat
import subprocess
import struct
import termios
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


def routes(value):
    return {pane: item.get("sessionId") for screen in value["screens"].values()
            for pane, item in screen["panes"].items()}


def terminal_size(pid):
    fd = os.open(f"/proc/{pid}/fd/0", os.O_RDONLY | os.O_NONBLOCK)
    try:
        rows, cols, _, _ = struct.unpack("HHHH", fcntl.ioctl(fd, termios.TIOCGWINSZ, bytes(8)))
        return rows, cols
    finally:
        os.close(fd)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--display", default=":96")
    parser.add_argument("--xvfb", default="Xvfb")
    parser.add_argument("--xvfb-lib", type=Path)
    parser.add_argument("--xdotool", default="xdotool")
    parser.add_argument("--geometry", type=Path)
    parser.add_argument("--setup-only", action="store_true")
    parser.add_argument("--long-list-check", action="store_true",
                        help="separate bounded Drawer overflow and active-row reveal check")
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
        parser.error("use a short --output path for the owned Unix socket")
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
        '[shells]\ndefault_profile_id = "fidelity-shell"\n\n'
        '[[shells.profiles]]\nid = "fidelity-shell"\nname = "Owned shell"\n'
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
        xvfb_env["LD_LIBRARY_PATH"] = str(args.xvfb_lib.resolve()) + (
            ":" + env["LD_LIBRARY_PATH"] if env.get("LD_LIBRARY_PATH") else "")
    result = {"binarySha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "harnessSha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "geometrySha256": hashlib.sha256(args.geometry.read_bytes()).hexdigest() if args.geometry else None,
              "checks": [], "screenshots": [], "states": {}, "passed": False,
              "visualReview": ["tabs-and-adjacent-plus-profile", "tab-context-no-tab-ellipsis",
                               "tab-shaped-drag-badge", "whole-pane-center-highlight",
                               "unpinned-overlay-no-top-toolbar"]}
    xvfb = display = app = window = None
    children = set()

    def x(*arguments):
        return subprocess.check_output([str(args.xdotool), *map(str, arguments)],
                                       env=env, text=True, timeout=12)

    def point(name):
        value = geometry[name]
        assert len(value) == 2 and all(isinstance(n, int) for n in value), (name, value)
        return value

    def region(name):
        value = geometry[name]
        assert len(value) == 4 and all(isinstance(n, int) for n in value), (name, value)
        return tuple(value)

    def changed_fraction(before, after):
        assert len(before) == len(after)
        changed = sum(sum(abs(before[i + c] - after[i + c]) for c in range(3)) > 25
                      for i in range(0, len(before), 4))
        return changed / (len(before) // 4)

    def move(name):
        x("mousemove", "--window", window, *point(name))
        display.sync()
        time.sleep(.2)

    def click(name, button=1):
        move(name)
        x("click", button)
        display.sync()
        time.sleep(.2)

    def key(name):
        display.type_key(name)
        display.sync()
        time.sleep(.1)

    def chord(modifier, name):
        display.key(modifier, True)
        try:
            key(name)
        finally:
            display.key(modifier, False)
            display.sync()

    def create_screen_shortcut():
        display.key("Control_L", True)
        display.key("Shift_L", True)
        try:
            key("space")
        finally:
            display.key("Shift_L", False)
            display.key("Control_L", False)
            display.sync()

    def type_text(value):
        x("type", "--clearmodifiers", "--delay", 3, value)
        display.sync()

    def capture(name):
        path = args.output / f"{name}.png"
        pane_drag.save_png(path, display.pixels(window, (0, 0, 1200, 760)))
        result["screenshots"].append(str(path))

    def settle_tabs():
        strip = region("tab.strip_region")
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            before = display.pixels(window, strip)
            time.sleep(.55)
            display.sync()
            if display.pixels(window, strip) == before:
                return
        raise TimeoutError("tab titles/controls did not settle for 550 ms")

    def state(name=None):
        def socket():
            candidates = list((config / "run").glob("spike-*.sock"))
            if len(candidates) > 1:
                raise RuntimeError("more than one owned service socket")
            return candidates[0] if candidates and stat.S_ISSOCK(candidates[0].lstat().st_mode) else None
        owned_socket = wait_until(socket, "owned service socket did not bind", seconds=6)
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

    def assert_routes(value):
        current = routes(value)
        for pid, shell in shells().items():
            if alive(pid):
                assert current.get(shell["pane"]) == shell["session"], (pid, shell, current)

    def drag_to(start, target, image, feedback_region=None):
        move(start)
        x("mousedown", 1)
        try:
            sx, sy = point(start)
            x("mousemove", "--window", window, sx + 16, sy + 22)
            display.sync()
            time.sleep(.2)
            move(target)
            capture(image)
            feedback = display.pixels(window, region(feedback_region)) if feedback_region else None
        finally:
            x("mouseup", 1)
            display.sync()
            time.sleep(.4)
        return feedback

    def close_app():
        nonlocal app, window
        if app and app.poll() is None and display and window:
            display.close_window(window)
            time.sleep(.4)
            if app.poll() is None:
                exists = subprocess.run([str(args.xdotool), "getwindowgeometry", "--shell", str(window)],
                                        env=env, capture_output=True, timeout=3).returncode == 0
                if exists:
                    if "window.confirm_close" in geometry:
                        click("window.confirm_close")
                    elif args.setup_only:
                        # Calibrated from the fixed 1200x760 setup capture.
                        x("mousemove", "--window", window, 600, 422)
                        x("click", 1)
                        display.sync()
            app.wait(timeout=12)
            assert app.returncode == 0, f"window exit {app.returncode}"
            result["normalWindowClose"] = True
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
            screen_id = initial["activeScreenId"]
            assert_routes(initial)
            capture("setup")
            result["checks"].append("owned-window-service-and-shell")
            if args.long_list_check:
                for count in range(2, 16):
                    create_screen_shortcut()
                    wait_until(lambda: len(state()["screenOrder"]) == count,
                               f"Screen shortcut did not create Screen {count}")
                long_list = state("long-list")
                last_id = long_list["screenOrder"][-1]
                result["muxActiveAfterShortcutCreation"] = long_list["activeScreenId"]
                capture("long-list-last-screen-auto-revealed")
                if not args.setup_only:
                    click("drawer.penultimate_visible_row")
                    wait_until(lambda: state()["activeScreenId"] == long_list["screenOrder"][-2],
                               "penultimate visible Screen row did not select")
                    click("drawer.last_visible_row")
                    wait_until(lambda: state()["activeScreenId"] == last_id,
                               "last Screen was not visible/selectable after auto-reveal")
                    result["checks"].append("active-last-screen-auto-revealed-and-clickable")
                    move("drawer.long_list_shift_point")
                    before_scroll = display.pixels(window, region("drawer.long_list_region"))
                    display.key("Shift_L", True)
                    try:
                        display.wheel(4)
                        display.sync()
                    finally:
                        display.key("Shift_L", False)
                        display.sync()
                    wait_until(lambda: display.pixels(window, region("drawer.long_list_region")) != before_scroll,
                               "Shift-wheel did not scroll overflowing Drawer list")
                    assert state()["activeScreenId"] == last_id, "Shift-wheel selected a Screen"
                    capture("long-list-shift-wheel-overflow")
                    result["checks"].append("modified-wheel-scrolls-overflow-without-screen-navigation")
                children |= set(performance.process_stats(app.pid)) - {app.pid}
                close_app()
            elif args.setup_only:
                initial_pid = next(pid for pid in shells() if alive(pid))
                click("terminal.initial") if "terminal.initial" in geometry else None
                type_text("exit")
                key("Return")
                wait_until(lambda: not alive(initial_pid), "setup shell did not exit normally", seconds=6)
                close_app()
            else:
                click("drawer.create")
                two_screens = wait_until(lambda: (s if len((s := state())["screenOrder"]) == 2 else None),
                                         "Drawer create did not add second Screen")
                second_id = next(id for id in two_screens["screenOrder"] if id != screen_id)
                capture("two-screens-create-position")
                click("drawer.create_second")
                three_screens = wait_until(lambda: (s if len((s := state())["screenOrder"]) == 3 else None),
                                           "Drawer create did not add third Screen")
                assert three_screens["screenOrder"].index(second_id) == 1
                click("drawer.first_screen")
                wait_until(lambda: state()["activeScreenId"] == screen_id, "initial Screen did not reopen")
                for count in range(2, 5):
                    chord("Alt_L", "t")
                    wait_until(lambda: len(state()["screens"][screen_id]["panes"]) == count,
                               f"tab {count} did not launch")
                settle_tabs()
                capture("tabs-controls-adjacent")
                click("tab.plus")
                wait_until(lambda: len(state()["screens"][screen_id]["panes"]) == 5,
                           "adjacent plus did not create tab")
                settle_tabs()
                click("tab.profile")
                capture("tab-profile-dropdown")
                key("Escape")
                result["checks"].append("adjacent-new-tab-and-profile-input")

                selected_before = drawer.selected(drawer.slots(state()["screens"][screen_id]["root"])[0])
                move("tab.inactive")
                context_before = display.pixels(window, region("tab.context_region"))
                click("tab.inactive", 3)
                capture("tab-right-context-menu")
                assert display.pixels(window, region("tab.context_region")) != context_before, (
                    "right click did not open a visible context menu")
                key("Escape")
                assert drawer.selected(drawer.slots(state()["screens"][screen_id]["root"])[0]) == selected_before
                result["checks"].append("inactive-tab-right-context-preserves-selection")

                click("tab.busy_target")
                busy_pane = drawer.selected(drawer.slots(state()["screens"][screen_id]["root"])[0])
                type_text("sleep 60")
                key("Return")
                wait_until(lambda: (s := state())["screens"][screen_id]["sessions"][
                    s["screens"][screen_id]["panes"][busy_pane]["sessionId"]]["busy"],
                    "target tab did not become busy")
                settle_tabs()
                click("tab.selected_other")
                settle_tabs()
                before_middle = state("before-middle-close")
                assert drawer.selected(drawer.slots(before_middle["screens"][screen_id]["root"])[0]) != busy_pane, (
                    "neighbor click did not leave busy tab inactive")
                unrelated = {pane: session for pane, session in routes(before_middle).items() if pane != busy_pane}
                prompt_before = display.pixels(window, region("prompt.region"))
                click("tab.busy_target", 2)
                capture("middle-close-assessment")
                assert changed_fraction(prompt_before, display.pixels(window, region("prompt.region"))) > .3, (
                    "middle click did not visibly open close assessment")
                assert busy_pane in state()["screens"][screen_id]["panes"], "middle click skipped busy assessment"
                click("prompt.keep")
                assert busy_pane in state()["screens"][screen_id]["panes"], "cancel removed target tab"
                settle_tabs()
                prompt_before = display.pixels(window, region("prompt.region"))
                click("tab.busy_target", 2)
                assert changed_fraction(prompt_before, display.pixels(window, region("prompt.region"))) > .3, (
                    "second middle click did not visibly open close assessment")
                click("prompt.confirm")
                closed = wait_until(lambda: (s if busy_pane not in (s := state())["screens"][screen_id]["panes"] else None),
                                    "confirmed middle close did not remove clicked tab")
                assert all(routes(closed).get(pane) == session for pane, session in unrelated.items())
                assert_routes(closed)
                result["checks"].append("middle-close-clicked-inactive-busy-tab-with-assessment")

                click("tab.drag_source")
                type_text("printf '\\033]0;FIDELITY-DRAG-TAB\\007'")
                key("Return")
                time.sleep(.25)
                capture("named-tab-before-drag")
                before_drag = state("before-center-drop")
                before_revision = before_drag["screens"][screen_id]["revision"]
                before_routes = routes(before_drag)
                before_region = display.pixels(window, region("group.center_region"))
                during_region = drag_to("tab.drag_source", "group.center", "center-drag-highlight-and-tab-badge",
                                        "group.center_region")
                assert during_region != before_region, (
                    "center target did not visibly highlight; check calibrated region")
                after_center = state("after-center-drop")
                assert after_center["screens"][screen_id]["revision"] == before_revision, (
                    "same-group center drop changed canonical layout revision")
                assert routes(after_center) == before_routes
                result["checks"].append("same-group-center-highlight-without-layout-mutation")

                drag_to("tab.drag_source", "group.edge_right", "edge-drag-highlight")
                split = wait_until(lambda: (s if len(drawer.slots((s := state())["screens"][screen_id]["root"])) == 2 else None),
                                   "right-edge drop did not split group")
                assert split["screens"][screen_id]["revision"] > before_revision
                assert routes(split) == before_routes
                assert_routes(split)
                capture("edge-split-result")
                result["checks"].append("right-edge-drop-splits-with-session-identity")

                left, right = drawer.slots(split["screens"][screen_id]["root"])
                click("drawer.pane_left")
                selected_left = wait_until(lambda: (s if drawer.selected(drawer.slots((s := state())["screens"][screen_id]["root"])[0])
                                            == left["paneIds"][0] else None), "left Drawer pane entry did not select pane")
                click("drawer.pane_right")
                selected_right = wait_until(lambda: (s if drawer.selected(drawer.slots((s := state())["screens"][screen_id]["root"])[1])
                                             == right["paneIds"][0] else None), "right Drawer pane entry did not select pane")
                assert routes(selected_left) == routes(selected_right) == before_routes
                right_marker = owned / "drawer-right-pane.marker"
                type_text(f"printf '%s' \"$SPLITTY_PANE_ID\" > {shlex.quote(str(right_marker))}")
                key("Return")
                observed_pane = wait_until(lambda: right_marker.read_text() if right_marker.exists() else None,
                                           "Drawer pane entry did not route terminal input")
                assert observed_pane == right["paneIds"][0], (observed_pane, right["paneIds"])
                capture("drawer-pane-entries")
                result["checks"].append("drawer-pane-entry-selects-canonical-pane")

                wheel_input = owned / "drawer-wheel-input.hex"
                wheel_ready = owned / "drawer-wheel-reader.ready"
                wheel_stop = owned / "drawer-wheel-reader.stop"
                reader = (
                    "import os,pathlib,select,sys,termios,time,tty\n"
                    "fd=sys.stdin.fileno(); out=sys.stdout.fileno(); old=termios.tcgetattr(fd)\n"
                    f"ready=pathlib.Path({str(wheel_ready)!r}); stop=pathlib.Path({str(wheel_stop)!r})\n"
                    "data=bytearray(); end=time.monotonic()+15\n"
                    "try:\n"
                    " tty.setcbreak(fd); os.write(out,b'\\x1b[?1000h\\x1b[?1006h'); ready.touch()\n"
                    " while not stop.exists() and time.monotonic()<end:\n"
                    "  readable,_,_=select.select([fd],[],[],.1)\n"
                    "  if readable: data.extend(os.read(fd,64))\n"
                    "finally:\n"
                    " os.write(out,b'\\x1b[?1006l\\x1b[?1000l')\n"
                    " termios.tcsetattr(fd,termios.TCSADRAIN,old)\n"
                    f"pathlib.Path({str(wheel_input)!r}).write_text(data.hex())\n"
                )
                # Keep the wheel-routing fixture independent of synthetic typing
                # throughput: launch a short command through the real terminal.
                reader_script = owned / "drawer-wheel-reader.py"
                reader_script.write_text(reader)
                click("terminal.right_body")
                type_text("python3 " + shlex.quote(str(reader_script)))
                key("Return")
                wait_until(lambda: wheel_ready.exists(), "terminal mouse-report reader did not start", seconds=5)
                for area in ("drawer.header", "drawer.list", "drawer.footer"):
                    click("drawer.first_screen")
                    wait_until(lambda: state()["activeScreenId"] == screen_id,
                               "screen did not reset before wheel")
                    move(area)
                    x("click", 5)
                    display.sync()
                    changed = wait_until(lambda: (s if (s := state())["activeScreenId"] == second_id else None),
                                         f"wheel over {area} did not select next Screen")
                    time.sleep(.35)
                    assert state()["activeScreenId"] == second_id, f"wheel over {area} skipped the immediate neighbor"
                    assert routes(changed) == before_routes
                click("drawer.first_screen")
                wait_until(lambda: state()["activeScreenId"] == screen_id,
                           "screen did not reset before Shift-wheel")
                before_overflow = state()["activeScreenId"]
                move("drawer.list")
                display.key("Shift_L", True)
                try:
                    display.wheel(5)
                    display.sync()
                finally:
                    display.key("Shift_L", False)
                    display.sync()
                assert state()["activeScreenId"] == before_overflow, "Shift-wheel selected a Screen"
                wheel_stop.touch()
                wait_until(lambda: wheel_input.exists(), "wheel-input reader did not finish", seconds=5)
                assert wheel_input.read_text() == "", "Drawer wheel sent bytes to the terminal PTY"
                capture("drawer-shift-wheel-overflow-path")
                capture("drawer-wheel-anywhere")
                result["checks"].append("drawer-wheel-anywhere-one-neighbor-per-event-and-shift-overflow-path")

                focused = drawer.selected(drawer.slots(state()["screens"][screen_id]["root"])[1])
                focused_pid = next(pid for pid, shell in shells().items()
                                   if alive(pid) and shell["pane"] == focused)
                click("drawer.pin")
                wait_until(lambda: tomllib.loads((config / "config.toml").read_text())["drawer"]["pinned"] is False,
                           "unpin did not persist")
                saved = (config / "config.toml").read_bytes()
                time.sleep(.3)
                open_overlay = display.pixels(window, region("drawer.overlay_region"))
                move("workspace.leave_drawer")
                hidden = wait_until(lambda: (p if changed_fraction(
                    open_overlay, p := display.pixels(window, region("drawer.overlay_region"))) > .7 else None),
                    "unpinned Drawer did not retract on leave")
                capture("unpinned-hidden-no-toolbar")
                hidden_size = terminal_size(focused_pid)
                move("drawer.left_edge")
                time.sleep(.25)
                edge_x, edge_y = point("drawer.left_edge")
                x("mousemove", "--window", window, edge_x + 1, edge_y)
                display.sync()
                time.sleep(.45)
                shown = wait_until(lambda: (p if changed_fraction(
                    hidden, p := display.pixels(window, region("drawer.overlay_region"))) > .7 else None),
                                   "left-edge dwell did not reveal Drawer")
                capture("unpinned-edge-overlay")
                assert (config / "config.toml").read_bytes() == saved, "edge reveal wrote pin settings"
                assert terminal_size(focused_pid) == hidden_size, "edge overlay resized terminal"
                move("workspace.leave_drawer")
                wait_until(lambda: changed_fraction(
                    shown, display.pixels(window, region("drawer.overlay_region"))) > .7,
                           "leaving overlay did not retract Drawer")
                assert (config / "config.toml").read_bytes() == saved, "overlay leave wrote pin settings"
                assert terminal_size(focused_pid) == hidden_size, "overlay leave resized terminal"
                capture("unpinned-retracted")
                result["checks"].append("unpinned-edge-dwell-overlay-retracts-without-persistence-write")
                assert_routes(state("final"))
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
            for pid in children:
                if alive(pid):
                    try:
                        os.kill(pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
            if children:
                time.sleep(.2)
            for pid in children:
                if alive(pid):
                    try:
                        os.kill(pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
            if children:
                time.sleep(.1)
            result["survivingChildren"] = [pid for pid in children if alive(pid)]
            result["passed"] = result["passed"] and not result["survivingChildren"] and not result.get("closeError")
            (args.output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps({"passed": result["passed"], "checks": result["checks"],
                              "error": result.get("error"), "result": str(args.output / "result.json")}, indent=2))
    if not result["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
