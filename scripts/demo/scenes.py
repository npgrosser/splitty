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
    theme: str = "splitty-classic"
    size: tuple[int, int] | None = None
    # Whether the scene needs a machine to reach over SSH.
    host: bool = False
    # More of the app's `config.toml`, such as a `[notifications]` table.
    config: str = ""
    # Seconds the window gets before the setup and after the last step.
    settle: float = 2.5
    tail: float = 1.4


def terminal(stage):
    stage.wait(0.5)
    stage.run("git log --graph --oneline --all", pause=1.3)
    stage.run("tests", pause=4.2)
    stage.run("git diff", pause=1.6)


def splits(stage):
    """A whole workspace from the keyboard: three columns of splits and tabs."""
    stage.wait(0.4)
    stage.run("monitor", pause=0.8)
    stage.key("alt+d", pause=0.6)
    stage.run("serve", pause=0.7)
    stage.key("alt+d", pause=0.6)
    stage.run("git log --graph --oneline --all", pause=0.8)
    stage.key("alt+t", pause=0.6)
    stage.run("build", pause=0.5)
    stage.key("alt+shift+Left", pause=0.5)
    stage.key("alt+shift+d", pause=0.6)
    stage.run("tests", pause=0.6)
    stage.key("alt+shift+Left", pause=0.5)
    stage.key("alt+shift+d", pause=0.6)
    stage.run("logs", pause=0.7)
    stage.key("alt+t", pause=0.6)
    stage.run("git diff", pause=0.9)
    stage.key("alt+Prior", pause=0.8)
    stage.key("alt+Next", pause=0.8)
    stage.key("alt+m", pause=1.4)
    stage.key("alt+m", pause=0.8)


def pointer_setup(stage):
    for command in ("monitor", "serve", "logs", "tests"):
        stage.run(command)
        stage.key("alt+t")
    stage.run("git log --graph --oneline --all")


def pointer(stage):
    """Five tabs become a layout by dragging them out of their group."""
    width, height = stage.hands.size
    # The tab bar's height, and where the five tabs of the setup are.
    bar = 17
    monitor, serve, logs, tests, graph = 55, 175, 285, 440, 650
    stage.wait(0.4)
    quick = dict(pause=0.35, pace=0.95)
    # The last tab takes the right half, two more the lower halves.
    stage.drag((graph, bar), (width - 110, height * 0.5), **quick)
    stage.drag((logs, bar), (width * 0.25, height - 70), **quick)
    stage.drag((serve, bar), (width * 0.75, height - 70), **quick)
    # The splits take the sizes their content needs.
    stage.drag((width * 0.5, height * 0.3), (width * 0.56, height * 0.33), pause=0.3, pace=0.9)
    stage.drag((width * 0.2, height * 0.5), (width * 0.21, height * 0.62), pause=0.3, pace=0.9)
    # A tab moves over to another group, beside the tab that is there.
    stage.drag((monitor + 180, bar), (width * 0.56 + 160, bar), **quick)
    stage.move(width * 0.5, height * 0.45)


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
    stage.click(70, 208, pause=1.2)
    stage.key("ctrl+shift+space", pause=1.0)
    stage.run("build", pause=1.2)
    rename(stage, "release")
    stage.wait(1.4)


def overview_setup(stage):
    """Three screens at work; the clip starts on the first."""
    stage.run("monitor")
    stage.key("alt+d")
    stage.run("serve")
    stage.key("alt+shift+d")
    stage.run("logs")
    rename(stage, "api")
    stage.key("ctrl+shift+space")
    stage.run("git log --graph --oneline --all")
    stage.key("alt+d")
    stage.run("tests")
    stage.key("alt+shift+d")
    stage.run("build")
    rename(stage, "review")
    stage.key("ctrl+shift+space")
    stage.run("logs")
    stage.key("alt+d")
    stage.run("monitor")
    rename(stage, "ops")
    stage.key("alt+1")


def overview(stage):
    """The whole window in a few seconds: screens, splits and tabs."""
    stage.wait(1.0)
    stage.key("ctrl+Tab", pause=1.5)
    stage.key("ctrl+Tab", pause=1.5)
    stage.key("ctrl+Tab", pause=1.2)
    stage.key("alt+m", pause=1.3)
    stage.key("alt+m", pause=0.8)


SETTINGS = (70, 768)


def customize_setup(stage):
    stage.run("serve")
    stage.key("alt+d")
    stage.run("git log --graph --oneline --all")
    stage.key("alt+shift+d")
    stage.run("tests")


THEME_SELECT = (430, 537)


def customize(stage):
    """Themes change the whole window at once; every key can be rebound."""
    stage.wait(0.5)
    stage.click(*SETTINGS, pause=0.9)
    stage.scroll(520, 500, 6, pause=0.5)
    stage.click(*THEME_SELECT, pause=0.5)
    # The list filters by what is typed; it opens again from the keyboard,
    # wherever a theme's own metrics have put it.
    for name in ("Splitty Aurora", "Catppuccin Mocha", "Windows Classic", "Splitty Classic"):
        stage.type(name, pause=0.4)
        stage.key("Return", label="", pause=1.5)
        stage.key("Return", label="", pause=0.5)
    stage.key("Escape", label="", pause=0.4)
    stage.key("alt+shift+k", pause=1.2)
    stage.type("split", pause=1.6)


SCENES = [
    Scene("overview", overview, setup=overview_setup, drawer=True),
    Scene("terminal", terminal),
    Scene("splits", splits),
    Scene("pointer", pointer, setup=pointer_setup),
    Scene("screens", screens, setup=screens_setup, drawer=True),
    Scene("customize", customize, setup=customize_setup, drawer=True),
]

from scenes_remote import SCENES as _REMOTE  # noqa: E402

SCENES += _REMOTE

from scenes_agents import SCENES as _AGENTS  # noqa: E402

SCENES += _AGENTS
