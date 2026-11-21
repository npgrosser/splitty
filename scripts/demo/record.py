#!/usr/bin/env python3
"""Record the clips of the README from scripted scenes.

Usage: record.py --app target/release/splitty-gpui --out <dir> [scene ...]

Each scene of `scenes.py` runs the desktop app in a private X server (Xvfb)
with its own home, configuration and shell. A scene first sets its window up
off camera, then plays: keys and text go through xdotool, the pointer through
XTest along a hand-like path. The clip is saved as an animated WebP and a GIF.
Nothing of the machine it runs on shows in a clip: the environment of the app
is built from scratch out of `home/`.

Needs Xvfb, xdotool, libX11, libXtst and Pillow (`python3-pil`); a font named
by `--font` has to be installed. The app renders in software there, so a clip
shows what happens, not how fast it happens on a GPU.
"""

from __future__ import annotations

import argparse
import ctypes
import dataclasses
import math
import os
import random
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
# The size of the window's content. The private display, and with it the
# clip, is taller by the title bar of a scene that shows one.
DEFAULT_SIZE = (1440, 900)
HOME = Path(__file__).resolve().parent / "home"


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
        self.lib.XFlush.argtypes = [ctypes.c_void_p]
        self.test = ctypes.CDLL("libXtst.so.6")
        self.test.XTestFakeMotionEvent.argtypes = [
            ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_ulong]
        self.test.XTestFakeButtonEvent.argtypes = [
            ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]

    def atom(self, name: str) -> int:
        self.lib.XInternAtom.restype = ctypes.c_ulong
        self.lib.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
        return self.lib.XInternAtom(self.display, name.encode(), 0)

    def set_atoms(self, window: int, name: str, values: list[str]):
        """Set a property that lists atoms, as a window manager's hints do."""
        self.lib.XChangeProperty.argtypes = [
            ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_int,
            ctypes.c_int, ctypes.POINTER(ctypes.c_ulong), ctypes.c_int]
        atoms = (ctypes.c_ulong * len(values))(*(self.atom(value) for value in values))
        self.lib.XChangeProperty(self.display, window, self.atom(name), 4, 32, 0, atoms,
                                 len(values))
        self.lib.XFlush(self.display)

    def pretend_desktop(self):
        """Stand in for a compositing window manager, as far as the app asks:
        with one, it draws its own title bar. This connection has to stay
        open, since it owns the compositor's selection."""
        self.lib.XCreateSimpleWindow.restype = ctypes.c_ulong
        self.lib.XCreateSimpleWindow.argtypes = [
            ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_int, ctypes.c_uint,
            ctypes.c_uint, ctypes.c_uint, ctypes.c_ulong, ctypes.c_ulong]
        self.lib.XSetSelectionOwner.argtypes = [
            ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_ulong]
        owner = self.lib.XCreateSimpleWindow(self.display, self.root, -10, -10, 1, 1, 0, 0, 0)
        for name in (f"_NET_WM_CM_S{self.root}", "_NET_WM_CM_S0"):
            self.lib.XSetSelectionOwner(self.display, self.atom(name), owner, 0)
        self.set_atoms(self.root, "_NET_SUPPORTED", [
            "_GTK_FRAME_EXTENTS", "_NET_WM_STATE", "_NET_WM_STATE_MAXIMIZED_VERT",
            "_NET_WM_STATE_MAXIMIZED_HORZ"])

    def maximized(self, window: int):
        """Tell a window it fills the screen, so its frame has no shadow."""
        self.set_atoms(window, "_NET_WM_STATE", [
            "_NET_WM_STATE_MAXIMIZED_VERT", "_NET_WM_STATE_MAXIMIZED_HORZ"])

    def point(self, x: float, y: float):
        """Put the pointer somewhere."""
        self.test.XTestFakeMotionEvent(self.display, -1, round(x), round(y), 0)
        self.lib.XFlush(self.display)

    def button(self, number: int, down: bool):
        self.test.XTestFakeButtonEvent(self.display, number, 1 if down else 0, 0)
        self.lib.XFlush(self.display)

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
    """What a scene does to the app: keys, text and the pointer, in real time.

    While `fast` is set (a scene's setup, off camera) nothing waits longer
    than the app needs and nothing is shown.
    """

    def __init__(self, hands: Screen, recorder: Recorder | None, xdotool: str, env: dict,
                 seed: str, top: int = 0):
        self.hands, self.recorder = hands, recorder
        self.xdotool, self.env = xdotool, env
        # A scene's positions are those of the window's content: `top` is the
        # height of the title bar above it, `size` what is left below.
        self.top = top
        self.size = (hands.size[0], hands.size[1] - top)
        # (start, end, label) of the key chips shown in the clip.
        self.keys: list[tuple[float, float, str]] = []
        self.pointer_used = False
        self.position = (float(self.size[0] - 4), float(self.size[1] - 4))
        self.fast = True
        self.random = random.Random(seed)
        # The demo home, for a scene that leaves a signal to a stand-in program.
        self.home: Path | None = None

    def now(self) -> float:
        return time.monotonic() - self.recorder.started_at

    def xdo(self, *args: str):
        subprocess.run([self.xdotool, *args], env=self.env, check=True,
                       stdout=subprocess.DEVNULL)

    def wait(self, seconds: float):
        time.sleep(min(seconds, 0.35) if self.fast else seconds)

    def key(self, chord: str, label: str | None = None, hold: float = 1.0, pause: float = 0.8):
        """Press a chord in xdotool's spelling and show it; `label=""` hides it."""
        shown = key_label(chord) if label is None else label
        if shown and not self.fast:
            self.keys.append((self.now(), self.now() + hold, shown))
        self.xdo("key", "--clearmodifiers", chord)
        self.wait(pause)

    def type(self, text: str, delay: float = 0.05, pause: float = 0.35):
        self.xdo("type", "--delay", "4" if self.fast else str(int(delay * 1000)), text)
        self.wait(pause)

    def run(self, command: str, pause: float = 0.9):
        """Type a shell command and run it."""
        self.type(command, pause=0.2)
        self.key("Return", label="", pause=pause)

    def point(self, x: float, y: float):
        self.hands.point(x, y + self.top)

    def at(self, x: float, y: float) -> tuple[float, float]:
        """A scene's position in the content: a negative one counts from the
        right or the bottom edge, where things anchored there stay."""
        return (float(x if x >= 0 else self.size[0] + x),
                float(y if y >= 0 else self.size[1] + y))

    def move(self, x: float, y: float, pace: float = 1.0):
        """Take the pointer to a place of the window as a hand would: along a
        slight arc, fast in the middle, and settling at the end."""
        start, end = self.position, self.at(x, y)
        distance = math.dist(start, end)
        self.position = end
        if self.fast or distance < 2:
            self.point(*end)
            return
        self.pointer_used = True
        # Farther targets take longer, by less than their distance (Fitts).
        duration = (0.16 + 0.085 * math.log2(distance / 18 + 1)) * pace
        duration *= self.random.uniform(0.9, 1.12)
        # The arc: both control points on one side of the straight line.
        normal = ((end[1] - start[1]) / distance, (start[0] - end[0]) / distance)
        bend = distance * self.random.uniform(0.06, 0.16) * self.random.choice((-1, 1))
        first = (start[0] + (end[0] - start[0]) * 0.3 + normal[0] * bend,
                 start[1] + (end[1] - start[1]) * 0.3 + normal[1] * bend)
        second = (start[0] + (end[0] - start[0]) * 0.72 + normal[0] * bend * 0.7,
                  start[1] + (end[1] - start[1]) * 0.72 + normal[1] * bend * 0.7)
        # A long reach lands a little past its target and comes back.
        past = None
        if distance > 260:
            over = self.random.uniform(3, 8)
            past = (end[0] + (end[0] - start[0]) / distance * over,
                    end[1] + (end[1] - start[1]) / distance * over)
        target = past or end
        steps = max(2, int(duration * 90))
        began = time.monotonic()
        for step in range(1, steps + 1):
            t = step / steps
            # Quick off the mark, slow into the target.
            t = 1 - (1 - t ** 0.9) ** 2.2
            u = 1 - t
            px = u ** 3 * start[0] + 3 * u * u * t * first[0] + 3 * u * t * t * second[0] + t ** 3 * target[0]
            py = u ** 3 * start[1] + 3 * u * u * t * first[1] + 3 * u * t * t * second[1] + t ** 3 * target[1]
            wobble = 0.5 * (1 - t)
            self.point(px + self.random.uniform(-wobble, wobble),
                       py + self.random.uniform(-wobble, wobble))
            rest = began + duration * step / steps - time.monotonic()
            if rest > 0:
                time.sleep(rest)
        if past:
            time.sleep(self.random.uniform(0.03, 0.07))
            for step in range(1, 6):
                self.point(past[0] + (end[0] - past[0]) * step / 5,
                           past[1] + (end[1] - past[1]) * step / 5)
                time.sleep(0.012)
        self.point(*end)

    def click(self, x: float, y: float, button: int = 1, pause: float = 0.5, pace: float = 1.0):
        self.move(x, y, pace)
        self.wait(0.08)
        self.hands.button(button, True)
        time.sleep(0.05)
        self.hands.button(button, False)
        self.wait(pause)

    def scroll(self, x: float, y: float, steps: int, pause: float = 0.4):
        """Turn the wheel over a place: down for positive steps."""
        self.move(x, y)
        for _ in range(abs(steps)):
            button = 5 if steps > 0 else 4
            self.hands.button(button, True)
            self.hands.button(button, False)
            time.sleep(0.02 if self.fast else 0.09)
        self.wait(pause)

    def drag(self, start: tuple[float, float], end: tuple[float, float], pause: float = 0.6,
             pace: float = 1.15):
        """Press at `start`, carry to `end`, release. The way there passes a
        point just off the start, so the app sees the drag begin."""
        start, end = self.at(*start), self.at(*end)
        self.move(*start)
        self.wait(0.1)
        self.hands.button(1, True)
        time.sleep(0.07)
        if self.fast:
            for step in range(1, 9):
                self.point(start[0] + (end[0] - start[0]) * step / 8,
                           start[1] + (end[1] - start[1]) * step / 8)
                time.sleep(0.03)
            self.position = (float(end[0]), float(end[1]))
        else:
            self.move(*end, pace=pace)
        self.wait(0.16)
        self.hands.button(1, False)
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
    """A home with a small project, a plain prompt and nothing personal:
    a copy of `home/` with a Git history of its own. It gets a short path
    where that is free, since the app shows a file's whole path in places."""
    home = Path(tempfile.gettempdir()) / "demo"
    if home.exists():
        home = root / "home"
    shutil.copytree(HOME, home)
    (home / ".hushlogin").touch()
    (home / ".bashrc").write_text(
        "PS1='\\[\\e[1;34m\\]\\w\\[\\e[0m\\] \\$ '\n"
        "alias ls='ls --color=auto'\n"
        "export PATH=\"$HOME/bin:$PATH\"\n")
    (home / ".bash_profile").write_text(
        "[ -f ~/.bashrc ] && . ~/.bashrc\ncd ~/project 2>/dev/null\n")
    project = home / "project"
    if not shutil.which("git"):
        return home
    stamp = [1_700_000_000]

    def git(*args):
        # Fixed names and dates: the same history, and so the same hashes,
        # on every machine.
        stamp[0] += 3600
        environment = {
            "GIT_AUTHOR_NAME": "Demo", "GIT_AUTHOR_EMAIL": "demo@example.com",
            "GIT_COMMITTER_NAME": "Demo", "GIT_COMMITTER_EMAIL": "demo@example.com",
            "GIT_AUTHOR_DATE": f"{stamp[0]} +0000", "GIT_COMMITTER_DATE": f"{stamp[0]} +0000",
            "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_SYSTEM": "/dev/null",
            "PATH": "/usr/bin:/bin", "HOME": str(home)}
        subprocess.run(["git", *args], cwd=project, env=environment, check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def commit(message: str, path: str, text: str | None = None):
        if text is not None:
            with open(project / path, "a") as file:
                file.write(text)
        git("add", path)
        git("commit", "-q", "-m", message)

    git("init", "-q", "-b", "main")
    commit("Start the service", "Cargo.toml")
    commit("Add the router", "src")
    commit("Describe the project", "README.md")
    git("switch", "-q", "-c", "search")
    commit("Sketch the search route", "src/router.rs", "\n// search: by title\n")
    commit("Rank the results", "src/router.rs", "// search: rank by recency\n")
    git("switch", "-q", "main")
    commit("Write the plan", "PLAN.md")
    git("merge", "-q", "--no-ff", "-m", "Merge the search route", "search")
    git("switch", "-q", "-c", "queue")
    commit("Retry failed jobs", "src/main.rs", "\n// queue: three retries\n")
    git("switch", "-q", "main")
    commit("Bump to 0.4.2", "Cargo.toml", "# 0.4.2\n")
    # Work in progress, for a colored diff.
    with open(project / "src/router.rs", "a") as file:
        file.write("\nfn search() -> String {\n    \"[]\".into()\n}\n")
    return home


def app_config(directory: Path, scene, font: str):
    directory.mkdir(mode=0o700)
    (directory / "config.toml").write_text(
        "[appearance]\n"
        f'font_family = "{font}"\n'
        f'theme = "{scene.theme}"\ntheme_mode = "fixed"\n'
        f'zoom = {scene.zoom}\nzoom_scope = "window"\n'
        # The app's own title bar, in the shape the theme gives it.
        + ('window_decoration = "splitty"\n' if scene.decorated else "") + "\n"
        "[drawer]\n"
        f"pinned = {'true' if scene.drawer else 'false'}\n"
        "edge_reveal = false\nshow_tips = false\nsidebar_width = 200\n"
        + (f"\n{scene.config}" if scene.config else ""))


def wait_for(check, what: str, seconds: float = 30):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = check()
        if value:
            return value
        time.sleep(0.2)
    raise RuntimeError(f"timed out: {what}")


def cursor_sprite(grow: float) -> Image.Image:
    """An arrow pointer with a dark outline, readable on any theme."""
    scale = 4
    points = [(0, 0), (0, 17), (4, 13), (7, 20), (10, 19), (7, 12), (12, 12)]
    image = Image.new("RGBA", (16 * scale, 24 * scale), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    scaled = [((x + 1.5) * scale, (y + 1.5) * scale) for x, y in points]
    draw.polygon(scaled, fill=(255, 255, 255, 255))
    draw.line(scaled + [scaled[0]], fill=(20, 20, 20, 255), width=scale + 1, joint="curve")
    return image.resize((round(16 * grow), round(24 * grow)), Image.LANCZOS)


def key_chip(label: str, font, grow: float) -> Image.Image:
    """A pressed key as a chip for the bottom of the clip."""
    left, top, right, bottom = font.getbbox(label)
    side = round(14 * grow)
    width, height = right - left + 2 * side, round(34 * grow)
    chip = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    draw = ImageDraw.Draw(chip)
    draw.rounded_rectangle((0, 0, width - 1, height - 1), radius=round(8 * grow),
                           fill=(24, 24, 28, 235), outline=(255, 255, 255, 70))
    draw.text((side - left, (height - (bottom - top)) / 2 - top), label,
              font=font, fill=(255, 255, 255, 255))
    return chip


def compose(recorder: Recorder, stage: Stage, length: float, size, font_path: str | None):
    """The recorded frames with the pointer and the key chips drawn in."""
    # The pointer and the chips keep their share of a clip of any width.
    grow = size[0] / 1280
    font = ImageFont.truetype(font_path, round(15 * grow))
    cursor = cursor_sprite(grow)
    chips = {label: key_chip(label, font, grow) for _, _, label in stage.keys}
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
            image.alpha_composite(chip, ((size[0] - chip.width) // 2,
                                         size[1] - chip.height - round(18 * grow)))
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


def record(scene, args, out: Path, display: str):
    content = scene.size or DEFAULT_SIZE
    top = scenes.TITLE_BAR if scene.decorated else 0
    size = (content[0], content[1] + top)
    root = Path(tempfile.mkdtemp(prefix="splitty-demo-"))
    xvfb = app = host = home = None
    try:
        home = demo_home(root)
        if args.fonts:
            # Fonts a theme names that the recording machine lacks.
            share = home / ".local" / "share"
            share.mkdir(parents=True)
            (share / "fonts").symlink_to(args.fonts.resolve())
        app_config(root / "config", scene, args.font)
        runtime = root / "run"
        runtime.mkdir(mode=0o700)
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
        if scene.hosts:
            # Machines to reach over SSH, for the scenes of remote screens.
            import remote
            host = remote.start(root, home, environment, args, scene.hosts)
        hands = Screen(display, size)
        if scene.decorated:
            hands.pretend_desktop()
        log = open(root / "app.log", "wb")
        app = subprocess.Popen([str(args.app)], env=environment, stdout=log, stderr=log)

        def window():
            found = subprocess.run([args.xdotool, "search", "--onlyvisible", "--pid", str(app.pid)],
                                   env=tools, capture_output=True, text=True).stdout.split()
            return found[-1] if found else None
        handle = wait_for(window, "the app's window")
        for command in (["windowmove", handle, "0", "0"],
                        ["windowsize", handle, str(size[0]), str(size[1])],
                        ["windowfocus", handle]):
            subprocess.run([args.xdotool, *command], env=tools, check=True)
        if scene.decorated:
            hands.maximized(int(handle))
        hands.point(size[0] - 4, size[1] - 4)
        time.sleep(scene.settle)
        stage = Stage(hands, None, args.xdotool, tools, scene.name, top)
        stage.home = home
        if scene.setup:
            scene.setup(stage)
            time.sleep(1.2)
        recorder = Recorder(Screen(display, size))
        stage.recorder, stage.fast = recorder, False
        recorder.start()
        time.sleep(0.5)
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
        if host:
            host.stop()
        if not args.keep:
            if home and home.parent != root:
                shutil.rmtree(home, ignore_errors=True)
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
    parser.add_argument("--display", type=int, default=87,
                        help="the first private display number")
    parser.add_argument("--font", default="",
                        help="an interface font for every theme; each theme's own without it")
    parser.add_argument("--font-file", default="/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                        help="a TrueType file for the key chips, with arrows")
    parser.add_argument("--theme", help="record in this UI theme instead of the scenes' own; "
                        "the clips are named after it")
    parser.add_argument("--fonts", type=Path,
                        help="a directory of fonts for the app, such as one a theme names")
    parser.add_argument("--keep", action="store_true", help="keep the scene's directory")
    parser.add_argument("--sshd-root", default=os.environ.get("SPLITTY_DEMO_SSHD_ROOT"),
                        help="an unpacked openssh-server for the remote scenes; sshd from PATH without")
    parser.add_argument("--server", type=Path,
                        help="the splitty server for the remote host; next to --app without")
    args = parser.parse_args()
    known = {scene.name: scene for scene in scenes.SCENES}
    unknown = [name for name in args.names if name not in known]
    if unknown:
        parser.error(f"unknown scene: {', '.join(unknown)}; there are {', '.join(known)}")
    args.out.mkdir(parents=True, exist_ok=True)
    args.app = args.app.resolve()
    for index, name in enumerate(args.names or list(known)):
        scene = known[name]
        if args.theme:
            scene = dataclasses.replace(scene, theme=args.theme, name=f"{name}-{args.theme}")
        # A display of its own per scene: the one before may not be gone yet.
        record(scene, args, args.out, f":{args.display + index}")


if __name__ == "__main__":
    sys.exit(main())
