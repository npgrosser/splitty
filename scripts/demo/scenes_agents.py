"""The scenes of work with coding agents, played with the stand-in `agent`
(see `home/bin/agent`): `review` hands a plan back through the viewer's
review, `agents` follows the status agents report (OSC 7501) through marks
and the window's notices, which its configuration turns on.

Pointer positions are pixels of the clip.
"""

from __future__ import annotations

from scenes import Scene, rename

# The split between the agent's terminal and the viewer, before and after
# the viewer gets more room.
SPLIT = (640, 400)
SPLIT_TO = (440, 400)
# The viewer's bar: its review button opens the annotation panel.
REVIEW_WAITING = (1203, 49)
# Passages of PLAN.md, rendered, with the panel open: start and end of each.
RESULTS = ((480, 381), (632, 381))
REBUILT = ((461, 644), (851, 644))
# The annotation panel: the comment field, "Suggest change", "Add" below the
# replacement it opens, and the verdict "Request changes".
COMMENT = (1093, 248)
SUGGEST = (1210, 172)
ADD = (1247, 423)
REQUEST_CHANGES = (1063, 121)


def review(stage):
    """An agent's plan, reviewed in a viewer pane and sent back."""
    stage.wait(0.2)
    # The agent works, writes PLAN.md and waits for `splitty review PLAN.md`.
    stage.run("agent plan", pause=3.4)
    stage.drag(SPLIT, SPLIT_TO, pause=0.15, pace=0.8)
    stage.click(*REVIEW_WAITING, pause=0.4)
    # A comment on a passage.
    stage.drag(*RESULTS, pause=0.2, pace=0.8)
    stage.click(*COMMENT, pause=0.2)
    stage.type("20 are enough here.", delay=0.035, pause=0.3)
    stage.key("ctrl+Return", pause=0.6)
    # A suggested change for another.
    stage.drag(*REBUILT, pause=0.2, pace=0.8)
    # Its field takes the keys at once, holding the passage.
    stage.click(*SUGGEST, pause=0.3)
    stage.key("ctrl+a", label="", pause=0.15)
    stage.type("A write updates the changed item only.", delay=0.03, pause=0.3)
    stage.click(*ADD, pause=0.5)
    # The verdict goes back to the waiting agent, which revises the plan;
    # the viewer follows the file.
    stage.click(*REQUEST_CHANGES, pause=3.0)


# "Show" on the window's notice of a program that waits or is done.
SHOW = (1218, 754)


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
    Scene("review", review),
    Scene("agents", agents, setup=agents_setup, drawer=True, config=NOTICES),
]
