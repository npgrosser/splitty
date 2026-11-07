#!/usr/bin/env python3
"""Measure normal Workspace key-to-response pixels on one isolated Xvfb display.

The response is a unique sequence number drawn by a quiet raw-PTY fixture.
Only one key is outstanding. Latency includes XTest dispatch, PTY/service/UI
work, renderer/X11 work and XGetImage sampling; it is not display presentation.
Run the same script, fixture, crop, display and sample count for both binaries.
"""
import argparse
import ctypes
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import platform
import random
import shlex
import signal
import statistics
import subprocess
import time

HERE = Path(__file__).parent

def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value

def distribution(values):
    ordered = sorted(values)
    def percentile(fraction):
        return ordered[max(0, math.ceil(len(ordered) * fraction) - 1)]
    return {"samples": len(values), "p50Ms": statistics.median(values),
            "p95Ms": percentile(.95), "p99Ms": percentile(.99),
            "minMs": ordered[0], "maxMs": ordered[-1],
            "meanMs": statistics.mean(values), "valuesMs": values}

def wait_until(predicate, label, seconds=12):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.005)
    raise TimeoutError(label)

def alive(pid):
    try:
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0] != "Z"
    except (FileNotFoundError, ProcessLookupError, PermissionError):
        return False

def terminate_owned_group(process, label, issues):
    """Stop only the process group this harness launched, escalating if needed."""
    if process.poll() is None:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        except OSError as error:
            issues.append(f"{label} SIGTERM: {error}")
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        except OSError as error:
            issues.append(f"{label} SIGKILL: {error}")
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            issues.append(f"{label} did not exit after SIGKILL")
        except OSError as error:
            issues.append(f"{label} wait after SIGKILL: {error}")
    except OSError as error:
        issues.append(f"{label} wait: {error}")

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--samples", type=int, default=120)
    parser.add_argument("--idle-seconds", type=float, default=5)
    parser.add_argument("--display", default=":95")
    parser.add_argument("--crop", default="25,225,320,110", help="window-relative x,y,width,height around fixture row 10")
    parser.add_argument("--xdotool", default="xdotool")
    parser.add_argument("--xvfb", default="Xvfb")
    parser.add_argument("--xvfb-lib", type=Path)
    args = parser.parse_args()
    if args.samples < 100:
        parser.error("at least 100 samples are required for a useful p99")
    crop = tuple(map(int, args.crop.split(",")))
    if len(crop) != 4 or crop[2] < 1 or crop[3] < 1:
        parser.error("--crop must be x,y,width,height")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    binary = args.binary.resolve(strict=True)
    fixture = HERE / "gpui-latency-fixture.py"
    helper_dir = HERE
    performance = load("performance", helper_dir / "gpui-performance.py")
    pane_drag = load("pane_drag", helper_dir / "gpui-pane-drag.py")
    env = os.environ.copy()
    env.pop("WAYLAND_DISPLAY", None)
    env["DISPLAY"] = args.display
    env["GPUI_BACKEND"] = "x11"
    xvfb_env = env.copy()
    if args.xvfb_lib:
        xvfb_env["LD_LIBRARY_PATH"] = str(args.xvfb_lib) + (":" + env["LD_LIBRARY_PATH"] if env.get("LD_LIBRARY_PATH") else "")
    result = {
        "binary": str(binary), "binarySha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "fixtureSha256": hashlib.sha256(fixture.read_bytes()).hexdigest(),
        "harnessSha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "recordedAtUtc": datetime.now(timezone.utc).isoformat(),
        "platform": platform.platform(), "logicalCpus": os.cpu_count(),
        "mode": "normal Workspace (no --probe/--workspace-smoke)", "display": args.display,
        "crop": crop, "samplesRequested": args.samples,
        "latencyDefinition": "XTest key request to first changed terminal-response crop via XGetImage; includes X11 sampling, not physical presentation",
        "cpuDefinition": "app and live descendant CPU; Xvfb and harness excluded; 100% is one logical core",
    }
    delay_seed = 0x5EED2026
    delay_random = random.Random(delay_seed)
    inter_key_delays = [delay_random.uniform(.020, .045) for _ in range(args.samples)]
    result["interKeyDelaySeed"] = delay_seed
    result["interKeyDelaysMs"] = [delay * 1000 for delay in inter_key_delays]
    server = app = display = window = None

    def x(*arguments):
        return subprocess.check_output([args.xdotool, *map(str, arguments)], env=env,
                                       text=True, timeout=10)

    with (output / "xvfb.log").open("w") as server_log, (output / "app.log").open("w") as app_log:
        try:
            display_number = args.display.lstrip(":")
            if (Path(f"/tmp/.X{display_number}-lock").exists() or
                    Path(f"/tmp/.X11-unix/X{display_number}").exists()):
                raise RuntimeError(f"DISPLAY {args.display} already in use")
            server = subprocess.Popen([args.xvfb, args.display, "-screen", "0", "1280x900x24",
                                       "-nolisten", "tcp", "-noreset", "-ac", "-xkbdir", "/usr/share/X11/xkb"],
                                      env=xvfb_env, stdout=server_log, stderr=server_log,
                                      start_new_session=True)
            def server_ready():
                if server.poll() is not None:
                    raise RuntimeError(f"Xvfb exited {server.returncode}")
                try:
                    return subprocess.run([args.xdotool, "getdisplaygeometry"], env=env,
                                          capture_output=True, timeout=2).returncode == 0
                except subprocess.TimeoutExpired:
                    return False
            wait_until(server_ready, "Xvfb not ready", 20)
            prior_display = os.environ.get("DISPLAY")
            os.environ["DISPLAY"] = args.display
            try:
                display = performance.Display()
            finally:
                if prior_display is None:
                    os.environ.pop("DISPLAY", None)
                else:
                    os.environ["DISPLAY"] = prior_display
            app = subprocess.Popen([str(binary)], env=env, stdout=app_log, stderr=app_log,
                                   start_new_session=True)
            def find_window():
                if app.poll() is not None:
                    raise RuntimeError(f"app exited before opening window: {app.returncode}")
                found = subprocess.run([args.xdotool, "search", "--onlyvisible", "--pid", str(app.pid)],
                                       env=env, capture_output=True, text=True)
                for candidate in found.stdout.splitlines():
                    fields = dict(line.split("=", 1) for line in x("getwindowgeometry", "--shell", candidate).splitlines() if "=" in line)
                    if fields.get("WIDTH") == "1200" and fields.get("HEIGHT") == "760":
                        return int(candidate)
                return None
            window = wait_until(find_window, "normal Workspace window not found", 30)
            x("windowfocus", window)
            x("mousemove", "--window", window, 300, 180, "click", 1)
            time.sleep(2)
            geometry = dict(line.split("=", 1) for line in x("getwindowgeometry", "--shell", window).splitlines() if "=" in line)
            origin = int(geometry["X"]), int(geometry["Y"])
            display.x.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
            display.x.XDefaultRootWindow.restype = ctypes.c_ulong
            root = display.x.XDefaultRootWindow(display.connection)
            def pixels(rectangle):
                left, top, width, height = rectangle
                return display.pixels(root, (origin[0] + left, origin[1] + top, width, height))
            def screenshot(name):
                path = output / f"{name}.png"
                pane_drag.save_png(path, pixels((0, 0, 1200, 760)))
                result.setdefault("screenshots", []).append(str(path))

            for name in ("ready", "ack.json", "ack.tmp", "done.json"):
                (output / name).unlink(missing_ok=True)
            x("type", "--clearmodifiers", "--delay", 2,
              f"python3 {shlex.quote(str(fixture))} {shlex.quote(str(output))}")
            display.type_key("Return")
            wait_until(lambda: (output / "ready").exists(), "raw-PTY fixture did not start", 8)
            time.sleep(.6)
            screenshot("fixture-ready")
            result["idle"] = performance.cpu_interval(app.pid, args.idle_seconds)

            costs = []
            for _ in range(100):
                started = time.monotonic_ns()
                pixels(crop)
                costs.append((time.monotonic_ns() - started) / 1e6)
            result["captureCost"] = distribution(costs)

            latencies = []
            captures = []
            keys = []
            alphabet = "abcdefghijklmnopqrstuvwxyz0123456789".replace("q", "")
            for index in range(args.samples):
                key = alphabet[index % len(alphabet)]
                before = pixels(crop)
                started = time.monotonic_ns()
                display.type_key(key)
                count = 0
                while (time.monotonic_ns() - started) < 2_000_000_000:
                    count += 1
                    if pixels(crop) != before:
                        break
                    time.sleep(.0005)
                else:
                    raise TimeoutError(f"response pixels did not change for sequence {index + 1}")
                elapsed = (time.monotonic_ns() - started) / 1e6
                def ack_matches():
                    path = output / "ack.json"
                    if not path.exists():
                        return False
                    ack = json.loads(path.read_text())
                    if ack["seq"] > index + 1:
                        raise RuntimeError(f"fixture accepted overlapping input at {index + 1}: {ack}")
                    return ack == {"seq": index + 1, "key": key}
                wait_until(ack_matches, f"missing sequence-tagged fixture reply {index + 1}", 2)
                latencies.append(elapsed)
                captures.append(count)
                keys.append(key)
                time.sleep(inter_key_delays[index])
            result["inputToChangedResponsePixels"] = distribution(latencies)
            result["captureCallsPerSample"] = {"min": min(captures),
                                               "median": statistics.median(captures),
                                               "max": max(captures), "values": captures}
            edges = [0, 5, 10, 15, 20, 30, 50, 100, float("inf")]
            result["latencyHistogramMs"] = {
                f"{edges[i]}-{edges[i + 1]}": sum(edges[i] <= value < edges[i + 1] for value in latencies)
                for i in range(len(edges) - 1)
            }
            result["inputKeys"] = "".join(keys)
            screenshot("final-response")
            display.type_key("q")
            wait_until(lambda: (output / "done.json").exists(), "fixture did not exit", 5)
            done = json.loads((output / "done.json").read_text())
            assert done == {"count": args.samples, "keys": keys}, f"fixture input mismatch: {done}"
            # The shell may resume before PTY busy metadata reaches the UI.
            # This pause is after all measured samples.
            time.sleep(1)
            result["completed"] = True
        except Exception as error:
            result["completed"] = False
            result["error"] = str(error)
        finally:
            issues = []
            try:
                descendants = set(performance.process_stats(app.pid)) - {app.pid} if app else set()
            except Exception as error:
                descendants = set()
                issues.append(f"descendant inspection: {error}")
            if app:
                close_requested = False
                if app.poll() is None and window and display:
                    try:
                        display.close_window(window)
                        close_requested = True
                    except (OSError, RuntimeError) as error:
                        issues.append(f"window close request: {error}")
                forced = False
                if close_requested:
                    try:
                        app.wait(timeout=2)
                    except subprocess.TimeoutExpired:
                        try:
                            # A stale/busy close assessment may legitimately
                            # prompt. Confirm only the visible owned dialog.
                            center = pixels((600, 340, 1, 1))
                            if all(channel > 200 for channel in center[:3]):
                                x("mousemove", "--window", window, 600, 422, "click", 1)
                                result["cleanupPromptConfirmed"] = True
                            app.wait(timeout=13)
                        except subprocess.TimeoutExpired:
                            forced = True
                            terminate_owned_group(app, "app", issues)
                        except Exception as error:
                            forced = True
                            issues.append(f"app close/confirmation: {error}")
                            terminate_owned_group(app, "app", issues)
                    except OSError as error:
                        forced = True
                        issues.append(f"app wait: {error}")
                        terminate_owned_group(app, "app", issues)
                else:
                    forced = True
                    terminate_owned_group(app, "app", issues)
                result["exitCode"] = app.returncode
                result["normalWindowClose"] = close_requested and not forced and app.returncode == 0
                deadline = time.monotonic() + 2
                while any(alive(pid) for pid in descendants) and time.monotonic() < deadline:
                    time.sleep(.05)
                result["survivingChildren"] = [pid for pid in descendants if alive(pid)]
            if display:
                try:
                    display.close()
                except Exception as error:
                    issues.append(f"X11 display close: {error}")
            if server:
                try:
                    terminate_owned_group(server, "Xvfb", issues)
                except Exception as error:
                    issues.append(f"Xvfb group cleanup: {error}")
            if issues:
                result["cleanupErrors"] = issues
            (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"completed": result["completed"],
                      "result": str(output / "result.json"),
                      "error": result.get("error")}, indent=2))
    if not result.get("completed") or not result.get("normalWindowClose") or result.get("exitCode") != 0 or result.get("survivingChildren"):
        raise SystemExit(1)

if __name__ == "__main__":
    main()
