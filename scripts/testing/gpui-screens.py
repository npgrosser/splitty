#!/usr/bin/env python3
"""Exercise local Screen controls with real mouse, keyboard, and shell sessions.

The script owns an isolated Xvfb display and a native GPUI window. It records
terminal screenshots as visual evidence, while shell PID/state files provide
machine-checked session continuity. Run with the native library and font
environment prepared for splitty-gpui; no internal smoke mode is used.
"""

import argparse
from collections import Counter
import ctypes
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import stat
import subprocess
import sys
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


def rgb(data):
    """XGetImage on this 24-bit Xvfb stores pixels as BGRA."""
    return tuple(data[index] for index in (2, 1, 0))


def text_bands(data, width, height, background):
    """Find horizontal terminal text bands from actual rendered pixels."""
    occupied = []
    for row in range(height):
        count = 0
        for column in range(width):
            offset = (row * width + column) * 4
            if max(abs(data[offset + channel] - background[2 - channel]) for channel in range(3)) > 55:
                count += 1
        occupied.append(count >= 35)
    starts = [row for row in range(height) if occupied[row] and (row == 0 or not occupied[row - 1])]
    ends = [row for row in range(height) if occupied[row] and (row == height - 1 or not occupied[row + 1])]
    return list(zip(starts, ends))


def marker_ink(data, width, bands, background):
    # The command echo and prior scrollback can precede the six identical M
    # rows. Select the repeated ink dimensions, not a fixed screen row index.
    dimensions = []
    for start, end in bands:
        columns = []
        for column in range(width):
            if any(max(abs(data[(row * width + column) * 4 + channel] - background[2 - channel])
                       for channel in range(3)) > 55 for row in range(start, end + 1)):
                columns.append(column)
        if not columns:
            raise AssertionError("Marker row has no visible ink")
        dimensions.append((columns[-1] - columns[0] + 1, end - start + 1))
    (marker_width, marker_height), count = Counter(dimensions).most_common(1)[0]
    if count < 3:
        raise AssertionError(f"Could not identify repeated marker ink dimensions: {dimensions}")
    return {"width": marker_width, "height": marker_height, "rows": count}


