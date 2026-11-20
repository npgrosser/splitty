#!/usr/bin/env python3
"""Record the clips of the README from scripted scenes.

Usage: record.py --app target/release/splitty-gpui --out <dir> [scene ...]

Each scene of `scenes.py` runs the desktop app in a private X server (Xvfb)
with its own home, configuration and shell, drives it with xdotool and is
saved as an animated WebP and a GIF. Nothing of the machine it runs on shows
in a clip: the environment of the app is built from scratch.

Needs Xvfb, xdotool, libX11 and Pillow (`python3-pil`); a font named by
`--font` has to be installed. The app renders in software there, so a clip
shows what happens, not how fast it happens on a GPU.
"""

from __future__ import annotations

import argparse
import ctypes
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

import scenes

FPS = 24
# The private display's size is the clip's: no scaling between them.
DEFAULT_SIZE = (880, 550)


class XImage(ctypes.Structure):
    pass


XImage._fields_ = [
    ("width", ctypes.c_int), ("height", ctypes.c_int), ("xoffset", ctypes.c_int),
    ("format", ctypes.c_int), ("data", ctypes.POINTER(ctypes.c_char)),
    ("byte_order", ctypes.c_int), ("bitmap_unit", ctypes.c_int),
    ("bitmap_bit_order", ctypes.c_int), ("bitmap_pad", ctypes.c_int),
    ("depth", ctypes.c_int), ("bytes_per_line", ctypes.c_int),
    ("bits_per_pixel", ctypes.c_int), ("red_mask", ctypes.c_ulong),
    ("green_mask", ctypes.c_ulong), ("blue_mask", ctypes.c_ulong),
    ("obdata", ctypes.c_void_p), ("create_image", ctypes.c_void_p),
    ("destroy_image", ctypes.CFUNCTYPE(ctypes.c_int, ctypes.POINTER(XImage))),
]


class Screen:
    """The private display, read through Xlib: its pixels and its pointer."""

    def __init__(self, display: str, size: tuple[int, int]):
        self.lib = ctypes.CDLL("libX11.so.6")
        self.lib.XOpenDisplay.restype = ctypes.c_void_p
        self.lib.XOpenDisplay.argtypes = [ctypes.c_char_p]
        self.lib.XDefaultRootWindow.restype = ctypes.c_ulong
        self.lib.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
        self.lib.XGetImage.restype = ctypes.POINTER(XImage)
        self.lib.XGetImage.argtypes = [
            ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_int,
            ctypes.c_uint, ctypes.c_uint, ctypes.c_ulong, ctypes.c_int]
        self.lib.XQueryPointer.argtypes = [
            ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong),
            ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_int),
            ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int),
            ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_uint)]
        self.display = self.lib.XOpenDisplay(display.encode())
        if not self.display:
            raise RuntimeError(f"cannot open display {display}")
        self.root = self.lib.XDefaultRootWindow(self.display)
        self.size = size

    def grab(self) -> bytes:
        """The screen as BGRX rows."""
        width, height = self.size
        image = self.lib.XGetImage(self.display, self.root, 0, 0, width, height, ~0, 2)
        if not image:
            raise RuntimeError("XGetImage failed")
        try:
            content = image.contents
            if content.bits_per_pixel != 32:
                raise RuntimeError(f"unsupported pixel size {content.bits_per_pixel}")
            return ctypes.string_at(content.data, content.bytes_per_line * height)
        finally:
            image.contents.destroy_image(image)

    def pointer(self) -> tuple[int, int]:
        root, child = ctypes.c_ulong(), ctypes.c_ulong()
        x, y, wx, wy = ctypes.c_int(), ctypes.c_int(), ctypes.c_int(), ctypes.c_int()
        mask = ctypes.c_uint()
        self.lib.XQueryPointer(self.display, self.root, root, child, x, y, wx, wy, mask)
        return x.value, y.value


