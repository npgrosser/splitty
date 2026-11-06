# Proposal

> Completed planning on 2026-11-06. Part 2 of the sequence in `frontend-client-core` (design, "Sequence"); independent of parts 1, 3 and 4. Related: `terminal-intelligence` (process state and OSC 133 feed the same session metadata), `multi-attach` (several windows on one screen make window-side decisions race).

## Why

Several facts about panes and screens are derived in each window, although the server holds all their inputs. Every client has to repeat them, and today they disagree between clients:

- **Screen names.** The server leaves a new screen's name empty. The window invents "Screen N". A screen created by the CLI shows its raw ID, and remote windows need a workaround against name collisions (`avoidHostNames`).
- **Pane titles.** The window chooses between OSC title and process name, recognizes shell names and shortens the home directory. The server has all of it: process name, working directory, base shell, OSC title and home directory. But its pane title is set only for managed jobs, so `splitty attach` and the CLI list shell panes without titles.
- **Close guard.** Whether closing loses something depends on whether a process other than the base shell runs. The window derives this from process events, and every window keeps its own copy.
- **Close on exit.** Each window showing a pane closes it when its session exits. With no window open, nothing happens. With two windows open, both send `close_pane`.
- **Launch defaults.** For a new tab, the window sends the default shell profile and its last known working directory. The server already implements inheritance for new sessions ("Split Session Inheritance"). Whether new tabs should inherit is to be checked against `workspace-model`.
- **Replay filter.** Each client removes clipboard operations and terminal report requests from replayed output. The core has the same rules for `splitty attach`, and the server knows which frames are replay.

## What Changes

- **Screen names.** The window keeps naming the screens it creates, at once, as today (`frontend-client-core` CC-10). The reducer names only screens created without a name ("Screen N", unique within the workspace), such as those from the CLI or the control API, for `create_screen` and for `move_pane_to_screen` into a new screen.
- **Display titles.** Session metadata carries a display title computed by the server, and the mux pane title follows it for shell panes, so CLI listings and every window show the same title.
- **Busy flag.** Session state in snapshots and metadata events carries a busy flag: a foreground process other than the base shell. The window's close guard checks the flag and the scratchpad text; asking the user stays in the window.
- **Close on exit.** When a session exits, the server closes its pane according to its own `close_on_exit` setting; managed job panes keep their outcome. Windows stop closing panes on exit. A host follows its own setting, as with the PTY size policy. No user action waits on this: the exit is a server event already, and the pane goes with the nudge that follows it.
- **Launch defaults.** The window sends a profile or working directory only when the user picked one, and the server applies its inheritance rules.
- **Replay filter.** The server filters replayed output before it sends it. Clients keep their own filter while older servers exist, because it guards against typing terminal replies into a shell. Only a native client may rely on the server's filter (provisional, tbd).

## Capabilities

Existing capabilities changed:

### Modified Capabilities

- `workspace-model`: default screen names; closing a pane on exit; launch inheritance for new tabs.
- `terminal-sessions`: display title and busy flag in Session Metadata Updates; Process Exit; replay filtering at the source.
- `mux-protocol`: the new fields, their capability flags, the pane title for shell panes.
- `cli`: plain-text pane and attach listings consume server titles and safely render controls in metadata.

## Impact

- **Core:** `mux` (reducer naming, exit handling as an op), `protocol` (dispatcher metadata, replay), `pty` (process info already present), `effect_runner`.
- **Server:** `client_session` (attach and replay, metadata to the window).
- **Frontend:** `stores/screens.svelte.ts` naming; `terminal_session` and `terminal_pane` title and exit handling; `close_guard`; `panes.ts` launch defaults.
- **Docs:** `docs/protocol.ts`.
- **Both transports:** new requests are added to the native operation catalog (`docs/native-operations.json`) as well as the HTTP routes, and new messages travel over the WebSocket and native connections alike (`native-client-transports`).

Out of scope: removing a screen that ran empty (the spec keeps it today; separate decision); notification filtering; the active screen or focused pane in the server's model.

## Carried-over design notes

- 2026-11-04, `frontend-client-core` inventory:
  - Screen naming in the window: `stores/screens.svelte.ts` (about lines 19–36) and `Workspace.svelte` (about lines 350 and 381–391). The ID fallback is in `screen_startup.ts` (about lines 65 and 103). The reducer keeps `name: None`.
  - Title rule: `terminal_session.ts` (about line 312), `shell.ts`, `terminal_pane.ts` (about lines 36–50). The server's metadata is in the dispatcher (about lines 848–933). The mux title is set only for jobs (`job_reducer`).
  - Close on exit: `terminal_pane.ts` (about lines 266–273).
  - Close guard predicate: `close_guard.ts` (about lines 28–57, 71–90, 102–139).
  - New tab launch defaults: `panes.ts` (about lines 52–63).
  - Replay rules in the core: `pty/replay.rs`.
