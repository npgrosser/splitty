"""The scenes of the README's clips, from one terminal to the whole window.

A scene is a name, how the app starts (drawer, zoom, theme, size) and a
function that plays it on a `Stage` (see `record.py`). Pointer positions are
in pixels of the clip.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Callable


@dataclass
class Scene:
    name: str
    play: Callable
    # What happens off camera before the clip starts.
    setup: Callable | None = None
    drawer: bool = False
    zoom: float = 1.25
    theme: str = "splitty-dark"
    size: tuple[int, int] | None = None
    # Whether the scene needs a machine to reach over SSH.
    host: bool = False
    # Seconds the window gets before the setup and after the last step.
    settle: float = 2.5
    tail: float = 1.4


def terminal(stage):
    stage.wait(0.5)
    stage.run("git log --graph --oneline --all", pause=1.3)
    stage.run("tests", pause=4.2)
    stage.run("git diff", pause=1.6)


def splits(stage):
    """A whole workspace from the keyboard: six terminals in splits and tabs."""
    stage.wait(0.4)
    stage.run("monitor", pause=1.0)
    stage.key("alt+d")
    stage.run("serve", pause=0.9)
    stage.key("alt+shift+d")
    stage.run("tests", pause=1.2)
    stage.key("alt+shift+Left", pause=0.6)
    stage.key("alt+shift+d")
    stage.run("logs", pause=0.9)
    stage.key("alt+t")
    stage.run("git log --graph --oneline --all", pause=1.0)
    stage.key("alt+shift+Right", pause=0.5)
    stage.key("alt+shift+Up", pause=0.5)
    stage.key("alt+t")
    stage.run("build", pause=2.6)
    stage.key("alt+Prior", pause=0.8)
    stage.key("alt+shift+Left", pause=0.6)
    stage.key("alt+m", pause=1.5)
    stage.key("alt+m", pause=0.9)


def pointer_setup(stage):
    for command in ("monitor", "serve", "logs", "tests"):
        stage.run(command)
        stage.key("alt+t")
    stage.run("git log --graph --oneline --all")


def pointer(stage):
    """Five tabs become a layout by dragging them out of their group."""
    width, height = stage.hands.size
    bar = 20
    stage.wait(0.5)
    # The last tab to the right half, the next one below it.
    stage.drag((470, bar), (width - 150, height * 0.5))
    stage.drag((365, bar), (width - 200, height - 110))
    # One to the bottom of the left group, one beside it.
    stage.drag((255, bar), (300, height - 110))
    stage.drag((150, bar), (width * 0.5 - 90, height - 150))
    # The splits take the sizes their content needs.
    stage.drag((width * 0.5, height * 0.25), (width * 0.58, height * 0.27), pace=1.0)
    stage.drag((width * 0.25, height * 0.5 + 16), (width * 0.24, height * 0.6), pace=1.0)
    stage.move(width * 0.7, height * 0.4)


def screens_setup(stage):
    """Four screens with work in them; the clip starts on the first."""
    stage.run("serve")
    stage.key("alt+d")
    stage.run("logs")
    rename(stage, "api")
    stage.key("ctrl+shift+space")
    stage.run("monitor")
    rename(stage, "system")
    stage.key("ctrl+shift+space")
    stage.run("tests")
    stage.key("alt+shift+d")
    stage.run("git log --graph --oneline --all")
    rename(stage, "review")
    stage.key("alt+1")


def rename(stage, name: str):
    stage.key("alt+shift+r", pause=0.6)
    stage.key("ctrl+a", label="", pause=0.15)
    stage.type(name, pause=0.25)
    stage.key("Return", label="", pause=0.7)


def screens(stage):
    stage.wait(0.6)
    stage.key("ctrl+Tab", pause=1.3)
    stage.key("ctrl+Tab", pause=1.3)
    stage.key("alt+1", pause=1.1)
    # The drawer lists every screen with what runs in it.
    stage.click(70, 232, pause=1.2)
    stage.key("ctrl+shift+space", pause=1.0)
    stage.run("build", pause=1.2)
    rename(stage, "release")
    stage.wait(1.4)


SCENES = [
    Scene("terminal", terminal),
    Scene("splits", splits),
    Scene("pointer", pointer, setup=pointer_setup),
    Scene("screens", screens, setup=screens_setup, drawer=True),
]

from scenes_remote import SCENES as _REMOTE  # noqa: E402

SCENES += _REMOTE
