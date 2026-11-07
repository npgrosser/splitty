#!/usr/bin/env python3
"""Exercise native tab drag/drop with real shell input on an isolated X11 display.

Requires the same native-library/font environment as splitty-gpui, xdotool,
libX11 and libXtst. Use Xvfb to prevent interactive focus interference. Captures
are evidence for visual review; shell PID assertions verify session continuity.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import struct
import subprocess
import tempfile
import time
import zlib

spec = importlib.util.spec_from_file_location("performance", Path(__file__).with_name("gpui-performance.py"))
performance = importlib.util.module_from_spec(spec)
spec.loader.exec_module(performance)


def save_png(path, data, width=1200, height=760):
    rows = bytearray()
    for y in range(height):
        rows.append(0)
        for offset in range(y * width * 4, (y + 1) * width * 4, 4):
            rows.extend((data[offset + 2], data[offset + 1], data[offset]))

    def chunk(kind, value):
        return struct.pack(">I", len(value)) + kind + value + struct.pack(">I", zlib.crc32(kind + value))

    path.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">2I5B", width, height, 8, 2, 0, 0, 0))
                     + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--xdotool", default="xdotool")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env.pop("WAYLAND_DISPLAY", None)
    display = performance.Display()
    window = None
    result = {"binary": str(args.binary.resolve()), "sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest(),
              "display": env.get("DISPLAY"), "checks": [], "screenshots": []}

    def x(*arguments):
        return subprocess.check_output([args.xdotool, *map(str, arguments)], env=env, text=True, timeout=15)

    def click(point):
        x("mousemove", "--window", window, *point, "click", 1)
        time.sleep(.5)

    def capture(label):
        path = args.output / (label + ".png")
        save_png(path, display.pixels(window, (0, 0, 1200, 760)))
        result["screenshots"].append(str(path))

    def drag(start, target, cancel=False):
        x("mousemove", "--window", window, *start, "mousedown", 1)
        time.sleep(.1)
        # Pass the built-in drag threshold before entering the target zone.
        x("mousemove", "--window", window, start[0] + 15, start[1] + 20)
        time.sleep(.15)
        x("mousemove", "--window", window, *target)
        time.sleep(.2)
        if cancel:
            display.type_key("Escape")
        x("mouseup", 1)
        time.sleep(.7)

    with tempfile.TemporaryDirectory(prefix="splitty-drag-") as directory, (args.output / "app.log").open("w") as log:
        app = subprocess.Popen([str(args.binary.resolve())], env=env, stdout=log, stderr=log, start_new_session=True)
        try:
            deadline = time.monotonic() + 30
            while window is None and time.monotonic() < deadline:
                if app.poll() is not None:
                    raise RuntimeError("Application exited before opening a window")
                found = subprocess.run([args.xdotool, "search", "--onlyvisible", "--pid", str(app.pid)], env=env, capture_output=True, text=True)
                for candidate in found.stdout.splitlines():
                    geometry = dict(line.split("=", 1) for line in x("getwindowgeometry", "--shell", candidate).splitlines() if "=" in line)
                    if geometry.get("WIDTH") == "1200" and geometry.get("HEIGHT") == "760":
                        window = int(candidate)
                        break
                time.sleep(.1)
            if window is None:
                raise TimeoutError("No 1200x760 native workspace appeared")
            time.sleep(3)
            x("windowfocus", window)
            click((150, 180))

            def shell_pid(label):
                path = Path(directory) / label
                display.clear_line()
                time.sleep(.1)
                command = f"printf 'DRAG_{label} session %s\\n' $$; printf %s $$ > {path}"
                x("type", "--clearmodifiers", "--delay", 5, command)
                display.type_key("Return")
                deadline = time.monotonic() + 5
                while not path.exists() and time.monotonic() < deadline:
                    time.sleep(.05)
                return int(path.read_text())

            left = shell_pid("left")
            click((960, 20))  # Split right.
            moving = shell_pid("moving")
            assert left != moving, "split must own a distinct shell"
            capture("initial-split")
            source, destination = (625, 62), (300, 300)
            edges = [("left", (10, 350), (25, 62), (900, 300)),
                     ("right", (1190, 350), (625, 62), (300, 300)),
                     ("above", (600, 50), (25, 62), (300, 550)),
                     ("below", (600, 720), (25, 407), (300, 220))]
            for name, edge, next_source, next_destination in edges:
                drag(source, destination)
                assert shell_pid("tab-" + name) == moving, "tab drop must select the original shell"
                capture("tab-" + name)
                drag((115, 62), edge)
                assert shell_pid("edge-" + name) == moving, "edge drop must retain the original shell"
                capture("edge-" + name)
                result["checks"].extend(["tab-before-" + name, "edge-" + name])
                source, destination = next_source, next_destination
            # A cancelled drop must keep both the split and the focused shell.
            before = display.pixels(window, (20, 384, 1160, 10))
            drag(source, destination, cancel=True)
            assert display.pixels(window, (20, 384, 1160, 10)) == before, "Escape must preserve the divider"
            assert shell_pid("cancelled") == moving, "Escape must not switch the session"
            capture("cancelled")
            result["checks"].append("escape-cancels")
            result["shellPids"] = {"left": left, "moving": moving}
            result["passed"] = True
        except Exception as error:
            result["passed"] = False
            result["error"] = str(error)
            if window is not None:
                capture("failure")
        finally:
            descendants = set(performance.process_stats(app.pid)) - {app.pid}
            if app.poll() is None and window is not None:
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
                path = Path(f"/proc/{pid}/stat")
                try:
                    if path.read_text().rsplit(")", 1)[1].split()[0] != "Z":
                        result["survivingChildren"].append(pid)
                        os.kill(pid, signal.SIGTERM)
                except FileNotFoundError:
                    pass
            display.close()
    (args.output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))
    if not result.get("passed") or not result.get("normalWindowClose") or result.get("exitCode") != 0 or result["survivingChildren"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
