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
    parser.add_argument("--close-confirmation", action="store_true", help="exercise pane, Screen, and owned-window close confirmation")
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

    def focused_shell_pid(label):
        """Check that cancelling a prompt restored terminal keyboard focus."""
        path = scratch / label
        path.unlink(missing_ok=True)
        display.key("Control_L", True)
        display.type_key("c")
        display.key("Control_L", False)
        display.sync()
        time.sleep(.1)
        send(f"printf '%s\\n' \"$$\" > '{path}'")
        return int(wait_until(lambda: path.read_text().strip() if path.exists() else None,
                              f"Focus did not return to the shell for {label}", seconds=8))

    def alt_w():
        display.key("Alt_L", True)
        display.type_key("w")
        display.key("Alt_L", False)
        display.sync()
        time.sleep(.4)

    def close_dialog(label):
        capture(label)
        return window_pixels((455, 300, 289, 160))

    def keep_open(label, prior):
        click((600, 393))
        wait_until(lambda: window_pixels((455, 300, 289, 160)) != prior,
                   f"Keep open did not dismiss {label}")
        capture(f"{label}-cancelled")

    def confirm_close():
        click((600, 422))

    def exercise_close_confirmation(a_pid):
        click((110, 62), 1)  # A second tab in the first Screen.
        b_pid = shell_pid("pane-b-initial")
        assert b_pid != a_pid, "New tab must own a second shell"
        click((40, 62))  # Return to the first tab, whose close X is stable.
        assert shell_pid("pane-a-before-close") == a_pid
        send("sleep 60")
        time.sleep(1)
        click((85, 62))  # First tab's close X while titled sleep 60.
        prompt = close_dialog("pane-button-prompt")
        assert alive(a_pid) and alive(b_pid), "Busy pane button must wait for confirmation"
        click((85, 62))  # Repeated gesture while a decision is pending.
        assert window_pixels((455, 300, 289, 160)) == prompt, "Repeated pane close changed the prompt"
        keep_open("pane-button", prompt)
        assert focused_shell_pid("pane-button-focus") == a_pid, "Pane cancel lost the shell or focus"
        result["checks"].extend(["pane-button-cancel", "pane-repeat-one-prompt", "pane-cancel-restores-focus"])

        send("sleep 60")
        time.sleep(1)
        alt_w()
        prompt = close_dialog("pane-shortcut-prompt")
        assert alive(a_pid), "Alt+W must wait for confirmation"
        keep_open("pane-shortcut", prompt)
        assert focused_shell_pid("pane-shortcut-focus") == a_pid, "Alt+W cancel lost the shell or focus"
        result["checks"].append("pane-alt-w-cancel")

        send("sleep 60")
        time.sleep(1)
        click((85, 62))
        close_dialog("pane-confirm-prompt")
        confirm_close()
        wait_until(lambda: not alive(a_pid), "Confirmed pane close left its shell alive", seconds=8)
        assert shell_pid("pane-b-after-a-close") == b_pid, "Closing A must preserve B"
        capture("pane-confirmed")
        result["checks"].append("pane-confirm-preserves-other-terminal")

        click((210, 20), 1)  # New second Screen.
        c_pid = shell_pid("screen-c-initial")
        assert c_pid not in (a_pid, b_pid)
        send("sleep 60")
        time.sleep(1)
        click((265, 20))  # Second Screen close X.
        prompt = close_dialog("screen-prompt")
        assert alive(c_pid), "Busy Screen close must wait for confirmation"
        keep_open("screen", prompt)
        assert focused_shell_pid("screen-c-focus") == c_pid, "Screen cancel lost its shell or focus"
        send("sleep 60")
        time.sleep(1)
        click((265, 20))
        close_dialog("screen-confirm-prompt")
        confirm_close()
        wait_until(lambda: not alive(c_pid), "Confirmed Screen close left its shell alive", seconds=8)
        assert shell_pid("pane-b-after-screen-close") == b_pid
        capture("screen-confirmed")
        result["checks"].extend(["screen-busy-cancel", "screen-busy-confirm"])

        click((210, 20), 1)  # A hidden busy Screen for owned-window close.
        d_pid = shell_pid("hidden-d-initial")
        marker = scratch / "hidden-window-output"
        send(f"(sleep 2; printf 'HIDDEN_WINDOW_OUTPUT\\n' | tee '{marker}') &")
        send("sleep 60")
        time.sleep(1)
        click((105, 20))  # Show first Screen; D remains busy and hidden.
        wait_until(lambda: marker.exists(), "Hidden Screen did not produce output", seconds=8)
        display.close_window(window)  # Real WM_DELETE_WINDOW request.
        time.sleep(.5)
        prompt = close_dialog("window-hidden-busy-prompt")
        assert app.poll() is None and alive(d_pid), "Window close did not wait for hidden busy Screen"
        keep_open("window", prompt)
        assert app.poll() is None and focused_shell_pid("window-b-focus") == b_pid, (
            "Window cancel lost the visible shell or focus"
        )
        click((210, 20))  # Reveal D and its output after cancellation.
        assert alive(d_pid), "Hidden busy Screen ended after window cancel"
        capture("window-hidden-output-after-cancel")
        click((105, 20))
        result["checks"].extend(["window-hidden-busy-cancel", "window-cancel-keeps-both-screens"])

        display.close_window(window)
        time.sleep(.5)
        close_dialog("window-confirm-prompt")
        confirm_close()
        wait_until(lambda: app.poll() is not None, "Confirmed window close did not exit", seconds=15)
        wait_until(lambda: not alive(b_pid) and not alive(d_pid),
                   "Confirmed window close left a shell alive", seconds=8)
        result["shellPids"] = {"paneA": a_pid, "paneB": b_pid, "screenC": c_pid, "hiddenD": d_pid}
        result["checks"].append("window-confirm-owned-cleanup")

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
            if args.close_confirmation:
                exercise_close_confirmation(a_pid)
            else:
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
                    click((600, 393))  # Keep open in the native prompt.
                    assert alive(b_pid), "Keep screen must retain the busy shell"
                    wait_until(lambda: window_pixels((455, 300, 289, 160)) != prompt_pixels,
                               "Keep screen did not dismiss the prompt")
                    capture("busy-close-cancelled")
                    cleared_prompt_pixels = window_pixels((455, 300, 289, 160))
                    click((290, 20))
                    wait_until(lambda: window_pixels((455, 300, 289, 160)) != cleared_prompt_pixels,
                               "Busy close prompt did not reopen")
                    capture("busy-close-confirm-prompt")
                    click((600, 422))  # Close in the native prompt.
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
