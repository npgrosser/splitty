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
    drawer: bool = False
    zoom: float = 1.1
    theme: str = "splitty-dark"
    size: tuple[int, int] | None = None
    # Seconds the window gets before the clip starts and after its last step.
    settle: float = 2.5
    tail: float = 1.2


def terminal(stage):
    stage.wait(0.6)
    stage.run("ls")
    stage.run("git log --oneline")
    stage.run("build", pause=3.2)


def splits(stage):
    stage.wait(0.5)
    stage.run("serve", pause=1.4)
    stage.key("alt+d")
    stage.run("git log --oneline", pause=0.8)
    stage.key("alt+shift+d")
    stage.run("build", pause=2.8)
    stage.key("alt+t")
    stage.run("ls", pause=0.8)
    stage.key("alt+Prior")
    stage.key("alt+shift+Left")
    stage.key("alt+m", pause=1.4)
    stage.key("alt+m", pause=1.0)
    stage.key("alt+r", pause=0.8)
    for _ in range(3):
        stage.key("Right", pause=0.45)
    stage.key("Escape", pause=0.6)


def screens(stage):
    stage.wait(0.5)
    stage.run("serve", pause=1.2)
    stage.key("ctrl+shift+space", pause=1.2)
    stage.run("git log --oneline", pause=0.8)
    stage.key("alt+shift+r", pause=0.7)
    stage.key("ctrl+a", label="", pause=0.2)
    stage.type("review", pause=0.3)
    stage.key("Return", label="", pause=0.9)
    stage.key("ctrl+shift+space", pause=1.2)
    stage.run("build", pause=2.6)
    stage.key("ctrl+Tab", pause=1.1)
    stage.key("ctrl+Tab", pause=1.1)
    stage.key("alt+1", pause=1.1)


def pointer(stage):
    stage.wait(0.4)
    stage.run("serve", pause=1.0)
    stage.key("alt+t", label="", pause=0.8)
    stage.run("git log --oneline", pause=0.8)
    # The second tab becomes a split on the right, then the split is resized
    # and the tab goes back beside the first.
    stage.drag((165, 17), (760, 300), duration=1.1, pause=1.2)
    stage.drag((440, 300), (300, 300), duration=0.9, pause=1.0)
    stage.drag((345, 17), (120, 17), duration=1.1, pause=1.2)


SCENES = [
    Scene("terminal", terminal),
    Scene("splits", splits),
    Scene("screens", screens, drawer=True),
    Scene("pointer", pointer),
]
