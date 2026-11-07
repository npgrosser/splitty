#!/usr/bin/env python3
"""Inspect the passive native Hosts view in a private real window.

The fixture owns its config, shell and an SSH spy. Pointer hit regions are
supplied by a separately calibrated geometry JSON. The control socket is read
only; Refresh/Retry and navigation use actual window input.
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
import time

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


def write_json(path, value):
    pending = path.with_name("." + path.name + ".pending")
    pending.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")
    pending.chmod(0o600)
    pending.replace(path)


def fixture(config, output, zoom, font_size):
    long_target = ".".join(("long-cache-target-" + str(i) + "-" + "x" * 30) for i in range(4))
    targets = ["build-cache.example", "empty-cache.example", "unknown-cache.example",
               long_target] + [f"extra-{i:02d}.example" for i in range(12)]
    aliases = {targets[0]: "Build cache", targets[1]: "Empty cache",
               targets[2]: "Never observed", targets[3]: "Long target " + "A" * 90}
    config.joinpath("config.toml").write_text(
        '[appearance]\ntheme_mode = "fixed"\ntheme = "splitty-dark"\n'
        f'zoom = {zoom}\nzoom_scope = "window"\nfont_size = {font_size}\n\n'
        '[drawer]\npinned = false\nsidebar_width = 280\n\n'
        '[shells]\ndefault_profile_id = "hosts-test"\n\n'
        '[[shells.profiles]]\nid = "hosts-test"\nname = "Owned shell"\n'
        f'command = {json.dumps(str(output.joinpath("sh").resolve()))}\n'
        'enabled = true\ntarget = "local"\nsource = "custom"\n'
    )
    write_json(config / "recent-hosts.json", {"version": 1, "hosts": [
        {"target": target, "lastConnected": 1760000000 - index}
        for index, target in enumerate(targets)]})
    write_json(config / "remote-host-preferences.json", {"version": 1, "hosts": [
        {"target": target, "alias": alias} for target, alias in aliases.items()]})
    write_json(config / "remote-host-order.json", {"version": 1, "order": targets})
    small = {"version": 1, "hosts": [
        {"target": targets[0], "lastSeen": 1760000100, "screens": [
            {"id": "work", "name": "Build Work", "panes": 3},
            {"id": "long-screen", "name": "Long cached Screen " + "N" * 150, "panes": 12}]},
        {"target": targets[1], "lastSeen": 1760000200, "screens": []},
        {"target": targets[3], "lastSeen": 1760000300, "screens": [
            {"id": "cached-long", "name": "Long target observation", "panes": 1}]},
    ]}
    write_json(config / "remote-catalog.json", small)
    return small, targets


def oversized_catalog():
    # Every row is valid under the stored catalog's 20 hosts × 200 Screens,
    # 256-character ID and 200-character name limits. The resulting directory
    # exceeds the native API reader's 1 MiB body ceiling after serialization.
    return {"version": 1, "hosts": [
        {"target": f"bulk-{host:02d}.example", "lastSeen": 1760000400,
         "screens": [
             {"id": f"h{host:02d}s{screen:03d}-" + "i" * 246,
              "name": "n" * 200, "panes": screen}
             for screen in range(200)]}
        for host in range(20)]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--display", default=":98")
    parser.add_argument("--xvfb", default="Xvfb")
    parser.add_argument("--xvfb-lib", type=Path)
    parser.add_argument("--xdotool", default="xdotool")
    parser.add_argument("--geometry", type=Path)
    parser.add_argument("--zoom", type=float, default=1.35)
    parser.add_argument("--font-size", type=int, default=0)
    parser.add_argument("--setup-only", action="store_true")
    parser.add_argument("--setup-hosts", action="store_true")
    args = parser.parse_args()
    if not args.setup_only and not args.geometry:
        parser.error("full acceptance requires screenshot-calibrated --geometry JSON")
    if not 0.5 <= args.zoom <= 3.0:
        parser.error("--zoom must be between 0.5 and 3.0")
    if args.font_size != 0 and not 8 <= args.font_size <= 24:
        parser.error("--font-size must be 0 or between 8 and 24")
    binary = args.binary.resolve(strict=True)
    geometry = json.loads(args.geometry.read_text()) if args.geometry else {}
    args.output.mkdir(parents=True, exist_ok=False)
    owned = args.output / "owned"
    owned.mkdir(mode=0o700)
    config = args.output / "config"
    config.mkdir(mode=0o700)
    spy_bin = args.output / "bin"
    spy_bin.mkdir(mode=0o700)
    if len(os.fsencode(config / "run" / ("spike-" + "0" * 32 + ".sock"))) > 107:
        parser.error("shorten --output for the owned Unix socket")
    wrapper = args.output / "sh"
    wrapper.write_text(
        "#!/bin/sh\n"
        f"cd {shlex.quote(str(owned.resolve()))} || exit 1\n"
        f"printf '%s\\t%s\\t%s\\n' \"$$\" \"$SPLITTY_PANE_ID\" \"$SPLITTY_SESSION_ID\" > {shlex.quote(str(owned.resolve()))}/shell-$$.tsv\n"
        "exec /bin/sh \"$@\"\n"
    )
    wrapper.chmod(0o700)
    ssh_spy = spy_bin / "ssh"
    ssh_spy.write_text("#!/bin/sh\n" +
                       f"printf '%s\\n' \"$*\" >> {shlex.quote(str(args.output / 'ssh-called.txt'))}\n" +
                       "exit 97\n")
    ssh_spy.chmod(0o700)
    small, targets = fixture(config, args.output, args.zoom, args.font_size)
    large = oversized_catalog()
    large_bytes = json.dumps(large, ensure_ascii=False).encode()
    assert len(large_bytes) > 1_500_000, len(large_bytes)
    result = {"binarySha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "harnessSha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "geometrySha256": hashlib.sha256(args.geometry.read_bytes()).hexdigest() if args.geometry else None,
              "initialCatalogSha256": hashlib.sha256((config / "remote-catalog.json").read_bytes()).hexdigest(),
              "largeCatalogBytes": len(large_bytes), "targets": targets,
              "checks": [], "states": {}, "screenshots": [], "passed": False}
    env = os.environ.copy()
    env.pop("WAYLAND_DISPLAY", None)
    env["DISPLAY"] = args.display
    env["GPUI_BACKEND"] = "x11"
    env["SPLITTY_CONFIG_DIR"] = str(config.resolve())
    env["PATH"] = str(spy_bin.resolve()) + ":" + env.get("PATH", "/usr/bin:/bin")
    for key in ("SPLITTY_SOCK", "SPLITTY_AGENT_SOCK", "SPLITTY_ADDR", "SPLITTY_TOKEN",
                "SPLITTY_SERVER", "SPLITTY_PANE_ID", "SPLITTY_SCREEN_ID", "SPLITTY_SESSION_ID"):
        env.pop(key, None)
    xvfb_env = env.copy()
    if args.xvfb_lib:
        xvfb_env["LD_LIBRARY_PATH"] = str(args.xvfb_lib) + (
            ":" + env["LD_LIBRARY_PATH"] if env.get("LD_LIBRARY_PATH") else "")
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

    def click(name):
        x("mousemove", "--window", window, *point(name))
        x("click", 1)
        display.sync()
        time.sleep(.2)

    def wheel(name, button):
        x("mousemove", "--window", window, *point(name))
        display.wheel(button)
        display.sync()
        time.sleep(.2)

    def key(name):
        display.type_key(name)
        display.sync()
        time.sleep(.12)

    def type_text(value):
        x("type", "--clearmodifiers", "--delay", 3, value)
        display.sync()

    def capture(name):
        path = args.output / f"{name}.png"
        pane_drag.save_png(path, display.pixels(window, (0, 0, 1200, 760)))
        result["screenshots"].append(str(path))

    def state(name=None):
        def socket():
            candidates = list((config / "run").glob("spike-*.sock"))
            assert len(candidates) <= 1
            return candidates[0] if candidates and stat.S_ISSOCK(candidates[0].lstat().st_mode) else None
        owned_socket = wait_until(socket, "owned service socket did not bind", seconds=6)
        body = subprocess.check_output(
            ["curl", "--fail", "--silent", "--show-error", "--max-time", "5",
             "--unix-socket", str(owned_socket), "http://localhost/api/mux/state"],
            text=True, timeout=7)
        value = json.loads(body)
        if name:
            result["states"][name] = value
        return value

    def shells():
        found = {}
        for marker in owned.glob("shell-*.tsv"):
            pid, pane, session = marker.read_text().strip().split("\t")
            found[int(pid)] = {"pane": pane, "session": session}
        return found

    def assert_no_ssh():
        assert not (args.output / "ssh-called.txt").exists(), "Hosts view invoked ssh"

    def close_app():
        nonlocal app, window
        if app and app.poll() is None and display and window:
            display.close_window(window)
            time.sleep(.4)
            if app.poll() is None:
                exists = subprocess.run([str(args.xdotool), "getwindowgeometry", "--shell", str(window)],
                                        env=env, capture_output=True, timeout=3).returncode == 0
                if exists:
                    capture("close-prompt")
                if exists and "window.confirm_close" in geometry:
                    click("window.confirm_close")
            app.wait(timeout=12)
            assert app.returncode == 0, f"window exit {app.returncode}"
            result["normalWindowClose"] = True
            result["exitCode"] = app.returncode
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
            wait_until(lambda: any(alive(pid) for pid in shells()), "owned shell did not start", seconds=30)
            initial = state("initial")
            screen = initial["activeScreenId"]
            pane = next(iter(initial["screens"][screen]["panes"]))
            session = initial["screens"][screen]["panes"][pane]["sessionId"]
            pid = next(pid for pid, shell in shells().items()
                       if alive(pid) and shell == {"pane": pane, "session": session})
            capture("setup")
            assert_no_ssh()
            result["checks"].append("isolated-window-catalog-and-terminal")
            if args.setup_only:
                type_text("exit")
                key("Return")
                wait_until(lambda: not alive(pid), "setup shell did not exit")
            elif args.setup_hosts:
                click("hosts.toolbar")
                time.sleep(.5)
                capture("hosts-initial")
                key("Escape")
                type_text("exit")
                key("Return")
                wait_until(lambda: not alive(pid), "setup shell did not exit")
            else:
                def terminal_marker(name):
                    marker = owned / f"{name}.marker"
                    type_text(f"printf '%s\\n' \"$$:$SPLITTY_PANE_ID:$SPLITTY_SESSION_ID\" > {shlex.quote(str(marker))}")
                    key("Return")
                    value = wait_until(lambda: marker.read_text().strip() if marker.exists() else None,
                                       f"{name} did not reach the terminal")
                    assert value == f"{pid}:{pane}:{session}", value

                terminal_marker("before-hosts")
                terminal_before = state("terminal-before-hosts")
                main_before = display.pixels(window, region("hosts.main_region"))
                click("hosts.toolbar")
                wait_until(lambda: display.pixels(window, region("hosts.main_region")) != main_before,
                           "Hosts navigation did not replace terminal view")
                time.sleep(.5)
                capture("hosts-initial")
                blank_count = display.pixels(window, region("hosts.count_blank_region"))
                for label in ("hosts.count_3_region", "hosts.count_12_region"):
                    assert display.pixels(window, region(label)) != blank_count, (
                        f"cached Screen pane count is not visibly rendered: {label}")
                assert_no_ssh()
                result["checks"].append("navigate-to-passive-hosts-with-cached-unknown-and-empty-observations")

                top_rows = display.pixels(window, region("hosts.list_region"))
                for _ in range(6):
                    wheel("hosts.list", 5)
                assert display.pixels(window, region("hosts.list_region")) != top_rows, (
                    "Hosts list did not scroll")
                capture("hosts-scrolled")
                assert display.pixels(window, region("hosts.count_1_region")) != blank_count, (
                    "long target Screen pane count is not visibly rendered")
                for _ in range(12):
                    wheel("hosts.list", 4)
                capture("hosts-top-before-refresh")
                key("r")
                time.sleep(.6)
                capture("hosts-keyboard-refresh")
                assert_no_ssh()
                result["checks"].append("hosts-scroll-and-keyboard-refresh-without-ssh")

                cached_rows = display.pixels(window, region("hosts.cached_rows_region"))
                status_ready = display.pixels(window, region("hosts.status_region"))
                refresh_button = display.pixels(window, region("hosts.refresh_region"))
                write_json(config / "remote-catalog.json", large)
                result["largeCatalogSha256"] = hashlib.sha256(
                    (config / "remote-catalog.json").read_bytes()).hexdigest()
                key("r")
                wait_until(lambda: display.pixels(window, region("hosts.status_region")) != status_ready,
                           "oversized response did not change Hosts status", seconds=10)
                time.sleep(1.5)
                failed_status = display.pixels(window, region("hosts.status_region"))
                assert failed_status != status_ready, "Hosts failure status did not persist"
                assert display.pixels(window, region("hosts.refresh_region")) != refresh_button, (
                    "failed refresh did not show Retry")
                assert display.pixels(window, region("hosts.cached_rows_region")) == cached_rows, (
                    "failed refresh replaced the last-good host rows")
                capture("hosts-refresh-failed-cached")
                assert_no_ssh()
                result["checks"].append("valid-oversized-catalog-causes-real-refresh-failure-with-last-good")

                key("Escape")
                wait_until(lambda: display.pixels(window, region("hosts.main_region")) == main_before,
                           "Escape did not return to terminal view", seconds=8)
                terminal_marker("after-failed-refresh")
                returned = state("terminal-after-failed-refresh")
                assert returned["activeScreenId"] == screen
                assert returned["screens"][screen]["panes"][pane]["sessionId"] == session
                assert terminal_before["screens"][screen]["panes"].keys() == returned["screens"][screen]["panes"].keys()
                assert alive(pid)
                capture("terminal-after-failed-refresh")
                result["checks"].append("failed-refresh-preserves-terminal-session-and-keyboard-focus")

                click("hosts.toolbar")
                time.sleep(.2)
                write_json(config / "remote-catalog.json", small)
                key("r")
                wait_until(lambda: display.pixels(window, region("hosts.status_region")) != failed_status,
                           "Retry did not clear Hosts failure", seconds=10)
                time.sleep(.6)
                assert display.pixels(window, region("hosts.refresh_region")) == refresh_button, (
                    "Retry did not restore Refresh control")
                assert display.pixels(window, region("hosts.cached_rows_region")) == cached_rows, (
                    "Retry did not restore the original host rows")
                capture("hosts-retry-recovered")
                assert_no_ssh()
                click("hosts.back")
                terminal_marker("after-retry-back")
                final = state("final")
                assert final["screens"][screen]["panes"][pane]["sessionId"] == session
                assert alive(pid)
                assert_no_ssh()
                result["checks"].append("retry-recovers-and-back-preserves-terminal-input")
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
                            app.wait(timeout=5)
            if display:
                display.close()
            if xvfb:
                os.killpg(xvfb.pid, signal.SIGTERM)
                xvfb.wait(timeout=5)
            result["survivingChildren"] = [pid for pid in children if alive(pid)]
            result["passed"] = (result["passed"] and not result["survivingChildren"]
                                and result.get("normalWindowClose", False)
                                and result.get("exitCode") == 0 and not result.get("closeError"))
            (args.output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps({"passed": result["passed"], "checks": result["checks"],
                              "error": result.get("error"), "result": str(args.output / "result.json")}, indent=2))
    if not result["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