def glyph_bounds(data, width, height):
    """Measure fixed chrome text ink against its surrounding background."""
    background = rgb(data[:4])
    points = []
    for row in range(height):
        for column in range(width):
            offset = (row * width + column) * 4
            if max(abs(data[offset + channel] - background[2 - channel]) for channel in range(3)) > 55:
                points.append((column, row))
    if not points:
        raise AssertionError("Screen label has no visible pixels")
    return {"width": max(point[0] for point in points) - min(point[0] for point in points) + 1,
            "height": max(point[1] for point in points) - min(point[1] for point in points) + 1}


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
    parser.add_argument("--navigation", action="store_true", help="exercise native maximize, Screen shortcuts, and restore routing")
    parser.add_argument("--navigation-keys-only", action="store_true", help="exercise physical shifted-digit Screen keys on a selected XKB layout")
    parser.add_argument("--maximize-x", type=int, default=850, help="header maximize/restore control X coordinate for --navigation")
    parser.add_argument("--keyboard-layout", choices=["us", "de"], help="apply this XKB layout on the isolated Xvfb display")
    parser.add_argument("--setxkbmap", default="setxkbmap")
    parser.add_argument("--xkb-tools-dir", type=Path, help="prepend private XKB tools to PATH for setxkbmap")
    parser.add_argument("--appearance-matrix", action="store_true", help="run isolated dark, light, font and fallback startup fixtures")
    parser.add_argument("--appearance-case", choices=["dark", "light", "font", "fallback"])
    parser.add_argument("--tab-management", action="store_true", help="exercise exact native tab placement and focused-group navigation")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    binary = args.binary.resolve(strict=True)
    if args.appearance_matrix:
        cases = {}
        report = {"binary": str(binary), "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                  "cases": cases, "passed": False}
        try:
            for case in ("dark", "light", "font", "fallback"):
                command = [sys.executable, str(Path(__file__).resolve()), str(binary),
                           "--output", str(args.output / case), "--display", args.display,
                           "--xdotool", str(args.xdotool), "--xvfb", str(args.xvfb),
                           "--appearance-case", case]
                if args.xvfb_lib:
                    command += ["--xvfb-lib", str(args.xvfb_lib)]
                completed = subprocess.run(command, capture_output=True, text=True, timeout=120)
                result_path = args.output / case / "result.json"
                if result_path.exists():
                    cases[case] = json.loads(result_path.read_text())
                if completed.returncode:
                    raise AssertionError(f"{case} appearance case failed; see {result_path}: {completed.stderr[-1000:]}")
            assert cases["light"]["pixels"]["terminalBackground"][0] > cases["dark"]["pixels"]["terminalBackground"][0] + 100
            assert cases["font"]["pixels"]["chromeBrandBounds"]["height"] > cases["dark"]["pixels"]["chromeBrandBounds"]["height"] + 2
            assert cases["fallback"]["pixels"]["chromeBrandBounds"]["height"] < cases["font"]["pixels"]["chromeBrandBounds"]["height"]
            assert all(case["pixels"]["terminalRowPitch"] == 21 for case in cases.values())
            for dimension in ("width", "height"):
                assert cases["font"]["pixels"]["terminalMarkerInk"][dimension] == cases["dark"]["pixels"]["terminalMarkerInk"][dimension], (
                    "18px proportional chrome setting changed native terminal glyph dimensions"
                )
            report["passed"] = True
        except Exception as error:
            report["error"] = str(error)
        (args.output / "result.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report, indent=2))
        if not report["passed"]:
            raise SystemExit(1)
        return
    env = os.environ.copy()
    env.pop("WAYLAND_DISPLAY", None)
    env["DISPLAY"] = args.display
    if args.xkb_tools_dir:
        env["PATH"] = str(args.xkb_tools_dir) + ":" + env.get("PATH", "")
    if args.appearance_case or args.tab_management:
        fixture = (args.output / "config").resolve()
        fixture.mkdir(mode=0o700)
        fixture.chmod(0o700)
        (fixture / "themes").mkdir(mode=0o700, exist_ok=True)
        theme = {"dark": "splitty-dark", "light": "splitty-light",
                 "font": "splitty-dark", "fallback": "unsupported-native"}.get(args.appearance_case, "splitty-dark")
        config = f'[appearance]\ntheme_mode = "fixed"\ntheme = "{theme}"\n'
        if args.tab_management:
            config += 'font_size = 0\n'
            for key in ("SPLITTY_SOCK", "SPLITTY_AGENT_SOCK", "SPLITTY_ADDR", "SPLITTY_TOKEN",
                        "SPLITTY_SERVER", "SPLITTY_PANE_ID", "SPLITTY_SCREEN_ID", "SPLITTY_SESSION_ID"):
                env.pop(key, None)
        if args.appearance_case == "font":
            config += 'font_family = "DejaVu Sans"\nfont_size = 18\n'
        (fixture / "config.toml").write_text(config)
        if args.appearance_case == "fallback":
            source = Path(__file__).resolve().parents[2] / "crates/splitty-core/src/theme/bundled/splitty-dark.toml"
            authored = source.read_text().replace('name = "Splitty Dark"', 'name = "Unsupported Native Values"')
            authored += ('surface-base = "oklch(0.5 0.2 90)"\n'
                         'font-family = "var(--missing-font)"\n'
                         'font-size = "2rem"\n\n'
                         '[terminal]\nbg = "color(display-p3 1 0 0)"\n')
            (fixture / "themes/unsupported-native.toml").write_text(authored)
        env["SPLITTY_CONFIG_DIR"] = str(fixture)
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

    def owned_state():
        """Read the app's authoritative workspace over its own owner socket."""
        run_dir = Path(env["SPLITTY_CONFIG_DIR"]) / "run"
        sockets = list(run_dir.glob("spike-*.sock"))
        if len(sockets) != 1 or not stat.S_ISSOCK(sockets[0].lstat().st_mode):
            raise RuntimeError(f"Expected exactly one owned control socket in {run_dir}")
        response = subprocess.run(["curl", "--fail", "--silent", "--show-error", "--max-time", "5",
                                   "--unix-socket", str(sockets[0]), "http://localhost/api/mux/state"],
                                  check=True, capture_output=True, text=True, timeout=7)
        state = json.loads(response.stdout)
        result["ownedControlSocket"] = sockets[0].name
        return state

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

    def shell_value_at(label, expression, point):
        path = scratch / label
        path.unlink(missing_ok=True)
        click(point, .1)
        display.key("Control_L", True)
        display.type_key("c")
        display.key("Control_L", False)
        display.sync()
        time.sleep(.1)
        send(f"printf '%s\\n' {expression} > '{path}'")
        return wait_until(lambda: path.read_text().strip() if path.exists() else None,
                          f"Shell did not write {label}", seconds=8)

    def shell_value(label, expression):
        return shell_value_at(label, expression, (300, 180))

    def shell_pid(label):
        return int(shell_value(label, '"$$"'))

    def shell_pid_at(label, point):
        return int(shell_value_at(label, '"$$"', point))

    def chord(*keys):
        for modifier in keys[:-1]:
            display.key(modifier, True)
        display.type_key(keys[-1])
        for modifier in reversed(keys[:-1]):
            display.key(modifier, False)
        display.sync()
        time.sleep(.4)

    def drag_tab(start, target, cancel=False):
        x("mousemove", "--window", window, *start, "mousedown", 1)
        time.sleep(.1)
        x("mousemove", "--window", window, start[0] + 15, start[1] + 20)
        time.sleep(.15)
        x("mousemove", "--window", window, *target)
        time.sleep(.25)
        if cancel:
            display.type_key("Escape")
        x("mouseup", 1)
        time.sleep(.8)

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

    def exercise_navigation(a_pid):
        marker = scratch / "hidden-max-output"
        send(f"(sleep 4; printf 'HIDDEN_MAX_OUTPUT\\n' | tee '{marker}') &")
        time.sleep(.2)
        send("sleep 120")
        time.sleep(.6)
        click((960, 20), .8)  # Split right, leaving A busy in the left group.
        b_pid = focused_shell_pid("nav-b-initial")
        assert b_pid != a_pid and alive(a_pid), "Split must retain busy A and create B"
        split_header = window_pixels((650, 47, 530, 32))
        capture("nav-split")

        chord("Alt_L", "m")
        max_header = window_pixels((650, 47, 530, 32))
        assert max_header != split_header, "Mod+M did not change the group viewport"
        capture("nav-maximized-shortcut")
        click((args.maximize_x, 20))  # Visible Restore control.
        assert window_pixels((650, 47, 530, 32)) == split_header, "Visible control did not restore the split viewport"
        capture("nav-restored-control")
        chord("Alt_L", "m")
        assert window_pixels((650, 47, 530, 32)) == max_header, "Mod+M did not maximize again"
        result["checks"].extend(["maximize-mod-m", "restore-visible-control"])

        click((110, 62), .8)  # New tab C inside maximized group B.
        c_pid = focused_shell_pid("nav-c-initial")
        assert c_pid not in (a_pid, b_pid)
        assert window_pixels((650, 47, 530, 32)) == max_header, "New tab lost maximized viewport"
        click((40, 62))
        assert shell_pid("nav-b-tab-return") == b_pid
        click((130, 62))
        assert shell_pid("nav-c-tab-return") == c_pid
        capture("nav-maximized-tabs")
        result["checks"].append("tabs-retain-maximize-and-sessions")

        chord("Control_L", "Shift_L", "space")  # New Screen 2.
        d_pid = focused_shell_pid("nav-screen-2-initial")
        assert d_pid not in (a_pid, b_pid, c_pid)
        capture("nav-new-screen-shortcut")
        chord("Alt_L", "Shift_L", "1")
        assert shell_pid("nav-screen-1-by-key") == c_pid, "Alt+Shift+1 did not select Screen 1"
        assert window_pixels((650, 47, 530, 32)) == max_header, "Screen 1 lost its maximize choice while hidden"
        chord("Alt_L", "Shift_L", "2")
        assert shell_pid("nav-screen-2-by-key") == d_pid, "Alt+Shift+2 did not select Screen 2"
        result["checks"].extend(["ctrl-shift-space-creates-screen", "shifted-digit-screen-selection", "maximize-survives-screen-switch"])

        # An unavailable index is consumed, not inserted into the shell line.
        chord("Alt_L", "Shift_L", "9")
        unavailable = scratch / "nav-index-9"
        send(f"printf '%s\\n' \"$$\" > '{unavailable}'")
        assert int(wait_until(lambda: unavailable.read_text().strip() if unavailable.exists() else None,
                              "Unavailable Screen index leaked into terminal input", seconds=8)) == d_pid
        result["checks"].append("unavailable-shifted-digit-is-consumed")

        header_before_editor = window_pixels((70, 0, 500, 42))
        click((250, 20))  # Screen 2 rename editor, with its original short label.
        x("type", "--clearmodifiers", "--delay", 5, "EditorSafe")
        chord("Alt_L", "Shift_L", "1")
        chord("Control_L", "Shift_L", "space")
        editor_before_tail = window_pixels((180, 0, 150, 42))
        x("type", "--clearmodifiers", "--delay", 5, "Tail")
        time.sleep(.2)
        assert window_pixels((180, 0, 150, 42)) != editor_before_tail, "Rename editor lost keyboard focus"
        capture("nav-editor-still-focused")
        display.type_key("Escape")
        time.sleep(.4)
        assert window_pixels((70, 0, 500, 42)) == header_before_editor, "Editor shortcuts changed Screens or label"
        assert shell_pid("nav-editor-focus-return") == d_pid, "Editor lost Screen 2 selection"
        capture("nav-editor-priority")
        result["checks"].append("editor-keeps-navigation-shortcuts")

        chord("Alt_L", "Shift_L", "1")
        assert shell_pid("nav-screen-1-max-return") == c_pid
        wait_until(lambda: marker.exists(), "Hidden A output did not run", seconds=8)
        click((165, 20))  # Close Screen 1; A is busy behind maximized C.
        prompt = close_dialog("nav-hidden-max-close-prompt")
        assert alive(a_pid) and alive(b_pid) and alive(c_pid), "Hidden busy A was not protected"
        keep_open("nav-hidden-max-close", prompt)
        assert focused_shell_pid("nav-max-focus-after-cancel") == c_pid, "Cancel did not restore C focus"
        assert window_pixels((650, 47, 530, 32)) == max_header, "Close cancellation lost maximize"
        result["checks"].append("hidden-max-group-close-cancel")

        # Repeat the hidden-busy close with a raw reader in visible C. This
        # makes terminal-input leakage from modal shortcuts observable byte for
        # byte, while the earlier prompt established that hidden A is counted.
        reader = scratch / "modal-reader.py"
        reader.write_text(
            "import os, sys, termios, tty\n"
            "from pathlib import Path\n"
            "folder = Path(sys.argv[1]); fd = 0; old = termios.tcgetattr(fd); data = bytearray()\n"
            "try:\n"
            "    tty.setraw(fd)\n"
            "    (folder / 'modal.ready').write_text('ready')\n"
            "    while True:\n"
            "        key = os.read(fd, 1)\n"
            "        if not key or key == b'q': break\n"
            "        data.extend(key)\n"
            "finally:\n"
            "    termios.tcsetattr(fd, termios.TCSANOW, old)\n"
            "    (folder / 'modal.bytes').write_bytes(data)\n"
        )
        send(f"python3 '{reader}' '{scratch}'")
        wait_until(lambda: (scratch / "modal.ready").exists(), "Modal raw reader did not start", seconds=8)
        time.sleep(.5)
        header_before_modal = window_pixels((70, 0, 1115, 42))
        click((165, 20))
        modal_prompt = close_dialog("nav-modal-priority-prompt")
        assert alive(a_pid), "Hidden A ended before modal routing check"
        header_with_modal = window_pixels((70, 0, 1115, 42))
        chord("Control_L", "Shift_L", "space")
        chord("Alt_L", "m")
        chord("Alt_L", "Shift_L", "2")
        assert window_pixels((70, 0, 1115, 42)) == header_with_modal, (
            "Modal shortcuts changed Screen membership, selection or maximize state"
        )
        assert all(channel > 200 for channel in window_pixels((470, 325, 1, 1))[:3]), (
            "Modal prompt disappeared after navigation shortcuts"
        )
        capture("nav-modal-shortcuts-blocked")
        keep_open("nav-modal-priority", modal_prompt)
        assert window_pixels((70, 0, 1115, 42)) == header_before_modal, (
            "Modal cancellation changed Screen membership, selection or maximize state"
        )
        display.type_key("q")
        wait_until(lambda: (scratch / "modal.bytes").exists(), "Modal cancel did not restore reader focus", seconds=8)
        assert (scratch / "modal.bytes").read_bytes() == b"", "Modal shortcuts leaked terminal input"
        assert shell_pid("nav-c-after-modal") == c_pid, "Modal cancel changed C shell"
        assert window_pixels((650, 47, 530, 32)) == max_header, "Modal cancel lost maximized group"
        result["checks"].append("modal-priority-blocks-navigation-and-terminal-input")

        # Move tab C to the edge of its own B/C slot. This is a valid move
        # only because that canonical slot has two tabs; it must restore view.
        drag_tab((130, 62), (1190, 350))
        capture("nav-move-restored")
        assert window_pixels((650, 47, 530, 32)) != max_header, "Admitted edge move did not restore viewport"
        assert shell_pid_at("nav-c-after-move", (1050, 180)) == c_pid, "Moved C changed shell"
        assert shell_pid_at("nav-b-after-move", (750, 180)) == b_pid, "B changed shell after move"
        assert shell_pid_at("nav-a-after-move", (300, 180)) == a_pid, "A changed shell after hidden output"
        capture("nav-hidden-output-after-restore")
        result["checks"].extend(["move-restores-canonical-layout", "hidden-output-and-pids-retained"])

        click((1050, 180))  # Focus C in the rightmost group.
        chord("Alt_L", "m")
        capture("nav-c-maximized-before-split")
        click((1070, 20), .8)  # Split below; admitted split restores viewport.
        e_pid = focused_shell_pid("nav-e-after-split")
        assert e_pid not in (a_pid, b_pid, c_pid, d_pid)
        assert shell_pid_at("nav-c-after-split", (1050, 180)) == c_pid
        capture("nav-split-restored")
        result["checks"].append("split-restores-canonical-layout")

        chord("Alt_L", "Shift_L", "2")
        assert shell_pid("nav-screen-2-final") == d_pid
        click((265, 20))  # Close idle Screen 2.
        wait_until(lambda: not alive(d_pid), "Screen 2 shell survived its close", seconds=8)
        click((165, 20))  # Close idle Screen 1; the catalog is now empty.
        wait_until(lambda: all(not alive(pid) for pid in (a_pid, b_pid, c_pid, e_pid)),
                   "Screen 1 shells survived its close", seconds=8)
        capture("nav-empty-workspace")
        chord("Control_L", "Shift_L", "space")
        f_pid = focused_shell_pid("nav-from-empty")
        assert f_pid not in (a_pid, b_pid, c_pid, d_pid, e_pid)
        capture("nav-new-screen-from-empty")
        result["checks"].append("ctrl-shift-space-from-empty")
        result["shellPids"] = {"a": a_pid, "b": b_pid, "c": c_pid, "screen2": d_pid,
                               "split": e_pid, "emptyFresh": f_pid}
        time.sleep(1)

    def exercise_navigation_keys_only(a_pid):
        chord("Control_L", "Shift_L", "space")
        b_pid = focused_shell_pid("nav-keys-screen-2")
        chord("Control_L", "Shift_L", "space")
        c_pid = focused_shell_pid("nav-keys-screen-3")
        assert len({a_pid, b_pid, c_pid}) == 3, "New Screen shortcut reused a shell"
        capture("nav-keys-three-screens")
        for index, expected in [(1, a_pid), (2, b_pid), (3, c_pid)]:
            chord("Alt_L", "Shift_L", str(index))
            assert shell_pid(f"nav-keys-index-{index}") == expected, (
                f"Actual shifted {index} key did not select catalog Screen {index}"
            )
            capture(f"nav-keys-selected-{index}")
        # A missing index must be consumed, leaving the current shell line clean.
        chord("Alt_L", "Shift_L", "9")
        unavailable = scratch / "nav-keys-index-9"
        send(f"printf '%s\\n' \"$$\" > '{unavailable}'")
        assert int(wait_until(lambda: unavailable.read_text().strip() if unavailable.exists() else None,
                              "Shifted 9 leaked into terminal input", seconds=8)) == c_pid
        result["checks"].extend(["actual-shifted-1", "actual-shifted-2", "actual-shifted-3",
                                  "actual-unavailable-shifted-9-consumed"])
        result["shellPids"] = {"screen1": a_pid, "screen2": b_pid, "screen3": c_pid}
        time.sleep(1)

    def exercise_appearance(a_pid):
        # The fixture is selected before the app starts. Exercise normal PTY
        # rendering here; no test-only settings path or OS preference change.
        time.sleep(.8)
        terminal_bg = rgb(window_pixels((900, 450, 1, 1)))
        chrome_bg = rgb(window_pixels((500, 20, 1, 1)))
        brand = glyph_bounds(window_pixels((10, 6, 24, 28)), 24, 28)
        result["pixels"] = {"terminalBackground": terminal_bg,
                            "chromeBackground": chrome_bg, "chromeBrandBounds": brand}
        if args.appearance_case == "light":
            assert min(terminal_bg) > 220 and min(chrome_bg) > 150, "Light theme did not reach chrome and terminal pixels"
        elif args.appearance_case == "fallback":
            assert terminal_bg == (16, 21, 28), "Unsupported terminal color did not use native fallback"
            assert chrome_bg != (0, 0, 0), "Unsupported chrome color left the window unusable"
        else:
            assert max(terminal_bg) < 90 and max(chrome_bg) < 100, "Dark theme did not reach chrome and terminal pixels"
        capture(f"appearance-{args.appearance_case}-startup")
        result["checks"].append("startup-chrome-and-terminal-colors")

        send("export SPLITTY_APPEARANCE_TOKEN=retained")
        send("clear")
        send("printf 'MMMMMMMMMMMMMMMM\\n%.0s' 1 2 3 4 5 6")
        time.sleep(.5)
        terminal_pixels = window_pixels((20, 80, 300, 250))
        bands = text_bands(terminal_pixels, 300, 250, terminal_bg)
        rows = [start for start, _ in bands]
        pitches = [after - before for before, after in zip(rows, rows[1:])]
        pitch = max(set(pitches), key=pitches.count) if pitches else None
        assert pitch == 21 and pitches.count(21) >= 3, f"Rendered terminal rows did not retain 21px pitch: {rows}"
        result["pixels"].update({"terminalRowStarts": rows, "terminalRowPitch": pitch,
                                 "terminalMarkerInk": marker_ink(terminal_pixels, 300, bands, terminal_bg)})
        capture(f"appearance-{args.appearance_case}-terminal-rows")
        result["checks"].append("terminal-21px-row-pitch")

        send("printf 'APPEARANCE_HISTORY_MARKER\\n'")
        chord("Alt_L", "d")  # Split right without assuming chrome control width.
        b_pid = focused_shell_pid("appearance-split-shell")
        assert b_pid != a_pid and alive(a_pid), "Split changed original shell"
        chord("Alt_L", "m")  # Maximize B.
        chord("Alt_L", "t")  # Add tab C without assuming tab strip width.
        c_pid = focused_shell_pid("appearance-tab-shell")
        assert c_pid not in (a_pid, b_pid), "Tab reused another shell"
        click((40, 62))
        assert shell_pid("appearance-original-tab") == b_pid, "Tab switch lost B"
        assert alive(c_pid), "Hidden tab C ended while B was selected"
        chord("Alt_L", "m")  # Restore A/B layout.
        click((300, 180))
        assert shell_pid("appearance-a-return") == a_pid, "Split/tab/maximize lost A"
        assert shell_value("appearance-a-state", '"$SPLITTY_APPEARANCE_TOKEN"') == "retained", "Shell state was lost"
        capture(f"appearance-{args.appearance_case}-history-retained")
        result["shellPids"] = {"original": a_pid, "split": b_pid, "tab": c_pid}
        result["checks"].append("pty-and-history-retained-through-layout")

    def exercise_tab_management(a_pid):
        # This is the real embedded service's existing owner-only read route.
        # The fixture path is owned by this run; no daemon discovery is used.
        wait_until(lambda: len(list((fixture / "run").glob("spike-*.sock"))) == 1,
                   "Embedded owner control socket did not appear", seconds=10)
        state = owned_state()
        screen_id = state["activeScreenId"]
        assert screen_id in state["screens"], "No active Screen in owned service"
        result["screenId"] = screen_id
        result["tabStates"] = []

        def slots(node):
            if node["type"] == "slot":
                return [node]
            return [slot for child in node["children"] for slot in slots(child)]

        def view():
            screen = owned_state()["screens"][screen_id]
            return screen, slots(screen["root"])

        def slot_with(pane):
            return next((slot for slot in view()[1] if pane in slot["paneIds"]), None)

        def selected(slot):
            return slot.get("activePaneId") or slot["paneIds"][0]

        def record(label):
            screen, current = view()
            result["tabStates"].append({"label": label, "revision": screen["revision"],
                                        "slots": [{"id": slot["id"], "paneIds": slot["paneIds"],
                                                   "activePaneId": slot.get("activePaneId")}
                                                  for slot in current]})
            capture(f"tabs-{label}")

        def identity(label, point=(300, 180)):
            value = shell_value_at(label, '"$SPLITTY_PANE_ID:$SPLITTY_SESSION_ID:$$"', point)
            pane, session, pid = value.split(":")
            assert pane and session and pid.isdecimal(), f"Missing owned PTY identity: {value}"
            return {"pane": pane, "session": session, "pid": int(pid)}

        def wait_order(pane, expected, label, active=None):
            def matches():
                slot = slot_with(pane)
                return slot if slot and slot["paneIds"] == expected and (active is None or selected(slot) == active) else None
            matched = wait_until(matches, f"{label}: authoritative tab order/selection did not settle", seconds=10)
            screen, _ = view()
            for item in identities:
                assert screen["panes"][item["pane"]]["sessionId"] == item["session"], (
                    f"{label}: pane/session identity changed"
                )
                assert alive(item["pid"]), f"{label}: original shell exited"
            record(label)
            return matched

        a = identity("tab-a-initial")
        assert a["pid"] == a_pid
        identities = [a]
        for name in ("b", "c", "d"):
            chord("Alt_L", "t")
            identities.append(identity(f"tab-{name}-initial"))
        a, b, c, d = identities
        assert len({item["pane"] for item in identities}) == 4
        assert len({item["session"] for item in identities}) == 4
        left = wait_order(a["pane"], [item["pane"] for item in identities], "initial-four", d["pane"])
        left_slot = left["id"]
        chord("Alt_L", "d")  # A distinct group makes maximization visible.
        e = identity("tab-e-split", (850, 180))
        identities.append(e)
        right_slot = wait_order(e["pane"], [e["pane"]], "split-right", e["pane"])["id"]
        assert right_slot != left_slot
        active_chip = rgb(window_pixels((620, 50, 1, 1)))
        inactive_row = rgb(window_pixels((900, 50, 1, 1)))
        assert active_chip != inactive_row, "Selected tab cannot be distinguished from its strip"
        click((300, 180))
        assert shell_pid("tab-d-before-maximize") == d["pid"]
        chord("Alt_L", "m")
        max_header = window_pixels((650, 47, 530, 32))
        record("maximized-left")

        # The selected D stays selected while its exact position changes.
        drag_tab((310, 62), (10, 62))  # Whole-chip padding must start the drag.
        wait_order(a["pane"], [d["pane"], a["pane"], b["pane"], c["pane"]],
                   "same-group-first", d["pane"])
        assert window_pixels((650, 47, 530, 32)) == max_header, "Reorder lost maximized group"
        drag_tab((30, 62), (260, 62))
        wait_order(a["pane"], [a["pane"], b["pane"], d["pane"], c["pane"]],
                   "same-group-middle", d["pane"])
        assert window_pixels((650, 47, 530, 32)) == max_header
        drag_tab((200, 62), (335, 62))
        wait_order(a["pane"], [a["pane"], b["pane"], c["pane"], d["pane"]],
                   "same-group-end", d["pane"])
        assert window_pixels((650, 47, 530, 32)) == max_header
        result["checks"].append("same-group-first-middle-end-retains-active-maximize-and-sessions")

        chord("Alt_L", "m")
        click((850, 180))
        chord("Alt_L", "t")
        f = identity("tab-f-right", (850, 180))
        identities.append(f)
        wait_order(e["pane"], [e["pane"], f["pane"]], "right-before-cross", f["pane"])
        drag_tab((285, 62), (705, 62))
        wait_order(e["pane"], [e["pane"], d["pane"], f["pane"]],
                   "cross-group-middle", d["pane"])
        wait_order(a["pane"], [a["pane"], b["pane"], c["pane"]], "cross-group-source")
        result["checks"].append("cross-group-exact-middle-retains-sessions")

        before_cancel = [(slot["id"], slot["paneIds"], selected(slot)) for slot in view()[1]]
        drag_tab((625, 62), (835, 62), cancel=True)
        assert [(slot["id"], slot["paneIds"], selected(slot)) for slot in view()[1]] == before_cancel, (
            "Escape changed authoritative tab layout or selection"
        )
        record("escape-cancelled")
        result["checks"].append("escape-cancels-tab-placement")

        # Overflow is created inside the right half-width group. The selected
        # last tab must remain reachable after the strip scrolls into view.
        for _ in range(8):
            chord("Alt_L", "t")
        right = wait_until(lambda: slot_with(e["pane"]) if slot_with(e["pane"]) and len(slot_with(e["pane"])["paneIds"]) == 11 else None,
                           "Overflow tabs did not appear", seconds=12)
        last = right["paneIds"][-1]
        wait_until(lambda: selected(slot_with(e["pane"])) == last,
                   "Newly created overflow tab was not selected")
        def revealed_x():
            row = window_pixels((605, 50, 550, 1))
            matches = [605 + column for column in range(550)
                       if rgb(row[column * 4:column * 4 + 4]) == active_chip]
            return matches if matches and max(matches) > 900 else None
        assert revealed_x(), "Selected overflow tab was not visible in the right strip"
        record("overflow-selected-last")
        chord("Control_L", "Shift_L", "space")
        away_pid = shell_pid("tab-overflow-other-screen")
        assert away_pid not in {item["pid"] for item in identities}
        chord("Alt_L", "Shift_L", "1")
        wait_until(revealed_x, "Returning to the Screen did not reveal its selected overflow tab")
        assert selected(slot_with(e["pane"])) == last
        record("overflow-screen-return-revealed")
        chord("Alt_L", "m")
        record("overflow-maximized")
        chord("Alt_L", "m")
        wait_until(revealed_x, "Restoring the narrower group clipped its selected overflow tab")
        record("overflow-narrow-restored-revealed")
        before_scroll = window_pixels((605, 47, 500, 32))
        x("mousemove", "--window", window, 900, 62)
        for _ in range(5):
            display.wheel(6)  # X11 horizontal wheel left.
        time.sleep(.5)
        assert window_pixels((605, 47, 500, 32)) != before_scroll, "Overflow strip did not scroll horizontally"
        record("overflow-scrolled-left")
        chord("Alt_L", "Page_Down")
        assert selected(slot_with(e["pane"])) == last, "Next at end wrapped instead of clamping"
        chord("Alt_L", "Page_Up")
        previous = right["paneIds"][-2]
        wait_until(lambda: selected(slot_with(e["pane"])) == previous,
                   "Previous did not select prior tab")
        chord("Alt_L", "Page_Down")
        wait_until(lambda: selected(slot_with(e["pane"])) == last,
                   "Next did not return to the last tab")
        visible = wait_until(revealed_x, "Keyboard selection did not reveal the last overflow tab")
        click((max(visible) - 30, 62))
        assert selected(slot_with(e["pane"])) == last, "Visible selected chip did not route to its tab"
        record("overflow-keyboard-revealed-last")
        for _ in range(12):
            chord("Alt_L", "Page_Up")
        first = right["paneIds"][0]
        wait_until(lambda: selected(slot_with(e["pane"])) == first,
                   "Previous did not reach first tab")
        chord("Alt_L", "Page_Up")
        assert selected(slot_with(e["pane"])) == first, "Previous at start wrapped instead of clamping"
        left_active = selected(slot_with(a["pane"]))
        chord("Alt_L", "Page_Down")
        assert selected(slot_with(a["pane"])) == left_active, "Focused-group keys changed other group"
        record("focused-group-clamped-keys")
        result["checks"].append("overflow-and-clamped-focused-group-keys")

        click((145, 20))  # Screen rename editor has shortcut priority.
        editor_active = selected(slot_with(e["pane"]))
        chord("Alt_L", "Page_Down")
        assert selected(slot_with(e["pane"])) == editor_active, "Editor leaked TabStep"
        display.type_key("Escape")
        record("editor-blocked-tabstep")
        result["checks"].append("editor-blocks-tab-navigation")
        chord("Alt_L", "Page_Up")
        wait_until(lambda: selected(slot_with(e["pane"])) == e["pane"],
                   "Could not select original right-hand shell for modal check")
        click((850, 180))
        send("sleep 60")
        wait_until(lambda: Path(f"/proc/{e['pid']}/task/{e['pid']}/children").read_text().strip(),
                   "Busy sleep never started in the selected shell", seconds=8)
        wait_until(lambda: view()[0]["sessions"].get(e["session"], {}).get("busy") is True,
                   "Service did not observe the selected shell as busy", seconds=8)
        alt_w()
        prompt = close_dialog("tabs-modal-priority-prompt")
        assert all(channel > 200 for channel in window_pixels((470, 325, 1, 1))[:3]), (
            "Busy pane close did not show a confirmation dialog"
        )
        modal_active = selected(slot_with(e["pane"]))
        chord("Alt_L", "Page_Down")
        assert selected(slot_with(e["pane"])) == modal_active, "Close dialog leaked TabStep"
        assert window_pixels((455, 300, 289, 160)) == prompt, "TabStep dismissed the close dialog"
        keep_open("tabs-modal-priority", prompt)
        assert focused_shell_pid("tab-after-modal-cancel") == e["pid"], "Close cancel lost original shell"
        result["checks"].append("modal-blocks-tab-navigation")
        capture("tabs-before-ordinary-close")
        click((670, 62))  # Close X of the revealed first right-hand tab E.
        wait_until(lambda: not alive(e["pid"]) and e["pane"] not in view()[0]["panes"],
                   "Ordinary tab close did not remove only E", seconds=10)
        assert all(alive(item["pid"]) for item in identities if item != e), (
            "Ordinary close ended an unrelated original shell"
        )
        record("ordinary-close-preserves-other-tabs")
        result["checks"].append("ordinary-tab-close-still-works-after-drag")
        result["shellPids"] = {name: item["pid"] for name, item in zip("abcdef", identities)}
        result["shellPids"]["otherScreen"] = away_pid

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
            if args.keyboard_layout:
                subprocess.run([args.setxkbmap, "-display", args.display, "-layout", args.keyboard_layout],
                               env=xvfb_env, check=True, capture_output=True, text=True, timeout=10)
                query = subprocess.check_output([args.setxkbmap, "-display", args.display, "-query"],
                                                env=xvfb_env, text=True, timeout=10)
                result["keyboardLayoutQuery"] = query
                if f"layout:     {args.keyboard_layout}" not in query:
                    raise RuntimeError(f"XKB layout did not switch to {args.keyboard_layout}: {query}")
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
            if args.tab_management:
                exercise_tab_management(a_pid)
            elif args.appearance_case:
                exercise_appearance(a_pid)
            elif args.navigation_keys_only:
                exercise_navigation_keys_only(a_pid)
            elif args.navigation:
                exercise_navigation(a_pid)
            elif args.close_confirmation:
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
                    before_close = None
                    if args.appearance_case or args.tab_management:
                        try:
                            before_close = window_pixels((455, 300, 289, 160))
                        except (RuntimeError, subprocess.CalledProcessError):
                            pass
                    display.close_window(window)
                    if args.appearance_case or args.tab_management:
                        time.sleep(.6)
                        if app.poll() is None and before_close is not None:
                            try:
                                if window_pixels((455, 300, 289, 160)) != before_close:
                                    capture("tabs-window-close-prompt" if args.tab_management else "appearance-window-close-prompt")
                                    confirm_close()
                                    result["checks"].append("window-close-confirmed")
                            except (RuntimeError, subprocess.CalledProcessError):
                                pass  # The window may already be gone during normal exit.
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
