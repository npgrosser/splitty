#!/usr/bin/env python3
"""Check native terminal mouse reports through a real window and raw PTY.

The app, shell, Xvfb display and clipboard belong to this invocation. The
fixture records exact input bytes, including legacy mouse bytes above 0x7f.
"""

import argparse
import ctypes
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shlex
import signal
import stat
import subprocess
import time


HERE = Path(__file__).resolve().parent
MARKER = "POINTERMARK4826"
SGR = re.compile(rb"\x1b\[<(\d+);(\d+);(\d+)([Mm])")


def helper(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


performance = helper("performance", "gpui-performance.py")
pane_drag = helper("pane_drag", "gpui-pane-drag.py")


def wait_until(predicate, message, seconds=12):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.05)
    raise TimeoutError(message)


def reports(data):
    found = [(int(button), int(col), int(row), kind.decode())
             for button, col, row, kind in SGR.findall(data)]
    assert b"".join(match.group() for match in SGR.finditer(data)) == data, data.hex()
    return found


def wheel_pair(found, first, second):
    # XTest wheel clicks are translated into three GPUI lines on this X11
    # stack. The adapter bounds each physical event to five reports.
    buttons = [button & 127 for button, _, _, kind in found if kind == "M"]
    assert len(buttons) == len(found), found
    assert 1 <= buttons.count(first) <= 5 and 1 <= buttons.count(second) <= 5, found
    assert buttons == [first] * buttons.count(first) + [second] * buttons.count(second), found


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--display", default=":95")
    parser.add_argument("--xdotool", default="xdotool")
    parser.add_argument("--xvfb", default="Xvfb")
    parser.add_argument("--xvfb-lib", type=Path)
    parser.add_argument("--setup-only", action="store_true", help="validate fixture startup before mouse support lands")
    parser.add_argument("--focus-loss", action="store_true", help="also verify an OS focus loss settles a held application press")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    binary = args.binary.resolve(strict=True)
    fixture_script = HERE / "gpui-pointer-fixture.py"
    owned = args.output / "fixture"
    owned.mkdir(mode=0o700)
    config = args.output / "config"
    config.mkdir(mode=0o700)
    (config / "config.toml").write_text('[appearance]\ntheme_mode = "fixed"\ntheme = "splitty-dark"\nfont_size = 0\n')
    env = os.environ.copy()
    env.pop("WAYLAND_DISPLAY", None)
    env["DISPLAY"] = args.display
    env["SPLITTY_CONFIG_DIR"] = str(config.resolve())
    for key in ("SPLITTY_SOCK", "SPLITTY_AGENT_SOCK", "SPLITTY_ADDR", "SPLITTY_TOKEN",
                "SPLITTY_SERVER", "SPLITTY_PANE_ID", "SPLITTY_SCREEN_ID", "SPLITTY_SESSION_ID"):
        env.pop(key, None)
    xvfb_env = env.copy()
    if args.xvfb_lib:
        xvfb_env["LD_LIBRARY_PATH"] = str(args.xvfb_lib) + (":" + env["LD_LIBRARY_PATH"] if env.get("LD_LIBRARY_PATH") else "")
    result = {"binary": str(binary), "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "fixtureSha256": hashlib.sha256(fixture_script.read_bytes()).hexdigest(),
              "display": args.display, "checks": [], "events": {}, "screenshots": [], "passed": False}
    xvfb = app = display = window = None
    sequence = 0

    def x(*arguments):
        return subprocess.check_output([str(args.xdotool), *map(str, arguments)], env=env, text=True, timeout=12)

    def mode(*modes, stop=False):
        nonlocal sequence
        sequence += 1
        command = {"seq": sequence, "modes": modes, "stop": stop}
        temp = owned / "control.next"
        temp.write_text(json.dumps(command))
        temp.replace(owned / "control.json")
        if not stop:
            wait_until(lambda: (json.loads((owned / "ack.json").read_text()).get("seq") == sequence)
                       if (owned / "ack.json").exists() else False, f"fixture did not acknowledge {modes}")
            time.sleep(.1)

    def raw():
        return (owned / "received.bin").read_bytes()

    def observe(label, action, quiet=False):
        before = len(raw())
        action()
        time.sleep(.25)
        data = raw()[before:]
        result["events"][label] = data.hex()
        if quiet:
            assert not data, f"{label} reached the PTY: {data.hex()}"
        return data

    def move(point):
        x("mousemove", "--window", window, *point)

    def click(point):
        x("mousemove", "--window", window, *point, "click", 1)

    def key_chord(*keys):
        for modifier in keys[:-1]:
            display.key(modifier, True)
        display.type_key(keys[-1])
        for modifier in reversed(keys[:-1]):
            display.key(modifier, False)
        display.sync()

    def capture(name):
        path = args.output / f"{name}.png"
        pane_drag.save_png(path, display.pixels(window, (0, 0, 1200, 760)))
        result["screenshots"].append(str(path))

    def no_survivors(pids):
        survivors = []
        for pid in pids:
            try:
                if Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0] != "Z":
                    survivors.append(pid)
            except FileNotFoundError:
                pass
        return survivors

    with (args.output / "xvfb.log").open("w") as xvfb_log, (args.output / "app.log").open("w") as app_log:
        try:
            number = args.display.lstrip(":")
            if Path(f"/tmp/.X11-unix/X{number}").exists() or Path(f"/tmp/.X{number}-lock").exists():
                raise RuntimeError(f"DISPLAY {args.display} is already in use")
            xvfb = subprocess.Popen([str(args.xvfb), args.display, "-screen", "0", "1280x900x24",
                                     "-nolisten", "tcp", "-noreset", "-ac", "-xkbdir", "/usr/share/X11/xkb"],
                                    env=xvfb_env, stdout=xvfb_log, stderr=xvfb_log, start_new_session=True)

            def ready():
                if xvfb.poll() is not None:
                    raise RuntimeError(f"Xvfb exited early: {xvfb.returncode}")
                try:
                    return subprocess.run([str(args.xdotool), "getdisplaygeometry"], env=env,
                                          capture_output=True, timeout=2).returncode == 0
                except subprocess.TimeoutExpired:
                    return False

            wait_until(ready, "Xvfb did not become ready", seconds=20)
            previous_display = os.environ.get("DISPLAY")
            os.environ["DISPLAY"] = args.display  # Display() calls XOpenDisplay(None).
            try:
                display = performance.Display()
            finally:
                if previous_display is None:
                    os.environ.pop("DISPLAY", None)
                else:
                    os.environ["DISPLAY"] = previous_display
            app = subprocess.Popen([str(binary)], env=env, stdout=app_log, stderr=app_log, start_new_session=True)

            def find_window():
                if app.poll() is not None:
                    raise RuntimeError(f"app exited before opening a window: {app.returncode}")
                found = subprocess.run([str(args.xdotool), "search", "--onlyvisible", "--pid", str(app.pid)],
                                       env=env, capture_output=True, text=True, timeout=3)
                for candidate in found.stdout.splitlines():
                    geometry = dict(line.split("=", 1) for line in x("getwindowgeometry", "--shell", candidate).splitlines() if "=" in line)
                    if geometry.get("WIDTH") == "1200" and geometry.get("HEIGHT") == "760":
                        return int(candidate)

            window = wait_until(find_window, "No 1200x760 native window appeared", seconds=30)
            time.sleep(2)
            x("windowfocus", window)
            click((180, 180))
            launch = f"python3 -u {shlex.quote(str(fixture_script))} {shlex.quote(str(owned.resolve()))}"
            x("type", "--clearmodifiers", "--delay", 2, launch)
            display.type_key("Return")
            wait_until(lambda: (owned / "ready").exists(), "raw PTY fixture did not start")
            wait_until(lambda: (owned / "received.bin").exists(), "raw PTY log did not appear")
            mode()
            capture("fixture-ready")
            result["checks"].append("fixture-started-in-owned-pty")
            if not args.setup_only:
                # Press/release are one-based, relative to terminal content.
                mode(1000, 1006)
                normal = reports(observe("sgr-press-release", lambda: click((300, 250))))
                assert len(normal) == 2 and normal[0][0] == 0 and normal[0][3] == "M" and normal[1][3] == "m", normal
                assert normal[0][1:3] == normal[1][1:3], normal
                result["checks"].append("sgr-press-release")

                for number, code, label in ((2, 1, "middle"), (3, 2, "right"),
                                             (8, 128, "navigate-back"), (9, 129, "navigate-forward")):
                    def press_button():
                        move((300, 250))
                        x("mousedown", number)
                        x("mouseup", number)
                    button_reports = reports(observe(f"sgr-{label}", press_button))
                    assert len(button_reports) == 2 and button_reports[0][0] == code, button_reports
                    assert button_reports[0][3] == "M" and button_reports[1][3] == "m", button_reports
                result["checks"].append("middle-right-navigation-buttons")

                mode(1002, 1006)
                observe("button-motion-before-press", lambda: move((310, 250)), quiet=True)
                def drag_motion():
                    x("mousedown", 1)
                    move((360, 260))
                    x("mouseup", 1)
                motion = reports(observe("button-motion-and-release", drag_motion))
                assert motion[0][0] == 0 and motion[0][3] == "M" and motion[-1][3] == "m", motion
                assert any(button & 32 for button, _, _, kind in motion[1:-1] if kind == "M"), motion
                result["checks"].append("button-motion-only-while-pressed")

                mode(1003, 1006)
                all_motion = reports(observe("all-motion", lambda: move((440, 270))))
                assert any(button & 32 for button, _, _, kind in all_motion if kind == "M"), all_motion
                result["checks"].append("unpressed-all-motion")

                mode(1002, 1006)
                move((340, 240))
                def wheel():
                    display.wheel(4)
                    display.wheel(5)
                wheels = reports(observe("tracked-wheel", wheel))
                wheel_pair(wheels, 64, 65)
                result["checks"].append("tracked-wheel-is-mouse-not-arrows")

                horizontal = reports(observe("horizontal-wheel", lambda: (display.wheel(6), display.wheel(7))))
                wheel_pair(horizontal, 66, 67)
                result["checks"].append("horizontal-wheel-reports")

                mode(1000, 1006)
                high_sgr = reports(observe("sgr-high-coordinate", lambda: click((1080, 250))))
                assert len(high_sgr) == 2, high_sgr
                col, row = high_sgr[0][1:3]
                mode(1000)
                legacy = observe("legacy-high-coordinate", lambda: click((1080, 250)))
                assert len(legacy) == 12 and legacy[:3] == b"\x1b[M" and legacy[6:9] == b"\x1b[M", legacy.hex()
                assert legacy[3:6] == bytes((32, col + 32, row + 32)), legacy.hex()
                assert legacy[4] > 127, f"legacy coordinate did not exercise non-UTF-8 byte: {legacy.hex()}"
                result["checks"].append("legacy-high-byte-preserved")

                mode(9)
                press_only = observe("press-only", lambda: click((340, 240)))
                assert press_only.startswith(b"\x1b[M") and len(press_only) == 6, press_only.hex()
                observe("press-only-wheel-fallback", lambda: display.wheel(4), quiet=True)
                result["checks"].append("press-only-ignores-wheel")

                mode(1002, 1006)
                move((340, 240))
                def release_outside():
                    x("mousedown", 1)
                    move((340, 20))  # Header lies outside the terminal element.
                    x("mouseup", 1)
                outside = reports(observe("release-outside-terminal", release_outside))
                assert outside[0][3] == "M" and outside[-1][3] == "m", outside
                result["checks"].append("outside-release-settles-original-gesture")

                move((340, 240))
                def shift_mid_application():
                    x("mousedown", 1)
                    display.key("Shift_L", True)
                    display.sync()
                    move((360, 250))
                    x("mouseup", 1)
                    display.key("Shift_L", False)
                    display.sync()
                shifted_release = reports(observe("shift-mid-application-drag", shift_mid_application))
                assert shifted_release[0][3] == "M" and shifted_release[-1][3] == "m", shifted_release
                result["checks"].append("shift-mid-application-still-releases")

                def shift_double_click():
                    display.key("Shift_L", True)
                    display.sync()  # xdotool uses a separate X connection.
                    x("mousemove", "--window", window, 100, 132, "click", "--repeat", 2, "--delay", 80, 1)
                    display.key("Shift_L", False)
                    display.sync()
                observe("shift-local-word-select", shift_double_click, quiet=True)
                capture("shift-selected-word")
                observe("copy-selection", lambda: key_chord("Control_L", "Shift_L", "c"), quiet=True)
                pasted = observe("paste-selection", lambda: key_chord("Control_L", "Shift_L", "v"))
                assert pasted == MARKER.encode(), f"Shift selection did not copy exact marker: {pasted!r}"
                result["checks"].append("shift-selection-private-copy-paste")

                def shift_release_mid_selection():
                    display.key("Shift_L", True)
                    display.sync()
                    move((10, 132))
                    x("mousedown", 1)
                    display.key("Shift_L", False)
                    display.sync()
                    move((125, 132))
                    x("mouseup", 1)
                    display.sync()
                observe("shift-released-mid-selection", shift_release_mid_selection, quiet=True)
                capture("shift-selection-latched")
                result["checks"].append("shift-started-selection-remains-local")

                if args.focus_loss:
                    mode(1002, 1006)
                    move((340, 240))
                    display.x.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
                    display.x.XDefaultRootWindow.restype = ctypes.c_ulong
                    root = display.x.XDefaultRootWindow(display.connection)
                    before = len(raw())
                    x("mousedown", 1)
                    wait_until(lambda: bool(SGR.search(raw()[before:])), "focus-loss press did not reach PTY")
                    x("windowfocus", root)
                    wait_until(lambda: int(x("getwindowfocus").strip()) == root,
                               "root did not receive X focus")
                    released = wait_until(lambda: raw()[before:] if len(SGR.findall(raw()[before:])) >= 2 else None,
                                          "focus loss did not settle held press")
                    release_reports = reports(released)
                    assert len(release_reports) == 2 and release_reports[0][3] == "M" and release_reports[1][3] == "m", release_reports
                    result["events"]["focus-loss-before-physical-up"] = released.hex()
                    x("mouseup", 1)
                    time.sleep(.25)
                    assert raw()[before:] == released, "physical release duplicated the settled gesture"
                    x("windowfocus", window)
                    resumed = reports(observe("after-focus-return", lambda: click((340, 240))))
                    assert len(resumed) == 2 and resumed[0][3] == "M" and resumed[1][3] == "m", resumed
                    result["checks"].append("focus-loss-settles-on-original-terminal")

                mode()
                observe("tracking-off-click", lambda: click((300, 250)), quiet=True)
                observe("tracking-off-wheel", lambda: display.wheel(4), quiet=True)
                result["checks"].append("tracking-off-restores-local-input")
            mode(stop=True)
            fixture_pid = int((owned / "ready").read_text())
            wait_until(lambda: not Path(f"/proc/{fixture_pid}/stat").exists()
                       or Path(f"/proc/{fixture_pid}/stat").read_text().rsplit(")", 1)[1].split()[0] == "Z",
                       "raw fixture did not exit normally", seconds=8)
            result["passed"] = True
        except Exception as error:
            result["error"] = repr(error)
            if display and window:
                try:
                    capture("failure")
                except Exception as capture_error:
                    result["captureError"] = repr(capture_error)
        finally:
            descendants = set(performance.process_stats(app.pid)) - {app.pid} if app else set()
            if app and app.poll() is None and display and window:
                try:
                    display.close_window(window)
                    app.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    # A running fixture can prompt for confirmation on close.
                    x("mousemove", "--window", window, 600, 422, "click", 1)
                    try:
                        app.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        os.killpg(app.pid, signal.SIGTERM)
                except Exception as error:
                    result["closeError"] = repr(error)
            if app:
                try:
                    app.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(app.pid, signal.SIGKILL)
                    app.wait(timeout=3)
                result["exitCode"] = app.returncode
                result["normalWindowClose"] = app.returncode == 0 and "closeError" not in result
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
                    try:
                        os.killpg(xvfb.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
            result["survivingChildren"] = no_survivors(descendants)
            result["passed"] = result["passed"] and result.get("exitCode") == 0 and not result["survivingChildren"]
            (args.output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result, indent=2))
    if not result["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
