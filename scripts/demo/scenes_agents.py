"""The scene of work with coding agents, played with the stand-in `agent`
(see `home/bin/agent`): `agents` follows the status agents report (OSC 7501)
through marks and the window's notices, which its configuration turns on.

Pointer positions are those of the window's content (see `scenes.py`).
"""

from __future__ import annotations

from scenes import Scene, rename

# "Show" on the window's notice of a program that waits or is done.
SHOW = (-62, -46)


def agents_setup(stage):
    """Four agents at work on three screens; the clip starts on the first."""
    stage.run("agent search")
    stage.key("alt+t")
    stage.run("agent docs")
    rename(stage, "api")
    stage.key("ctrl+shift+space")
    stage.run("agent queue")
    stage.key("alt+d")
    stage.run("logs")
    stage.key("alt+shift+Left")
    rename(stage, "queue")
    stage.key("ctrl+shift+space")
    stage.run("agent deps")
    rename(stage, "deps")
    stage.key("alt+1")
    stage.key("alt+Prior")


def signal(stage, task: str, event: str):
    """Let a stand-in agent go on to its next part (see `home/bin/agent`)."""
    (stage.home / ".agent" / f"{task}.{event}").touch()


def agents(stage):
    """Which agent needs you: one asks, gets its answer and finishes."""
    stage.wait(1.8)
    signal(stage, "queue", "ask")
    stage.wait(1.4)
    # The notice of the waiting agent leads to it.
    stage.click(*SHOW, pause=0.9)
    stage.key("y", pause=0.8)
    # A next prompt for it, drafted while it works.
    stage.key("alt+j", pause=0.6)
    stage.type("Next: cap the retries in config.toml ", delay=0.045, pause=0.1)
    signal(stage, "docs", "finish")
    stage.type("and note the limit in the README.", delay=0.045, pause=0.9)
    # Another agent is done; a look at it takes its mark.
    stage.click(*SHOW, pause=1.2)
    signal(stage, "queue", "finish")
    stage.wait(1.8)
    # Back to the one that finished, where the next prompt waits.
    stage.click(*SHOW, pause=1.6)


NOTICES = "[notifications]\ntoasts = true\n"

SCENES = [
    Scene("agents", agents, setup=agents_setup, drawer=True, config=NOTICES),
]
