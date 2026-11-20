"""The scenes with a remote host, `devbox` (see `remote.py`).

Pointer positions are those of the window's content (see `scenes.py`), with
the drawer pinned and the two local screens of `local_setup` above the host's
group.
"""

from __future__ import annotations

import time

from scenes import FINISH, Scene, rename

HOSTS = (70, -108)
# The host's line in the Hosts popover, and New Screen in its panel.
DEVBOX = (330, -93)
NEW_SCREEN = (340, -203)
# The host's line in the drawer: its ports button and unplug button.
PORTS = (182, 293)
UNPLUG = (147, 293)
REMOTE_SCREEN = (80, 330)
# The detected port in the ports popover.
PORT_8080 = (340, 411)
# Where the terminal's context menu opens, and its "Open Folder in" entry:
# the menu is as tall as the room below that place and so ends at the
# window's bottom edge.
MENU_AT = (700, 500)
OPEN_FOLDER_IN = (790, -110)
LOCAL_SCREEN = (80, 114)


def connect(stage, host: str):
    """Off camera: open a host from the Hosts list by its name and start a
    screen on it."""
    stage.click(*HOSTS)
    stage.type(host)
    stage.key("Return", label="")
    # Connecting takes the host's time, not the stage's.
    time.sleep(3)
    stage.click(*NEW_SCREEN)
    time.sleep(2)


def overview_setup(stage):
    """Two local screens and one on each of two hosts, all at work; the clip
    starts on the first."""
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
    time.sleep(FINISH)
    stage.key("alt+shift+d")
    stage.run("build")
    time.sleep(FINISH)
    rename(stage, "review")
    connect(stage, "devbox")
    stage.run("serve")
    stage.key("alt+d")
    stage.run("monitor")
    rename(stage, "backend")
    connect(stage, "staging")
    stage.run("logs")
    stage.key("alt+d")
    stage.run("serve 9000")
    rename(stage, "deploy")
    time.sleep(1)
    stage.key("alt+1")
    stage.move(900, 620)


def overview(stage):
    """The whole window in a few seconds: local screens and the screens of
    two hosts beside them, in splits and tabs."""
    stage.wait(1.0)
    stage.key("ctrl+Tab", pause=1.2)
    stage.key("ctrl+Tab", pause=1.7)
    stage.key("ctrl+Tab", pause=1.7)
    stage.key("alt+m", pause=1.2)
    stage.key("alt+m", pause=0.8)
    stage.key("ctrl+Tab", pause=1.0)


def local_setup(stage):
    """Two local screens at work; the clip starts on the first."""
    stage.run("logs")
    stage.key("alt+d")
    stage.run("tests")
    # Ended while it has the focus, the test run leaves no unread mark.
    time.sleep(FINISH)
    rename(stage, "api")
    stage.key("ctrl+shift+space")
    stage.run("monitor")
    rename(stage, "system")
    stage.key("alt+1")


def screens_setup(stage):
    """Two local screens at work; the clip starts on the first."""
    stage.run("serve")
    stage.key("alt+d")
    stage.run("logs")
    rename(stage, "api")
    stage.key("ctrl+shift+space")
    stage.run("monitor")
    rename(stage, "system")
    stage.key("alt+1")


def screens(stage):
    """Screens on this computer, a new one, and one on a host beside them."""
    stage.wait(0.8)
    stage.key("ctrl+Tab", pause=1.2)
    stage.key("alt+1", pause=1.0)
    stage.key("ctrl+shift+space", pause=0.9)
    stage.run("git log --graph --oneline --all", pause=0.8)
    rename(stage, "review")
    # A host's screens open beside the local ones.
    stage.click(*HOSTS, pause=0.7)
    stage.click(*DEVBOX, pause=1.6)
    stage.click(*NEW_SCREEN, pause=1.2)
    stage.run("monitor", pause=1.0)
    stage.key("alt+d")
    stage.run("serve", pause=1.4)
    stage.key("ctrl+Tab", pause=1.3)
    stage.key("ctrl+shift+Tab", pause=1.6)


def reconnect_setup(stage):
    """The local screens, and a remote one with two programs at work."""
    local_setup(stage)
    stage.click(*HOSTS)
    stage.click(*DEVBOX)
    # Connecting takes the host's time, not the stage's.
    time.sleep(3)
    stage.click(*NEW_SCREEN)
    time.sleep(2)
    stage.run("monitor")
    stage.key("alt+d")
    stage.run("serve")
    time.sleep(2)
    stage.move(900, 560)


def reconnect(stage):
    """The host is closed here and keeps working: its screen is back as it
    was, with what ran meanwhile."""
    stage.wait(1.6)
    stage.move(100, UNPLUG[1])
    stage.click(*UNPLUG, pause=2.2)
    stage.click(*HOSTS, pause=0.9)
    stage.click(*DEVBOX, pause=3.2)


def ports_setup(stage):
    """The local screens, and a remote one where `serve` listens on 8080."""
    local_setup(stage)
    stage.click(*HOSTS)
    stage.click(*DEVBOX)
    # Connecting takes the host's time, not the stage's.
    time.sleep(3)
    stage.click(*NEW_SCREEN)
    time.sleep(2)
    stage.run("serve")
    # The host reports the new listener after a moment.
    time.sleep(4)
    stage.move(900, 560)


def ports(stage):
    """A port on the host, forwarded with a click and asked from here."""
    stage.wait(1.2)
    stage.click(*PORTS, pause=1.3)
    stage.click(*PORT_8080, pause=1.0)
    # Forwarded now: the list offers its local address.
    stage.click(*PORTS, pause=1.4)
    stage.key("Escape", label="", pause=0.5)
    stage.click(*LOCAL_SCREEN, pause=0.8)
    stage.run("curl -s localhost:8080/api/items", pause=2.0)
    stage.click(*REMOTE_SCREEN, pause=1.6)
    # A remote terminal's folder opens in an editor on this computer.
    stage.click(*MENU_AT, button=3, pause=0.8)
    stage.move(*OPEN_FOLDER_IN)
    stage.wait(1.8)
    stage.key("Escape", label="", pause=0.8)


SCENES = [
    Scene("overview", overview, setup=overview_setup, drawer=True, hosts=("devbox", "staging")),
    Scene("screens", screens, setup=screens_setup, drawer=True, hosts=("devbox",)),
    Scene("reconnect", reconnect, setup=reconnect_setup, drawer=True, hosts=("devbox",)),
    Scene("ports", ports, setup=ports_setup, drawer=True, hosts=("devbox",)),
]
