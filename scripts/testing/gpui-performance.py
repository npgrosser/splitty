#!/usr/bin/env python3
"""Measure an owned GPUI probe through X11, without a production daemon.

Run in the same prepared native-library/font environment as the application.
Requires xdotool (or --xdotool PATH), libX11 and libXtst. The application must
open its terminal/Markdown probe; pass --app-argument=--probe when required.
Latency is XTest request to first changed X11 terminal-buffer pixels, including
X server round trips and capture sampling. It is not physical display latency.
"""
import argparse
import ctypes as C
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import signal
import statistics
import subprocess
import tempfile
import time


class XImage(C.Structure):
    _fields_ = [("width", C.c_int), ("height", C.c_int), ("xoffset", C.c_int),
                ("format", C.c_int), ("data", C.c_void_p), ("byte_order", C.c_int),
                ("bitmap_unit", C.c_int), ("bitmap_bit_order", C.c_int),
                ("bitmap_pad", C.c_int), ("depth", C.c_int),
                ("bytes_per_line", C.c_int), ("bits_per_pixel", C.c_int)]


class ClientData(C.Union):
    _fields_ = [("l", C.c_long * 5), ("b", C.c_char * 20)]


class ClientMessage(C.Structure):
    _fields_ = [("type", C.c_int), ("serial", C.c_ulong), ("send_event", C.c_int),
                ("display", C.c_void_p), ("window", C.c_ulong),
                ("message_type", C.c_ulong), ("format", C.c_int), ("data", ClientData)]


class XEvent(C.Union):
    _fields_ = [("client", ClientMessage), ("pad", C.c_long * 24)]


class Display:
    def __init__(self):
        self.x = C.CDLL("libX11.so.6")
        self.xt = C.CDLL("libXtst.so.6")
        signatures = {
            "XOpenDisplay": ([C.c_char_p], C.c_void_p),
            "XCloseDisplay": ([C.c_void_p], C.c_int),
            "XSync": ([C.c_void_p, C.c_int], C.c_int),
            "XGetImage": ([C.c_void_p, C.c_ulong, C.c_int, C.c_int, C.c_uint,
                           C.c_uint, C.c_ulong, C.c_int], C.POINTER(XImage)),
            "XDestroyImage": ([C.POINTER(XImage)], C.c_int),
            "XStringToKeysym": ([C.c_char_p], C.c_ulong),
            "XKeysymToKeycode": ([C.c_void_p, C.c_ulong], C.c_uint),
            "XInternAtom": ([C.c_void_p, C.c_char_p, C.c_int], C.c_ulong),
            "XSendEvent": ([C.c_void_p, C.c_ulong, C.c_int, C.c_long,
                            C.POINTER(XEvent)], C.c_int),
        }
        for name, (args, result) in signatures.items():
            function = getattr(self.x, name)
            function.argtypes, function.restype = args, result
        # Xlib's default handler exits the harness on a vanished window, bypassing
        # Python cleanup. Returning here lets the failed request become an ordinary
        # exception while the finally block still closes the owned application.
        handler_type = C.CFUNCTYPE(C.c_int, C.c_void_p, C.c_void_p)
        self.error_handler = handler_type(lambda _display, _event: 0)
        self.x.XSetErrorHandler.argtypes = [handler_type]
        self.x.XSetErrorHandler.restype = C.c_void_p
        self.x.XSetErrorHandler(self.error_handler)
        self.xt.XTestFakeKeyEvent.argtypes = [C.c_void_p, C.c_uint, C.c_int, C.c_ulong]
        self.xt.XTestFakeButtonEvent.argtypes = [C.c_void_p, C.c_uint, C.c_int, C.c_ulong]
        self.connection = self.x.XOpenDisplay(None)
        if not self.connection:
            raise RuntimeError("Cannot open DISPLAY")

    def sync(self):
        self.x.XSync(self.connection, False)

    def key(self, name, down):
        symbol = self.x.XStringToKeysym(name.encode("ascii"))
        code = self.x.XKeysymToKeycode(self.connection, symbol)
        if not code:
            raise RuntimeError(f"No X keycode for {name}")
        self.xt.XTestFakeKeyEvent(self.connection, code, down, 0)

    def type_key(self, name):
        self.key(name, True)
        self.key(name, False)
        self.sync()

    def clear_line(self):
        self.key("Control_L", True)
        self.type_key("u")
        self.key("Control_L", False)
        self.sync()

    def wheel(self, button):
        self.xt.XTestFakeButtonEvent(self.connection, button, True, 0)
        self.xt.XTestFakeButtonEvent(self.connection, button, False, 0)
        self.sync()

    def pixels(self, window, rectangle):
        x, y, width, height = rectangle
        image = self.x.XGetImage(self.connection, window, x, y, width, height,
                                C.c_ulong(-1).value, 2)
        if not image:
            raise RuntimeError("XGetImage failed")
        try:
            return C.string_at(image.contents.data,
                               image.contents.bytes_per_line * image.contents.height)
        finally:
            self.x.XDestroyImage(image)

    def close_window(self, window):
        event = XEvent()
        event.client.type = 33  # ClientMessage
        event.client.display = self.connection
        event.client.window = window
        event.client.message_type = self.x.XInternAtom(self.connection, b"WM_PROTOCOLS", False)
        event.client.format = 32
        event.client.data.l[0] = self.x.XInternAtom(self.connection, b"WM_DELETE_WINDOW", False)
        self.x.XSendEvent(self.connection, window, False, 0, C.byref(event))
        self.sync()

    def close(self):
        self.x.XCloseDisplay(self.connection)


