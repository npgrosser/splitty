#!/usr/bin/env python3
"""Exercise the native local shell picker in an owned Xvfb workspace.

The generated config and shell processes live only below --output. Profile
identity is checked from real launched argv/PIDs; the raw PTY fixtures record
whether picker keys, pointer motion or wheel events reached a terminal.
"""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time


HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("performance", HERE / "gpui-performance.py")
performance = importlib.util.module_from_spec(spec)
spec.loader.exec_module(performance)
spec = importlib.util.spec_from_file_location("pane_drag", HERE / "gpui-pane-drag.py")
pane_drag = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pane_drag)

SHELL_FIXTURE = r'''import json, os, pathlib, select, sys, termios, tty
label, root = sys.argv[1:3]
root = pathlib.Path(root)
pid = os.getpid()
fd = sys.stdin.fileno()
original = termios.tcgetattr(fd)
identity = {"label": label, "pid": pid, "argv": sys.argv, "cwd": os.getcwd()}
(root / f"profile-{pid}.json").write_text(json.dumps(identity))
try:
    tty.setraw(fd)
    os.write(sys.stdout.fileno(), (f"PROFILE_{label}_{pid}\r\n").encode() + b"\x1b[?1003h\x1b[?1006h")
    with (root / f"input-{pid}.bin").open("wb", buffering=0) as recorded:
        while not (root / f"stop-{pid}").exists():
            if (root / f"emit-{pid}").exists():
                (root / f"emit-{pid}").unlink()
                os.write(sys.stdout.fileno(), (f"OUTPUT_{label}_{pid}\r\n").encode())
            ready, _, _ = select.select([fd], [], [], .03)
            if ready:
                data = os.read(fd, 65536)
                if not data: break
                recorded.write(data)
finally:
    try: os.write(sys.stdout.fileno(), b"\x1b[?1003l\x1b[?1006l")
    except OSError: pass
    termios.tcsetattr(fd, termios.TCSANOW, original)
'''


def wait_until(predicate, description, seconds=15):
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


