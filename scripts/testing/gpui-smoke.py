#!/usr/bin/env python3
"""Run the built GPUI probe on real Wayland/X11 windows and retain evidence."""

import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys

TIMEOUT_SECONDS = 45


def records(path):
    found = []
    for line in path.read_text(errors="replace").splitlines():
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            found.append(value)
    return found


def run(binary, evidence_dir, backend, web_spike, screenshot_requested, workspace=False):
    stdout = evidence_dir / f"{backend}.stdout.log"
    stderr = evidence_dir / f"{backend}.stderr.log"
    screenshot = evidence_dir / f"{backend}.png"
    screenshot.unlink(missing_ok=True)  # A prior run cannot satisfy this run.
    env = os.environ.copy()
    if backend == "wayland":
        env.pop("DISPLAY", None)
    else:
        env.pop("WAYLAND_DISPLAY", None)
    command = [str(binary), "--workspace-smoke" if workspace else "--smoke"]
    if screenshot_requested:
        command.extend(["--screenshot", str(screenshot)])
    if web_spike:
        command.append("--web-spike")

    timed_out = False
    launch_error = None
    with stdout.open("wb") as out, stderr.open("wb") as err:
        try:
            process = subprocess.Popen(
                command, env=env, stdout=out, stderr=err, start_new_session=True
            )
        except OSError as error:
            launch_error = str(error)
            exit_code = None
        else:
            try:
                exit_code = process.wait(timeout=TIMEOUT_SECONDS)
            except subprocess.TimeoutExpired:
                timed_out = True
                # The new session owns the probe and any CEF children it launched.
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    process.wait()
                exit_code = process.returncode

    parsed = records(stdout) + records(stderr)
    probe_name = "splitty-local-workspace" if workspace else "splitty-gpui"
    probes = [item for item in parsed if item.get("probe") == probe_name]
    shutdowns = [item for item in parsed if "probeShutdown" in item]
    graphics = [item["nativeGraphics"] for item in parsed if "nativeGraphics" in item]
    probe = probes[0] if len(probes) == 1 else None
    shutdown = shutdowns[0] if len(shutdowns) == 1 else None
    image_bytes = screenshot.stat().st_size if screenshot.is_file() else 0
    passed = (
        not timed_out
        and exit_code == 0
        and probe is not None
        and probe.get("passed") is True
        and shutdown is not None
        and shutdown.get("probeShutdown") == "complete"
        and (not screenshot_requested or image_bytes > 0)
    )
    return {
        "backend": backend,
        "passed": passed,
        "exitCode": exit_code,
        "timedOut": timed_out,
        "launchError": launch_error,
        "probe": probe,
        "graphics": graphics[-1] if graphics else None,
        "probeShutdown": shutdown.get("probeShutdown") if shutdown else None,
        "evidenceKind": "runtime-log-and-screenshot" if image_bytes else "runtime-log",
        "screenshotRequested": screenshot_requested,
        "screenshot": str(screenshot) if screenshot_requested else None,
        "screenshotBytes": image_bytes,
        "stdout": str(stdout),
        "stderr": str(stderr),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--evidence-dir", type=Path, required=True)
    parser.add_argument("--backend", choices=("wayland", "x11", "both"), required=True)
    parser.add_argument("--web-spike", action="store_true")
    parser.add_argument("--workspace", action="store_true", help="Check the local screen with splits and tabs")
    parser.add_argument("--screenshot", action="store_true",
                        help="Require a native PNG from a capture-enabled binary")
    args = parser.parse_args()
    if args.workspace and (args.web_spike or args.screenshot):
        parser.error("--workspace cannot be combined with --web-spike or --screenshot")

    binary = args.binary.resolve()
    evidence_dir = args.evidence_dir.resolve()
    evidence_dir.mkdir(parents=True, exist_ok=True)
    backends = ["wayland", "x11"] if args.backend == "both" else [args.backend]
    result = {"binary": str(binary), "webSpike": args.web_spike,
              "screenshotRequested": args.screenshot, "workspace": args.workspace,
              "runs": []}
    if not binary.is_file() or not os.access(binary, os.X_OK):
        result["error"] = "--binary must name an executable file"
    else:
        unavailable = [
            backend for backend in backends
            if not os.environ.get("WAYLAND_DISPLAY" if backend == "wayland" else "DISPLAY")
        ]
        if unavailable:
            result["error"] = f"Display unavailable for: {', '.join(unavailable)}"
        else:
            for backend in backends:
                result["runs"].append(run(binary, evidence_dir, backend,
                                          args.web_spike, args.screenshot, args.workspace))
    result["evidenceKind"] = (
        "runtime-log-and-screenshot" if result["runs"] and all(
            item["screenshotBytes"] > 0 for item in result["runs"]
        ) else "runtime-log"
    )
    result["passed"] = len(result["runs"]) == len(backends) and all(
        item["passed"] for item in result["runs"]
    )
    summary = evidence_dir / "summary.json"
    summary.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"passed": result["passed"], "summary": str(summary)}))
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
