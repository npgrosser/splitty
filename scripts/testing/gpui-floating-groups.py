#!/usr/bin/env python3
"""Exercise native floating groups in an owned Xvfb workspace.

Uses the Screen-transfer raw PTY fixture and the isolated service's read-only
owner socket. The script never changes user settings or desktop focus.
"""

import argparse
import ctypes
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
import tomllib


HERE = Path(__file__).resolve().parent
FIXTURE = HERE / "gpui-screen-transfer-fixture.py"
SGR = re.compile(rb"\x1b\[<([0-9]+);([0-9]+);([0-9]+)([Mm])")


def helper(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


performance = helper("performance", "gpui-performance.py")
pane_drag = helper("pane_drag", "gpui-pane-drag.py")


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


def in_float(screen, pane):
    return next((entry for entry in screen["floating"]
                 if any(pane in slot["paneIds"] for slot in slots(entry["root"]))), None)


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
    parser.add_argument("--xclip", type=Path, help="private X11 clipboard reader")
    parser.add_argument("--setup-only", action="store_true", help="validate fixture and root tabs before Float Tab")
    parser.add_argument("--float-tab-x", type=int, default=0,
                        help="calibrated toolbar Float Tab X coordinate")
    args = parser.parse_args()
    if not args.setup_only and not args.xclip:
        parser.error("full floating acceptance requires --xclip")
    binary = args.binary.resolve(strict=True)
    args.output.mkdir(parents=True, exist_ok=False)
    owned = args.output / "fixture"
    owned.mkdir(mode=0o700)
    config = args.output / "config"
    config.mkdir(mode=0o700)
    config_file = config / "config.toml"
    config_file.write_text(
        '[appearance]\ntheme_mode = "fixed"\ntheme = "splitty-dark"\nfont_size = 0\n'
        'zoom = 1.0\nzoom_scope = "window"\n\n'
        '[shells]\ndefault_profile_id = "floating-fixture"\n\n'
        '[[shells.profiles]]\nid = "floating-fixture"\nname = "Floating fixture"\n'
        f'command = {json.dumps(str(Path(sys.executable).resolve()))}\n'
        f'args = {json.dumps(["-u", str(FIXTURE.resolve()), str(owned.resolve())])}\n'
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
              "fixtureSha256": hashlib.sha256(FIXTURE.read_bytes()).hexdigest(),
              "display": args.display, "checks": [], "screenshots": [], "states": {},
              "ptyEvents": {}, "paintedMarkerPixels": {}, "passed": False}
    xvfb = app = display = window = None
    sequences = {}

    def x(*arguments):
        return subprocess.check_output([str(args.xdotool), *map(str, arguments)], env=env,
                                       text=True, timeout=12)

    def identities():
        return {entry["pid"]: entry for path in owned.glob("ready-*.json")
                if (entry := json.loads(path.read_text()))}

    def fresh(exclude):
        return next((entry for entry in identities().values()
                     if entry["pid"] not in exclude and alive(entry["pid"])), None)

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
        assert not delta, f"{label} leaked PTY bytes: {delta}"

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

    def move(x_pos, y_pos):
        x("mousemove", "--window", window, x_pos, y_pos)
        display.sync()
        time.sleep(.12)

    def drag(start, end, cancel=None):
        x("mousemove", "--window", window, *start, "mousedown", 1)
        time.sleep(.15)
        x("mousemove", "--window", window, start[0] + 16, start[1] + 12)
        time.sleep(.12)
        x("mousemove", "--window", window, *end)
        time.sleep(.15)
        if cancel:
            cancel()
        x("mouseup", 1)
        display.sync()
        time.sleep(.5)

    def window_size():
        lines = x("getwindowgeometry", "--shell", window).splitlines()
        geometry = dict(line.split("=", 1) for line in lines if "=" in line)
        return int(geometry["WIDTH"]), int(geometry["HEIGHT"])

    def pixels(rect=None):
        width, height = window_size()
        return display.pixels(window, rect or (0, 0, width, height))

    def capture(label):
        width, height = window_size()
        path = args.output / f"{label}.png"
        pane_drag.save_png(path, pixels(), width, height)
        result["screenshots"].append(str(path))

    def owned_state(label=None):
        sockets = list((config / "run").glob("spike-*.sock"))
        if len(sockets) != 1 or not stat.S_ISSOCK(sockets[0].lstat().st_mode):
            raise RuntimeError("expected exactly one socket in the owned config")
        response = subprocess.check_output(
            ["curl", "--fail", "--silent", "--show-error", "--max-time", "5",
             "--unix-socket", str(sockets[0]), "http://localhost/api/mux/state"],
            text=True, timeout=7)
        state = json.loads(response)
        if label:
            result["states"][label] = state
        return state

    def screen(screen_id):
        return owned_state()["screens"][screen_id]

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
        return sequence

    def last_size(pid):
        path = owned / f"sizes-{pid}.jsonl"
        return json.loads(path.read_text().splitlines()[-1]) if path.exists() and path.stat().st_size else None

    def painted_marker(label, pid, rect):
        time.sleep(.3)
        before = pixels(rect)
        sequence = command(pid, "emit")  # A write acknowledgment is not a paint.
        def painted():
            after = pixels(rect)
            count = sum(before[offset:offset + 3] != after[offset:offset + 3]
                        for offset in range(0, len(before), 4))
            return count if count > 100 else None
        changed = wait_until(painted, f"{label} did not paint LIVE_{pid}_{sequence}")
        result["paintedMarkerPixels"][label] = changed
        capture(label)

    def float_rect(entry):
        # The float canvas covers the padded workspace viewport, including
        # the tiled tab row. At 100% chrome: toolbar 42px, footer 30px,
        # workspace padding 4px each side.
        width, height = window_size()
        left = 4
        top = 46
        content_width = width - 8
        content_height = height - 80
        bounds = entry["bounds"]
        return (left + round(bounds["x"] * content_width),
                top + round(bounds["y"] * content_height),
                round(bounds["w"] * content_width), round(bounds["h"] * content_height))

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
                    lines = x("getwindowgeometry", "--shell", candidate).splitlines()
                    geometry = dict(line.split("=", 1) for line in lines if "=" in line)
                    if geometry.get("WIDTH") == "1200" and geometry.get("HEIGHT") == "760":
                        return int(candidate)

            window = wait_until(find_window, "no 1200x760 native window", seconds=30)
            x("windowfocus", window)
            a = wait_until(lambda: fresh(set()), "initial fixture did not launch", seconds=30)
            screen_id = owned_state("initial")["activeScreenId"]
            assert route(owned_state(), a["pane"]) == screen_id
            capture("initial-root")
            no_input("new-root-tab", lambda: chord("Alt_L", "t"))
            b = wait_until(lambda: fresh({a["pid"]}), "second tiled tab did not launch")
            wait_until(lambda: b["pane"] in screen(screen_id)["panes"], "second tab route missing")
            capture("two-root-tabs")
            result["checks"].append("owned-fixture-and-two-root-tabs")

            if not args.setup_only:
                if not args.float_tab_x:
                    raise ValueError("full floating acceptance requires --float-tab-x")

                no_input("source-find", lambda: (chord("Control_L", "f"),
                         x("type", "--clearmodifiers", "--delay", "2", f'HISTORY_{b["pid"]}')))
                if args.xclip:
                    no_input("copy-source-query", lambda: (chord("Control_L", "a"), chord("Control_L", "c")))
                    copied = subprocess.check_output([str(args.xclip), "-selection", "clipboard", "-o"],
                                                     env=env, timeout=8)
                    assert copied == f'HISTORY_{b["pid"]}'.encode(), copied
                capture("root-history-find")
                no_input("close-source-find", lambda: display.type_key("Escape"))

                def select_source_word():
                    display.key("Shift_L", True)
                    move(62, 91)
                    x("click", "--repeat", 2, "--delay", 80, 1)
                    display.key("Shift_L", False)
                    display.sync()
                before_selection = raw()[b["pid"]]
                select_source_word()
                delta = raw()[b["pid"]][len(before_selection):]
                assert not re.search(rb"\x1b\[<(?:0|2|32|34);", delta), delta
                result["ptyEvents"]["source-selection"] = {str(b["pid"]): delta.hex()}
                no_input("copy-source-selection", lambda: chord("Control_L", "Shift_L", "c"))
                copied = subprocess.check_output([str(args.xclip), "-selection", "clipboard", "-o"],
                                                 env=env, timeout=8)
                assert copied == f'PANE_{b["pid"]}'.encode(), copied
                capture("root-selection")
                result["checks"].append("root-terminal-history-query-and-selection-captured")

                before_b_size = wait_until(lambda: last_size(b["pid"]), "root B size missing")
                move(args.float_tab_x, 20)
                no_input("float-focused-b", lambda: click(args.float_tab_x, 20))
                floating_b = wait_until(lambda: in_float(screen(screen_id), b["pane"]),
                                        "Float Tab did not create canonical floating B")
                b_float_id = floating_b["id"]
                root_panes = [pane for slot in slots(screen(screen_id)["root"]) for pane in slot["paneIds"]]
                assert root_panes == [a["pane"]], root_panes
                assert screen(screen_id)["panes"][b["pane"]]["sessionId"] == b["session"]
                assert alive(a["pid"]) and alive(b["pid"])
                wait_until(lambda: last_size(b["pid"]) != before_b_size,
                           "floating B did not claim smaller terminal geometry")
                capture("first-floating-group")
                result["checks"].append("float-tab-retains-pane-session-pid-and-new-size")

                subprocess.run([str(args.xclip), "-selection", "clipboard", "-i"],
                               input=b"FLOAT_SELECTION_SENTINEL", env=env, check=True, timeout=8)
                no_input("recopy-float-selection", lambda: chord("Control_L", "Shift_L", "c"))
                copied = subprocess.check_output([str(args.xclip), "-selection", "clipboard", "-o"],
                                                 env=env, timeout=8)
                assert copied == f'PANE_{b["pid"]}'.encode(), copied
                no_input("reopen-float-find", lambda: chord("Control_L", "f"))
                no_input("copy-float-query", lambda: (chord("Control_L", "a"), chord("Control_L", "c")))
                copied = subprocess.check_output([str(args.xclip), "-selection", "clipboard", "-o"],
                                                 env=env, timeout=8)
                assert copied == f'HISTORY_{b["pid"]}'.encode(), copied
                capture("floating-history-find")
                no_input("close-float-find", lambda: display.type_key("Escape"))
                result["checks"].append("floating-retains-terminal-selection-and-search-query")

                bx, by, bw, bh = float_rect(floating_b)
                before_picker = screen(screen_id)
                move(bx + bw - 15, by + 49)
                no_input("open-floating-profile-picker", lambda: click(bx + bw - 15, by + 49))
                capture("floating-profile-picker")
                no_input("dismiss-floating-profile-picker", lambda: display.type_key("Escape"))
                assert screen(screen_id)["root"] == before_picker["root"]
                assert screen(screen_id)["floating"] == before_picker["floating"]
                assert len(identities()) == 2
                capture("floating-profile-picker-dismissed")
                result["checks"].append("floating-profile-picker-opens-and-dismisses-without-pty-input")

                result["initialFloatingRect"] = [bx, by, bw, bh]
                painted_marker("floating-live-output", b["pid"], (bx + 5, by + 95, min(350, bw - 10), 70))
                result["checks"].append("floating-terminal-live-output-paints")

                no_input("floating-new-tab", lambda: chord("Alt_L", "t"))
                d = wait_until(lambda: fresh({a["pid"], b["pid"]}),
                               "new tab in floating group did not launch")
                floated = wait_until(lambda: in_float(screen(screen_id), d["pane"]),
                                     "new tab did not join floating group")
                assert floated["id"] == b_float_id
                assert {pane for slot in slots(floated["root"]) for pane in slot["paneIds"]} == {
                    b["pane"], d["pane"]}
                assert screen(screen_id)["panes"][d["pane"]]["sessionId"] == d["session"]
                capture("floating-two-tabs")
                no_input("floating-cycle-previous", lambda: chord("Alt_L", "Page_Up"))
                wait_until(lambda: b["pane"] in [slot.get("activePaneId") for slot in
                                      slots(in_float(screen(screen_id), b["pane"])["root"])],
                           "floating tab cycle did not select B")
                no_input("floating-cycle-next", lambda: chord("Alt_L", "Page_Down"))
                wait_until(lambda: d["pane"] in [slot.get("activePaneId") for slot in
                                      slots(in_float(screen(screen_id), d["pane"])["root"])],
                           "floating tab cycle did not select D")
                result["checks"].append("floating-new-tab-and-cycle-retain-group-and-sessions")

                before_split = screen(screen_id)
                no_input("root-only-split-blocked-on-float", lambda: chord("Alt_L", "d"))
                assert screen(screen_id)["root"] == before_split["root"]
                assert screen(screen_id)["floating"] == before_split["floating"]
                assert len(identities()) == 3
                capture("floating-root-only-split-unavailable")
                result["checks"].append("root-only-split-unavailable-on-floating-focus")

                # A floating header owns maximize; the tiled root must stop
                # claiming visible size while this slot is maximized.
                floating_b = in_float(screen(screen_id), b["pane"])
                bx, by, bw, bh = float_rect(floating_b)
                root_size_before = (owned / f'sizes-{a["pid"]}.jsonl').read_text()
                root_live_crop = (5, 130, 250, 80)
                root_before_output = pixels(root_live_crop)
                move(bx + bw - 50, by + 16)
                no_input("maximize-floating-group", lambda: click(bx + bw - 50, by + 16))
                capture("floating-maximized")
                command(a["pid"], "emit")
                time.sleep(.35)
                assert (owned / f'sizes-{a["pid"]}.jsonl').read_text() == root_size_before, (
                    "hidden tiled root published a visible size")
                no_input("restore-floating-group", lambda: chord("Alt_L", "m"))
                def hidden_output_painted():
                    after = pixels(root_live_crop)
                    changed = sum(root_before_output[offset:offset + 3] != after[offset:offset + 3]
                                  for offset in range(0, len(after), 4))
                    return changed if changed > 100 else None
                result["paintedMarkerPixels"]["hidden-root-after-float-restore"] = wait_until(
                    hidden_output_painted, "hidden root output did not paint after restore")
                capture("floating-restored-with-hidden-root-output")
                assert all(alive(entry["pid"]) for entry in (a, b, d))
                result["checks"].append("float-maximize-hides-root-size-claims-and-restores-background-output")

                # Move and resize change canonical normalized bounds once,
                # while header/grip pointer input stays out of the PTYs.
                floating_b = in_float(screen(screen_id), b["pane"])
                bx, by, bw, bh = float_rect(floating_b)
                move(bx + 100, by + 16)
                old_bounds = floating_b["bounds"]
                no_input("move-floating-header", lambda: drag(
                    (bx + 100, by + 16), (bx, by - 24)))
                moved = wait_until(lambda: in_float(screen(screen_id), b["pane"])
                                   if in_float(screen(screen_id), b["pane"])["bounds"] != old_bounds else None,
                                   "floating header drag did not persist new bounds")
                assert moved["bounds"]["x"] < old_bounds["x"]
                assert moved["bounds"]["w"] == old_bounds["w"]
                capture("floating-moved")
                result["checks"].append("floating-header-move-retains-session-without-pty-input")

                bx, by, bw, bh = float_rect(moved)
                d_size_before = wait_until(lambda: last_size(d["pid"]), "floating D size missing")
                move(bx + bw - 8, by + bh - 8)
                no_input("resize-floating-grip", lambda: drag(
                    (bx + bw - 8, by + bh - 8), (bx + bw - 90, by + bh - 48)))
                resized = wait_until(lambda: in_float(screen(screen_id), b["pane"])
                                     if in_float(screen(screen_id), b["pane"])["bounds"] != moved["bounds"] else None,
                                     "floating resize grip did not persist new bounds")
                assert resized["bounds"]["w"] < moved["bounds"]["w"]
                assert resized["bounds"]["h"] < moved["bounds"]["h"]
                wait_until(lambda: last_size(d["pid"]) != d_size_before,
                           "resized floating terminal did not publish new visible size")
                capture("floating-resized")
                result["checks"].append("floating-resize-measures-visible-terminal-without-pty-input")

                bx, by, bw, bh = float_rect(resized)
                move(bx + 90, by + 16)
                no_input("escape-cancel-float-drag", lambda: drag(
                    (bx + 90, by + 16), (bx + 155, by + 45),
                    cancel=lambda: display.type_key("Escape")))
                assert in_float(screen(screen_id), b["pane"])["bounds"] == resized["bounds"]
                capture("floating-escape-cancelled")
                result["checks"].append("escape-cancels-float-gesture-before-server-mutation")

                move(bx + 90, by + 16)
                d_size_before = (owned / f'sizes-{d["pid"]}.jsonl').read_text()
                def zoom_during_drag():
                    x("mousedown", 1)
                    x("mousemove", "--window", window, bx + 155, by + 45)
                    chord("Control_L", "equal")
                    wait_until(lambda: tomllib.loads(config_file.read_text())["appearance"]["zoom"] == 1.1,
                               "held-drag zoom did not persist")
                    wait_until(lambda: (owned / f'sizes-{d["pid"]}.jsonl').read_text() != d_size_before,
                               "held-drag zoom did not remeasure visible terminal")
                    x("mouseup", 1)
                no_input("zoom-cancel-float-drag", zoom_during_drag)
                assert in_float(screen(screen_id), b["pane"])["bounds"] == resized["bounds"]
                capture("floating-zoom-cancelled")
                zoomed_size = (owned / f'sizes-{d["pid"]}.jsonl').read_text()
                no_input("reset-zoom-after-cancel", lambda: chord("Control_L", "0"))
                wait_until(lambda: tomllib.loads(config_file.read_text())["appearance"]["zoom"] == 1.,
                           "zoom reset did not persist")
                wait_until(lambda: (owned / f'sizes-{d["pid"]}.jsonl').read_text() != zoomed_size,
                           "zoom reset did not remeasure floating terminal")
                result["checks"].append("zoom-cancels-stale-float-drag")

                # A viewport resize changes display projection and PTY size,
                # never authoritative float bounds until a fresh gesture.
                bx, by, bw, bh = float_rect(resized)
                move(bx + 90, by + 16)
                d_size_before = (owned / f'sizes-{d["pid"]}.jsonl').read_text()
                def viewport_during_drag():
                    x("mousedown", 1)
                    x("mousemove", "--window", window, bx + 155, by + 45)
                    x("windowsize", window, 1080, 700)
                    wait_until(lambda: window_size() == (1080, 700), "viewport did not resize")
                    wait_until(lambda: (owned / f'sizes-{d["pid"]}.jsonl').read_text() != d_size_before,
                               "viewport resize did not remeasure floating terminal")
                    x("mouseup", 1)
                no_input("viewport-cancel-float-drag", viewport_during_drag)
                assert in_float(screen(screen_id), b["pane"])["bounds"] == resized["bounds"]
                capture("floating-viewport-cancelled")
                narrow_size = (owned / f'sizes-{d["pid"]}.jsonl').read_text()
                x("windowsize", window, 1200, 760)
                wait_until(lambda: window_size() == (1200, 760), "viewport did not restore")
                wait_until(lambda: (owned / f'sizes-{d["pid"]}.jsonl').read_text() != narrow_size,
                           "restored viewport did not remeasure floating terminal")
                result["checks"].append("viewport-change-cancels-stale-float-drag")

                bx, by, bw, bh = float_rect(resized)
                move(bx + 90, by + 16)
                def deactivate_during_drag():
                    x("mousedown", 1)
                    x("mousemove", "--window", window, bx + 155, by + 45)
                    display.x.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
                    display.x.XDefaultRootWindow.restype = ctypes.c_ulong
                    root = display.x.XDefaultRootWindow(display.connection)
                    x("windowfocus", root)
                    wait_until(lambda: int(x("getwindowfocus").strip()) == root,
                               "private X root did not receive focus")
                    x("mouseup", 1)
                    x("windowfocus", window)
                no_input("deactivation-cancel-float-drag", deactivate_during_drag)
                assert in_float(screen(screen_id), b["pane"])["bounds"] == resized["bounds"]
                capture("floating-deactivation-cancelled")
                result["checks"].append("window-deactivation-cancels-float-drag")

                # Add a second float over the first one. An uncovered root
                # click is expected app mouse input; all float controls remain
                # isolated from both terminals.
                click(90, 160)
                no_input("new-tiled-tab-before-second-float", lambda: chord("Alt_L", "t"))
                c = wait_until(lambda: fresh({a["pid"], b["pid"], d["pid"]}),
                               "second floating candidate did not launch")
                assert c["pane"] in [pane for slot in slots(screen(screen_id)["root"])
                                     for pane in slot["paneIds"]]
                move(args.float_tab_x, 20)
                no_input("float-second-group", lambda: click(args.float_tab_x, 20))
                floating_c = wait_until(lambda: in_float(screen(screen_id), c["pane"]),
                                        "second floating group did not appear")
                assert floating_c["id"] != b_float_id
                assert len(screen(screen_id)["floating"]) == 2
                capture("overlapping-floating-groups")
                result["checks"].append("second-floating-group-overlaps-with-stable-identities")

                def pointer(target, point, wheel=False):
                    move(*point)
                    before = raw()
                    if wheel:
                        display.wheel(4)
                        display.sync()
                    else:
                        x("click", 1)
                    wait_until(lambda: len(raw()[target["pid"]]) > len(before[target["pid"]]),
                               "topmost floating terminal did not receive pointer input")
                    after = raw()
                    assert all(after[pid] == value for pid, value in before.items()
                               if pid != target["pid"]), ("pointer leaked behind topmost float", before, after)
                    reports = SGR.findall(after[target["pid"]][len(before[target["pid"]]):])
                    assert reports and any(int(code) == (64 if wheel else 0) for code, _, _, _ in reports), reports
                    result["ptyEvents"][f'{"wheel" if wheel else "click"}-{target["pid"]}-{len(result["ptyEvents"])}'] = {
                        str(target["pid"]): after[target["pid"]][len(before[target["pid"]]):].hex()}

                # Geometry is calibrated against the frozen screenshot; these
                # points are inside default C, exposed moved B, and overlap.
                pointer(c, (500, 370))
                pointer(c, (500, 370), wheel=True)
                before_raise = screen(screen_id)["floating"]
                pointer(d, (245, 360))
                assert screen(screen_id)["floating"] == before_raise, (
                    "local stack raise changed canonical floating layout")
                pointer(d, (500, 370))
                pointer(d, (500, 370), wheel=True)
                capture("floating-local-stack-raised")
                result["checks"].append("overlap-routes-mouse-wheel-only-to-topmost-and-raise-is-local")

                # Raise C through its exposed right edge, then transfer its
                # floating tab with the existing tab menu to a new Screen.
                pointer(c, (850, 370))
                cx, cy, cw, ch = float_rect(floating_c)
                move(cx + 55, cy + 49)
                no_input("open-floating-transfer-menu", lambda: click(cx + 55, cy + 49, 3))
                capture("floating-transfer-menu")
                no_input("transfer-floating-c-to-new-screen", lambda: display.type_key("Return"))
                target_id = wait_until(lambda: route(owned_state(), c["pane"])
                                       if route(owned_state(), c["pane"]) != screen_id else None,
                                       "floating C did not transfer to New Screen")
                state = owned_state("after-floating-screen-transfer")
                assert state["screens"][target_id]["panes"][c["pane"]]["sessionId"] == c["session"]
                assert in_float(state["screens"][screen_id], c["pane"]) is None
                assert c["pane"] in [pane for slot in slots(state["screens"][target_id]["root"])
                                      for pane in slot["paneIds"]]
                assert all(alive(entry["pid"]) for entry in (a, b, c, d))
                capture("floating-tab-followed-to-new-screen")
                painted_marker("floating-transferred-live-output", c["pid"], (5, 130, 350, 70))
                result["checks"].append("floating-tab-transfers-to-screen-root-with-same-session")

                no_input("return-to-screen-one", lambda: chord("Alt_L", "Shift_L", "1"))
                capture("screen-one-before-empty-root")
                move(50, 62)
                no_input("open-last-tiled-tab-menu", lambda: click(50, 62, 3))
                capture("last-tiled-tab-menu")
                no_input("move-last-tiled-pane-to-second-screen", lambda: display.type_key("Return"))
                wait_until(lambda: route(owned_state(), a["pane"]) == target_id
                           and not any(slot["paneIds"] for slot in slots(screen(screen_id)["root"])),
                           "last tiled pane did not leave an empty root beside floating B/D")
                assert owned_state()["screens"][target_id]["panes"][a["pane"]]["sessionId"] == a["session"]
                assert in_float(screen(screen_id), b["pane"])["id"] == b_float_id
                assert all(alive(entry["pid"]) for entry in (a, b, c, d))
                no_input("return-to-empty-root-screen", lambda: chord("Alt_L", "Shift_L", "1"))
                capture("empty-tiled-root-with-floating-group")
                result["checks"].append("floating-group-keeps-screen-when-tiled-root-becomes-empty")

                floating_b = in_float(screen(screen_id), b["pane"])
                bx, by, bw, bh = float_rect(floating_b)
                click(bx + 50, by + 49)
                wait_until(lambda: b["pane"] in [slot.get("activePaneId") for slot in
                                      slots(in_float(screen(screen_id), b["pane"])["root"])],
                           "B was not selected in its floating group before Dock")
                display.key("Shift_L", True)
                move(bx + 60, by + 76)
                x("click", "--repeat", 2, "--delay", 80, 1)
                display.key("Shift_L", False)
                display.sync()
                no_input("copy-pre-dock-selection", lambda: chord("Control_L", "Shift_L", "c"))
                copied = subprocess.check_output([str(args.xclip), "-selection", "clipboard", "-o"],
                                                 env=env, timeout=8)
                assert copied == f'PANE_{b["pid"]}'.encode(), copied
                capture("pre-dock-floating-selection")
                move(bx + bw - 105, by + 16)
                no_input("dock-floating-group-into-empty-root", lambda: click(bx + bw - 105, by + 16))
                wait_until(lambda: in_float(screen(screen_id), b["pane"]) is None
                           and {pane for slot in slots(screen(screen_id)["root"])
                                for pane in slot["paneIds"]} == {b["pane"], d["pane"]},
                           "Dock did not move the complete group into the empty root")
                state = owned_state("after-dock-into-empty-root")
                assert state["screens"][screen_id]["panes"][b["pane"]]["sessionId"] == b["session"]
                assert state["screens"][screen_id]["panes"][d["pane"]]["sessionId"] == d["session"]
                assert all(alive(entry["pid"]) for entry in (a, b, c, d))
                capture("floating-group-docked-into-empty-root")
                result["checks"].append("dock-complete-group-into-empty-root-retains-both-sessions")

                subprocess.run([str(args.xclip), "-selection", "clipboard", "-i"],
                               input=b"DOCK_SELECTION_SENTINEL", env=env, check=True, timeout=8)
                no_input("recopy-docked-selection", lambda: chord("Control_L", "Shift_L", "c"))
                copied = subprocess.check_output([str(args.xclip), "-selection", "clipboard", "-o"],
                                                 env=env, timeout=8)
                assert copied == f'PANE_{b["pid"]}'.encode(), copied
                no_input("reopen-docked-find", lambda: chord("Control_L", "f"))
                no_input("copy-docked-query", lambda: (chord("Control_L", "a"), chord("Control_L", "c")))
                copied = subprocess.check_output([str(args.xclip), "-selection", "clipboard", "-o"],
                                                 env=env, timeout=8)
                assert copied == f'HISTORY_{b["pid"]}'.encode(), copied
                capture("docked-history-find")
                no_input("close-docked-find", lambda: display.type_key("Escape"))
                painted_marker("docked-live-output", b["pid"], (5, 125, 450, 170))
                result["checks"].append("dock-retains-selection-query-and-painted-pty-output")
                result["identities"] = {"root": a, "firstFloat": b, "secondFloat": c, "floatTab": d}

            for pid in identities():
                if alive(pid):
                    command(pid, "stop")
            wait_until(lambda: all(not alive(pid) for pid in identities()),
                       "owned fixtures did not exit normally", seconds=8)
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
