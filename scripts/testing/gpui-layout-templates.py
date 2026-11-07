#!/usr/bin/env python3
"""Private real-window acceptance for native saved layout templates.

Runs a real /bin/sh behind an owned profile wrapper. Command decisions are
proved by owned marker files created by that shell, not by simulated PTY echo.
The control socket is read only; all template mutations go through the window.
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
import subprocess
import sys
import time
import yaml


HERE = Path(__file__).resolve().parent


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
        result = predicate()
        if result:
            return result
        time.sleep(.06)
    raise TimeoutError(description)


def alive(pid):
    try:
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0] != "Z"
    except FileNotFoundError:
        return False


def shell_wrapper(path, owned):
    # Name the wrapper `sh`: the service's restore-mode argument builder
    # recognizes it as a shell. exec retains this PID for identity checks.
    text = ("#!/bin/sh\n"
            "cd " + shlex.quote(str(owned)) + " || exit 1\n"
            "printf '%s\\t%s\\t%s\\n' \"$$\" \"$SPLITTY_PANE_ID\" \"$SPLITTY_SESSION_ID\" > "
            + shlex.quote(str(owned)) + "/shell-$$.tsv\n"
            "exec /bin/sh \"$@\"\n")
    path.write_text(text)
    path.chmod(0o700)
    return text


def seed_command_template(config, owned):
    # JSON is valid YAML. The seed is isolated under this run's config; the
    # window must still load it and submit its actual apply operation.
    command = f"printf 'RUN\\n' >> {shlex.quote(str(owned / 'executed.marker'))}"
    template = {
        "name": "Command demo",
        "panes": {
            "plain": {"dir": str(owned)},
            "run": {"command": command, "restore_mode": "shell", "dir": str(owned)},
            "float": {"dir": str(owned)},
        },
        "layout": {"split": "horizontal", "children": [
            {"slot": "plain", "ratio": 1}, {"slot": "run", "ratio": 1}]},
        "floating": [{"layout": {"slot": "float"},
                      "bounds": {"x": .35, "y": .25, "w": .42, "h": .45}}],
    }
    directory = config / "templates"
    directory.mkdir(mode=0o700)
    (directory / "command-demo.yaml").write_text(json.dumps(template, indent=2) + "\n")
    return template


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--display", default=":97")
    parser.add_argument("--xvfb", default="Xvfb")
    parser.add_argument("--xvfb-lib", type=Path)
    parser.add_argument("--xdotool", default="xdotool")
    parser.add_argument("--setup-only", action="store_true",
                        help="validate owned shell and modal geometry before feature acceptance")
    parser.add_argument("--geometry", type=Path,
                        help="screenshot-calibrated control coordinates for the frozen candidate")
    args = parser.parse_args()
    if not args.setup_only and not args.geometry:
        parser.error("full acceptance requires --geometry")
    geometry = json.loads(args.geometry.read_text()) if args.geometry else {}
    binary = args.binary.resolve(strict=True)
    args.output.mkdir(parents=True, exist_ok=False)
    owned = args.output / "owned"
    owned.mkdir(mode=0o700)
    config = args.output / "config"
    socket_name = config / "run" / ("spike-" + "0" * 32 + ".sock")
    if len(os.fsencode(socket_name)) > 107:
        parser.error("output path is too long for the owned Linux Unix socket; use a short /tmp path")
    config.mkdir(mode=0o700)
    wrapper = args.output / "sh"
    wrapper_text = shell_wrapper(wrapper, owned.resolve())
    (config / "config.toml").write_text(
        '[appearance]\ntheme_mode = "fixed"\ntheme = "splitty-dark"\nfont_size = 0\n\n'
        '[shells]\ndefault_profile_id = "template-test"\n\n'
        '[[shells.profiles]]\nid = "template-test"\nname = "Owned shell"\n'
        f'command = {json.dumps(str(wrapper.resolve()))}\n'
        'enabled = true\ntarget = "local"\nsource = "custom"\n')
    seeded = seed_command_template(config, owned.resolve())
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
    result = {"binary": str(binary), "binarySha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "harnessSha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "wrapperSha256": hashlib.sha256(wrapper_text.encode()).hexdigest(),
              "seedTemplateSha256": hashlib.sha256(json.dumps(seeded, sort_keys=True).encode()).hexdigest(),
              "geometrySha256": hashlib.sha256(args.geometry.read_bytes()).hexdigest() if args.geometry else None,
              "checks": [], "screenshots": [], "states": {}, "passed": False}
    xvfb = app = display = window = None
    known_children = set()

    def x(*arguments):
        return subprocess.check_output([str(args.xdotool), *map(str, arguments)],
                                       env=env, text=True, timeout=12)

    def shells():
        found = {}
        for path in owned.glob("shell-*.tsv"):
            pid, pane, session = path.read_text().strip().split("\t")
            found[int(pid)] = {"pid": int(pid), "pane": pane, "session": session}
        return found

    def capture(label):
        path = args.output / f"{label}.png"
        pane_drag.save_png(path, display.pixels(window, (0, 0, 1200, 760)))
        result["screenshots"].append(str(path))

    def api_read(path):
        def owned_socket():
            sockets = list((config / "run").glob("spike-*.sock"))
            if len(sockets) > 1:
                raise RuntimeError("more than one owned service socket")
            if len(sockets) == 1 and stat.S_ISSOCK(sockets[0].lstat().st_mode):
                return sockets[0]
            return None
        # The PTY can start just before the read-only control socket is bound.
        socket = wait_until(owned_socket, "owned service socket did not bind", seconds=5)
        response = subprocess.check_output(
            ["curl", "--fail", "--silent", "--show-error", "--max-time", "5",
             "--unix-socket", str(socket), f"http://localhost{path}"],
            text=True, timeout=7)
        return json.loads(response)

    def owned_state(label=None):
        state = api_read("/api/mux/state")
        if label:
            result["states"][label] = state
        return state

    def shell_routes(state):
        routes = {}
        for screen_id, screen in state["screens"].items():
            for pane_id, pane in screen["panes"].items():
                routes[pane_id] = (screen_id, pane.get("sessionId"))
        return routes

    def assert_shell_identity(shell, state):
        routes = shell_routes(state)
        assert shell["pane"] in routes, f"shell {shell['pid']} has no canonical pane"
        assert routes[shell["pane"]][1] == shell["session"], (
            "shell session changed", shell, routes[shell["pane"]])
        assert alive(shell["pid"]), f"shell {shell['pid']} exited"

    def chord(*keys):
        for modifier in keys[:-1]:
            display.key(modifier, True)
        display.type_key(keys[-1])
        for modifier in reversed(keys[:-1]):
            display.key(modifier, False)
        display.sync()

    def click(x_pos, y_pos):
        x("mousemove", "--window", window, x_pos, y_pos)
        display.sync()
        x("click", 1)
        display.sync()
        time.sleep(.2)

    def control(key):
        point = geometry[key]
        assert len(point) == 2, (key, point)
        click(*point)

    def type_text(value):
        if value:
            x("type", "--clearmodifiers", "--delay", "3", value)
            display.sync()

    def set_field(key, value):
        control(key)
        chord("Control_L", "a")
        type_text(value)

    def press(key):
        display.type_key(key)
        display.sync()
        time.sleep(.15)

    def open_templates():
        region = (250, 170, 700, 420)
        before = display.pixels(window, region)
        chord("Alt_L", "p")
        def painted():
            after = display.pixels(window, region)
            changed = sum(before[offset:offset + 3] != after[offset:offset + 3]
                          for offset in range(0, len(after), 4))
            return changed if changed > 2000 else None
        wait_until(painted, "template modal did not paint", seconds=5)

    def child_command(shell_pid, expected):
        path = Path(f"/proc/{shell_pid}/task/{shell_pid}/children")
        if not path.exists():
            return None
        for value in path.read_text().split():
            pid = int(value)
            cmdline = Path(f"/proc/{pid}/cmdline")
            if cmdline.exists() and expected in cmdline.read_bytes().replace(b"\0", b" "):
                return pid
        return None

    def snapshot_shells():
        return {pid: entry for pid, entry in shells().items() if alive(pid)}

    def loaded_template(slug):
        # The owner control socket intentionally omits template CRUD. Inspect
        # only this run's private saved file; reload/apply still use the UI.
        path = config / "templates" / f"{slug}.yaml"
        return yaml.safe_load(path.read_text()) if path.exists() else None

    with (args.output / "xvfb.log").open("w") as xvfb_log, (args.output / "app.log").open("w") as app_log:
        try:
            number = args.display.lstrip(":")
            if Path(f"/tmp/.X11-unix/X{number}").exists() or Path(f"/tmp/.X{number}-lock").exists():
                raise RuntimeError(f"DISPLAY {args.display} is already in use")
            xvfb = subprocess.Popen([str(args.xvfb), args.display, "-screen", "0", "1280x900x24",
                                     "-nolisten", "unix", "-nolisten", "tcp", "-listen", "local",
                                     "-noreset", "-ac", "-xkbdir", "/usr/share/X11/xkb"],
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
            shell = wait_until(lambda: next((entry for entry in shells().values() if alive(entry["pid"])), None),
                               "owned /bin/sh did not launch", seconds=30)
            state = owned_state("initial")
            assert_shell_identity(shell, state)
            assert not (owned / "executed.marker").exists()
            capture("initial-shell")
            result["checks"].append("real-owned-shell-and-isolated-seeded-template")

            if not args.setup_only:
                origin_id = state["activeScreenId"]
                origin_name = state["screens"][origin_id]["name"]
                leak = owned / "modal-leak.marker"
                open_templates()
                capture("library-input-isolation")
                type_text(f"touch {shlex.quote(str(leak))}")
                press("Return")
                time.sleep(.25)
                assert not leak.exists(), "library keystrokes reached the real shell"
                press("Escape")
                result["checks"].append("modal-keystrokes-do-not-execute-in-real-shell")

                open_templates()
                control("library.load_command_initial")
                press("Escape")
                post_dismiss = owned / "post-dismiss.marker"
                type_text(f"touch {shlex.quote(str(post_dismiss))}")
                press("Return")
                wait_until(post_dismiss.exists, "dismissed modal did not restore shell focus")
                time.sleep(.25)
                unchanged = owned_state()["screens"][origin_id]
                assert unchanged["root"] == state["screens"][origin_id]["root"]
                assert unchanged["floating"] == state["screens"][origin_id]["floating"]
                assert_shell_identity(shell, owned_state())
                capture("dismissed-library-no-stale-reopen")
                result["checks"].append("dismissed-library-reply-does-not-reopen-or-steal-focus")

                chord("Alt_L", "t")
                second = wait_until(lambda: next((entry for pid, entry in snapshot_shells().items()
                                                  if pid != shell["pid"]), None), "second shell missing")
                assert_shell_identity(second, owned_state())
                control("toolbar.float_tab")
                wait_until(lambda: len(owned_state()["screens"][origin_id]["floating"]) == 1,
                           "second pane did not become floating")
                capture("source-with-floating-shell")
                result["checks"].append("source-tiled-and-floating-real-shells")

                # A foreground process, unlike a completed printf, is visible
                # to server-side extraction as live launch data.
                type_text("sleep 610")
                press("Return")
                sleep_b = wait_until(lambda: child_command(second["pid"], b"sleep 610"),
                                     "floating foreground sleep did not start")
                control("source.tiled_body")
                type_text("sleep 620")
                press("Return")
                sleep_a = wait_until(lambda: child_command(shell["pid"], b"sleep 620"),
                                     "tiled foreground sleep did not start")
                known_children.update((sleep_a, sleep_b))
                assert alive(sleep_a) and alive(sleep_b)
                wait_until(lambda: all(owned_state()["screens"][origin_id]["sessions"][entry["session"]]["busy"]
                                       for entry in (shell, second)),
                           "service did not observe both foreground commands", seconds=12)
                capture("real-foreground-commands")
                result["checks"].append("two-distinct-live-foreground-commands")

                open_templates()
                capture("template-library-initial")
                control("library.save_current")
                set_field("extract.name", "Reviewed layout")
                set_field("extract.root", str(owned.resolve()))
                capture("extract-relative-root")
                control("extract.submit")
                capture("save-command-review")
                # The two command rows are sorted by canonical pane ID.
                omit_name = sorted([shell["pane"], second["pane"]])[0]
                row = sorted([shell["pane"], second["pane"]]).index(omit_name)
                control(f"save.command_{row}")
                set_field("save.name", "Reviewed layout")
                set_field("save.slug", "reviewed-layout")
                control("save.submit")
                saved = wait_until(lambda: loaded_template("reviewed-layout"),
                                   "UI save did not persist reviewed layout")
                assert len(saved.get("floating", [])) == 1, saved
                assert len([pane for pane in saved["panes"].values() if pane.get("command")]) == 1, saved
                assert not saved["panes"][omit_name].get("command"), saved
                assert not saved["panes"][omit_name].get("restore_mode"), saved
                assert all(pane.get("dir", "").startswith(".") for pane in saved["panes"].values()), saved
                result["savedTemplate"] = saved
                capture("saved-library-reloaded")
                result["checks"].append("extract-save-relative-root-command-subset-and-float")

                # Reload the saved file through the window and create a Screen.
                control("library.load_reviewed")
                control("apply.prepare")
                capture("saved-template-review")
                control("review.run_saved")
                saved_screen = wait_until(lambda: next((sid for sid in owned_state()["screenOrder"]
                                                        if sid != origin_id), None),
                                          "saved template did not create a Screen")
                wait_until(lambda: len(snapshot_shells()) >= 4,
                           "reloaded saved template did not launch both shells")
                retained_command = next(pane["command"] for pane in saved["panes"].values()
                                        if pane.get("command"))
                expected_sleep = b"sleep 610" if "sleep 610" in retained_command else b"sleep 620"
                saved_shells = [entry for entry in snapshot_shells().values()
                                if shell_routes(owned_state()).get(entry["pane"], (None,))[0] == saved_screen]
                assert len(saved_shells) == 2, saved_shells
                saved_sleep = wait_until(lambda: next((child_command(entry["pid"], expected_sleep)
                                                      for entry in saved_shells
                                                      if child_command(entry["pid"], expected_sleep)), None),
                                         "saved selected foreground command did not rerun")
                known_children.add(saved_sleep)
                assert not (owned / "executed.marker").exists()
                capture("saved-template-new-screen")
                result["checks"].append("saved-command-subset-reruns-exactly-one-live-foreground-command")

                # Command-bearing seed: layout-only never writes the marker.
                open_templates()
                control("library.load_command_after_save")
                control("apply.prepare")
                capture("seed-command-review-layout-only")
                control("review.layout_seed")
                layout_screen = wait_until(lambda: next((sid for sid in owned_state()["screenOrder"]
                                                         if sid not in {origin_id, saved_screen}), None),
                                           "layout-only Screen did not appear")
                wait_until(lambda: len(owned_state()["screens"][layout_screen]["panes"]) == 3,
                           "layout-only layout did not include tiled and floating panes")
                time.sleep(.4)
                assert not (owned / "executed.marker").exists(), "layout-only executed a command"
                capture("layout-only-with-floating-pane")
                result["checks"].append("layout-only-new-screen-runs-no-command")

                # A second application must run exactly the displayed command.
                open_templates()
                control("library.load_command_after_save")
                control("apply.prepare")
                capture("seed-command-review-run")
                before_screens = set(owned_state()["screenOrder"])
                control("review.run_seed")
                capture("immediate-after-submit")
                run_screen = wait_until(lambda: next((sid for sid in owned_state()["screenOrder"]
                                                      if sid not in before_screens), None),
                                        "command Screen did not appear")
                wait_until(lambda: (owned / "executed.marker").exists(),
                           "actual shell did not execute reviewed marker command")
                assert (owned / "executed.marker").read_text() == "RUN\n"
                capture("command-executed-screen")
                result["checks"].append("run-commands-executes-exactly-one-real-shell-marker")

                # Return to the original Screen, whose hidden float still has
                # a foreground process. Review cancellation must preserve it.
                chord("Alt_L", "Shift_L", "1")
                wait_until(lambda: owned_state()["activeScreenId"] == origin_id,
                           "could not return to original Screen")
                before_replace = owned_state("before-replacement")
                assert_shell_identity(shell, before_replace)
                assert_shell_identity(second, before_replace)
                open_templates()
                control("library.load_command_after_save")
                control("apply.current")
                control("apply.prepare")
                capture("replacement-review-busy-hidden-float")
                # Review mode has no text editor. A typed command and Enter
                # must neither reach the shell nor authorize execution.
                review_leak = owned / "review-leak.marker"
                type_text(f"touch {shlex.quote(str(review_leak))}")
                press("Return")
                assert not review_leak.exists()
                assert owned_state()["screens"][origin_id] == before_replace["screens"][origin_id]
                capture("review-keys-isolated")
                result["checks"].append("review-keys-do-not-run-command-or-change-target")
                press("Escape")
                wait_until(lambda: max(display.pixels(window, (251, 300, 1, 1))[:3]) < 20,
                           "Escape did not dismiss template Review modal")
                assert_shell_identity(shell, owned_state())
                assert_shell_identity(second, owned_state())
                assert alive(sleep_a) and alive(sleep_b)
                result["checks"].append("replacement-cancel-preserves-busy-tiled-and-hidden-floating-shells")

                # Reconfirm and replace with layout only. Exactly one apply op
                # ends both old sessions, including the hidden floating one.
                open_templates()
                control("library.load_command_after_save")
                control("apply.current")
                control("apply.prepare")
                control("review.layout_replace")
                wait_until(lambda: not alive(shell["pid"]) and not alive(second["pid"]),
                           "confirmed replacement did not end both old sessions")
                wait_until(lambda: len([entry for entry in snapshot_shells().values()
                                        if shell_routes(owned_state()).get(entry["pane"], (None,))[0] == origin_id]) == 3,
                           "replacement did not launch all three new terminal shells")
                replaced = owned_state("after-replacement")
                assert replaced["screens"][origin_id]["name"] == origin_name
                assert len(replaced["screens"][origin_id]["panes"]) == 3
                assert len(replaced["screens"][origin_id]["floating"]) == 1
                assert all(entry["pane"] in shell_routes(replaced) and alive(pid)
                           for pid, entry in snapshot_shells().items()
                           if pid not in {shell["pid"], second["pid"]})
                assert (owned / "executed.marker").read_text() == "RUN\n"
                assert alive(saved_sleep), "replacement affected an unrelated Screen's foreground command"
                capture("confirmed-replacement")
                result["checks"].append("replacement-ends-captured-sessions-retains-unrelated-identities")

                # The saved Screen's live foreground command makes whole-window
                # shutdown a busy close, not a test cleanup.
                before_close_pixels = display.pixels(window, (380, 260, 440, 220))
                known_children.update(set(performance.process_stats(app.pid)) - {app.pid})
                display.close_window(window)
                wait_until(lambda: display.pixels(window, (380, 260, 440, 220)) != before_close_pixels,
                           "busy window close prompt did not paint")
                capture("busy-window-close-prompt")
                assert app.poll() is None, "window exited without busy confirmation"
                control("window.confirm_close")
                app.wait(timeout=12)
                assert app.returncode == 0, f"window exit {app.returncode}"
                assert all(not alive(pid) for pid in shells()), "owned shell survived confirmed close"
                assert all(not alive(pid) for pid in known_children), "owned process survived confirmed close"
                result["checks"].append("normal-confirmed-window-close-joins-owned-shells")
            else:
                open_templates()
                capture("initial-template-library")
                click(390, 381)
                capture("calibration-extract-form")
                click(300, 426)
                type_text(str(owned.resolve()))
                click(390, 468)
                time.sleep(.3)
                capture("calibration-save-form")
                press("Escape")
                open_templates()
                click(390, 421)
                capture("calibration-apply-form")
                click(390, 482)
                capture("calibration-apply-review")
                press("Escape")
                capture("calibration-after-review-escape")
                click(905, 281)  # Visible Cancel; keep calibration independent of Escape.
                click(100, 120)
                type_text("exit")
                press("Return")
                wait_until(lambda: not alive(shell["pid"]), "setup shell did not exit normally")

            result["passed"] = True
        except Exception as error:
            result["error"] = repr(error)
            if display and window:
                try:
                    capture("failure")
                except Exception as capture_error:
                    result["captureError"] = repr(capture_error)
        finally:
            descendants = (set(performance.process_stats(app.pid)) - {app.pid} if app else set()) | known_children
            if app and app.poll() is None and display and window:
                try:
                    prompt_rect = (455, 300, 289, 160)
                    before_prompt = display.pixels(window, prompt_rect)
                    display.close_window(window)
                    time.sleep(.5)
                    if app.poll() is None and display.pixels(window, prompt_rect) != before_prompt:
                        capture("cleanup-close-prompt")
                        click(600, 422)
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
