#!/usr/bin/env python3
"""Owned X11 real-window acceptance for the first native Settings editor.

Use --setup-only to capture the window, then run one calibrated phase at a time.
Geometry JSON supplies named screenshot-calibrated [x,y] points and [x,y,w,h]
regions. All edits use window input; the mux API is read only. Each phase has a
new private config, shell, Unix socket and display and saves PNG evidence.
"""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shlex
import signal
import stat
import struct
import subprocess
import termios
import time
import tomllib
import fcntl


HERE = Path(__file__).resolve().parent


def helper(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


performance = helper("performance", "gpui-performance.py")
pane_drag = helper("pane_drag", "gpui-pane-drag.py")
fidelity = helper("fidelity", "gpui-workspace-fidelity.py")


def wait_until(predicate, description, seconds=15):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.08)
    raise TimeoutError(description)


def pty_size(pid):
    fd = os.open(f"/proc/{pid}/fd/0", os.O_RDONLY | os.O_NONBLOCK)
    try:
        rows, cols, _, _ = struct.unpack("HHHH", fcntl.ioctl(fd, termios.TIOCGWINSZ, bytes(8)))
        return rows, cols
    finally:
        os.close(fd)


def changed_fraction(before, after):
    assert len(before) == len(after)
    pixels = len(before) // 4
    changed = sum(sum(abs(before[i + c] - after[i + c]) for c in range(3)) > 25
                  for i in range(0, len(before), 4))
    return changed / pixels


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--phase", choices=("open", "theme", "typography", "terminal", "workspace", "rejection"),
                        default="open")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--display", default=":96")
    parser.add_argument("--xvfb", default="Xvfb")
    parser.add_argument("--xvfb-lib", type=Path)
    parser.add_argument("--xdotool", default="xdotool")
    parser.add_argument("--geometry", type=Path)
    parser.add_argument("--setup-only", action="store_true")
    args = parser.parse_args()
    if not args.setup_only and not args.geometry:
        parser.error("acceptance phases require screenshot-calibrated --geometry JSON")
    geometry = json.loads(args.geometry.read_text()) if args.geometry else {}
    if args.geometry:
        shared = {"drawer.settings", "settings.form_region", "settings.content_region",
                  "settings.close", "window.confirm_close"}
        phase_points = {
            "open": {"settings.nav_focus_region", "settings.nav.appearance", "settings.nav.terminal",
                     "settings.nav.workspace", "settings.content_wheel", "settings.theme_mode.select",
                     "drawer.new_screen", "drawer.screen_rows_region", "terminal.body"},
            "theme": {"settings.theme_region", "settings.theme.select", "settings.theme.option",
                      "settings.theme_dark.select", "settings.theme_dark.option",
                      "settings.theme_light.select", "settings.theme_light.option",
                      "settings.theme_mode.select", "settings.theme_mode.option",
                      "settings.theme_mode.reset", "settings.theme.reset"},
            "typography": {"settings.font_size.select", "settings.font_size.option",
                           "settings.font_family.select", "settings.font_family.option",
                           "expected.font_family", "settings.content_wheel",
                           "settings.zoom_scope.select", "settings.zoom_scope.option",
                           "settings.zoom.select", "settings.zoom.option", "settings.zoom.reset"},
            "terminal": {"terminal.body", "terminal.indexed_color_point", "terminal.palette_region",
                         "workspace.chrome_region", "settings.nav.terminal",
                         "settings.terminal_theme.select", "settings.terminal_theme.option",
                         "settings.close_on_exit.select", "settings.close_on_exit.option"},
            "workspace": {"settings.nav.workspace", "settings.drawer_width.select",
                          "settings.drawer_width.option", "settings.drawer_pinned.switch"},
            "rejection": {"settings.theme.select", "settings.theme.option"},
        }
        needed = shared | (set() if args.setup_only else phase_points[args.phase])
        missing = sorted(needed - geometry.keys())
        if missing:
            parser.error("missing calibrated geometry: " + ", ".join(missing))
    binary = args.binary.resolve(strict=True)
    args.output.mkdir(parents=True, exist_ok=False)
    owned = args.output / "owned"
    owned.mkdir(mode=0o700)
    config = args.output / "config"
    config.mkdir(mode=0o700)
    if len(os.fsencode(config / "run" / ("spike-" + "0" * 32 + ".sock"))) > 107:
        parser.error("use a short --output path for the private Unix socket")
    wrapper = args.output / "sh"
    wrapper.write_text(
        "#!/bin/sh\n"
        f"cd {shlex.quote(str(owned.resolve()))} || exit 1\n"
        f"printf '%s\\t%s\\t%s\\n' \"$$\" \"$SPLITTY_PANE_ID\" \"$SPLITTY_SESSION_ID\" > {shlex.quote(str(owned.resolve()))}/shell-$$.tsv\n"
        "exec /bin/sh \"$@\"\n"
    )
    wrapper.chmod(0o700)
    ui_theme = "splitty-light" if args.phase == "terminal" else "splitty-dark"
    (config / "config.toml").write_text(
        '[appearance]\n'
        'theme_mode = "fixed"\n'
        f'theme = "{ui_theme}"\n'
        'theme_dark = "splitty-light"\n'
        'theme_light = "splitty-dark"\n'
        'font_size = 0\nfont_family = ""\nzoom = 1.0\nzoom_scope = "window"\n\n'
        '[terminal]\ntheme = "default"\nclose_on_exit = "always"\n\n'
        '[drawer]\npinned = true\nsidebar_width = 280\n\n'
        '[shells]\ndefault_profile_id = "settings-test"\n\n'
        '[[shells.profiles]]\nid = "settings-test"\nname = "Owned shell"\n'
        f'command = {json.dumps(str(wrapper.resolve()))}\n'
        'enabled = true\ntarget = "local"\nsource = "custom"\n'
    )
    env = os.environ.copy()
    env.pop("WAYLAND_DISPLAY", None)
    env["DISPLAY"] = args.display
    env["GPUI_BACKEND"] = "x11"
    env["SPLITTY_CONFIG_DIR"] = str(config.resolve())
    for key in ("SPLITTY_SOCK", "SPLITTY_AGENT_SOCK", "SPLITTY_ADDR", "SPLITTY_TOKEN",
                "SPLITTY_SERVER", "SPLITTY_PANE_ID", "SPLITTY_SCREEN_ID", "SPLITTY_SESSION_ID"):
        env.pop(key, None)
    xvfb_env = env.copy()
    if args.xvfb_lib:
        xvfb_env["LD_LIBRARY_PATH"] = str(args.xvfb_lib.resolve()) + (
            ":" + env["LD_LIBRARY_PATH"] if env.get("LD_LIBRARY_PATH") else "")
    result = {"phase": args.phase, "binarySha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "harnessSha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "geometrySha256": hashlib.sha256(args.geometry.read_bytes()).hexdigest() if args.geometry else None,
              "checks": [], "screenshots": [], "states": {}, "config": {}, "passed": False,
              "visualReview": ["settings-sections-and-keyboard-focus", "theme-and-palette-colors",
                               "field-pending-error-and-reset", "terminal-marker-history"]}
    xvfb = display = app = window = None
    children = set()

    def x(*arguments):
        return subprocess.check_output([str(args.xdotool), *map(str, arguments)],
                                       env=env, text=True, timeout=12)

    def point(name):
        value = geometry[name]
        assert len(value) == 2 and all(isinstance(n, int) for n in value), (name, value)
        return value

    def region(name):
        value = geometry[name]
        assert len(value) == 4 and all(isinstance(n, int) for n in value), (name, value)
        return tuple(value)

    def move(name):
        x("mousemove", "--window", window, *point(name))
        display.sync()
        time.sleep(.15)

    def click(name, button=1):
        move(name)
        x("click", button)
        display.sync()
        time.sleep(.18)

    def key(name):
        display.type_key(name)
        display.sync()
        time.sleep(.1)

    def type_text(value):
        x("type", "--clearmodifiers", "--delay", 3, value)
        display.sync()

    def capture(name):
        path = args.output / f"{name}.png"
        pane_drag.save_png(path, display.pixels(window, (0, 0, 1200, 760)))
        result["screenshots"].append(str(path))

    def config_now(label=None):
        value = tomllib.loads((config / "config.toml").read_text())
        if label:
            result["config"][label] = value
        return value

    def settle_config():
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline:
            before = (config / "config.toml").read_bytes()
            time.sleep(.5)
            if (config / "config.toml").read_bytes() == before:
                return
        raise TimeoutError("owned config did not settle for 500 ms")

    def saved(group, field, expected):
        return wait_until(lambda: (v if (v := config_now())[group][field] == expected else None),
                          f"{group}.{field} did not persist {expected!r}")

    def settle_content():
        content = region("settings.content_region")
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            before = display.pixels(window, content)
            time.sleep(.55)
            display.sync()
            if display.pixels(window, content) == before:
                return
        raise TimeoutError("Settings content did not settle for 550 ms after save")

    def state(label=None):
        def socket():
            candidates = list((config / "run").glob("spike-*.sock"))
            if len(candidates) > 1:
                raise RuntimeError("more than one owned service socket")
            return candidates[0] if candidates and stat.S_ISSOCK(candidates[0].lstat().st_mode) else None
        owned_socket = wait_until(socket, "owned service socket did not bind", seconds=6)
        body = subprocess.check_output(
            ["curl", "--fail", "--silent", "--show-error", "--max-time", "5",
             "--unix-socket", str(owned_socket), "http://localhost/api/mux/state"],
            text=True, timeout=7)
        value = json.loads(body)
        if label:
            result["states"][label] = value
        return value

    def shells():
        found = {}
        for marker in owned.glob("shell-*.tsv"):
            pid, pane, session = marker.read_text().strip().split("\t")
            found[int(pid)] = {"pane": pane, "session": session}
        return found

    def assert_routes(value, expected=None):
        current = fidelity.routes(value)
        for pid, shell in shells().items():
            if fidelity.alive(pid):
                assert current.get(shell["pane"]) == shell["session"], (pid, shell, current)
        if expected is not None:
            assert current == expected, (current, expected)

    def choose(name, group, field, expected, route_snapshot):
        click(name + ".select")
        capture(name + "-choices")
        click(name + ".option")
        saved(group, field, expected)
        settle_content()
        config_now(name)
        assert_routes(state(), route_snapshot)
        capture(name + "-confirmed")

    def open_settings(label, route_snapshot):
        before = (config / "config.toml").read_bytes()
        click("drawer.settings")
        wait_until(lambda: changed_fraction(
            workspace_form_pixels, display.pixels(window, region("settings.form_region"))) > .015,
                   "Settings did not replace workspace content")
        time.sleep(.25)
        capture(label)
        assert (config / "config.toml").read_bytes() == before, "opening Settings wrote config"
        assert_routes(state(), route_snapshot)

    def close_settings(label, route_snapshot, escape=False):
        settings_pixels = display.pixels(window, region("settings.form_region"))
        if escape:
            key("Escape")
        else:
            click("settings.close")
        wait_until(lambda: changed_fraction(
            settings_pixels, display.pixels(window, region("settings.form_region"))) > .015,
                   "Settings did not return to workspace")
        capture(label)
        assert_routes(state(), route_snapshot)

    def close_app():
        nonlocal app, window
        if app and app.poll() is None and display and window:
            display.close_window(window)
            time.sleep(.4)
            if app.poll() is None:
                exists = subprocess.run([str(args.xdotool), "getwindowgeometry", "--shell", str(window)],
                                        env=env, capture_output=True, timeout=3).returncode == 0
                if exists:
                    target = point("window.confirm_close") if "window.confirm_close" in geometry else (600, 422)
                    x("mousemove", "--window", window, *target)
                    x("click", 1)
                    display.sync()
            app.wait(timeout=12)
            assert app.returncode == 0, f"window exit {app.returncode}"
            result["normalWindowClose"] = True
        app = None
        window = None

    with (args.output / "xvfb.log").open("w") as xvfb_log, (args.output / "app.log").open("w") as app_log:
        try:
            number = args.display.lstrip(":")
            if Path(f"/tmp/.X11-unix/X{number}").exists() or Path(f"/tmp/.X{number}-lock").exists():
                raise RuntimeError(f"DISPLAY {args.display} is already in use")
            xvfb = subprocess.Popen([str(args.xvfb), args.display, "-screen", "0", "1280x900x24",
                                     "-nolisten", "unix", "-nolisten", "tcp", "-listen", "local",
                                     "-noreset", "-ac", "-xkbdir", "/usr/share/X11/xkb"],
                                    env=xvfb_env, stdout=xvfb_log, stderr=xvfb_log,
                                    start_new_session=True)
            def ready():
                if xvfb.poll() is not None:
                    raise RuntimeError(f"Xvfb exited: {(args.output / 'xvfb.log').read_text()[-2000:]}")
                try:
                    return subprocess.run([str(args.xdotool), "getdisplaygeometry"], env=env,
                                          capture_output=True, timeout=2).returncode == 0
                except subprocess.TimeoutExpired:
                    return False
            wait_until(ready, "private Xvfb did not start", seconds=20)
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
                    raise RuntimeError(f"app exited before window: {app.returncode}")
                found = subprocess.run([str(args.xdotool), "search", "--onlyvisible", "--pid", str(app.pid)],
                                       env=env, capture_output=True, text=True, timeout=3)
                for candidate in found.stdout.splitlines():
                    values = dict(line.split("=", 1) for line in x("getwindowgeometry", "--shell", candidate).splitlines()
                                  if "=" in line)
                    if values.get("WIDTH") == "1200" and values.get("HEIGHT") == "760":
                        return int(candidate)
            window = wait_until(find_window, "no native 1200x760 window", seconds=30)
            x("windowfocus", window)
            wait_until(lambda: any(fidelity.alive(pid) for pid in shells()), "owned shell did not start", seconds=30)
            wait_until(lambda: all(item["status"] == "running" for item in
                        state()["screens"][state()["activeScreenId"]]["sessions"].values()),
                        "initial session did not reach running", seconds=20)
            initial = state("initial")
            original_routes = fidelity.routes(initial)
            assert_routes(initial)
            settle_config()
            config_now("initial")
            time.sleep(.3)
            (args.output / "initial-config.bin").write_bytes((config / "config.toml").read_bytes())
            capture("workspace-initial")
            workspace_form_pixels = display.pixels(window, region("settings.form_region")) if args.geometry else None
            result["checks"].append("owned-window-service-and-session")

            if args.setup_only:
                if args.geometry:
                    open_settings("settings-initial", original_routes)
                    key("Escape")
                close_app()
            elif args.phase == "open":
                open_settings("appearance-open-no-write", original_routes)
                before_focus = display.pixels(window, region("settings.nav_focus_region"))
                appearance_content = display.pixels(window, region("settings.content_region"))
                key("Tab")
                assert display.pixels(window, region("settings.nav_focus_region")) != before_focus, (
                    "Tab did not show focus on first Settings navigation item")
                capture("tab-focus-appearance-nav")
                key("Tab")
                key("Return")
                wait_until(lambda: display.pixels(window, region("settings.content_region")) != appearance_content,
                           "keyboard did not navigate from Appearance to Terminal")
                capture("keyboard-terminal-group")
                result["checks"].append("keyboard-section-navigation-and-visible-focus")
                click("settings.nav.appearance")
                before_scroll = display.pixels(window, region("settings.content_region"))
                move("settings.content_wheel")
                for _ in range(3):
                    display.wheel(5)
                display.sync()
                wait_until(lambda: display.pixels(window, region("settings.content_region")) != before_scroll,
                           "Appearance content wheel did not reveal lower fields")
                capture("appearance-content-scrolled-to-zoom-scope")
                assert_routes(state(), original_routes)
                result["checks"].append("appearance-content-scroll-owned-without-screen-navigation")
                click("settings.nav.terminal")
                capture("terminal-group")
                click("settings.nav.workspace")
                capture("workspace-group")
                move("settings.content_wheel")
                display.wheel(5)
                display.sync()
                assert_routes(state(), original_routes)
                result["checks"].append("pointer-groups-and-local-wheel-isolation")
                click("settings.nav.appearance")
                click("settings.theme_mode.select")
                capture("settings-popup")
                key("Escape")
                capture("popup-escape-keeps-settings")
                assert_routes(state(), original_routes)
                key("Escape")
                wait_until(lambda: changed_fraction(
                    workspace_form_pixels, display.pixels(window, region("settings.form_region"))) < .015,
                           "second Escape did not close Settings")
                capture("settings-escape-restored-workspace")
                key("Return")
                wait_until(lambda: changed_fraction(
                    workspace_form_pixels, display.pixels(window, region("settings.form_region"))) > .015,
                           "Settings trigger did not regain keyboard focus after Escape")
                capture("trigger-return-reopened-settings")
                close_settings("settings-close-button-restored-workspace", original_routes)
                marker = owned / "focus-return.marker"
                click("terminal.body")
                type_text(f"printf '%s' \"$SPLITTY_PANE_ID\" > {shlex.quote(str(marker))}")
                key("Return")
                wait_until(lambda: marker.exists(), "Settings close did not return terminal focus")
                assert marker.read_text() in original_routes
                assert (config / "config.toml").read_bytes() == (args.output / "initial-config.bin").read_bytes()
                result["checks"].append("open-popup-escape-and-trigger-focus-preserve-session-and-config")
                open_settings("settings-before-new-screen", original_routes)
                click("drawer.new_screen")
                created = wait_until(lambda: (s if len((s := state())["screenOrder"]) == 2 else None),
                                     "Drawer New screen did not create Screen")
                new_id = next(id for id in created["screenOrder"] if id != initial["activeScreenId"])
                new_pane = next(iter(created["screens"][new_id]["panes"]))
                new_session = created["screens"][new_id]["panes"][new_pane]["sessionId"]
                new_pid = wait_until(lambda: next((pid for pid, shell in shells().items()
                                                   if fidelity.alive(pid) and shell == {
                                                       "pane": new_pane, "session": new_session}), None),
                                     "new Screen did not launch owned shell")
                new_marker = owned / "new-screen-input.marker"
                type_text(f"printf '%s' \"$$:$SPLITTY_PANE_ID:$SPLITTY_SESSION_ID\" > {shlex.quote(str(new_marker))}")
                key("Return")
                wait_until(lambda: new_marker.exists(), "New screen remained behind Settings or lost terminal focus")
                assert new_marker.read_text() == f"{new_pid}:{new_pane}:{new_session}"
                assert_routes(state("after-new-screen"))
                capture("drawer-new-screen-exits-settings")
                result["checks"].append("drawer-new-screen-exits-settings-and-routes-new-terminal")
                two_screen_routes = fidelity.routes(state())
                open_settings("settings-before-drawer-wheel", two_screen_routes)
                settings_pixels = display.pixels(window, region("settings.form_region"))
                rows_before = display.pixels(window, region("drawer.screen_rows_region"))
                move("drawer.settings")
                display.wheel(4)
                display.sync()
                wait_until(lambda: changed_fraction(
                    settings_pixels, display.pixels(window, region("settings.form_region"))) > .015,
                    "Drawer wheel did not leave Settings for a Screen")
                wait_until(lambda: display.pixels(window, region("drawer.screen_rows_region")) != rows_before,
                           "Drawer wheel did not visually select the preceding Screen")
                old_pid = next(pid for pid, shell in shells().items() if fidelity.alive(pid)
                               and shell["pane"] in original_routes)
                old_shell = shells()[old_pid]
                wheel_marker = owned / "drawer-wheel-original.marker"
                type_text(f"printf '%s' \"$$:$SPLITTY_PANE_ID:$SPLITTY_SESSION_ID\" > {shlex.quote(str(wheel_marker))}")
                key("Return")
                wait_until(lambda: wheel_marker.exists(), "Drawer wheel left original terminal without input focus")
                expected_marker = f"{old_pid}:{old_shell['pane']}:{old_shell['session']}"
                assert wheel_marker.read_text() == expected_marker, (
                    "Drawer wheel focused the wrong terminal", wheel_marker.read_text(), expected_marker)
                capture("drawer-footer-wheel-exits-settings-to-original-screen")
                result["checks"].append("drawer-wheel-from-settings-selects-neighbor-and-focuses-original-terminal")
                close_app()
            elif args.phase == "theme":
                open_settings("theme-settings", original_routes)
                before_theme = display.pixels(window, region("settings.theme_region"))
                choose("settings.theme", "appearance", "theme", "splitty-light", original_routes)
                assert changed_fraction(before_theme, display.pixels(window, region("settings.theme_region"))) > .15
                choose("settings.theme_dark", "appearance", "theme_dark", "splitty-dark", original_routes)
                choose("settings.theme_light", "appearance", "theme_light", "splitty-light", original_routes)
                choose("settings.theme_mode", "appearance", "theme_mode", "auto", original_routes)
                click("settings.theme_mode.reset")
                saved("appearance", "theme_mode", "fixed")
                settle_content()
                click("settings.theme.reset")
                saved("appearance", "theme", "splitty-dark")
                settle_content()
                capture("theme-reset-confirmed")
                result["checks"].append("theme-mode-dark-light-choice-and-reset-confirmed")
                close_app()
            elif args.phase == "typography":
                open_settings("typography-settings", original_routes)
                choose("settings.font_size", "appearance", "font_size", 15, original_routes)
                family = geometry["expected.font_family"]
                assert isinstance(family, str) and family
                choose("settings.font_family", "appearance", "font_family", family, original_routes)
                move("settings.content_wheel")
                before_scroll = display.pixels(window, region("settings.content_region"))
                for _ in range(3):
                    display.wheel(5)
                display.sync()
                wait_until(lambda: display.pixels(window, region("settings.content_region")) != before_scroll,
                           "Appearance content wheel did not scroll after font change")
                capture("typography-scrolled-to-zoom-scope")
                choose("settings.zoom_scope", "appearance", "zoom_scope", "terminals", original_routes)
                choose("settings.zoom", "appearance", "zoom", 1.2, original_routes)
                click("settings.zoom.reset")
                saved("appearance", "zoom", 1.0)
                settle_content()
                capture("zoom-reset-confirmed")
                assert_routes(state(), original_routes)
                result["checks"].append("font-size-family-zoom-scope-and-reset-confirmed")
                close_app()
            elif args.phase == "terminal":
                marker = owned / "palette-marker"
                type_text("printf '\\033[41m  INDEXED RED PALETTE MARKER  \\033[0m\\n'; "
                          f"printf '%s' \"$$:$SPLITTY_PANE_ID:$SPLITTY_SESSION_ID\" > {shlex.quote(str(marker))}")
                key("Return")
                observed = wait_until(lambda: marker.read_text() if marker.exists() else None,
                                      "indexed marker did not reach terminal")
                pid, pane, session = observed.split(":", 2)
                assert int(pid) in shells() and shells()[int(pid)] == {"pane": pane, "session": session}
                terminal_size = pty_size(int(pid))
                time.sleep(.25)
                capture("indexed-marker-before-palette")
                chrome_before = display.pixels(window, region("workspace.chrome_region"))
                terminal_before = display.pixels(window, region("terminal.palette_region"))
                open_settings("terminal-settings", original_routes)
                click("settings.nav.terminal")
                capture("terminal-controls")
                choose("settings.terminal_theme", "terminal", "theme", "splitty-dark", original_routes)
                choose("settings.close_on_exit", "terminal", "close_on_exit", "never", original_routes)
                close_settings("indexed-marker-after-palette", original_routes)
                assert display.pixels(window, region("workspace.chrome_region")) == chrome_before, (
                    "terminal-only palette changed UI chrome")
                assert changed_fraction(terminal_before, display.pixels(window, region("terminal.palette_region"))) > .2, (
                    "named palette did not visibly change terminal colors")
                assert pty_size(int(pid)) == terminal_size and fidelity.alive(int(pid))
                assert marker.read_text() == observed
                result["checks"].append("independent-indexed-terminal-palette-preserves-history-session-size-and-chrome")
                indexed_point = point("terminal.indexed_color_point")
                old_indexed = display.pixels(window, (*indexed_point, 1, 1))
                display.key("Alt_L", True)
                try:
                    key("t")
                finally:
                    display.key("Alt_L", False)
                    display.sync()
                new_state = wait_until(lambda: (s if len(fidelity.routes(s := state())) == len(original_routes) + 1 else None),
                                       "new tab did not launch after palette selection")
                new_pane = next(p for p in fidelity.routes(new_state) if p not in original_routes)
                new_session = fidelity.routes(new_state)[new_pane]
                new_pid = wait_until(lambda: next((child for child, shell in shells().items()
                                                   if fidelity.alive(child) and shell == {
                                                       "pane": new_pane, "session": new_session}), None),
                                     "new tab has no owned shell")
                new_marker = owned / "palette-new-marker"
                type_text("printf '\\033[41m  INDEXED RED PALETTE MARKER  \\033[0m\\n'; "
                          f"printf '%s' \"$$:$SPLITTY_PANE_ID:$SPLITTY_SESSION_ID\" > {shlex.quote(str(new_marker))}")
                key("Return")
                wait_until(lambda: new_marker.exists(), "new tab did not accept indexed marker")
                assert new_marker.read_text() == f"{new_pid}:{new_pane}:{new_session}"
                capture("new-tab-inherits-indexed-palette")
                new_indexed = display.pixels(window, (*indexed_point, 1, 1))
                assert all(abs(old_indexed[i] - new_indexed[i]) <= 8 for i in range(3)), (
                    "new tab did not inherit named indexed palette", old_indexed, new_indexed)
                assert all(fidelity.routes(new_state).get(pane) == session
                           for pane, session in original_routes.items())
                assert_routes(state())
                result["checks"].append("new-tab-inherits-palette-and-retains-original-session")
                close_app()
            elif args.phase == "workspace":
                open_settings("workspace-settings", original_routes)
                click("settings.nav.workspace")
                capture("workspace-controls")
                choose("settings.drawer_width", "drawer", "sidebar_width", 260, original_routes)
                click("settings.drawer_pinned.switch")
                saved("drawer", "pinned", False)
                capture("drawer-unpinned-in-settings")
                assert_routes(state(), original_routes)
                result["checks"].append("drawer-width-and-pin-confirmed-with-session-preservation")
                close_app()
            elif args.phase == "rejection":
                open_settings("rejection-before", original_routes)
                before = (config / "config.toml").read_bytes()
                blocked_save = config / "config.toml.tmp"
                blocked_save.mkdir(mode=0o700)
                try:
                    click("settings.theme.select")
                    click("settings.theme.option")
                    time.sleep(1.0)
                    assert (config / "config.toml").read_bytes() == before, (
                        "owned blocked save unexpectedly persisted update")
                    capture("rejected-theme-visible-status")
                    assert_routes(state(), original_routes)
                    result["checks"].append("owned-persistence-rejection-keeps-confirmed-value-and-terminal")
                finally:
                    blocked_save.rmdir()
                close_app()
            result["passed"] = True
        except Exception as error:
            result["error"] = repr(error)
            if display and window:
                try:
                    capture("failure")
                except Exception as capture_error:
                    result["captureError"] = repr(capture_error)
        finally:
            if app:
                children |= set(performance.process_stats(app.pid)) - {app.pid}
                if app.poll() is None:
                    try:
                        close_app()
                    except Exception as close_error:
                        result["closeError"] = repr(close_error)
                        if app and app.poll() is None:
                            os.killpg(app.pid, signal.SIGTERM)
                            try:
                                app.wait(timeout=3)
                            except subprocess.TimeoutExpired:
                                os.killpg(app.pid, signal.SIGKILL)
                                app.wait(timeout=3)
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
            for pid in children:
                if fidelity.alive(pid):
                    try:
                        os.kill(pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
            if children:
                time.sleep(.2)
            for pid in children:
                if fidelity.alive(pid):
                    try:
                        os.kill(pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
            result["survivingChildren"] = [pid for pid in children if fidelity.alive(pid)]
            result["passed"] = result["passed"] and not result["survivingChildren"] and not result.get("closeError")
            (args.output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps({"passed": result["passed"], "checks": result["checks"],
                              "error": result.get("error"), "result": str(args.output / "result.json")}, indent=2))
    if not result["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
