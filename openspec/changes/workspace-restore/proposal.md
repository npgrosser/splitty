# Proposal

> Draft captured on 2026-10-09. Not planned yet. Specs, design and tasks are written when this change is next. Depends on: Milestones 1 and 5 (done); the findings that session status never leaves `starting` and that pane cwd is never updated in the model (docs/issues/openspec-baseline-findings.md, workspace-model and server-modes). Builds on `server-modes`, `workspace-model`, `terminal-sessions` and `cli`. Related: `session-scratchpad` (108), whose scratchpads survive an embedded server only once sessions come back; `terminal-intelligence` (Milestone 6), which can reuse the agent channel introduced here; `remote-screens` (107), which keeps processes alive on another host.

## Why

Agentic work in Splitty often runs in Claude Code sessions spread over several screens and panes. When the app closes, Windows reboots or WSL shuts down, all of it is gone: the layout, the working directories, and which agent conversation ran where. Rebuilding that by hand means re-splitting panes, `cd`-ing around and finding each conversation again in `claude --resume`.

Daemon mode does not cover this. It keeps processes alive while the app is closed, but it is opt-in, the Windows app never uses it, and after a daemon restart or reboot it respawns each pane with its original start command only: a pane in which `claude` was typed comes back as an empty shell. Embedded mode, which every desktop user gets by default, keeps nothing.

Embedded mode exists so that Splitty feels like a classic terminal application: no background daemon, and no visible client/server split. Classic terminals (Windows Terminal, Terminal.app, Ghostty) reopen their last windows and tabs on start. Restoring the last workspace fits that model and does not need a daemon. Agent sessions are the best case for it: Claude Code keeps its conversations on disk, so Splitty only has to remember which conversation ran in which pane to bring it back.

## What Changes