class Recorder(threading.Thread):
    """Grabs the screen at a steady rate and keeps the frames that differ."""

    def __init__(self, screen: Screen):
        super().__init__(daemon=True)
        self.screen = screen
        self.frames: list[tuple[float, bytes, tuple[int, int]]] = []
        self.stop_flag = threading.Event()
        self.started_at = 0.0

    def run(self):
        self.started_at = time.monotonic()
        previous = None
        tick = 0
        while not self.stop_flag.is_set():
            pixels, pointer = self.screen.grab(), self.screen.pointer()
            if (pixels, pointer) != previous:
                self.frames.append((time.monotonic() - self.started_at, pixels, pointer))
                previous = (pixels, pointer)
            tick += 1
            delay = self.started_at + tick / FPS - time.monotonic()
            if delay > 0:
                time.sleep(delay)

    def finish(self) -> float:
        self.stop_flag.set()
        self.join()
        return time.monotonic() - self.started_at


class Stage:
    """What a scene does to the app: keys, text and the pointer, in real time."""

    def __init__(self, display: str, window: str, recorder: Recorder, xdotool: str, env: dict):
        self.display, self.window, self.recorder = display, window, recorder
        self.xdotool, self.env = xdotool, env
        # (start, end, label) of the key chips shown in the clip.
        self.keys: list[tuple[float, float, str]] = []
        self.pointer_used = False
        self.position = (0, 0)

    def now(self) -> float:
        return time.monotonic() - self.recorder.started_at

    def xdo(self, *args: str):
        subprocess.run([self.xdotool, *args], env=self.env, check=True,
                       stdout=subprocess.DEVNULL)

    def wait(self, seconds: float):
        time.sleep(seconds)

    def key(self, chord: str, label: str | None = None, hold: float = 1.1, pause: float = 0.9):
        """Press a chord in xdotool's spelling and show it; `label=""` hides it."""
        shown = key_label(chord) if label is None else label
        if shown:
            self.keys.append((self.now(), self.now() + hold, shown))
        self.xdo("key", "--clearmodifiers", chord)
        self.wait(pause)

    def type(self, text: str, delay: float = 0.055, pause: float = 0.4):
        self.xdo("type", "--delay", str(int(delay * 1000)), text)
        self.wait(pause)

    def run(self, command: str, pause: float = 1.0):
        """Type a shell command and run it."""
        self.type(command, pause=0.25)
        self.key("Return", label="", pause=pause)

    def move(self, x: int, y: int, duration: float = 0.5):
        """Glide the pointer to a place of the window."""
        self.pointer_used = True
        start, steps = self.position, max(1, int(duration * 60))
        for step in range(1, steps + 1):
            # Ease in and out, as a hand does.
            t = step / steps
            t = t * t * (3 - 2 * t)
            self.xdo("mousemove", str(round(start[0] + (x - start[0]) * t)),
                     str(round(start[1] + (y - start[1]) * t)))
            time.sleep(duration / steps)
        self.position = (x, y)

    def click(self, x: int, y: int, button: int = 1, pause: float = 0.6):
        self.move(x, y)
        self.xdo("click", str(button))
        self.wait(pause)

    def drag(self, start: tuple[int, int], end: tuple[int, int], duration: float = 0.9,
             pause: float = 0.8):
        self.move(*start)
        self.xdo("mousedown", "1")
        self.wait(0.15)
        self.move(*end, duration=duration)
        self.wait(0.25)
        self.xdo("mouseup", "1")
        self.wait(pause)


def key_label(chord: str) -> str:
    """`alt+shift+d` as a person reads it."""
    names = {"ctrl": "Ctrl", "alt": "Alt", "shift": "Shift", "super": "Super",
             "Return": "Enter", "Escape": "Esc", "space": "Space", "Tab": "Tab",
             "Left": "←", "Right": "→", "Up": "↑", "Down": "↓",
             "Prior": "PageUp", "Next": "PageDown"}
    return "+".join(names.get(part, part.upper() if len(part) == 1 else part)
                    for part in chord.split("+"))