def process_stats(root):
    """CPU jiffies and current RSS for the process and all live descendants."""
    records = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            fields = (entry / "stat").read_text().rsplit(")", 1)[1].split()
            if fields[0] != "Z":
                records[int(entry.name)] = (int(fields[1]), int(fields[11]) + int(fields[12]),
                                             int(fields[21]) * os.sysconf("SC_PAGE_SIZE"))
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            pass
    selected = {root}
    while True:
        added = {pid for pid, (parent, _, _) in records.items() if parent in selected}
        if added <= selected:
            break
        selected |= added
    return {pid: records[pid][1:] for pid in selected if pid in records}


def cpu_interval(root, seconds, action=None):
    before = process_stats(root)
    last = dict(before)
    start = time.monotonic()
    if action:
        action()
    while time.monotonic() - start < seconds:
        time.sleep(min(0.1, max(0, seconds - (time.monotonic() - start))))
        last.update(process_stats(root))
    after = process_stats(root)
    last.update(after)
    elapsed = time.monotonic() - start
    ticks = sum(value[0] - before.get(pid, (0, 0))[0] for pid, value in last.items())
    main_ticks = last.get(root, (0, 0))[0] - before.get(root, (0, 0))[0]
    hz = os.sysconf("SC_CLK_TCK")
    return {"wallSeconds": elapsed, "mainCpuPercentOneCore": 100 * main_ticks / hz / elapsed,
            "treeCpuPercentOneCore": 100 * ticks / hz / elapsed,
            "treeRssMiBAtEnd": sum(value[1] for value in after.values()) / 1024 ** 2,
            "observedPids": sorted(last)}


def distribution(values):
    if not values:
        return {"samples": 0}
    ordered = sorted(values)
    return {"samples": len(values), "medianMs": statistics.median(values),
            "p95Ms": ordered[min(len(ordered) - 1, int(len(ordered) * .95))],
            "minMs": min(values), "maxMs": max(values), "valuesMs": values}


def first_change(display, window, rectangle, before, action, timeout=3):
    start = time.monotonic_ns()
    action()
    captures = 0
    while (time.monotonic_ns() - start) / 1e9 < timeout:
        captures += 1
        if display.pixels(window, rectangle) != before:
            return (time.monotonic_ns() - start) / 1e6, captures
        time.sleep(.002)
    raise TimeoutError("Terminal pixels did not change after injected input")