- **Restore snapshot in every server mode**: the server the desktop app hosts keeps a snapshot of the workspace on disk: screens, order, names, layout including floating windows, and per pane a *restore recipe*. It updates the snapshot while the app runs (after workspace changes and when a pane's foreground process changes, throttled), not only on exit, because a Windows shutdown or `wsl --shutdown` ends the backend without a clean stop. A daemon keeps using `state.json`, and its respawn after a restart uses the same recipes.
- **Restore on start**: when the app starts with an empty workspace and a snapshot exists, it restores the snapshot. A user setting chooses between restoring the last session (default) and starting empty, as in classic terminals. The restore shows no server or daemon terms; a short notice offers to start empty instead, in case a restored process misbehaves.
- **Restore recipes per pane**: what comes back depends on what was running:

  | Pane at snapshot time | After restore |
  |---|---|
  | Shell at its prompt | Shell in the last working directory |
  | Claude Code with a known conversation | `claude --resume <id>` with the carried-over permission flags, started right away |
  | Claude Code, conversation unknown | `claude --resume` (the picker), started right away |
  | Any other foreground process | Shell in the last working directory with the command typed in but not run; the user presses Enter |

  Arbitrary commands are never run automatically: rerunning `make deploy` or `git push` after a reboot is not safe. Resuming a Claude Code conversation is, because it only loads the conversation and waits for input.
- **Claude Code integration**: an opt-in Claude Code hook, shipped as a Claude Code plugin, reports the conversation ID and permission mode of the Claude Code session in a pane to Splitty whenever a conversation starts, resumes, is cleared, compacted or forked. The hook finds its pane through the environment the pane's shell passes on (`SPLITTY_SOCK`, `SPLITTY_SESSION_ID`) and submits a workspace operation over the control socket; outside Splitty it does nothing. Without the hook, restore falls back to the picker.
- **Permission mode carried over**: a session started with `--dangerously-skip-permissions` (or another `--permission-mode`) is resumed in that mode. Only known permission, model and effort flags are carried over, never the full original command line, which can contain an initial prompt that would be sent again. A pane restored with permissions bypassed is visibly marked.

Out of scope:
- Reopening a single closed pane or screen (Ctrl+Shift+T). Deferred; it can reuse the restore recipes.
- Restoring terminal output or scrollback. Snapshots never contain terminal output.
- Keeping processes alive across an app restart; that remains daemon mode, and on another host `remote-screens`.
- Continuing an agent turn that was running when the app closed. The conversation comes back; the interrupted turn does not resume by itself, and Splitty never sends input to a restored session on its own.
- Resume support for agents other than Claude Code (Codex, aider and others). The recipe mechanism should allow adding them later.
- Snapshots from `splitty serve` or other embedded servers not hosted by the desktop app.
- Agent states (waiting for input, needs approval) from the same hook channel; that belongs to `terminal-intelligence`.

## Capabilities

To be decided when the change is next. Likely candidates:

### New Capabilities
- `workspace-restore`: the snapshot (contents, when it is written, which server writes it), restore on start and the user setting, the restore recipes and the rule that arbitrary commands are typed but not run, the carried-over permission flags and the bypass marker.
- `agent-integration`: how an agent running in a pane reports its conversation ID and mode to Splitty (the CLI command the hook calls, the operation it submits, the shipped Claude Code plugin). Could instead be part of `workspace-restore`; a separate capability fits better if `terminal-intelligence` reuses the channel for agent states.

### Modified Capabilities
- `server-modes`: "Two Server Modes" and "Embedded Mode Lifetime" state that an embedded server persists nothing; they change so that persistence means process continuity, and the desktop-hosted embedded server writes and reads a restore snapshot. "Session Respawn On Daemon Start" respawns from the restore recipes instead of the start command only. "Workspace Persistence In Daemon Mode" lists the new per-session fields.
- `workspace-model`: the session record gains the agent information, and an operation sets it ("Layout Operations" lists every operation).
- `terminal-sessions`: "Startup Command and Restore Mode" gains a mode that types the command without running it.
- `cli`: the command the hook calls.
- `access-control`: possibly, to make the snapshot (and `state.json`) owner-only (0600), since command lines can contain secrets. This tightens the rules; it weakens nothing.
- `mux-protocol`: only if clients need to know that the workspace was restored (for the "start empty" notice).

## Impact

- Server: tracking the foreground process and working directory per pane in a form the snapshot can use, writing and reading the snapshot in embedded mode, the restore recipes, the operation for the agent information, the respawn path shared with daemon mode.
- Desktop shells: the restore setting; on Windows the snapshot lives in the selected WSL distribution's config directory, so each distribution has its own.
- Frontend: the "restored, start empty instead" notice, the bypass marker on restored panes, the setting.
- CLI: the command the hook calls.
- New artifact: a Claude Code plugin with the hook, and a way to install it from Splitty.
- `docs/architecture.md` (Server Modes table), `docs/protocol.ts` for the new operation and session fields.

Open questions for the design:
- Where the recipe data lives. Pane cwd and foreground process come from the PTY layer's process polling, while the reducer is pure and does no I/O. Either the effect runner turns process changes into operations that update the model (one more source of revisions), or the snapshot combines the workspace with live process data held beside it.
- Whether to restore all screens at once or start the active screen's panes right away and the others when a screen is first shown, as browsers do with restored tabs. Several Claude Code processes starting at once are heavy.
- What the hook reports and when: `SessionStart` carries the new conversation ID after `/clear`, `/resume` and forks; whether the permission mode should also follow mid-session mode switches (Shift+Tab), which needs a hook on a later event.
- How the plugin is installed: offered by the app, `splitty` CLI, or documented only.
- What happens when a restored pane's working directory no longer exists (e.g. a removed git worktree).
- Ordering on shutdown: the snapshot must be taken before the server closes its sessions, so that they are not recorded as ended.

## Carried-over design notes

Input for `design.md`:
- 2026-10-09: embedded mode makes Splitty a classic terminal application without a background daemon and with the client/server architecture hidden. Writing a restore snapshot from an embedded server is in line with that; a background process or user-visible server terms are not.
- 2026-10-09: considered and set aside: (1) making the daemon the default everywhere, including the Windows app, which contradicts the principle above and still loses processes on reboot; (2) building restore on templates, which extracts in the frontend instead of on the server, is manual, and relaunches a fresh agent instead of resuming one. Daemon mode and remote hosts stay complementary: they keep an agent working while the app is closed, which restore cannot.
- 2026-10-09, code check: `template_extract.ts` already builds a pane's command from the session's last command line, including `--dangerously-skip-permissions`; the server's process polling provides cmdline, name and cwd of the foreground process (`pty/procinfo.rs`). `SessionEntry.command` only holds the start command, which is why a daemon respawns a typed `claude` as a plain shell.
- 2026-10-09, findings doc: session status never leaves `starting`, so restore must not rely on it to skip ended sessions; pane cwd and title are never updated after creation; `state.json` is written with the default umask.
- 2026-10-09, Claude Code CLI reference and hooks reference: `--resume <id>` searches the current project, its worktrees and then every other project on the machine, and attaches to the session if it still runs as a Claude Code background session; `--continue` picks the most recent conversation in the directory and is wrong with two agent panes in one repository; `--session-id <uuid>` lets a launcher choose the ID up front, but `/clear` replaces it; hooks inherit the agent's environment; hook input carries `session_id`, `cwd` and, for most events, `permission_mode`; `SessionStart` reports `source` `startup`, `resume`, `clear`, `compact` or `fork`; plugins can ship hooks in `hooks/hooks.json`. Check these again when the change is next.
- 2026-10-09, prior art: tmux-resurrect restores only a whitelist of programs; zellij's session resurrection shows commands with "Press ENTER to run" instead of running them.
- 2026-10-09, from `session-scratchpad`: keeping scratchpads in embedded mode is deferred, blocked because an embedded server does not restore its sessions. Restoring sessions with their IDs unblocks that follow-up.
- 2026-10-09, relation to `terminal-intelligence`: its Level 2 plans to detect agent states by parsing terminal output. The hook channel from this change could deliver the same states (Notification and Stop hooks) without parsing.