def demo_home(root: Path) -> Path:
    """A home with a small project, a plain prompt and nothing personal."""
    home = root / "home"
    project = home / "project"
    (project / "src").mkdir(parents=True)
    (home / ".hushlogin").touch()
    (home / ".bashrc").write_text(
        "PS1='\\[\\e[1;34m\\]\\w\\[\\e[0m\\] \\$ '\n"
        "alias ls='ls --color=auto'\n"
        "export PATH=\"$HOME/bin:$PATH\"\n")
    (home / ".bash_profile").write_text(
        "[ -f ~/.bashrc ] && . ~/.bashrc\ncd ~/project 2>/dev/null\n")
    (project / "README.md").write_text("# demo\n\nA small service.\n")
    (project / "Cargo.toml").write_text('[package]\nname = "demo"\nversion = "0.1.0"\n')
    (project / "src" / "main.rs").write_text(
        'fn main() {\n    println!("listening on :8080");\n}\n')
    (project / "notes.txt").write_text("ship the release\n")
    binaries = home / "bin"
    binaries.mkdir()
    # Stand-ins with a little output, so a clip needs no toolchain or network.
    build = binaries / "build"
    build.write_text(
        "#!/bin/sh\n"
        "for crate in core net http router server demo; do\n"
        "  printf '\\033[1;32m   Compiling\\033[0m %s v0.1.0\\n' \"$crate\"; sleep 0.35\n"
        "done\n"
        "printf '\\033[1;32m    Finished\\033[0m release in 2.1s\\n'\n")
    serve = binaries / "serve"
    serve.write_text(
        "#!/bin/sh\n"
        "printf '\\033[1;36mdemo\\033[0m listening on http://localhost:8080\\n'\n"
        "n=0; while :; do n=$((n+1)); sleep 1.2\n"
        "  printf '\\033[2m%s\\033[0m GET /api/items/%s \\033[32m200\\033[0m\\n' \"$(date +%T)\" \"$n\"\n"
        "done\n")
    for script in (build, serve):
        script.chmod(0o755)
    git = {"GIT_AUTHOR_NAME": "Demo", "GIT_AUTHOR_EMAIL": "demo@example.com",
           "GIT_COMMITTER_NAME": "Demo", "GIT_COMMITTER_EMAIL": "demo@example.com",
           "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_SYSTEM": "/dev/null",
           "PATH": "/usr/bin:/bin", "HOME": str(home)}
    if shutil.which("git"):
        run = lambda *args: subprocess.run(["git", *args], cwd=project, env=git, check=True,
                                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        run("init", "-q", "-b", "main")
        for message, name in [("Start the service", "Cargo.toml"), ("Add the router", "src/main.rs"),
                              ("Describe the project", "README.md"), ("Plan the release", "notes.txt")]:
            run("add", name)
            run("commit", "-q", "-m", message)
    return home


def app_config(directory: Path, scene, font: str):
    directory.mkdir(mode=0o700)
    (directory / "config.toml").write_text(
        "[appearance]\n"
        f'font_family = "{font}"\n'
        f'theme = "{scene.theme}"\ntheme_mode = "fixed"\n'
        f'zoom = {scene.zoom}\nzoom_scope = "window"\n\n'
        "[drawer]\n"
        f"pinned = {'true' if scene.drawer else 'false'}\n"
        "edge_reveal = false\nshow_tips = false\nsidebar_width = 200\n")


def wait_for(check, what: str, seconds: float = 30):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = check()
        if value:
            return value
        time.sleep(0.2)
    raise RuntimeError(f"timed out: {what}")


def cursor_sprite() -> Image.Image:
    """An arrow pointer with a dark outline, readable on any theme."""
    scale = 4
    points = [(0, 0), (0, 17), (4, 13), (7, 20), (10, 19), (7, 12), (12, 12)]
    image = Image.new("RGBA", (16 * scale, 24 * scale), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    scaled = [((x + 1.5) * scale, (y + 1.5) * scale) for x, y in points]
    draw.polygon(scaled, fill=(255, 255, 255, 255))
    draw.line(scaled + [scaled[0]], fill=(20, 20, 20, 255), width=scale + 1, joint="curve")
    return image.resize((16, 24), Image.LANCZOS)


def key_chip(label: str, font) -> Image.Image:
    """A pressed key as a chip for the bottom of the clip."""
    left, top, right, bottom = font.getbbox(label)
    width, height = right - left + 28, 34
    chip = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    draw = ImageDraw.Draw(chip)
    draw.rounded_rectangle((0, 0, width - 1, height - 1), radius=8,
                           fill=(24, 24, 28, 235), outline=(255, 255, 255, 70))
    draw.text((14 - left, (height - (bottom - top)) / 2 - top), label,
              font=font, fill=(255, 255, 255, 255))
    return chip


def compose(recorder: Recorder, stage: Stage, length: float, size, font_path: str | None):
    """The recorded frames with the pointer and the key chips drawn in."""
    font = ImageFont.truetype(font_path, 15)
    cursor = cursor_sprite()
    chips = {label: key_chip(label, font) for _, _, label in stage.keys}
    # A chip appearing or leaving is a frame of its own.
    moments = sorted({0.0} | {frame[0] for frame in recorder.frames}
                     | {t for start, end, _ in stage.keys for t in (start, end)})
    moments = [t for t in moments if t < length]
    images, durations = [], []
    index = 0
    for position, moment in enumerate(moments):
        while index + 1 < len(recorder.frames) and recorder.frames[index + 1][0] <= moment:
            index += 1
        _, pixels, pointer = recorder.frames[index]
        image = Image.frombuffer("RGB", size, pixels, "raw", "BGRX", 0, 1).convert("RGBA")
        shown = [label for start, end, label in stage.keys if start <= moment < end]
        if shown:
            chip = chips[shown[-1]]
            image.alpha_composite(chip, ((size[0] - chip.width) // 2, size[1] - chip.height - 18))
        if stage.pointer_used:
            image.alpha_composite(cursor, (max(0, pointer[0] - 1), max(0, pointer[1] - 1)))
        following = moments[position + 1] if position + 1 < len(moments) else length
        images.append(image.convert("RGB"))
        durations.append(max(20, round((following - moment) * 1000)))
    return images, durations


def save(images, durations, out: Path, name: str):
    webp = out / f"{name}.webp"
    images[0].save(webp, save_all=True, append_images=images[1:], duration=durations,
                   loop=0, lossless=True, method=4, minimize_size=True)
    gif = out / f"{name}.gif"
    # One palette for the whole clip keeps colors steady between frames.
    sample = Image.new("RGB", (images[0].width, images[0].height * min(4, len(images))))
    for row, image in enumerate(images[::max(1, len(images) // 4)][:4]):
        sample.paste(image, (0, row * image.height))
    palette = sample.quantize(colors=255, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE)
    frames = [image.quantize(palette=palette, dither=Image.Dither.NONE) for image in images]
    frames[0].save(gif, save_all=True, append_images=frames[1:], duration=durations,
                   loop=0, optimize=False, disposal=1)
    return webp, gif


def record(scene, args, out: Path):
    size = scene.size or DEFAULT_SIZE
    root = Path(tempfile.mkdtemp(prefix="splitty-demo-"))
    xvfb = app = None
    try:
        home = demo_home(root)
        app_config(root / "config", scene, args.font)
        runtime = root / "run"
        runtime.mkdir(mode=0o700)
        display = f":{args.display}"
        library_path = os.environ.get("LD_LIBRARY_PATH", "")
        xvfb = subprocess.Popen(
            [args.xvfb, display, "-screen", "0", f"{size[0]}x{size[1]}x24", "-dpi", "96",
             "-nolisten", "tcp", *args.xvfb_arg],
            env={"PATH": os.environ["PATH"], "LD_LIBRARY_PATH": library_path},
            stdout=subprocess.DEVNULL, stderr=open(root / "xvfb.log", "wb"))
        tools = {"DISPLAY": display, "PATH": os.environ["PATH"], "LD_LIBRARY_PATH": library_path}
        time.sleep(1)

        def display_ready():
            # A client that connects while the server still starts can hang.
            try:
                return subprocess.run([args.xdotool, "getdisplaygeometry"], env=tools,
                                      capture_output=True, timeout=2).returncode == 0
            except subprocess.TimeoutExpired:
                return False
        try:
            wait_for(display_ready, "the private display")
        except RuntimeError as error:
            raise RuntimeError(f"{error}: {(root / 'xvfb.log').read_text()[-600:]}") from None
        # Only what the app needs: nothing of the recording machine's session.
        environment = {
            "HOME": str(home), "USER": "demo", "LOGNAME": "demo", "SHELL": "/bin/bash",
            "PATH": "/usr/local/bin:/usr/bin:/bin", "LANG": "C.UTF-8", "TERM": "xterm-256color",
            "DISPLAY": display, "GPUI_BACKEND": "x11", "XDG_RUNTIME_DIR": str(runtime),
            "SPLITTY_CONFIG_DIR": str(root / "config"), "LD_LIBRARY_PATH": library_path,
        }
        log = open(root / "app.log", "wb")
        app = subprocess.Popen([str(args.app)], env=environment, stdout=log, stderr=log)

        def window():
            found = subprocess.run([args.xdotool, "search", "--onlyvisible", "--pid", str(app.pid)],
                                   env=tools, capture_output=True, text=True).stdout.split()
            return found[-1] if found else None
        handle = wait_for(window, "the app's window")
        for command in (["windowmove", handle, "0", "0"],
                        ["windowsize", handle, str(size[0]), str(size[1])],
                        ["windowfocus", handle], ["mousemove", str(size[0] - 4), str(size[1] - 4)]):
            subprocess.run([args.xdotool, *command], env=tools, check=True)
        time.sleep(scene.settle)
        screen = Screen(display, size)
        recorder = Recorder(screen)
        stage = Stage(display, handle, recorder, args.xdotool, tools)
        stage.position = (size[0] - 4, size[1] - 4)
        recorder.start()
        time.sleep(0.4)
        scene.play(stage)
        time.sleep(scene.tail)
        length = recorder.finish()
        if app.poll() is not None:
            raise RuntimeError(f"the app exited during the scene; see {root / 'app.log'}")
        images, durations = compose(recorder, stage, length, size, args.font_file)
        webp, gif = save(images, durations, out, scene.name)
        print(f"{scene.name}: {length:.1f}s, {len(images)} frames, "
              f"webp {webp.stat().st_size // 1024} KiB, gif {gif.stat().st_size // 1024} KiB")
    finally:
        for process in (app, xvfb):
            if process and process.poll() is None:
                process.terminate()
                try:
                    process.wait(5)
                except subprocess.TimeoutExpired:
                    process.kill()
        if not args.keep:
            shutil.rmtree(root, ignore_errors=True)
        else:
            print(f"kept {root}")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("names", nargs="*", help="scenes to record; all without one")
    parser.add_argument("--app", type=Path, required=True, help="the splitty-gpui executable")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--xvfb", default="Xvfb")
    parser.add_argument("--xvfb-arg", action="append", default=[],
                        help="an extra argument for Xvfb, such as a font path")
    parser.add_argument("--xdotool", default="xdotool")
    parser.add_argument("--display", type=int, default=87)
    parser.add_argument("--font", default="Ubuntu Sans", help="the interface font, installed")
    parser.add_argument("--font-file", default="/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                        help="a TrueType file for the key chips, with arrows")
    parser.add_argument("--keep", action="store_true", help="keep the scene's directory")
    args = parser.parse_args()
    known = {scene.name: scene for scene in scenes.SCENES}
    unknown = [name for name in args.names if name not in known]
    if unknown:
        parser.error(f"unknown scene: {', '.join(unknown)}; there are {', '.join(known)}")
    args.out.mkdir(parents=True, exist_ok=True)
    args.app = args.app.resolve()
    for name in args.names or list(known):
        record(known[name], args, args.out)


if __name__ == "__main__":
    sys.exit(main())
