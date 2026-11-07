#!/usr/bin/env python3
"""Exercise captured-tab Screen transfer in an owned native Xvfb workspace.

The read-only owner socket verifies authoritative routes and sessions. A raw
PTY fixture proves that transfer retains the actual process, output and query.
This script uses an isolated config/display and never opens the user's shell.
"""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import signal
import stat
import subprocess
import sys
import time


HERE = Path(__file__).resolve().parent


def helper(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


performance = helper("performance", "gpui-performance.py")
pane_drag = helper("pane_drag", "gpui-pane-drag.py")
SGR_RIGHT = re.compile(rb"\x1b\[<2;\d+;\d+[Mm]")


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


def slots(node):
    if node["type"] == "slot":
        return [node]
    return [slot for child in node["children"] for slot in slots(child)]


def selected(screen):
    current = [slot for slot in slots(screen["root"]) if slot.get("activePaneId")]
    return [slot["activePaneId"] for slot in current]


def route(state, pane):
    return next((screen_id for screen_id, screen in state["screens"].items()
                 if pane in screen["panes"]), None)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--display", default=":97")
    parser.add_argument("--xvfb", default="Xvfb")
    parser.add_argument("--xvfb-lib", type=Path)
    parser.add_argument("--xdotool", default="xdotool")
    parser.add_argument("--xclip", type=Path, help="private display clipboard reader")
    parser.add_argument("--visible-trigger-x", type=int, default=0,
                        help="calibrated X coordinate of the first tab's visible ⋯ button")
    parser.add_argument("--setup-only", action="store_true", help="capture initial layout without transfer actions")
    args = parser.parse_args()
    if not args.setup_only and not args.xclip:
        parser.error("full transfer acceptance requires --xclip for exact selection and query checks")
    binary = args.binary.resolve(strict=True)
    args.output.mkdir(parents=True, exist_ok=False)
    owned = args.output / "fixture"
    owned.mkdir(mode=0o700)
    config = args.output / "config"
    config.mkdir(mode=0o700)
    fixture = HERE / "gpui-screen-transfer-fixture.py"
    (config / "config.toml").write_text(
        '[appearance]\ntheme_mode = "fixed"\ntheme = "splitty-dark"\nfont_size = 0\n\n'
        '[shells]\ndefault_profile_id = "screen-transfer-fixture"\n\n'
        '[[shells.profiles]]\nid = "screen-transfer-fixture"\nname = "Transfer fixture"\n'
        f'command = {json.dumps(str(Path(sys.executable).resolve()))}\n'
        f'args = {json.dumps(["-u", str(fixture.resolve()), str(owned.resolve())])}\n'
        'enabled = true\ntarget = "local"\nsource = "custom"\n')
    env = os.environ.copy()
    env.pop("WAYLAND_DISPLAY", None)
    env["DISPLAY"] = args.display
    env["SPLITTY_CONFIG_DIR"] = str(config.resolve())
    for key in ("SPLITTY_SOCK", "SPLITTY_AGENT_SOCK", "SPLITTY_ADDR", "SPLITTY_TOKEN",
                "SPLITTY_SERVER", "SPLITTY_PANE_ID", "SPLITTY_SCREEN_ID", "SPLITTY_SESSION_ID"):
        env.pop(key, None)
    xvfb_env = env.copy()
    if args.xvfb_lib:
        xvfb_env["LD_LIBRARY_PATH"] = str(args.xvfb_lib) + (
            ":" + env["LD_LIBRARY_PATH"] if env.get("LD_LIBRARY_PATH") else "")
    result = {"binary": str(binary), "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "harnessSha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "fixtureSha256": hashlib.sha256(fixture.read_bytes()).hexdigest(),
              "display": args.display, "checks": [], "screenshots": [], "ptyEvents": {},
              "states": {}, "passed": False}
    xvfb = app = display = window = None
    sequences = {}

    def x(*arguments):
        return subprocess.check_output([str(args.xdotool), *map(str, arguments)], env=env,
                                       text=True, timeout=12)

    def identities():
        return {entry["pid"]: entry for path in owned.glob("ready-*.json")
                if (entry := json.loads(path.read_text()))}

    def raw():
        return {pid: (owned / f"input-{pid}.bin").read_bytes() for pid in identities()}

    def no_input(label, action):
        before = raw()
        action()
        time.sleep(.2)
        after = raw()
        delta = {pid: value[len(before.get(pid, b"")):] for pid, value in after.items()
                 if value != before.get(pid, b"")}
        result["ptyEvents"][label] = {str(pid): value.hex() for pid, value in delta.items()}
        assert not delta, f"{label} leaked terminal input: {delta}"

    def chord(*keys):
        for modifier in keys[:-1]:
            display.key(modifier, True)
        display.type_key(keys[-1])
        for modifier in reversed(keys[:-1]):
            display.key(modifier, False)
        display.sync()

    def click(x_pos, y_pos, button=1):
        x("mousemove", "--window", window, x_pos, y_pos)
        display.sync()
        x("click", button)
        display.sync()
        time.sleep(.25)

    def capture(label):
        path = args.output / f"{label}.png"
        pane_drag.save_png(path, display.pixels(window, (0, 0, 1200, 760)))
        result["screenshots"].append(str(path))

    def painted_marker(label, pid):
        # The fixture starts with two visible text rows and a blank fourth
        # row. Its emit action writes LIVE_<pid> on that fourth row. Sample
        # only that terminal area, excluding tab chrome, search and status.
        crop = (5, 135, 350, 65)
        time.sleep(.3)
        before = display.pixels(window, crop)
        command(pid, "emit")  # This acknowledges a PTY write, not a paint.

        def painted():
            after = display.pixels(window, crop)
            changed = sum(before[offset:offset + 3] != after[offset:offset + 3]
                          for offset in range(0, len(before), 4))
            return changed if changed > 100 else None

        changed = wait_until(painted, f"{label} was written but did not paint in terminal content")
        result.setdefault("paintedMarkerPixels", {})[label] = changed
        capture(label)

    def owned_state(label=None):
        sockets = list((config / "run").glob("spike-*.sock"))
        if len(sockets) != 1 or not stat.S_ISSOCK(sockets[0].lstat().st_mode):
            raise RuntimeError("expected exactly one socket inside the owned config")
        response = subprocess.check_output(
            ["curl", "--fail", "--silent", "--show-error", "--max-time", "5",
             "--unix-socket", str(sockets[0]), "http://localhost/api/mux/state"],
            text=True, timeout=7)
        state = json.loads(response)
        if label:
            result["states"][label] = state
        return state

    def command(pid, action):
        sequence = sequences.get(pid, 0) + 1
        sequences[pid] = sequence
        next_path = owned / f"control-{pid}.next"
        next_path.write_text(json.dumps({"seq": sequence, "action": action}))
        next_path.replace(owned / f"control-{pid}.json")
        if action != "stop":
            wait_until(lambda: json.loads((owned / f"ack-{pid}.json").read_text()).get("seq") == sequence
                       if (owned / f"ack-{pid}.json").exists() else False,
                       f"fixture {pid} did not acknowledge {action}")

    def last_size(pid):
        path = owned / f"sizes-{pid}.jsonl"
        return json.loads(path.read_text().splitlines()[-1]) if path.exists() and path.stat().st_size else None

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

            wait_until(ready, "private Xvfb did not become ready", seconds=20)
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
                    raise RuntimeError(f"app exited before opening window: {app.returncode}")
                found = subprocess.run([str(args.xdotool), "search", "--onlyvisible", "--pid", str(app.pid)],
                                       env=env, capture_output=True, text=True, timeout=3)
                for candidate in found.stdout.splitlines():
                    geometry = dict(line.split("=", 1) for line in x("getwindowgeometry", "--shell", candidate).splitlines()
                                    if "=" in line)
                    if geometry.get("WIDTH") == "1200" and geometry.get("HEIGHT") == "760":
                        return int(candidate)

            window = wait_until(find_window, "no 1200x760 native window", seconds=30)
            x("windowfocus", window)
            a = wait_until(lambda: next((entry for entry in identities().values() if alive(entry["pid"])), None),
                           "initial fixture did not launch", seconds=30)
            a_id = owned_state("initial")["activeScreenId"]
            assert route(owned_state(), a["pane"]) == a_id
            capture("initial-screen-a")

            no_input("create-second-a-tab", lambda: chord("Alt_L", "t"))
            c = wait_until(lambda: next((entry for entry in identities().values()
                                         if entry["pid"] != a["pid"] and alive(entry["pid"])), None),
                           "second A tab did not launch")
            wait_until(lambda: route(owned_state(), c["pane"]) == a_id,
                       "second tab not in source Screen")
            assert c["session"] != a["session"]
            capture("two-tabs-on-source")

            no_input("source-find-query", lambda: (chord("Control_L", "f"),
                     x("type", "--clearmodifiers", "--delay", "2", f'HISTORY_{c["pid"]}')))
            capture("source-history-find")
            if args.xclip:
                no_input("source-copy-query", lambda: (chord("Control_L", "a"), chord("Control_L", "c")))
                copied = subprocess.check_output([str(args.xclip), "-selection", "clipboard", "-o"],
                                                 env=env, timeout=8)
                assert copied == f'HISTORY_{c["pid"]}'.encode(), copied
            no_input("close-source-find", lambda: display.type_key("Escape"))
            if args.xclip:
                def select_source_word():
                    display.key("Shift_L", True)
                    x("mousemove", "--window", window, 62, 91)
                    display.sync()
                    x("click", "--repeat", 2, "--delay", 80, 1)
                    display.key("Shift_L", False)
                    display.sync()
                before_selection = raw()[c["pid"]]
                select_source_word()
                selection_delta = raw()[c["pid"]][len(before_selection):]
                assert not re.search(rb"\x1b\[<(?:0|2|32|34);", selection_delta), (
                    "Shift selection sent a press or drag to the app", selection_delta)
                result["ptyEvents"]["select-source-word"] = {str(c["pid"]): selection_delta.hex()}
                capture("source-selection")
                no_input("copy-source-selection", lambda: chord("Control_L", "Shift_L", "c"))
                copied = subprocess.check_output([str(args.xclip), "-selection", "clipboard", "-o"],
                                                 env=env, timeout=8)
                assert copied == f'PANE_{c["pid"]}'.encode(), ("source selection mismatch", copied)
                result["checks"].append("source-terminal-selection-copied-before-transfer")
            result["checks"].append("second-source-tab-has-retained-history-and-find-query")

            no_input("create-screen-b", lambda: chord("Control_L", "Shift_L", "space"))
            b = wait_until(lambda: next((entry for entry in identities().values()
                                         if entry["pid"] not in (a["pid"], c["pid"]) and alive(entry["pid"])), None),
                           "target Screen fixture did not launch")
            # The owner service's activeScreenId is a server hint. Native
            # Screen navigation is client-local, so route by the new pane.
            b_id = wait_until(lambda: route(owned_state(), b["pane"]),
                              "target Screen did not own its new pane")
            assert b_id != a_id
            capture("existing-target-screen-b")
            no_input("split-target-screen-b", lambda: chord("Alt_L", "d"))
            d = wait_until(lambda: next((entry for entry in identities().values()
                                         if entry["pid"] not in (a["pid"], b["pid"], c["pid"])
                                         and alive(entry["pid"])), None),
                           "target Screen did not create a second group")
            wait_until(lambda: owned_state()["screens"][b_id]["root"]["type"] == "split",
                       "target Screen did not settle its split")
            c_source_size = wait_until(lambda: last_size(c["pid"]), "source PTY size missing")
            b_split_size = wait_until(lambda: last_size(b["pid"]), "split target PTY size missing")
            assert b_split_size["cols"] < c_source_size["cols"], (b_split_size, c_source_size)
            capture("target-screen-b-different-geometry")
            result["checks"].append("existing-target-has-different-terminal-geometry")
            no_input("return-to-source-a", lambda: chord("Alt_L", "Shift_L", "1"))
            capture("returned-to-source-a")
            click(45, 62)
            wait_until(lambda: a["pane"] in selected(owned_state()["screens"][a_id]),
                       "source first tab was not selected")
            assert c["pane"] not in selected(owned_state()["screens"][a_id])
            capture("nonselected-source-tab")
            result["checks"].append("nonselected-source-captured-before-menu")

            if args.setup_only:
                result["checks"].append("setup-only-geometry-and-owned-fixture")
            else:
                # Menu coordinates are calibrated against the frozen build's
                # screenshots; state checks below reject wrong-tab transfers.
                before_menu = owned_state("before-dismiss")
                no_input("open-nonselected-context-menu", lambda: click(180, 62, 3))
                capture("nonselected-context-menu")
                no_input("dismiss-transfer-menu", lambda: display.type_key("Escape"))
                capture("dismissed-menu")
                after_dismiss = owned_state("after-dismiss")
                assert after_dismiss["screens"][a_id]["root"] == before_menu["screens"][a_id]["root"]
                assert route(after_dismiss, c["pane"]) == a_id
                assert a["pane"] in selected(after_dismiss["screens"][a_id])
                result["checks"].append("context-menu-dismissal-keeps-nonselected-source-and-route")

                if not args.visible_trigger_x:
                    raise ValueError("full transfer acceptance requires --visible-trigger-x")
                no_input("open-nonselected-visible-affordance",
                         lambda: click(args.visible_trigger_x, 62))
                capture("nonselected-visible-affordance-menu")
                no_input("dismiss-nonselected-visible-menu", lambda: display.type_key("Escape"))
                after_visible_dismiss = owned_state("after-visible-dismiss")
                assert after_visible_dismiss["screens"][a_id]["root"] == before_menu["screens"][a_id]["root"]
                assert route(after_visible_dismiss, c["pane"]) == a_id
                assert a["pane"] in selected(after_visible_dismiss["screens"][a_id])
                capture("nonselected-visible-menu-dismissed")
                result["checks"].append("visible-menu-dismissal-keeps-nonselected-source-and-route")

                no_input("open-nonselected-for-transfer", lambda: click(180, 62, 3))
                capture("nonselected-menu-before-existing-transfer")
                no_input("choose-existing-target", lambda: display.type_key("Return"))
                wait_until(lambda: route(owned_state(), c["pane"]) == b_id,
                           "captured nonselected tab did not transfer to Screen B")
                state = owned_state("after-existing-transfer")
                assert a["pane"] in state["screens"][a_id]["panes"]
                assert c["pane"] not in state["screens"][a_id]["panes"]
                assert state["screens"][b_id]["panes"][c["pane"]]["sessionId"] == c["session"]
                wait_until(lambda: last_size(c["pid"]) != c_source_size,
                           "transferred PTY did not claim target's smaller geometry")
                assert last_size(c["pid"])["cols"] < c_source_size["cols"]
                assert all(alive(entry["pid"]) for entry in (a, b, c, d))
                capture("existing-transfer-followed")
                result["checks"].append("nonselected-tab-transfers-to-existing-screen-with-same-session-and-new-size")

                if args.xclip:
                    no_input("recopy-transferred-selection", lambda: chord("Control_L", "Shift_L", "c"))
                    copied = subprocess.check_output([str(args.xclip), "-selection", "clipboard", "-o"],
                                                     env=env, timeout=8)
                    assert copied == f'PANE_{c["pid"]}'.encode(), (
                        "transferred terminal lost local selection", copied)
                    result["checks"].append("transferred-terminal-selection-retained")

                no_input("reopen-transferred-find", lambda: chord("Control_L", "f"))
                capture("transferred-history-find")
                if args.xclip:
                    no_input("copy-transferred-query", lambda: (chord("Control_L", "a"), chord("Control_L", "c")))
                    copied = subprocess.check_output([str(args.xclip), "-selection", "clipboard", "-o"],
                                                     env=env, timeout=8)
                    assert copied == f'HISTORY_{c["pid"]}'.encode(), (
                        "the followed transferred terminal lost its search editor state", copied)
                    result["checks"].append("transferred-search-query-retained-on-same-terminal")
                no_input("close-transferred-find", lambda: display.type_key("Escape"))
                painted_marker("transferred-live-output", c["pid"])
                result["checks"].append("transferred-pty-output-continues")

                # The visible ⋯ affordance is tested on the selected moved tab.
                # Its X is read from the frozen candidate screenshot once.
                no_input("open-visible-transfer-affordance",
                         lambda: click(args.visible_trigger_x, 62))
                capture("visible-affordance-menu")
                no_input("choose-new-screen", lambda: (
                    display.type_key("Down"), display.type_key("Return")))
                c_id = wait_until(lambda: route(owned_state(), c["pane"])
                                  if route(owned_state(), c["pane"]) not in (a_id, b_id) else None,
                                  "New Screen transfer did not create a distinct destination")
                state = owned_state("after-new-screen-transfer")
                assert state["screens"][c_id]["panes"][c["pane"]]["sessionId"] == c["session"]
                assert c["pane"] not in state["screens"][b_id]["panes"]
                assert all(alive(entry["pid"]) for entry in (a, b, c, d))
                capture("new-screen-transfer-followed")
                result["checks"].append("visible-affordance-transfers-same-session-to-new-screen")

                no_input("return-to-final-source-a", lambda: chord("Alt_L", "Shift_L", "1"))
                capture("source-a-before-final-transfer")
                state = owned_state()
                assert list(state["screens"][a_id]["panes"]) == [a["pane"]]
                no_input("open-final-source-menu", lambda: click(50, 62, 3))
                capture("final-source-menu")
                no_input("choose-b-for-final-source", lambda: display.type_key("Return"))
                wait_until(lambda: route(owned_state(), a["pane"]) == b_id
                           and a_id not in owned_state()["screens"],
                           "last source tab did not move and remove emptied Screen A")
                state = owned_state("after-final-source-removal")
                assert state["screens"][b_id]["panes"][a["pane"]]["sessionId"] == a["session"]
                assert state["screenOrder"] == [b_id, c_id], state["screenOrder"]
                assert all(alive(entry["pid"]) for entry in (a, b, c, d))
                time.sleep(.5)
                capture("final-source-screen-removed")
                result["checks"].append("final-source-removal-preserves-all-sessions-and-catalog-order")

                before_pointer = raw()[a["pid"]]
                click(300, 200, 3)
                wait_until(lambda: len(raw()[a["pid"]]) > len(before_pointer),
                           "terminal-body right click did not reach the followed PTY")
                pointer_report = raw()[a["pid"]][len(before_pointer):]
                assert SGR_RIGHT.search(pointer_report), pointer_report
                result["ptyEvents"]["terminal-body-right-click"] = pointer_report.hex()
                result["checks"].append("terminal-body-right-click-still-routes-to-application")

                painted_marker("final-source-live-output", a["pid"])
                result["checks"].append("final-source-pty-output-continues-after-screen-removal")
                result["identities"] = {label: entry for label, entry in
                                        (("a", a), ("b", b), ("c", c), ("target-split", d))}

            for pid in identities():
                if alive(pid):
                    command(pid, "stop")
            wait_until(lambda: all(not alive(pid) for pid in identities()),
                       "owned fixture did not exit normally", seconds=8)
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
                except Exception as close_error:
                    result["closeError"] = repr(close_error)
            if app:
                try:
                    app.wait(timeout=2)
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
            result["survivingChildren"] = [pid for pid in descendants if alive(pid)]
            result["passed"] = result["passed"] and result.get("exitCode") == 0 and not result["survivingChildren"]
            (args.output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps({"passed": result["passed"], "checks": result["checks"],
                              "error": result.get("error"), "result": str(args.output / "result.json")}, indent=2))
    if not result["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
