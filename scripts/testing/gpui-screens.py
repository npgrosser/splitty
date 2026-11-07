#!/usr/bin/env python3
"""Exercise local Screen controls with real mouse, keyboard, and shell sessions.

The script owns an isolated Xvfb display and a native GPUI window. It records
terminal screenshots as visual evidence, while shell PID/state files provide
machine-checked session continuity. Run with the native library and font
environment prepared for splitty-gpui; no internal smoke mode is used.
"""

import argparse
import ctypes
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time


def load_helper(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


performance = load_helper("performance", "gpui-performance.py")
pane_drag = load_helper("pane_drag", "gpui-pane-drag.py")


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
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--display", default=":95")
    parser.add_argument("--xdotool", default="xdotool")
    parser.add_argument("--xvfb", default="Xvfb")
    parser.add_argument("--xvfb-lib", type=Path)
    parser.add_argument("--busy-close", action="store_true", help="also exercise the Linux busy-process close prompt")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    binary = args.binary.resolve(strict=True)
    env = os.environ.copy()
    env.pop("WAYLAND_DISPLAY", None)
    env["DISPLAY"] = args.display
    xvfb_env = env.copy()
    if args.xvfb_lib:
        xvfb_env["LD_LIBRARY_PATH"] = str(args.xvfb_lib) + (":" + env["LD_LIBRARY_PATH"] if env.get("LD_LIBRARY_PATH") else "")
    result = {"binary": str(binary), "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "display": args.display, "checks": [], "screenshots": []}
    xvfb = None
    app = None
    display = None
    window = None

    def x(*arguments, timeout=15):
        return subprocess.check_output([str(args.xdotool), *map(str, arguments)], env=env, text=True, timeout=timeout)

    def click(point, pause=.4):
        x("mousemove", "--window", window, *point, "click", 1)
        time.sleep(pause)

    def capture(label):
        path = args.output / f"{label}.png"
        pane_drag.save_png(path, window_pixels((0, 0, 1200, 760)))
        result["screenshots"].append(str(path))

    def window_pixels(rectangle):
        # The isolated display's root includes the visible GPUI window pixels.
        geometry = dict(line.split("=", 1) for line in x("getwindowgeometry", "--shell", window).splitlines() if "=" in line)
        display.x.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
        display.x.XDefaultRootWindow.restype = ctypes.c_ulong
        root = display.x.XDefaultRootWindow(display.connection)
        result["rootWindow"] = root
        result["windowGeometry"] = geometry
        left, top, width, height = rectangle
        return display.pixels(root, (int(geometry["X"]) + left, int(geometry["Y"]) + top, width, height))

    def send(command):
        x("type", "--clearmodifiers", "--delay", 2, command)
        display.type_key("Return")

    def shell_value(label, expression):
        path = scratch / label
        path.unlink(missing_ok=True)
        click((300, 180), .1)
        display.key("Control_L", True)
        display.type_key("c")
        display.key("Control_L", False)
        display.sync()
        time.sleep(.1)
        send(f"printf '%s\\n' {expression} > '{path}'")
        return wait_until(lambda: path.read_text().strip() if path.exists() else None,
                          f"Shell did not write {label}", seconds=8)

    def shell_pid(label):
        return int(shell_value(label, '"$$"'))

    with tempfile.TemporaryDirectory(prefix="splitty-screens-") as directory, \
            (args.output / "xvfb.log").open("w") as xvfb_log, \
            (args.output / "app.log").open("w") as app_log:
        scratch = Path(directory)
        try:
            display_number = args.display.lstrip(":")
            if (Path(f"/tmp/.X11-unix/X{display_number}").exists() or
                    Path(f"/tmp/.X{display_number}-lock").exists()):
                raise RuntimeError(f"DISPLAY {args.display} is already in use")
            xvfb = subprocess.Popen([str(args.xvfb), args.display, "-screen", "0", "1280x900x24",
                                     "-nolisten", "tcp", "-noreset", "-ac", "-xkbdir", "/usr/share/X11/xkb"],
                                    env=xvfb_env, stdout=xvfb_log, stderr=xvfb_log, start_new_session=True)
            def xvfb_ready():
                if xvfb.poll() is not None:
                    raise RuntimeError(f"Xvfb exited early ({xvfb.returncode})")
                try:
                    return subprocess.run([str(args.xdotool), "getdisplaygeometry"], env=env,
                                          capture_output=True, timeout=2).returncode == 0
                except subprocess.TimeoutExpired:
                    return False

            wait_until(xvfb_ready, "Xvfb did not become ready", seconds=20)
            # Display() calls XOpenDisplay(None), which reads this process's
            # DISPLAY rather than the subprocess environment passed above.
            previous_display = os.environ.get("DISPLAY")
            os.environ["DISPLAY"] = args.display
            try:
                display = performance.Display()
            finally:
                if previous_display is None:
                    os.environ.pop("DISPLAY", None)
                else:
                    os.environ["DISPLAY"] = previous_display
            app = subprocess.Popen([str(binary)], env=env, stdout=app_log, stderr=app_log,
                                   start_new_session=True)

            def find_window():
                if app.poll() is not None:
                    raise RuntimeError(f"Application exited before opening a window ({app.returncode})")
                found = subprocess.run([str(args.xdotool), "search", "--onlyvisible", "--pid", str(app.pid)],
                                       env=env, capture_output=True, text=True)
                for candidate in found.stdout.splitlines():
                    geometry = dict(line.split("=", 1) for line in x("getwindowgeometry", "--shell", candidate).splitlines() if "=" in line)
                    if geometry.get("WIDTH") == "1200" and geometry.get("HEIGHT") == "760":
                        return int(candidate)
                return None

            window = wait_until(find_window, "No 1200x760 native workspace appeared", seconds=30)
            result["windowId"] = window
            time.sleep(3)
            x("windowfocus", window)
            capture("initial")
            a_pid = shell_pid("a-initial")
            send("export SPLITTY_SCREEN_TOKEN=alpha")
            time.sleep(.3)
            capture("a-history")
            # The background writer runs after A is hidden. The marker file
            # confirms execution; the later screenshot records terminal output.
            hidden = scratch / "hidden-a"
            send(f"(sleep 3; printf 'HIDDEN_A_OUTPUT\\n' | tee '{hidden}') &")
            click((210, 20), 1)  # + Screen after the first Screen chip.
            capture("two-screens")
            b_pid = shell_pid("b-initial")
            assert b_pid != a_pid, "+ Screen must launch a distinct shell"
            result["checks"].append("add-screen-distinct-shell")
            wait_until(lambda: hidden.exists(), "Hidden A output did not run", seconds=8)

            before_rename = window_pixels((85, 0, 520, 42))
            click((250, 20))  # Rename icon on the second Screen chip.
            x("type", "--clearmodifiers", "--delay", 5, "Screen Beta")
            display.type_key("Return")
            time.sleep(.6)
            renamed_header = window_pixels((85, 0, 520, 42))
            assert renamed_header != before_rename, "Enter must change the Screen label"
            capture("renamed-beta")
            click((273, 20))  # The longer label shifts B's rename icon.
            x("type", "--clearmodifiers", "--delay", 5, "Discard This Name")
            display.type_key("Escape")
            time.sleep(.5)
            assert window_pixels((85, 0, 520, 42)) == renamed_header, "Escape must cancel the rename"
            result["checks"].extend(["rename-enter", "rename-escape-cancels"])
            capture("rename-cancelled")

            click((105, 20))  # Select A.
            assert shell_pid("a-return") == a_pid, "Switching back must retain shell A"
            assert shell_value("a-state", '"$SPLITTY_SCREEN_TOKEN"') == "alpha", "A shell state was lost"
            capture("a-hidden-output-visible")
            result["checks"].extend(["a-shell-retained", "a-state-retained", "hidden-output-delivered"])
            click((210, 20))  # Select B.
            assert shell_pid("b-return") == b_pid, "Switching back must retain shell B"
            capture("b-return")
            result["checks"].append("b-shell-retained")

            if args.busy_close:
                click((300, 180), .1)
                send("sleep 20")
                time.sleep(1)
                click((290, 20))
                prompt_pixels = window_pixels((455, 300, 289, 160))
                capture("busy-close-prompt")
                assert alive(b_pid), "Busy close must wait for confirmation"
                click((600, 407))  # Keep screen in the native prompt.
                assert alive(b_pid), "Keep screen must retain the busy shell"
                wait_until(lambda: window_pixels((455, 300, 289, 160)) != prompt_pixels,
                           "Keep screen did not dismiss the prompt")
                capture("busy-close-cancelled")
                cleared_prompt_pixels = window_pixels((455, 300, 289, 160))
                click((290, 20))
                wait_until(lambda: window_pixels((455, 300, 289, 160)) != cleared_prompt_pixels,
                           "Busy close prompt did not reopen")
                capture("busy-close-confirm-prompt")
                click((600, 435))  # Close screen in the native prompt.
                result["checks"].extend(["busy-close-cancel", "busy-close-confirm"])
            else:
                click((290, 20))  # Close B, after the longer rename.
            wait_until(lambda: not alive(b_pid), "Closing B did not end its shell", seconds=8)
            assert shell_pid("a-after-b-close") == a_pid, "Closing B must preserve A"
            capture("second-closed")
            result["checks"].append("close-second-preserves-first")
            click((165, 20))  # Close A, the final Screen.
            wait_until(lambda: not alive(a_pid), "Closing the final Screen did not end its shell", seconds=8)
            capture("no-screens")
            result["checks"].append("close-last-empty-state")
            click((600, 380))  # New screen in the empty-state canvas.
            c_pid = shell_pid("c-new")
            assert c_pid not in (a_pid, b_pid), "New screen must launch a fresh shell"
            capture("new-screen-fresh-shell")
            result["checks"].append("new-screen-fresh-shell")
            result["shellPids"] = {"a": a_pid, "b": b_pid, "fresh": c_pid}
            result["passed"] = True
        except Exception as error:
            result["passed"] = False
            result["error"] = str(error)
            if display is not None and window is not None:
                try:
                    capture("failure")
                except Exception:
                    pass
        finally:
            descendants = set(performance.process_stats(app.pid)) - {app.pid} if app else set()
            if app is not None:
                if app.poll() is None and window is not None and display is not None:
                    display.close_window(window)
                try:
                    app.wait(timeout=15)
                    result["normalWindowClose"] = True
                except subprocess.TimeoutExpired:
                    result["normalWindowClose"] = False
                    os.killpg(app.pid, signal.SIGTERM)
                    app.wait(timeout=5)
                result["exitCode"] = app.returncode
                result["survivingChildren"] = []
                for pid in descendants:
                    if alive(pid):
                        result["survivingChildren"].append(pid)
                        os.kill(pid, signal.SIGTERM)
            if display is not None:
                display.close()
            if xvfb is not None:
                if xvfb.poll() is None:
                    os.killpg(xvfb.pid, signal.SIGTERM)
                xvfb.wait(timeout=5)
    (args.output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))
    if not result.get("passed") or not result.get("normalWindowClose") or result.get("exitCode") != 0 or result.get("survivingChildren"):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