def run_mode(args, web):
    env = os.environ.copy()
    env.pop("WAYLAND_DISPLAY", None)
    env["GPUI_BACKEND"] = "x11"
    label = "cef-on" if web else "cef-off"
    log_path = args.output.with_name(args.output.stem + "-" + label + ".log")
    report = {"cef": web, "log": str(log_path), "backend": "X11",
              "backgroundWorkload": ("Animated Chromium fixture updates every 100ms"
                                     if web else "CEF disabled"),
              "idleDefinition": "No injected user input; configured background workload remains active"}
    command = [str(args.binary), *args.app_argument, *(["--web-spike"] if web else [])]
    display = Display()
    window = None
    descendants = set()
    with log_path.open("w") as log, tempfile.TemporaryDirectory(prefix="splitty-gpui-perf-") as temp:
        process = subprocess.Popen(command, env=env, stdout=log, stderr=log, start_new_session=True)
        try:
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError(f"App exited during startup: {process.returncode}; see {log_path}")
                found = subprocess.run([args.xdotool, "search", "--onlyvisible", "--pid", str(process.pid)],
                                       env=env, capture_output=True, text=True, timeout=3)
                for candidate in found.stdout.splitlines():
                    geometry = subprocess.check_output([args.xdotool, "getwindowgeometry", "--shell", candidate],
                                                       env=env, text=True, timeout=3)
                    fields = dict(line.split("=", 1) for line in geometry.splitlines() if "=" in line)
                    if int(fields["WIDTH"]) >= 1000:
                        window = int(candidate)
                        report["windowSize"] = [int(fields["WIDTH"]), int(fields["HEIGHT"])]
                        break
                if window is not None:
                    break
                time.sleep(.1)
            if window is None:
                raise TimeoutError("No owned probe window appeared")
            subprocess.run([args.xdotool, "windowfocus", str(window)], env=env, check=True, timeout=3)
            subprocess.run([args.xdotool, "mousemove", "--window", str(window), "180", "180"],
                           env=env, check=True, timeout=3)
            rectangle = (12, 70, 500, max(100, report["windowSize"][1] - 400))
            report["captureRectangle"] = rectangle
            time.sleep(args.warmup)
            # Browser startup can change focus; restore the terminal explicitly
            # after initialization, before measuring either implementation.
            subprocess.run([args.xdotool, "windowfocus", str(window), "mousemove", "--window",
                            str(window), "180", "180", "click", "1"], env=env, check=True, timeout=3)
            time.sleep(.2)
            report["idle"] = cpu_interval(process.pid, args.idle_seconds)
            captures = []
            for _ in range(10):
                start = time.monotonic_ns()
                display.pixels(window, rectangle)
                captures.append((time.monotonic_ns() - start) / 1e6)
            report["captureCost"] = distribution(captures)
            latencies = []
            for _ in range(args.samples):
                display.clear_line()
                time.sleep(.08)
                before = display.pixels(window, rectangle)
                latency, _ = first_change(display, window, rectangle, before, lambda: display.type_key("a"))
                latencies.append(latency)
                time.sleep(.08)
            report["typingToFirstPixelChange"] = distribution(latencies)
            display.clear_line()
            time.sleep(.1)
            done = Path(temp) / "output-complete"
            shell = ("i=0; while [ \"$i\" -lt " + str(args.output_lines)
                     + " ]; do printf 'native output %06d abcdefghijklmnopqrstuvwxyz 0123456789\\n' \"$i\"; "
                     + "i=$((i+1)); done; printf done > " + str(done))
            subprocess.run([args.xdotool, "type", "--clearmodifiers", "--delay", "5", shell],
                           env=env, check=True, timeout=15)
            time.sleep(.15)
            output_started = time.monotonic()

            def output_work():
                display.type_key("Return")
                while not done.exists():
                    if process.poll() is not None or time.monotonic() - output_started > 30:
                        raise TimeoutError("Output fixture did not finish")
                    time.sleep(.01)
                report["outputProducerSeconds"] = time.monotonic() - output_started
                # Wait for the observed terminal region to settle after producer
                # completion. This is explicitly an observable settling metric,
                # not proof of an exact final-frame presentation timestamp.
                previous = display.pixels(window, rectangle)
                stable_since = time.monotonic()
                deadline = stable_since + 15
                while time.monotonic() < deadline:
                    time.sleep(.02)
                    current = display.pixels(window, rectangle)
                    if current != previous:
                        previous, stable_since = current, time.monotonic()
                    if time.monotonic() - stable_since >= .15:
                        break
                else:
                    raise TimeoutError("Terminal pixels did not settle after output completed")
                report["outputObservedSettledSeconds"] = time.monotonic() - output_started

            report["output"] = cpu_interval(process.pid, 0, output_work)
            report["outputLines"] = args.output_lines
            time.sleep(.3)
            scroll_times = []
            for _ in range(args.samples):
                before = display.pixels(window, rectangle)
                latency, _ = first_change(display, window, rectangle, before, lambda: display.wheel(4))
                scroll_times.append(latency)
                time.sleep(.08)
            report["scrollToFirstPixelChange"] = distribution(scroll_times)
            report["idleAfterInteraction"] = cpu_interval(process.pid, args.idle_seconds)
            descendants = set(process_stats(process.pid)) - {process.pid}
            report["completed"] = True
        except Exception as error:
            report["completed"] = False
            report["error"] = str(error)
        finally:
            descendants |= set(process_stats(process.pid)) - {process.pid}
            if process.poll() is None and window is not None:
                display.close_window(window)
            try:
                process.wait(timeout=15)
                report["normalWindowClose"] = True
            except subprocess.TimeoutExpired:
                report["normalWindowClose"] = False
                os.killpg(process.pid, signal.SIGTERM)
                process.wait(timeout=5)
            for pid in descendants:
                if Path(f"/proc/{pid}/stat").exists():
                    try:
                        fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
                        if fields[0] != "Z":
                            report.setdefault("survivingChildren", []).append(pid)
                            os.kill(pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
            report["exitCode"] = process.returncode
            display.close()
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--xdotool", default="xdotool")
    parser.add_argument("--app-argument", action="append", default=[])
    parser.add_argument("--modes", choices=["off", "on", "both"], default="both")
    parser.add_argument("--warmup", type=float, default=3)
    parser.add_argument("--idle-seconds", type=float, default=5)
    parser.add_argument("--samples", type=int, default=20)
    parser.add_argument("--output-lines", type=int, default=12000)
    parser.add_argument("--renderer", required=True, help="Verified device/backend description")
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    results = {"binary": str(args.binary), "sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest(),
               "recordedAtUtc": datetime.now(timezone.utc).isoformat(),
               "display": os.environ.get("DISPLAY"), "logicalCpus": os.cpu_count(),
               "platform": platform.platform(), "renderer": args.renderer, "profile": "release (caller verified)",
               "latencyDefinition": "XTest request to first changed X11 terminal-buffer pixels; includes X server/capture cost; not physical display latency",
               "cpuDefinition": "Main process and observed live descendant CPU, 100% = one logical core; harness/X server excluded",
               "runs": []}
    modes = [False, True] if args.modes == "both" else [args.modes == "on"]
    for web in modes:
        run = run_mode(args, web)
        results["runs"].append(run)
        args.output.write_text(json.dumps(results, indent=2) + "\n")
        print(json.dumps(run), flush=True)
    if not all(run.get("completed") and run.get("normalWindowClose") and run.get("exitCode") == 0
               and not run.get("survivingChildren") for run in results["runs"]):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
