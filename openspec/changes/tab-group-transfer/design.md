# Design

## Context

See [proposal.md](proposal.md). `move_pane_to_screen` already moves one pane with its session record between Screens without effects: it takes the pane out of the source as `close_pane` would, appends it to the target's first slot as its active tab, creates a target right after the source on request and removes a source left empty. The native client captures the tab, submits the op once, and verifies the target by reading the catalog and the Screens again before it follows (`workspace-model`, Native Screen Transfer Admission, Native Cross-Screen Session Retention, Native Screen Transfer Navigation). The command layer gate of `tab-group-drag` refuses commands whose capability a server did not announce.

The WebSocket handler of every released server parses an op type it does not know as a screen op; that fails, the handler logs it and returns: no `mux.op_failed`, no error event, the connection stays open (`mux-protocol`, Malformed and Unknown Messages). The control API answers 400 `invalid op`. A window that sent the new op to such a server would never hear back, so the gate is needed, not a courtesy.

## Goals / Non-Goals

**Goals:** one atomic op for a whole group; the rules of `move_pane_to_screen` wherever they apply; the same capture, verification and follow as a single tab's transfer in the native client; a capability and a gate row for older servers.

**Non-Goals:** a general multi-pane transfer, a group placed beside the target's groups, a web client entry.

## Decisions

### TGT-1: One op naming the slot

`move_tab_group_to_screen` names the source Screen (the envelope), the slot, an optional `activePaneId`, the target and `createScreen`/`screenName` as `move_pane_to_screen` does. Every check runs before anything changes: unknown source, target or slot, a node that is no slot, an empty slot, a managed job's pane in it, an `activePaneId` outside it, a created target that exists, or the source as target. Then the panes leave the source one after the other through the same path as `close_pane` (a floating entry left empty goes, splits collapse) and join the target's first slot in tab order. The op returns no effect. Alternative: one `move_pane_to_screen` per tab, which is neither atomic nor one change for other windows.

### TGT-2: The rules of a single pane's move

A managed job's pane refuses the whole group, as it refuses `move_pane_to_screen`. Viewer panes move with their group. A floating group moves too and lands in the target's tiled grid. The panes join the target's first slot, as a single pane does, rather than forming a group of their own. A source left without panes is removed and, when it was active, the target becomes active; a new target is placed right after the source and named by the server when no name is given.

### TGT-3: Shown tab

The client names the tab its window shows in the group as `activePaneId`; without it the group's active tab is shown, else its first. The window follows to that tab after verification, so it stays in front.

### TGT-4: Capability and malformed requests

`tab-group-transfer-v1` closes the session capabilities of `mux.welcome`. Over the WebSocket a request without `slotId` or `targetScreenId` is ignored like any malformed op; a parsed one that is rejected answers its sender with `mux.op_failed` naming the op and the source. The control API answers a malformed one with 400 and nudges both Screens with `cli_op` after an accepted one.

### TGT-5: Native client

The tab menu offers "Move Group to Screen" for a group of more than one tab, tiled or floating, with the choices of "Move to Screen"; a single tab keeps only the existing entry. The window captures the group from the menu's tab: its slot, its tabs with their sessions in order, and the tab it shows. Before sending it checks again that the slot still holds exactly those tabs with those sessions and none of them belongs to a managed job. The pending transfer remembers every moved pane and the op whose `mux.op_failed` rejects it; verification requires every pane on the target with its session. The gate row maps the command to `tab-group-transfer-v1`; a refused command settles the window's pending transfer as rejected before the one status message.

## Architecture impact

None in `architecture/*.c4`: the changes stay inside the existing components (`splitty-core`'s mux and protocol, the server's session and service modules, client-core's workspace and native workspace, the GPUI workspace view), with no new crate, top-level module or dependency. `docs/architecture.md` gets the op beside `move_pane_to_screen` and in the cross-Screen transfer paragraph.

## Provisional decisions

2026-11-14, open to feedback, each also listed in `openspec/open-decisions.md`:

- **TGT-1** (provisional, tbd): one atomic op naming the slot.
- **TGT-2** (provisional, tbd): a job's pane refuses the whole group; viewer panes and floating groups move; the panes join the target's first slot; the emptied source goes.
- **TGT-3** (provisional, tbd): the window's shown tab is named and followed.
- **TGT-4** (provisional, tbd): a malformed request is ignored over the WebSocket like other malformed ops, not answered with `mux.op_failed`.
- **TGT-5** (provisional, tbd): the menu entry is for groups of more than one tab and is offered whatever the server; the command layer gate decides.