def config_text(python, script, owned):
    profiles = [
        ("base", "Base shell", python, ["-u", str(script), "base", str(owned)], True),
        ("choice", "Chosen shell", python, ["-u", str(script), "choice", str(owned)], True),
        ("missing", "Unavailable shell", "/no/such/splitty-shell", [], True),
        ("disabled", "Disabled shell", python, ["-u", str(script), "disabled", str(owned)], False),
    ]
    lines = ['[appearance]', 'theme_mode = "fixed"', 'theme = "splitty-dark"', 'font_size = 0',
             '', '[shells]', 'default_profile_id = "base"']
    for identity, name, command, args, enabled in profiles:
        lines += ['', '[[shells.profiles]]', f'id = {json.dumps(identity)}',
                  f'name = {json.dumps(name)}', f'command = {json.dumps(str(command))}',
                  f'args = {json.dumps(args)}', f'enabled = {str(enabled).lower()}',
                  'target = "local"', 'source = "custom"']
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--display", default=":95")
    parser.add_argument("--xvfb", default="Xvfb")
    parser.add_argument("--xvfb-lib", type=Path)
    parser.add_argument("--xdotool", default="xdotool")
    parser.add_argument("--setup-only", action="store_true", help="capture picker for geometry calibration")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    binary = args.binary.resolve(strict=True)
    owned = args.output / "profiles"
    owned.mkdir(mode=0o700)
    script = args.output / "profile_shell.py"
    script.write_text(SHELL_FIXTURE)
    config = args.output / "config"
    config.mkdir(mode=0o700)
    (config / "config.toml").write_text(config_text(Path(sys.executable).resolve(), script.resolve(), owned.resolve()))
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
              "harnessSha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "fixtureSha256": hashlib.sha256(SHELL_FIXTURE.encode()).hexdigest(),
              "display": args.display, "checks": [], "screenshots": [], "ptyEvents": {}, "passed": False}
    xvfb = app = display = window = None

    def x(*arguments):
        return subprocess.check_output([str(args.xdotool), *map(str, arguments)], env=env, text=True, timeout=12)

    def profiles():
        return [json.loads(path.read_text()) for path in owned.glob("profile-*.json")]

    def raw():
        return {identity["pid"]: (owned / f'input-{identity["pid"]}.bin').read_bytes()
                for identity in profiles() if (owned / f'input-{identity["pid"]}.bin').exists()}

    def no_input(label, action):
        before = raw()
        action()
        time.sleep(.2)
        after = raw()
        delta = {pid: value[len(before.get(pid, b"")):] for pid, value in after.items()
                 if value != before.get(pid, b"")}
        result["ptyEvents"][label] = {str(pid): value.hex() for pid, value in delta.items()}
        assert not delta, f"{label} reached a PTY: {delta}"

    def chord(*keys):
        for modifier in keys[:-1]:
            display.key(modifier, True)
        display.type_key(keys[-1])
        for modifier in reversed(keys[:-1]):
            display.key(modifier, False)
        display.sync()

    def capture(label):
        path = args.output / f"{label}.png"
        pane_drag.save_png(path, display.pixels(window, (0, 0, 1200, 760)))
        result["screenshots"].append(str(path))

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
            base = wait_until(lambda: next((entry for entry in profiles() if entry["label"] == "base"), None),
                              "configured base shell did not launch", seconds=30)
            capture("base-terminal")
            result["checks"].append("owned-config-launches-default-profile")
            x("windowfocus", window, "mousemove", "--window", window, 1180, 62)
            no_input("open-picker", lambda: x("click", 1))
            capture("picker-open")
            result["checks"].append("picker-open-capture")
            if not args.setup_only:
                no_input("picker-key-isolation", lambda: display.type_key("x"))
                no_input("picker-hover", lambda: x("mousemove", "--window", window, 930, 245))
                no_input("picker-backdrop-hover", lambda: x("mousemove", "--window", window, 500, 245))
                no_input("picker-wheel", lambda: display.wheel(5))
                capture("picker-hover-wheel")
                no_input("unavailable-click", lambda: x("mousemove", "--window", window, 930, 150, "click", 1))
                assert len(profiles()) == 1, "unavailable entry launched a process"
                capture("unavailable-remains-disabled")
                result["checks"].append("unavailable-profile-disabled-and-pointer-isolated")
                no_input("picker-escape", lambda: display.type_key("Escape"))
                before = raw()[base["pid"]]
                x("type", "--clearmodifiers", "r")
                wait_until(lambda: len(raw()[base["pid"]]) > len(before), "focus did not return to base PTY")
                assert raw()[base["pid"]][len(before):] == b"r"
                result["checks"].append("picker-input-isolated-and-escape-restores-pty")

                x("mousemove", "--window", window, 1180, 62)
                no_input("open-picker-for-outside-dismiss", lambda: x("click", 1))
                no_input("outside-dismiss", lambda: x("mousemove", "--window", window, 500, 245, "click", 1))
                capture("outside-click-dismissed-picker")
                before = raw()[base["pid"]]
                x("type", "--clearmodifiers", "u")
                wait_until(lambda: len(raw()[base["pid"]]) > len(before), "outside click did not return focus")
                assert raw()[base["pid"]][len(before):] == b"u"
                result["checks"].append("outside-dismiss-restores-pty-focus")

                x("mousemove", "--window", window, 1180, 62)
                no_input("open-picker-for-choice", lambda: x("click", 1))
                no_input("choose-next-profile", lambda: (display.type_key("Down"), display.type_key("Return")))
                chosen = wait_until(lambda: next((entry for entry in profiles() if entry["label"] == "choice"), None),
                                    "explicit chosen profile did not launch", seconds=20)
                assert chosen["cwd"] == base["cwd"], (base, chosen)
                capture("chosen-profile-terminal")
                result["checks"].append("keyboard-choice-launches-configured-program-and-inherits-cwd")

                before_pixels = display.pixels(window, (0, 85, 1190, 300))
                (owned / f'emit-{chosen["pid"]}').touch()
                wait_until(lambda: display.pixels(window, (0, 85, 1190, 300)) != before_pixels,
                           "chosen profile output did not paint", seconds=8)
                capture("chosen-output-continues")
                result["checks"].append("terminal-output-continues-after-picker-choice")

                no_input("plain-tab", lambda: chord("Alt_L", "t"))
                wait_until(lambda: len([entry for entry in profiles() if entry["label"] == "choice"]) >= 2,
                           "ordinary tab did not inherit chosen profile")
                assert all(entry["cwd"] == chosen["cwd"] for entry in profiles() if entry["label"] == "choice")
                capture("plain-tab-inherits-choice")
                result["checks"].append("ordinary-tab-inherits-selected-profile")

                no_input("split-inherits", lambda: chord("Alt_L", "d"))
                wait_until(lambda: len([entry for entry in profiles() if entry["label"] == "choice"]) >= 3,
                           "split did not inherit chosen profile")
                capture("split-inherits-choice")
                result["checks"].append("ordinary-split-inherits-selected-profile")

            result["passed"] = True
        except Exception as error:
            result["error"] = repr(error)
            if display and window:
                try:
                    capture("failure")
                except Exception as capture_error:
                    result["captureError"] = repr(capture_error)
        finally:
            children = set(performance.process_stats(app.pid)) - {app.pid} if app else set()
            for identity in profiles():
                (owned / f'stop-{identity["pid"]}').touch()
            for identity in profiles():
                try:
                    wait_until(lambda pid=identity["pid"]: not alive(pid), f'profile {identity["pid"]} did not stop', seconds=6)
                except TimeoutError:
                    result.setdefault("stopErrors", []).append(identity["pid"])
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
            result["survivingChildren"] = [pid for pid in children if alive(pid)]
            result["profileProcesses"] = profiles()
            result["passed"] = result["passed"] and result.get("exitCode") == 0 and not result["survivingChildren"]
            (args.output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result, indent=2))
    if not result["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
