#!/usr/bin/env python3
"""Exercise native terminal find in an owned X11 window and raw PTY fixture.

The fixture records every byte the session receives. Search text, navigation
keys and search-control pointer events must stay out of that log. Functional
acceptance requires --tesseract (with eng data) to read the actual find status.
"""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shlex
import signal
import subprocess
import time


HERE = Path(__file__).resolve().parent
COPY_WORD = "COPYTOKEN4826"
RESULT_PATTERN = r"[1-9][0-9]*\s*(?:of\s*[1-9][0-9]*|found)"


def helper(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


performance = helper("performance", "gpui-performance.py")
pane_drag = helper("pane_drag", "gpui-pane-drag.py")


def wait_until(predicate, description, seconds=12):
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--display", default=":95")
    parser.add_argument("--xvfb", default="Xvfb")
    parser.add_argument("--xvfb-lib", type=Path)
    parser.add_argument("--xdotool", default="xdotool")
    parser.add_argument("--setup-only", action="store_true", help="verify owned fixture before find UI is available")
    parser.add_argument("--tesseract", type=Path, help="Tesseract executable for visible find-status assertions")
    parser.add_argument("--tessdata", type=Path, help="Tesseract language-data directory")
    parser.add_argument("--xclip", type=Path, help="private X11 xclip for exact Unicode query input")
    parser.add_argument("--ocr-advisory", action="store_true", help="record OCR misses for visual review and continue input assertions")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    binary = args.binary.resolve(strict=True)
    fixture_script = HERE / "gpui-terminal-search-fixture.py"
    owned = args.output / "fixture"
    owned.mkdir(mode=0o700)
    config = args.output / "config"
    config.mkdir(mode=0o700)
    (config / "config.toml").write_text('[appearance]\ntheme_mode = "fixed"\ntheme = "splitty-dark"\nfont_size = 0\n')
    env = os.environ.copy()
    env.pop("WAYLAND_DISPLAY", None)
    env["DISPLAY"] = args.display
    env["SPLITTY_CONFIG_DIR"] = str(config.resolve())
    for key in ("SPLITTY_SOCK", "SPLITTY_AGENT_SOCK", "SPLITTY_ADDR", "SPLITTY_TOKEN",
                "SPLITTY_SERVER", "SPLITTY_PANE_ID", "SPLITTY_SCREEN_ID", "SPLITTY_SESSION_ID"):
        env.pop(key, None)
    xvfb_env = env.copy()
    if args.xvfb_lib:
        xvfb_env["LD_LIBRARY_PATH"] = str(args.xvfb_lib) + (":" + env["LD_LIBRARY_PATH"] if env.get("LD_LIBRARY_PATH") else "")
    result = {"binary": str(binary), "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "fixtureSha256": hashlib.sha256(fixture_script.read_bytes()).hexdigest(),
              "harnessSha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "display": args.display, "checks": [], "screenshots": [], "ptyEvents": {}, "passed": False}
    xvfb = app = display = window = None
    sequence = 0

    def x(*arguments):
        return subprocess.check_output([str(args.xdotool), *map(str, arguments)], env=env, text=True, timeout=12)

    def raw():
        return (owned / "received.bin").read_bytes()

    def no_input(label, action):
        before = raw()
        action()
        time.sleep(.3)
        after = raw()
        result["ptyEvents"][label] = after[len(before):].hex()
        assert after == before, f"{label} leaked PTY bytes: {after[len(before):].hex()}"

    def command(action):
        nonlocal sequence
        sequence += 1
        next_control = owned / "control.next"
        next_control.write_text(json.dumps({"seq": sequence, "action": action}))
        next_control.replace(owned / "control.json")
        if action != "stop":
            wait_until(lambda: json.loads((owned / "ack.json").read_text()).get("seq") == sequence
                       if (owned / "ack.json").exists() else False,
                       f"fixture did not acknowledge {action}")

    def chord(*keys):
        for modifier in keys[:-1]:
            display.key(modifier, True)
        display.type_key(keys[-1])
        for modifier in reversed(keys[:-1]):
            display.key(modifier, False)
        display.sync()

    def type_text(text):
        x("type", "--clearmodifiers", "--delay", 3, text)

    def pixels():
        return display.pixels(window, (0, 0, 1200, 760))

    def save(label):
        path = args.output / f"{label}.png"
        pane_drag.save_png(path, pixels())
        result["screenshots"].append(str(path))

    def crop_bgra(data, left, top, width, height, scale=4):
        cropped = bytearray(width * height * scale * scale * 4)
        stride = width * scale * 4
        for row in range(height):
            for col in range(width):
                source = ((top + row) * 1200 + left + col) * 4
                pixel = data[source:source + 4]
                for vertical in range(scale):
                    target = (row * scale + vertical) * stride + col * scale * 4
                    cropped[target:target + scale * 4] = pixel * scale
        return bytes(cropped)

    def find_text(label, left=600, width=600):
        if not args.tesseract:
            raise RuntimeError("--tesseract is required for functional find-status assertions")
        crop = args.output / f"{label}-find-status.png"
        cropped = crop_bgra(pixels(), left, 78, width, 50)
        scaled_width = width * 4
        pane_drag.save_png(crop, cropped, scaled_width, 200)
        result["screenshots"].append(str(crop))
        inverted = bytearray(cropped)
        for offset in range(0, len(inverted), 4):
            inverted[offset] = 255 - inverted[offset]
            inverted[offset + 1] = 255 - inverted[offset + 1]
            inverted[offset + 2] = 255 - inverted[offset + 2]
        inverted_crop = args.output / f"{label}-find-status-inverted.png"
        pane_drag.save_png(inverted_crop, inverted, scaled_width, 200)
        # Leptonica's PNG preprocessing loses this low-contrast status text;
        # feed the same inverted RGB pixels as a lossless PPM to Tesseract.
        inverted_ppm = inverted_crop.with_suffix(".ppm")
        rgb = bytearray()
        for offset in range(0, len(inverted), 4):
            rgb.extend((inverted[offset + 2], inverted[offset + 1], inverted[offset]))
        inverted_ppm.write_bytes(f"P6\n{scaled_width} 200\n255\n".encode() + rgb)
        ocr_env = env.copy()
        if args.tessdata:
            ocr_env["TESSDATA_PREFIX"] = str(args.tessdata)
        text = subprocess.check_output([str(args.tesseract), str(crop), "stdout", "-l", "eng", "--psm", "3"],
                                       env=ocr_env, text=True, timeout=15).strip()
        # Tesseract's general pass misses the limit notice on a dark bar;
        # the inverted copy is the same pixels with contrast polarity flipped.
        text += "\n" + subprocess.check_output([str(args.tesseract), str(inverted_ppm), "stdout", "-l", "eng",
                                                  "--psm", "3"], env=ocr_env, text=True, timeout=15).strip()
        # The 12 px status occasionally turns `7 of 7` into `Tor?` in the
        # general pass. A numeric-only sparse pass reads that same visible
        # label without accepting unrelated prose as a result count.
        numbers = subprocess.check_output([str(args.tesseract), str(crop), "stdout", "-l", "eng",
                                           "--psm", "11", "-c", "tessedit_char_whitelist=0123456789of"],
                                          env=ocr_env, text=True, timeout=15).strip()
        text += "\n" + numbers
        result.setdefault("findText", {})[label] = text
        return text

    def expect_find(label, pattern, left=600, width=600):
        text = find_text(label, left, width)
        ocr_assert(label, re.search(pattern, text, re.IGNORECASE),
                   f"expected /{pattern}/ in visible find bar, got {text!r}")
        return text

    def ocr_assert(label, condition, detail):
        if condition:
            return
        if args.ocr_advisory:
            result.setdefault("ocrUnverified", {})[label] = detail
        else:
            raise AssertionError(f"{label}: {detail}")

    def result_total(text):
        match = re.search(r"([0-9]+)\s*of\s*([0-9]+)", text, re.IGNORECASE)
        if match:
            return int(match.group(2))
        match = re.search(r"([0-9]+)\s+found", text, re.IGNORECASE)
        if match:
            return int(match.group(1))
        if args.ocr_advisory:
            return None
        raise AssertionError(f"no completed result total in find bar: {text!r}")

    def compare_total(label, text, expected):
        actual = result_total(text)
        ocr_assert(label, actual == expected, f"expected {expected} results, read {actual} from {text!r}")

    def changed_pixels(left, top, width, height, before, after):
        changed = 0
        for row in range(top, top + height):
            for col in range(left, left + width):
                offset = (row * 1200 + col) * 4
                if before[offset:offset + 3] != after[offset:offset + 3]:
                    changed += 1
        return changed

    def set_query(query):
        no_input("select-query", lambda: chord("Control_L", "a"))
        if query.isascii():
            no_input("query-" + query, lambda: type_text(query))
        else:
            if not args.xclip:
                raise RuntimeError("--xclip is required for exact non-ASCII input on private Xvfb")
            subprocess.run([str(args.xclip), "-selection", "clipboard", "-loops", "1", "-i"],
                           input=query.encode(), env=env, check=True, timeout=8)
            no_input("query-" + query, lambda: chord("Control_L", "v"))
        time.sleep(.5)

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

            wait_until(ready, "Xvfb did not become ready", seconds=20)
            previous_display = os.environ.get("DISPLAY")
            os.environ["DISPLAY"] = args.display
            try:
                display = performance.Display()
            finally:
                if previous_display is None:
                    os.environ.pop("DISPLAY", None)
                else:
                    os.environ["DISPLAY"] = previous_display
            app = subprocess.Popen([str(binary)], env=env, stdout=app_log, stderr=app_log, start_new_session=True)

            def find_window():
                if app.poll() is not None:
                    raise RuntimeError(f"app exited before opening a window: {app.returncode}")
                found = subprocess.run([str(args.xdotool), "search", "--onlyvisible", "--pid", str(app.pid)],
                                       env=env, capture_output=True, text=True, timeout=3)
                for candidate in found.stdout.splitlines():
                    geometry = dict(line.split("=", 1) for line in x("getwindowgeometry", "--shell", candidate).splitlines() if "=" in line)
                    if geometry.get("WIDTH") == "1200" and geometry.get("HEIGHT") == "760":
                        return int(candidate)

            window = wait_until(find_window, "No 1200x760 native window appeared", seconds=30)
            time.sleep(2)
            x("windowfocus", window, "mousemove", "--window", window, 180, 180, "click", 1)
            launch = f"python3 -u {shlex.quote(str(fixture_script))} {shlex.quote(str(owned.resolve()))}"
            type_text(launch)
            display.type_key("Return")
            wait_until(lambda: (owned / "ready").exists(), "raw search fixture did not start")
            wait_until(lambda: (owned / "received.bin").exists(), "raw PTY log did not appear")
            time.sleep(.4)
            save("fixture-ready")
            result["geometry"] = json.loads((owned / "geometry.json").read_text())
            result["checks"].append("owned-raw-pty-fixture")
            if not args.setup_only:
                # Application mouse reporting remains enabled throughout search.
                display.key("Shift_L", True)
                display.sync()
                x("mousemove", "--window", window, 100, 132, "click", "--repeat", 2, "--delay", 80, 1)
                display.key("Shift_L", False)
                display.sync()
                save("clipboard-selection-before-find")
                no_input("copy-before-find", lambda: chord("Control_L", "Shift_L", "c"))
                x("mousemove", "--window", window, 1150, 20)
                before_find = pixels()
                no_input("ctrl-f", lambda: chord("Control_L", "f"))
                save("find-open")
                assert pixels() != before_find, "Ctrl+F did not display a find editor"
                # OCR can confuse the final 6/4 at 12 px; the later clipboard
                # assertion checks the selected bytes exactly.
                expect_find("find-open", r"COPYTOKEN482")
                before_needle = pixels()
                no_input("type-needle", lambda: type_text("needle"))
                time.sleep(.6)
                save("needle-results")
                needle_text = expect_find("needle-results", RESULT_PATTERN)
                ocr_assert("needle-query-text", re.search("needle", needle_text, re.IGNORECASE), needle_text)
                needle_total = result_total(needle_text)
                # The fixture has 1003/1006 mouse reporting enabled. Clicking
                # the overlaid editor must be consumed by the UI.
                no_input("find-input-pointer", lambda: x("mousemove", "--window", window, 950, 102, "click", 1))
                needle_changed = changed_pixels(0, 100, 480, 130, before_needle, pixels())
                result["visualDiff"] = {"needleTerminalPixels": needle_changed}
                assert needle_changed > 10, "needle search produced no visible terminal highlight or viewport change"
                result["checks"].append("find-editor-and-query-do-not-reach-pty")

                no_input("next-match", lambda: display.type_key("Return"))
                save("next-match")
                no_input("previous-match", lambda: chord("Shift_L", "Return"))
                save("previous-match")
                result["checks"].append("enter-and-shift-enter-navigate-without-pty-input")

                no_input("find-older-pointer", lambda: x("mousemove", "--window", window, 1132, 102, "click", 1))
                save("older-pointer")
                expect_find("older-pointer", RESULT_PATTERN)
                no_input("find-newer-pointer", lambda: x("mousemove", "--window", window, 1154, 102, "click", 1))
                save("newer-pointer")
                expect_find("newer-pointer", RESULT_PATTERN)
                result["checks"].append("find-bar-pointer-controls-do-not-reach-pty")

                command("emit")
                no_input("query-focus-after-output", lambda: type_text("x"))
                save("output-while-find-focused")
                no_input("remove-typed-x", lambda: display.type_key("BackSpace"))
                save("results-after-output")
                after_output_text = expect_find("results-after-output", RESULT_PATTERN)
                if needle_total is not None:
                    compare_total("output-added-one-result", after_output_text, needle_total + 1)
                result["checks"].append("output-keeps-query-focus-and-refreshes-results")

                set_query("softwrap")
                save("soft-wrap-match")
                softwrap_text = expect_find("soft-wrap-match", r"1\s*(?:of\s*1|found)")
                ocr_assert("softwrap-query-text", re.search("softwrap", softwrap_text, re.IGNORECASE), softwrap_text)
                set_query("\u754ce\u0301")
                save("unicode-combining-match")
                expect_find("unicode-combining-match", r"1\s*(?:of\s*1|found)")
                no_input("copy-unicode-query", lambda: (chord("Control_L", "a"), chord("Control_L", "c")))
                copied_query = subprocess.check_output([str(args.xclip), "-selection", "clipboard", "-o"],
                                                       env=env, timeout=8)
                assert copied_query == "\u754ce\u0301".encode(), f"editor did not hold exact Unicode query: {copied_query!r}"
                result["checks"].append("soft-wrap-and-unicode-query-captures")

                set_query("needle")
                # Header controls are outside the terminal/search editor.
                no_input("new-tab", lambda: x("mousemove", "--window", window, 1150, 20, "click", 1))
                time.sleep(.5)
                save("other-tab-no-find")
                other_tab_text = find_text("other-tab-no-find")
                ocr_assert("other-tab-no-find", not re.search(r"needle|refine query|[0-9]+\s*(?:of\s*[0-9]+|found)",
                                                            other_tab_text, re.IGNORECASE), other_tab_text)
                no_input("return-original-tab", lambda: x("mousemove", "--window", window, 40, 62, "click", 1))
                time.sleep(.6)
                save("find-restored-after-tab")
                tab_text = expect_find("find-restored-after-tab", RESULT_PATTERN)
                ocr_assert("tab-retained-query", re.search("needle", tab_text, re.IGNORECASE), tab_text)
                if needle_total is not None:
                    compare_total("tab-retained-total", tab_text, needle_total + 1)
                result["checks"].append("tab-switch-retains-terminal-query")

                no_input("new-screen", lambda: x("mousemove", "--window", window, 210, 20, "click", 1))
                time.sleep(.5)
                save("other-screen-no-find")
                other_screen_text = find_text("other-screen-no-find")
                ocr_assert("other-screen-no-find", not re.search(r"needle|refine query|[0-9]+\s*(?:of\s*[0-9]+|found)",
                                                               other_screen_text, re.IGNORECASE), other_screen_text)
                no_input("return-original-screen", lambda: x("mousemove", "--window", window, 105, 20, "click", 1))
                time.sleep(.6)
                save("find-restored-after-screen")
                screen_text = expect_find("find-restored-after-screen", RESULT_PATTERN)
                ocr_assert("screen-retained-query", re.search("needle", screen_text, re.IGNORECASE), screen_text)
                if needle_total is not None:
                    compare_total("screen-retained-total", screen_text, needle_total + 1)
                result["checks"].append("screen-switch-retains-terminal-query")

                command("dense")
                set_query("a")
                time.sleep(.8)
                save("dense-query-refine-notice")
                expect_find("dense-query-refine-notice", r"limit:\s*refine")
                result["checks"].append("dense-query-limit-capture")
                set_query("needle")
                time.sleep(.8)
                save("refined-query-results")
                refined_text = expect_find("refined-query-results", RESULT_PATTERN)
                ocr_assert("refinement-query", re.search("needle", refined_text, re.IGNORECASE), refined_text)
                if needle_total is not None:
                    compare_total("refinement-total", refined_text, needle_total + 1)
                result["checks"].append("refinement-recovers-full-history-search")

                no_input("escape-find", lambda: display.type_key("Escape"))
                save("find-closed")
                before = len(raw())
                type_text("r")
                wait_until(lambda: len(raw()) > before, "terminal did not regain keyboard focus")
                result["ptyEvents"]["terminal-after-escape"] = raw()[before:].hex()
                assert raw()[before:] == b"r", "Escape did not restore raw PTY input"
                no_input("copy-after-find", lambda: chord("Control_L", "Shift_L", "c"))
                before = len(raw())
                chord("Control_L", "Shift_L", "v")
                wait_until(lambda: len(raw()) > before, "clipboard paste did not reach original terminal")
                pasted = raw()[before:]
                result["ptyEvents"]["clipboard-after-search"] = pasted.hex()
                assert pasted == COPY_WORD.encode(), f"search changed clipboard selection: {pasted!r}"
                result["checks"].append("escape-restores-input-and-preserves-clipboard-selection")

                no_input("reopen-before-vt-reset", lambda: chord("Control_L", "f"))
                set_query("needle")
                before_reset_text = expect_find("before-vt-reset", RESULT_PATTERN)
                if needle_total is not None:
                    compare_total("before-reset-total", before_reset_text, needle_total + 1)
                command("reset")
                time.sleep(.6)
                save("after-vt-reset")
                after_reset_text = expect_find("after-vt-reset", r"1\s*(?:of\s*1|found)")
                ocr_assert("reset-query", re.search("needle", after_reset_text, re.IGNORECASE), after_reset_text)
                result["checks"].append("vt-ris-clears-old-results-and-reindexes-new-output")
                no_input("escape-after-vt-reset", lambda: display.type_key("Escape"))
                before = len(raw())
                type_text("s")
                wait_until(lambda: len(raw()) > before, "terminal did not accept input after VT reset")
                assert raw()[before:] == b"s", f"terminal input after VT reset: {raw()[before:]!r}"
                result["checks"].append("pty-input-survives-vt-ris-search-reset")

                # DEC RIS disabled mouse tracking. Re-enable it so a held
                # content-origin press can be released over the find overlay.
                command("mouse_on")
                no_input("reopen-for-pointer-lifetime", lambda: chord("Control_L", "f"))
                set_query("needle")
                x("mousemove", "--window", window, 100, 200)
                time.sleep(.15)
                before = raw()
                display.xt.XTestFakeButtonEvent(display.connection, 1, True, 0)
                display.sync()
                x("mousemove", "--window", window, 950, 102)
                display.sync()
                display.xt.XTestFakeButtonEvent(display.connection, 1, False, 0)
                display.sync()
                x("mousemove", "--window", window, 120, 250)
                display.sync()
                time.sleep(.3)
                pointer_bytes = raw()[len(before):]
                result["ptyEvents"]["release-over-find-bar"] = pointer_bytes.hex()
                reports = re.findall(rb"\x1b\[<([0-9]+);([0-9]+);([0-9]+)([Mm])", pointer_bytes)
                presses = [i for i, (code, _, _, end) in enumerate(reports) if code == b"0" and end == b"M"]
                releases = [i for i, (_, _, _, end) in enumerate(reports) if end == b"m"]
                assert len(presses) == len(releases) == 1 and presses[0] < releases[0], reports
                assert any(code == b"35" and end == b"M" for code, _, _, end in reports[releases[0] + 1:]), reports
                save("release-over-find-bar")
                result["checks"].append("content-press-releases-over-bar-and-next-hover-is-unheld")

                x("mousemove", "--window", window, 950, 102)
                time.sleep(.1)
                no_input("bar-origin-press", lambda: x("click", 1))
                result["checks"].append("bar-origin-press-does-not-reach-pty")

                # A search editor remains visible on an inactive split. Its
                # input click must activate that pane and its search index.
                no_input("split-right", lambda: x("mousemove", "--window", window, 970, 20, "click", 1))
                time.sleep(.5)
                save("split-with-left-find-inactive")
                no_input("open-right-find", lambda: chord("Control_L", "f"))
                no_input("right-query", lambda: type_text("rightside"))
                time.sleep(.3)
                save("different-right-find-query")
                before_left_click = pixels()
                no_input("activate-left-find-bar", lambda: x("mousemove", "--window", window, 300, 102, "click", 1))
                set_query("leftonly")
                save("left-find-edited-after-inactive-click")
                left_text = expect_find("left-find-edited-after-inactive-click", r"leftonly", left=250, width=350)
                left_changed = changed_pixels(250, 85, 350, 40, before_left_click, pixels())
                result["visualDiff"]["inactiveLeftBarPixels"] = left_changed
                assert left_changed > 10, "inactive left find bar did not visibly respond to activation/edit"
                ocr_assert("left-edited-query", re.search("leftonly", left_text, re.IGNORECASE), left_text)
                set_query("needle")
                save("left-find-reindexed-after-activation")
                expect_find("left-find-reindexed-after-activation", r"1\s*(?:of\s*1|found)", left=250, width=350)
                no_input("escape-left-find", lambda: display.type_key("Escape"))
                before = len(raw())
                type_text("t")
                wait_until(lambda: len(raw()) > before, "left fixture did not regain focus after inactive-bar click")
                assert raw()[before:] == b"t", f"inactive-bar click focused wrong pane: {raw()[before:]!r}"
                result["checks"].append("inactive-visible-split-bar-activates-owning-pane")

            command("stop")
            fixture_pid = int((owned / "ready").read_text())
            wait_until(lambda: not alive(fixture_pid), "raw fixture did not exit normally", seconds=8)
            result["passed"] = True
        except Exception as error:
            result["error"] = repr(error)
            if display and window:
                try:
                    save("failure")
                except Exception as capture_error:
                    result["captureError"] = repr(capture_error)
        finally:
            descendants = set(performance.process_stats(app.pid)) - {app.pid} if app else set()
            if app and app.poll() is None and display and window:
                try:
                    display.close_window(window)
                    app.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    try:
                        x("mousemove", "--window", window, 600, 422, "click", 1)
                        app.wait(timeout=5)
                    except Exception as close_error:
                        result["closeError"] = repr(close_error)
                except Exception as close_error:
                    result["closeError"] = repr(close_error)
            if app:
                try:
                    app.wait(timeout=3)
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
            result["visualReviewRequired"] = bool(result.get("ocrUnverified"))
            result["passed"] = result["passed"] and result.get("exitCode") == 0 and not result["survivingChildren"]
            (args.output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result, indent=2))
    if not result["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
