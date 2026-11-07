#!/usr/bin/env python3
"""Exercise native terminal links in a private X11 window with an OS-opener spy.

No URL reaches the network or user desktop: this process prepends successful
spy commands for every launcher in the pinned Linux GPUI/open path, including
WSL's powershell.exe. The fixture records exact PTY bytes under 1003/1006.
"""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shlex
import signal
import subprocess
import time


HERE = Path(__file__).resolve().parent
SGR = re.compile(rb"\x1b\[<([0-9]+);([0-9]+);([0-9]+)([Mm])")
OSC_TARGET = "https://example.invalid/osc-target-4826?from=label"
PLAIN_TARGET = "http://example.invalid/plain-4826?x=1"
WRAP_TARGET = "https://example.invalid/softwrap-4826?q=1"
NEW_TARGET = "https://example.invalid/new-4826"


def helper(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


performance = helper("performance", "gpui-performance.py")
pane_drag = helper("pane_drag", "gpui-pane-drag.py")


def wait_until(predicate, description, seconds=12):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.06)
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
    parser.add_argument("--xvfb", default="Xvfb")
    parser.add_argument("--xvfb-lib", type=Path)
    parser.add_argument("--xdotool", default="xdotool")
    parser.add_argument("--setup-only", action="store_true", help="capture fixture geometry before link code is ready")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    binary = args.binary.resolve(strict=True)
    fixture_script = HERE / "gpui-terminal-links-fixture.py"
    owned = args.output / "fixture"
    owned.mkdir(mode=0o700)
    config = args.output / "config"
    config.mkdir(mode=0o700)
    (config / "config.toml").write_text('[appearance]\ntheme_mode = "fixed"\ntheme = "splitty-dark"\nfont_size = 0\n')
    spy_dir = args.output / "opener-spy"
    spy_dir.mkdir(mode=0o700)
    spy_log = args.output / "opener.tsv"
    spy_log.touch(mode=0o600)
    spy = spy_dir / "spy.sh"
    spy.write_text('#!/bin/sh\ntarget=""\nfor value do target="$value"; done\n'
                   'if [ -n "${OPEN_RS_TARGET:-}" ]; then target="$OPEN_RS_TARGET"; fi\n'
                   'printf "%s\\t%s\\n" "$0" "$target" >> "$SPLITTY_LINK_SPY_LOG"\nexit 0\n')
    spy.chmod(0o700)
    for name in ("powershell.exe", "xdg-open", "gio", "gnome-open", "kde-open"):
        (spy_dir / name).symlink_to(spy.name)
    env = os.environ.copy()
    env.pop("WAYLAND_DISPLAY", None)
    env["DISPLAY"] = args.display
    env["SPLITTY_CONFIG_DIR"] = str(config.resolve())
    env["SPLITTY_LINK_SPY_LOG"] = str(spy_log.resolve())
    env["PATH"] = str(spy_dir.resolve()) + os.pathsep + env.get("PATH", "/usr/bin:/bin")
    env["BROWSER"] = str((spy_dir / "xdg-open").resolve())
    for key in ("SPLITTY_SOCK", "SPLITTY_AGENT_SOCK", "SPLITTY_ADDR", "SPLITTY_TOKEN",
                "SPLITTY_SERVER", "SPLITTY_PANE_ID", "SPLITTY_SCREEN_ID", "SPLITTY_SESSION_ID"):
        env.pop(key, None)
    xvfb_env = env.copy()
    if args.xvfb_lib:
        xvfb_env["LD_LIBRARY_PATH"] = str(args.xvfb_lib) + (":" + env["LD_LIBRARY_PATH"] if env.get("LD_LIBRARY_PATH") else "")
    result = {"binary": str(binary), "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "harnessSha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "fixtureSha256": hashlib.sha256(fixture_script.read_bytes()).hexdigest(),
              "display": args.display, "setupOnly": args.setup_only,
              "checks": [], "screenshots": [], "ptyEvents": {}, "passed": False}
    xvfb = app = display = window = None
    sequence = 0

    def x(*arguments):
        return subprocess.check_output([str(args.xdotool), *map(str, arguments)], env=env, text=True, timeout=12)

    def raw():
        return (owned / "received.bin").read_bytes()

    def opened():
        return [{"program": Path(program).name, "target": target}
                for program, target in (line.split("\t", 1) for line in spy_log.read_text().splitlines())]

    def command(action):
        nonlocal sequence
        sequence += 1
        next_control = owned / "control.next"
        next_control.write_text(json.dumps({"seq": sequence, "action": action}))
        next_control.replace(owned / "control.json")
        if action != "stop":
            wait_until(lambda: json.loads((owned / "ack.json").read_text()).get("seq") == sequence
                       if (owned / "ack.json").exists() else False,
                       f"fixture did not acknowledge {action}")

    def chord(*keys):
        for modifier in keys[:-1]:
            display.key(modifier, True)
        display.type_key(keys[-1])
        for modifier in reversed(keys[:-1]):
            display.key(modifier, False)
        display.sync()

    def move(col, row):
        x("mousemove", "--window", window, col, row)
        display.sync()
        time.sleep(.1)

    def button(down):
        display.xt.XTestFakeButtonEvent(display.connection, 1, down, 0)
        display.sync()

    def capture(label):
        path = args.output / f"{label}.png"
        pane_drag.save_png(path, display.pixels(window, (0, 0, 1200, 760)))
        result["screenshots"].append(str(path))

    def no_open(label, action, local=False):
        before_open = opened()
        before_raw = raw()
        action()
        time.sleep(.35)
        assert opened() == before_open, f"{label} opened a URL: {opened()[len(before_open):]}"
        delta = raw()[len(before_raw):]
        result["ptyEvents"][label] = delta.hex()
        if local:
            assert not delta, f"{label} leaked PTY bytes: {delta.hex()}"
        return delta

    def ctrl_click(label, position, expected=None, release_control_early=False):
        move(*position)
        before_open = len(opened())
        before_raw = raw()
        display.key("Control_L", True)
        button(True)
        if release_control_early:
            display.key("Control_L", False)
            display.sync()
        button(False)
        if not release_control_early:
            display.key("Control_L", False)
            display.sync()
        if expected is None:
            time.sleep(.35)
            assert len(opened()) == before_open, f"{label} unexpectedly opened: {opened()[before_open:]}"
        else:
            wait_until(lambda: len(opened()) == before_open + 1, f"{label} did not reach private opener")
            time.sleep(.25)
            assert len(opened()) == before_open + 1, f"{label} reached opener more than once"
            assert opened()[-1]["target"] == expected, (label, opened()[-1], expected)
            assert opened()[-1]["program"] in {"powershell.exe", "xdg-open", "gio", "gnome-open", "kde-open"}
        delta = raw()[len(before_raw):]
        result["ptyEvents"][label] = delta.hex()
        if expected is not None:
            assert not delta, f"{label} leaked PTY bytes: {delta.hex()}"

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
            old_display = os.environ.get("DISPLAY")
            os.environ["DISPLAY"] = args.display
            try:
                display = performance.Display()
            finally:
                if old_display is None:
                    os.environ.pop("DISPLAY", None)
                else:
                    os.environ["DISPLAY"] = old_display
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
            time.sleep(1.5)
            x("windowfocus", window, "mousemove", "--window", window, 180, 300, "click", 1)
            launch = f"python3 -u {shlex.quote(str(fixture_script))} {shlex.quote(str(owned.resolve()))}"
            x("type", "--clearmodifiers", "--delay", 2, launch)
            display.type_key("Return")
            wait_until(lambda: (owned / "ready").exists(), "raw link fixture did not start")
            wait_until(lambda: (owned / "received.bin").exists(), "raw PTY log did not appear")
            time.sleep(.4)
            result["geometry"] = json.loads((owned / "geometry.json").read_text())
            capture("fixture-ready")
            assert not opened(), f"terminal output launched an opener: {opened()}"
            result["checks"].append("fixture-output-does-not-open-url")
            if not args.setup_only:
                no_open("ordinary-hover", lambda: move(40, 90))

                def preview_hover():
                    move(800, 300)
                    display.key("Control_L", True)
                    display.sync()
                    move(40, 90)
                    time.sleep(.2)
                    capture("osc8-ctrl-hover-preview")
                    display.key("Control_L", False)
                    display.sync()

                no_open("ctrl-hover-preview", preview_hover)
                no_open("ordinary-click", lambda: x("click", 1))
                result["checks"].append("ctrl-hover-and-ordinary-click-do-not-open")

                ctrl_click("osc8-target-not-label", (40, 90), OSC_TARGET)
                capture("osc8-activated")
                result["checks"].append("osc8-opens-explicit-target-once-without-pty-input")
                ctrl_click("plain-http", (50, 111), PLAIN_TARGET)
                result["checks"].append("plain-http-opens-once")
                ctrl_click("softwrapped-url", (45, 153), WRAP_TARGET)
                capture("softwrapped-url-activated")
                result["checks"].append("softwrapped-plain-url-opens-complete-target")

                ctrl_click("blocked-osc8-decoy", (60, 174))
                ctrl_click("blocked-file", (60, 195))
                ctrl_click("blocked-editor", (300, 195))
                ctrl_click("blocked-plain-javascript", (500, 195))
                result["checks"].append("blocked-schemes-and-osc8-precedence-never-open")

                move(40, 216)
                before_open = opened()
                before_raw = raw()
                display.key("Control_L", True)
                button(True)
                command("mutate")
                # The fixture's ACK means bytes were written; let the app
                # consume that PTY output before testing the release hit.
                time.sleep(.3)
                button(False)
                display.key("Control_L", False)
                display.sync()
                time.sleep(.35)
                assert opened() == before_open, "changed target opened after pointer press"
                assert raw() == before_raw, "changed-target gesture leaked PTY bytes"
                capture("changed-target-cancelled")
                result["checks"].append("changed-target-between-press-release-cancels")

                ctrl_click("fresh-changed-target", (40, 216), NEW_TARGET)
                result["checks"].append("fresh-hit-opens-new-target")
                move(40, 90)
                no_open("drag-cancelled", lambda: (
                    display.key("Control_L", True), button(True), move(250, 90), button(False),
                    display.key("Control_L", False), display.sync()), local=True)
                move(40, 90)
                no_open("outside-release-cancelled", lambda: (
                    display.key("Control_L", True), button(True), move(300, 20), button(False),
                    display.key("Control_L", False), display.sync()), local=True)
                move(40, 90)
                no_open("exit-reenter-held-link-cancelled", lambda: (
                    display.key("Control_L", True), button(True), move(300, 20), move(40, 90),
                    button(False), display.key("Control_L", False), display.sync()), local=True)
                result["checks"].append("drag-outside-and-reenter-release-do-not-open-or-leak")

                ctrl_click("modifier-release-before-up", (40, 90), OSC_TARGET, release_control_early=True)
                result["checks"].append("admitted-link-stays-local-after-modifier-release")

                move(200, 237)
                before = raw()
                no_open("ordinary-app-click", lambda: x("click", 1))
                reports = [(int(code), kind) for code, _, _, kind in SGR.findall(raw()[len(before):])]
                assert any(code == 0 and kind == b"M" for code, kind in reports), reports
                assert any(kind == b"m" for _, kind in reports), reports
                result["checks"].append("ordinary-application-mouse-reports-remain")

                move(5, 237)
                before = raw()
                display.key("Shift_L", True)
                button(True)
                move(140, 237)
                button(False)
                display.key("Shift_L", False)
                display.sync()
                assert raw() == before, "Shift selection leaked application mouse bytes"
                capture("shift-selection")
                before = len(raw())
                chord("Control_L", "Shift_L", "c")
                chord("Control_L", "Shift_L", "v")
                wait_until(lambda: len(raw()) > before, "private selection paste did not reach PTY")
                pasted = raw()[before:]
                result["ptyEvents"]["shift-selection-paste"] = pasted.hex()
                assert pasted.strip() == b"SELECTWORD4826", pasted
                result["checks"].append("shift-selection-preserves-private-copy-paste")

            command("stop")
            fixture_pid = int((owned / "ready").read_text())
            wait_until(lambda: not alive(fixture_pid), "raw fixture did not exit normally", seconds=8)
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
                    try:
                        x("mousemove", "--window", window, 600, 422, "click", 1)
                        app.wait(timeout=5)
                    except Exception as close_error:
                        result["closeError"] = repr(close_error)
                except Exception as close_error:
                    result["closeError"] = repr(close_error)
            if app:
                try:
                    app.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(app.pid, signal.SIGTERM)
                    try:
                        app.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        os.killpg(app.pid, signal.SIGKILL)
                        app.wait(timeout=3)
                result["exitCode"] = app.returncode
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
            result["openerCalls"] = opened()
            result["survivingChildren"] = [pid for pid in descendants if alive(pid)]
            result["passed"] = result["passed"] and result.get("exitCode") == 0 and not result["survivingChildren"]
            (args.output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result, indent=2))
    if not result["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
