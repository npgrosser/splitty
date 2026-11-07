#!/usr/bin/env python3
"""Measure native display zoom in owned X11 windows and raw PTYs.

Run with the same local GPUI library/font environment as the other private
window smokes. `--matrix` runs both scopes at 50, 125 and 300 percent. This
never touches the user's desktop, browser or settings directory.
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
import statistics
import subprocess
import sys
import time
import tomllib


HERE = Path(__file__).resolve().parent
LINK = "https://example.invalid/zoom-7319"
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


def rgb(data, width, x, y):
    offset = (y * width + x) * 4
    return data[offset + 2], data[offset + 1], data[offset]


def terminal_geometry(data, level, width=1200, height=760):
    # At x=600 the one-pane terminal has a long uninterrupted background run.
    # This finds its actual top after native chrome scaled by the window rem.
    anchor = height // 2
    base = rgb(data, width, 600, anchor)
    top = anchor
    while top > 40 and rgb(data, width, 600, top - 1) == base:
        top -= 1
    bottom = anchor
    while bottom < height - 1 and rgb(data, width, 600, bottom + 1) == base:
        bottom += 1
    if bottom - top < 80:
        raise AssertionError(f"no long terminal background run at x=600: {top}..{bottom}")
    bands = []
    active = None
    for y in range(top + 1, min(bottom, top + 330)):
        ink = sum(1 for x in range(5, 230)
                  if max(abs(a - b) for a, b in zip(rgb(data, width, x, y), base)) > 75)
        if ink >= 5 and active is None:
            active = y
        if ink < 5 and active is not None:
            bands.append((active, y))
            active = None
    if active is not None:
        bands.append((active, min(bottom, top + 330)))
    merged = []
    for start, end in bands:
        if merged and start - merged[-1][1] <= max(2, round(3 * level)):
            merged[-1] = (merged[-1][0], end)
        else:
            merged.append((start, end))
    bands = merged
    if len(bands) < 3:
        raise AssertionError(f"fewer than three terminal text bands: {bands}")
    pitch = round(statistics.median(after[0] - before[0]
                                    for before, after in zip(bands, bands[1:])))
    return {"top": top, "bottom": bottom, "background": base,
            "bands": bands, "rowPitch": pitch,
            "glyphHeight": round(sum(end - start for start, end in bands[-3:]) / 3),
            "linkY": top + pitch // 2}


def config_text(python, fixture, owned, scope, level):
    return (f'[appearance]\ntheme_mode = "fixed"\ntheme = "splitty-dark"\n'
            f'zoom = {level}\nzoom_scope = "{scope}"\n\n'
            '[drawer]\npinned = false\n\n'
            f'[shells]\ndefault_profile_id = "zoom-fixture"\n\n'
            f'[[shells.profiles]]\nid = "zoom-fixture"\nname = "Zoom fixture"\n'
            f'command = {json.dumps(str(python))}\n'
            f'args = {json.dumps(["-u", str(fixture), str(owned)])}\n'
            'enabled = true\ntarget = "local"\nsource = "custom"\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--display", default=":97")
    parser.add_argument("--xvfb", default="Xvfb")
    parser.add_argument("--xvfb-lib", type=Path)
    parser.add_argument("--xdotool", default="xdotool")
    parser.add_argument("--xclip", type=Path, help="owned X11 clipboard reader for exact find-query checks")
    parser.add_argument("--scope", choices=["window", "terminals"], default="window")
    parser.add_argument("--level", type=float, default=1.25)
    parser.add_argument("--config-dir", type=Path, help="restart with a prior case's owned config directory")
    parser.add_argument("--matrix", action="store_true")
    parser.add_argument("--setup-only", action="store_true")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    args.output.mkdir(parents=True, exist_ok=False)
    if args.matrix:
        cases = {}
        passed = True
        for scope in ("window", "terminals"):
            for level in (.5, 1.25, 3.):
                key = f"{scope}-{int(level * 100)}"
                command = [sys.executable, str(Path(__file__).resolve()), str(binary),
                           "--output", str(args.output / key), "--display", args.display,
                           "--scope", scope, "--level", str(level), "--xvfb", str(args.xvfb),
                           "--xdotool", str(args.xdotool)]
                if args.xvfb_lib:
                    command += ["--xvfb-lib", str(args.xvfb_lib)]
                if args.xclip:
                    command += ["--xclip", str(args.xclip)]
                if args.setup_only:
                    command += ["--setup-only"]
                run = subprocess.run(command, capture_output=True, text=True, timeout=120)
                path = args.output / key / "result.json"
                cases[key] = json.loads(path.read_text()) if path.exists() else {"stderr": run.stderr[-1000:]}
                passed &= run.returncode == 0 and cases[key].get("passed", False)
                if level == 1.25 and run.returncode == 0 and not args.setup_only:
                    restart_key = f"{scope}-persisted-restart"
                    restart = [sys.executable, str(Path(__file__).resolve()), str(binary),
                               "--output", str(args.output / restart_key), "--display", args.display,
                               "--scope", scope, "--level", "1.5", "--xvfb", str(args.xvfb),
                               "--xdotool", str(args.xdotool),
                               "--config-dir", str(args.output / key / "config")]
                    if args.xvfb_lib:
                        restart += ["--xvfb-lib", str(args.xvfb_lib)]
                    if args.xclip:
                        restart += ["--xclip", str(args.xclip)]
                    restarted = subprocess.run(restart, capture_output=True, text=True, timeout=120)
                    restart_path = args.output / restart_key / "result.json"
                    cases[restart_key] = (json.loads(restart_path.read_text()) if restart_path.exists()
                                          else {"stderr": restarted.stderr[-1000:]})
                    passed &= restarted.returncode == 0 and cases[restart_key].get("passed", False)
        comparison_error = None
        if passed:
            try:
                terminal_tops = []
                for level in (50, 125, 300):
                    window_case = cases[f"window-{level}"]["geometry"]
                    terminal_case = cases[f"terminals-{level}"]["geometry"]
                    terminal_tops.append(terminal_case["top"])
                    assert abs(window_case["rowPitch"] - terminal_case["rowPitch"]) <= 1
                    assert abs(window_case["glyphHeight"] - terminal_case["glyphHeight"]) <= 2
                    assert abs(window_case["top"] / (level / 100) - 79) <= 3
                    if level == 50:
                        assert window_case["top"] < terminal_case["top"] - 15
                    else:
                        assert window_case["top"] > terminal_case["top"] + 10
                assert max(terminal_tops) - min(terminal_tops) <= 2
            except AssertionError as error:
                passed = False
                comparison_error = repr(error)
        report = {"binary": str(binary), "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                  "harnessSha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                  "cases": cases, "comparisonError": comparison_error, "passed": passed}
        (args.output / "result.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps({"passed": passed, "cases": {key: case.get("error", "passed") for key, case in cases.items()}}, indent=2))
        if not passed:
            raise SystemExit(1)
        return

    owned = args.output / "fixture"
    config = (args.config_dir or args.output / "config").resolve()
    if not config.is_relative_to(args.output.parent.resolve()):
        raise ValueError("config directory must belong to this output matrix")
    config.mkdir(mode=0o700, exist_ok=True)
    fixture = HERE / "gpui-display-zoom-fixture.py"
    config_file = config / "config.toml"
    if config_file.exists():
        saved = tomllib.loads(config_file.read_text())
        assert saved["appearance"]["zoom_scope"] == args.scope
        assert abs(saved["appearance"]["zoom"] - args.level) < .001
        assert saved["drawer"]["pinned"] is False, "zoom fixture needs full-width terminal geometry"
        owned = Path(saved["shells"]["profiles"][0]["args"][-1]).resolve()
        if not owned.is_relative_to(args.output.parent.resolve()):
            raise ValueError("fixture directory must belong to this output matrix")
    else:
        config_file.write_text(config_text(Path(sys.executable).resolve(), fixture.resolve(), owned.resolve(), args.scope, args.level))
    owned.mkdir(mode=0o700, exist_ok=True)
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
    for key in ("SPLITTY_SOCK", "SPLITTY_AGENT_SOCK", "SPLITTY_ADDR", "SPLITTY_TOKEN",
                "SPLITTY_SERVER", "SPLITTY_PANE_ID", "SPLITTY_SCREEN_ID", "SPLITTY_SESSION_ID"):
        env.pop(key, None)
    xvfb_env = env.copy()
    if args.xvfb_lib:
        xvfb_env["LD_LIBRARY_PATH"] = str(args.xvfb_lib) + (":" + env["LD_LIBRARY_PATH"] if env.get("LD_LIBRARY_PATH") else "")
    result = {"binary": str(binary), "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "harnessSha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "fixtureSha256": hashlib.sha256(fixture.read_bytes()).hexdigest(),
              "scope": args.scope, "level": args.level, "setupOnly": args.setup_only,
              "display": args.display, "checks": [], "screenshots": [], "passed": False}
    xvfb = app = display = window = None
    sequences = {}

    def fixture_command(pid, action):
        sequence = sequences.get(pid, 0) + 1
        sequences[pid] = sequence
        next_path = owned / f"control-{pid}.next"
        next_path.write_text(json.dumps({"seq": sequence, "action": action}))
        next_path.replace(owned / f"control-{pid}.json")
        if action != "stop":
            wait_until(lambda: json.loads((owned / f"ack-{pid}.json").read_text()).get("seq") == sequence
                       if (owned / f"ack-{pid}.json").exists() else False,
                       f"fixture {pid} did not acknowledge {action}")
    with (args.output / "xvfb.log").open("w") as xvfb_log, (args.output / "app.log").open("w") as app_log:
        try:
            number = args.display.lstrip(":")
            if Path(f"/tmp/.X11-unix/X{number}").exists() or Path(f"/tmp/.X{number}-lock").exists():
                raise RuntimeError(f"DISPLAY {args.display} is already in use")
            xvfb = subprocess.Popen([str(args.xvfb), args.display, "-screen", "0", "1280x900x24",
                                     "-nolisten", "tcp", "-noreset", "-ac", "-xkbdir", "/usr/share/X11/xkb"],
                                    env=xvfb_env, stdout=xvfb_log, stderr=xvfb_log, start_new_session=True)

            def x(*arguments):
                return subprocess.check_output([str(args.xdotool), *map(str, arguments)], env=env, text=True, timeout=12)

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
                    raise RuntimeError(f"app exited before opening window: {app.returncode}")
                found = subprocess.run([str(args.xdotool), "search", "--onlyvisible", "--pid", str(app.pid)],
                                       env=env, capture_output=True, text=True, timeout=3)
                for candidate in found.stdout.splitlines():
                    geometry = dict(line.split("=", 1) for line in x("getwindowgeometry", "--shell", candidate).splitlines() if "=" in line)
                    if geometry.get("WIDTH") == "1200" and geometry.get("HEIGHT") == "760":
                        return int(candidate)

            window = wait_until(find_window, "No 1200x760 native window", seconds=30)
            fixture_pid = wait_until(lambda: next((pid for pid in
                                                  (int(path.stem.split("-")[1]) for path in owned.glob("ready-*.json"))
                                                  if alive(pid)), None),
                                     "owned raw PTY profile did not start")
            size_file = owned / f"sizes-{fixture_pid}.jsonl"
            wait_until(size_file.exists, "PTY dimensions missing")
            time.sleep(.6)

            def capture(label):
                path = args.output / f"{label}.png"
                image = display.pixels(window, (0, 0, 1200, 760))
                pane_drag.save_png(path, image)
                result["screenshots"].append(str(path))
                return image

            image = capture("startup")
            geometry = terminal_geometry(image, args.level)
            result["geometry"] = geometry
            result["fixturePid"] = fixture_pid
            result["ptySizes"] = [json.loads(line) for line in size_file.read_text().splitlines()]
            result["checks"].append("owned-fixture-and-rendered-terminal-geometry")
            assert alive(fixture_pid), "fixture exited during startup"

            def raw(pid=fixture_pid):
                return (owned / f"input-{pid}.bin").read_bytes()

            def all_raw():
                return {path.stem.split("-")[1]: path.read_bytes()
                        for path in owned.glob("input-*.bin")}

            def chord(*keys):
                for modifier in keys[:-1]:
                    display.key(modifier, True)
                display.type_key(keys[-1])
                for modifier in reversed(keys[:-1]):
                    display.key(modifier, False)
                display.sync()

            def move(x_pos, y_pos):
                x("mousemove", "--window", window, x_pos, y_pos)
                display.sync()
                time.sleep(.1)

            def button(down):
                display.xt.XTestFakeButtonEvent(display.connection, 1, down, 0)
                display.sync()

            def no_input(label, action):
                before = all_raw()
                action()
                time.sleep(.2)
                delta = {pid: data[len(before.get(pid, b"")):]
                         for pid, data in all_raw().items()
                         if data != before.get(pid, b"")}
                result.setdefault("ptyEvents", {})[label] = {pid: data.hex() for pid, data in delta.items()}
                assert not delta, f"{label} leaked PTY input: {delta}"

            def opener_targets():
                return [line.split("\t", 1)[1] for line in spy_log.read_text().splitlines()]

            def owned_state():
                sockets = list((config / "run").glob("spike-*.sock"))
                if len(sockets) != 1 or not stat.S_ISSOCK(sockets[0].lstat().st_mode):
                    raise RuntimeError("expected exactly one owned workspace socket")
                response = subprocess.check_output(
                    ["curl", "--fail", "--silent", "--show-error", "--max-time", "5",
                     "--unix-socket", str(sockets[0]), "http://localhost/api/mux/state"],
                    text=True, timeout=7)
                return json.loads(response)

            def changed_pixels(first, second, left, top, right, bottom):
                return sum(first[offset:offset + 3] != second[offset:offset + 3]
                           for y in range(top, bottom) for x_pos in range(left, right)
                           for offset in [(y * 1200 + x_pos) * 4])

            if not args.setup_only:
                # The native baseline row is 21 px; zoom must project that
                # same row metric under both scopes, independent of chrome.
                expected = round(21 * args.level)
                assert abs(geometry["rowPitch"] - expected) <= 1, (geometry, expected)
                result["checks"].append("rendered-row-pitch-matches-confirmed-zoom")
                assert not spy_log.read_text(), "terminal output launched a browser"
                result["checks"].append("output-does-not-open-link")

                x("windowfocus", window)
                move(40, geometry["linkY"])
                before_open = len(opener_targets())
                no_input("scaled-link-click", lambda: (
                    display.key("Control_L", True), button(True), button(False),
                    display.key("Control_L", False), display.sync()))
                wait_until(lambda: len(opener_targets()) == before_open + 1,
                           "scaled OSC8 link did not reach private opener")
                time.sleep(.2)
                assert opener_targets() == [LINK], opener_targets()
                capture("scaled-link-activated")
                result["checks"].append("scaled-link-hit-target-and-local-captured-gesture")

                # Ordinary tracked mouse input must still use actual scaled
                # cells rather than the unscaled 21 px row metric.
                move(40, geometry["top"] + max(2, geometry["rowPitch"] // 2))
                before = raw()
                button(True)
                button(False)
                wait_until(lambda: len(raw()) > len(before), "scaled app mouse report missing")
                reports = [(int(code), int(col), int(row), kind)
                           for code, col, row, kind in SGR.findall(raw()[len(before):])]
                assert any(row == 1 and kind == b"M" for _, _, row, kind in reports), reports
                assert any(row == 1 and kind == b"m" for _, _, row, kind in reports), reports
                result.setdefault("ptyEvents", {})["scaled-app-mouse"] = raw()[len(before):].hex()
                result["checks"].append("scaled-application-mouse-row")

                move(700, geometry["top"] + geometry["rowPitch"] * 2)
                no_input("find-query", lambda: (chord("Control_L", "f"),
                           x("type", "--clearmodifiers", "--delay", 2, "ZOOM_LINK")))
                time.sleep(.5)
                capture("scaled-find-bar")
                if args.xclip:
                    no_input("copy-find-query", lambda: (chord("Control_L", "a"), chord("Control_L", "c")))
                    copied = subprocess.check_output([str(args.xclip), "-selection", "clipboard", "-o"],
                                                     env=env, timeout=8)
                    assert copied == b"ZOOM_LINK", f"find editor query differs: {copied!r}"
                    result["checks"].append("scaled-find-keeps-editor-focus-and-query")
                else:
                    result["checks"].append("scaled-find-no-pty-input-visual-query-unverified")
                no_input("close-find", lambda: display.type_key("Escape"))

                if args.level == 3. and args.scope == "window":
                    before_popup = capture("high-zoom-before-popup")
                    popup_y = geometry["top"] - round(geometry["rowPitch"] * .75)
                    no_input("high-zoom-profile-popup", lambda: x(
                        "mousemove", "--window", window, 1152, popup_y, "click", 1))
                    time.sleep(.3)
                    popup = capture("high-zoom-profile-popup")
                    assert changed_pixels(before_popup, popup, 700, geometry["top"], 1190, 650) > 500, (
                        "profile popup did not render inside the high-zoom viewport")
                    no_input("dismiss-high-zoom-popup", lambda: display.type_key("Escape"))
                    result["checks"].append("high-zoom-profile-popup-visible-and-dismissible")

                    toolbar_before = capture("high-zoom-toolbar-before-scroll")
                    def scroll_toolbar():
                        move(960, 64)
                        for _ in range(5):
                            display.wheel(7)
                    no_input("high-zoom-toolbar-scroll", scroll_toolbar)
                    time.sleep(.25)
                    toolbar_after = capture("high-zoom-toolbar-after-scroll")
                    assert changed_pixels(toolbar_before, toolbar_after, 200, 0, 1195, 125) > 500, (
                        "wide toolbar did not reveal its clipped controls")
                    result["checks"].append("high-zoom-compact-toolbar-remains-scrollable")

                    no_input("high-zoom-split", lambda: chord("Alt_L", "d"))
                    split_pid = wait_until(lambda: next((pid for pid in
                                                        (int(path.stem.split("-")[1]) for path in owned.glob("ready-*.json"))
                                                        if pid != fixture_pid and alive(pid)), None),
                                           "high-zoom split did not launch a second owned PTY")
                    split_state = owned_state()
                    screen_id = split_state["activeScreenId"]
                    split_root = split_state["screens"][screen_id]["root"]
                    assert split_root["type"] == "split" and len(split_root["children"]) == 2
                    split_image = capture("high-zoom-split-before-drag")
                    result["splitRootBeforeDrag"] = split_root
                    left_sizes = size_file.read_text()
                    right_size_file = owned / f"sizes-{split_pid}.jsonl"
                    wait_until(right_size_file.exists, "second split PTY did not report size")
                    right_sizes = right_size_file.read_text()

                    # A resize gesture started in the old pixel geometry must
                    # be discarded when zoom changes before its mouse release.
                    no_input("high-zoom-divider-down", lambda: (
                        move(600, geometry["top"] + 180), button(True)))
                    no_input("zoom-cancels-held-divider", lambda: chord("Control_L", "minus"))
                    def saved_zoom_250():
                        try:
                            return tomllib.loads(config_file.read_text())["appearance"]["zoom"] == 2.5
                        except (KeyError, ValueError):
                            return False
                    wait_until(saved_zoom_250, "zoom while divider held did not persist 250%")
                    wait_until(lambda: changed_pixels(
                        split_image, display.pixels(window, (0, 0, 1200, 760)),
                        12, 155, 560, 235) > 1000,
                        "zoom while divider held did not render 250% chrome")
                    wait_until(lambda: size_file.read_text() != left_sizes
                               and right_size_file.read_text() != right_sizes,
                               "both visible split PTYs did not remeasure at 250%")
                    time.sleep(.25)
                    before_release = owned_state()["screens"][screen_id]["root"]
                    result["splitRootBeforeRelease"] = before_release
                    assert before_release["sizes"] == split_root["sizes"], (
                        "held divider mutated split before mouse release", split_root, before_release)
                    no_input("release-cancelled-divider", lambda: (move(720, geometry["top"] + 180), button(False)))
                    time.sleep(.4)
                    after_drag = owned_state()["screens"][screen_id]["root"]
                    result["splitRootAfterDrag"] = after_drag
                    assert after_drag["sizes"] == split_root["sizes"], (
                        "stale divider drag mutated split sizes after zoom", split_root, after_drag)
                    assert alive(fixture_pid) and alive(split_pid)
                    capture("high-zoom-cancelled-divider")
                    result["checks"].append("zoom-cancels-stale-divider-drag-and-keeps-both-ptys")
                    fixture_command(split_pid, "stop")
                    wait_until(lambda: not alive(split_pid), "split fixture did not exit")

                if args.level == 1.25 and args.config_dir is None:
                    no_input("rapid-zoom-shortcuts", lambda: (
                        chord("Control_L", "equal"), chord("Control_L", "equal"),
                        chord("Control_L", "minus")))

                    def saved_zoom():
                        try:
                            return tomllib.loads(config_file.read_text())["appearance"]["zoom"]
                        except (KeyError, ValueError):
                            return None

                    wait_until(lambda: saved_zoom() == 1.5,
                               "ordered rapid keys did not persist final 150% zoom", seconds=20)
                    time.sleep(.5)
                    scaled = terminal_geometry(capture("rapid-zoom-150"), 1.5)
                    assert abs(scaled["rowPitch"] - 32) <= 1, scaled
                    assert alive(fixture_pid), "zoom replaced the original PTY"
                    result["postShortcutGeometry"] = scaled
                    result["checks"].append("rapid-keys-ordered-persisted-and-original-pty-retained")

                    no_input("new-hidden-tab", lambda: chord("Alt_L", "t"))
                    second_pid = wait_until(lambda: next((pid for pid in
                                                         (int(path.stem.split("-")[1]) for path in owned.glob("ready-*.json"))
                                                         if pid != fixture_pid and alive(pid)), None),
                                            "second owned PTY did not start")
                    result["secondFixturePid"] = second_pid
                    assert alive(fixture_pid) and alive(second_pid)
                    time.sleep(.4)
                    hidden_sizes = size_file.read_text()
                    no_input("zoom-while-first-tab-hidden", lambda: chord("Control_L", "equal"))
                    wait_until(lambda: saved_zoom() == 1.75,
                               "hidden-pane zoom did not persist 175%")
                    time.sleep(.3)
                    assert size_file.read_text() == hidden_sizes, "hidden PTY published a size claim"
                    fixture_command(fixture_pid, "emit")
                    no_input("reactivate-hidden-tab", lambda: chord("Alt_L", "Page_Up"))
                    def at_revealed_zoom():
                        try:
                            measured = terminal_geometry(display.pixels(window, (0, 0, 1200, 760)), 1.75)
                            return measured if abs(measured["rowPitch"] - 37) <= 1 else None
                        except AssertionError:
                            return None

                    wait_until(at_revealed_zoom, "revealed pane did not reach 175% rendered geometry")
                    revealed = terminal_geometry(capture("hidden-tab-reactivated"), 1.75)
                    assert abs(revealed["rowPitch"] - 37) <= 1, revealed
                    wait_until(lambda: size_file.read_text() != hidden_sizes,
                               "reactivated pane did not publish new PTY size")
                    assert alive(fixture_pid) and alive(second_pid)
                    result["checks"].append("hidden-pty-keeps-output-and-identity-through-zoom")

                    no_input("restore-150-after-reveal", lambda: chord("Control_L", "minus"))
                    wait_until(lambda: saved_zoom() == 1.5,
                               "restored 150% level was not persisted")
                    time.sleep(.4)
                    restored = terminal_geometry(capture("restored-150-after-reveal"), 1.5)
                    assert abs(restored["rowPitch"] - 32) <= 1, restored
                    result["checks"].append("revealed-pane-resizes-at-current-zoom")

                    no_input("reopen-retained-find", lambda: chord("Control_L", "f"))
                    if args.xclip:
                        no_input("copy-retained-query", lambda: (chord("Control_L", "a"), chord("Control_L", "c")))
                        copied = subprocess.check_output([str(args.xclip), "-selection", "clipboard", "-o"],
                                                         env=env, timeout=8)
                        assert copied == b"ZOOM_LINK", f"tab switch lost find query: {copied!r}"
                        result["checks"].append("query-retained-on-hidden-tab")
                    x("type", "--clearmodifiers", "--delay", 2, "HISTORY7319")
                    time.sleep(.4)
                    capture("retained-history-find")
                    no_input("close-history-find", lambda: display.type_key("Escape"))
                    result["checks"].append("history-find-visual-review-required")

                    no_input("reset-zoom", lambda: chord("Control_L", "0"))
                    wait_until(lambda: saved_zoom() == 1., "Ctrl+0 did not persist 100%")
                    def at_reset_zoom():
                        try:
                            measured = terminal_geometry(display.pixels(window, (0, 0, 1200, 760)), 1.)
                            return measured if abs(measured["rowPitch"] - 21) <= 1 else None
                        except AssertionError:
                            return None

                    wait_until(at_reset_zoom, "Ctrl+0 did not render baseline 21px rows")
                    capture("reset-100")
                    no_input("restore-150-after-reset", lambda: (
                        chord("Control_L", "equal"), chord("Control_L", "equal"),
                        chord("Control_L", "equal")))
                    wait_until(lambda: saved_zoom() == 1.5,
                               "ordered restore after Ctrl+0 did not reach 150%")
                    result["checks"].append("ctrl-zero-reset-and-ordered-restore-without-pty-input")

                    fixture_command(second_pid, "stop")
                    wait_until(lambda: not alive(second_pid), "second fixture did not exit")

            fixture_command(fixture_pid, "stop")
            wait_until(lambda: not alive(fixture_pid), "fixture did not exit normally", seconds=8)
            result["passed"] = True
        except Exception as error:
            result["error"] = repr(error)
            if display and window:
                try:
                    path = args.output / "failure.png"
                    pane_drag.save_png(path, display.pixels(window, (0, 0, 1200, 760)))
                    result["screenshots"].append(str(path))
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
            result["openerCalls"] = spy_log.read_text().splitlines()
            result["passed"] = result["passed"] and result.get("exitCode") == 0 and not result["survivingChildren"]
            (args.output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result, indent=2))
    if not result["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
