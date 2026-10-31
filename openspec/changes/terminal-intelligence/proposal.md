# Proposal

> Draft migrated from roadmap/06-terminal-intelligence.md on 2026-10-09. Not planned yet. Specs and tasks are written when this change is next. Depends on: the PTY read-path fixes in docs/issues/openspec-baseline-findings.md (a re-attach must keep the session's reader, and output must be read while no client streams the session); `terminal-sessions`; the notification events (`process.*`, `session.exited`, `notification`), which have no baseline spec yet. Related: `mcp-server` (the opposite direction, Splitty as a tool for agents) and the viewer pane from `splitty-open`.

## Why

Terminal emulators traditionally do not know what happens in their sessions. Splitty owns the PTYs, so the server can read the byte stream itself and derive semantic events from it: which command finished with which exit code, whether an agent is waiting for input or for a tool approval, and eventually talk to agents through a structured protocol instead of scraping their output. Doing this on the server gives one result for every client and keeps working while no window is open, which is what notifications about unattended sessions need.

## What Changes

The roadmap describes three independent levels, built in the order 1 → 2 → 3. When this change is next it will most likely be split into one change per level.

**Already implemented** (March 2026, `f4fa994`, `41396c7`, design in `docs/osc-133-semantic-prompts.md`): OSC 133 A/B/C/D parsing on the server, a per-session semantic state (phase, last exit code, command count) sent with `created`/`attached`, `osc133` events to streaming clients, `process.idle` on command completion (10 s cooldown), and auto-injected bash/zsh integration without rc-file changes. The title-icon idle heuristic the roadmap calls the "first generic app parser" was removed in favor of OSC 133. None of this is specced yet.

- **Level 1 — finish shell integration**
  - Keep semantic tracking running while no client streams a session. Today the scanner sits in the client read path, so a command that finishes while the window is closed is missed, contrary to the documented design.
  - fish integration (the roadmap names bash, zsh and fish; only bash and zsh exist, and bash only when started as a plain interactive shell).
  - OSC 7 (working directory) parsing; the working directory currently comes from process polling only. OSC 0/2 titles are already parsed.
  - Spec the existing Level 1 behavior.
- **Level 2 — app-specific parsers**
  - A server-side parser registry selected by the foreground process name (detected with the existing process polling). The parser switches automatically when the foreground process changes (shell → vim → shell).
  - Parsers emit semantic app states, e.g. Claude Code waiting for input, waiting for tool approval or thinking, and the vim mode.
  - User-facing config in the home config file: built-in parsers are enabled by default and can be switched off per binary; matching is a glob on the binary name only.
  - First concrete consumer: a **read-only diff preview** of file edits an agent proposes, reconstructed from the terminal output, available before and after the edit is accepted. Rewritten from the roadmap: it planned the preview as a webview (iframe) pane, but webview panes were disabled in August 2026, their server side was removed in `5912c28`, and `access-control` forbids frames (`frame-src 'none'`). The preview has to be a native UI pane.
- **Level 3 — agent protocol (ACP)**
  - Splitty as an ACP client and agents as ACP servers: a dedicated UI for diffs, tool approvals and the conversation, without a terminal pane, for any agent that speaks ACP. The Level 2 diff UI stays; only its data source changes from terminal scraping to structured messages.
- **Delivery**: every level reports through the existing notification events (drawer badges, toasts, desktop notifications).

Out of scope:
- A headless terminal emulator or screen state on the server; the scanner stays a byte-stream tap.
- An external plugin API for parsers (dynamic libraries or WASM); the internal boundary is only designed so it could become one later.
- Per-session parser overrides and matching on working directory, environment or arguments.
- Editing in the diff preview; it is visualization only, not an editor.
- Exposing Splitty to agents as a tool (`mcp-server`).

## Capabilities

To be decided when the change is next. Likely candidates:

### New Capabilities
- `shell-integration`: OSC 133 semantic prompts, the per-session semantic state and its snapshot, injected integration scripts, OSC 7. Could instead become part of `terminal-sessions`.
- `notifications`: the `process.finished`, `process.idle`, `session.exited` and `notification` events and their replay on reconnect (the roadmap's `session.crashed` and `app.idle` no longer exist; a crash is `session.exited` with a non-zero code). Every level delivers through them and no spec covers them yet.
- `app-state-detection` (Level 2): parser selection, the parser config, app-state events, the diff preview's data.
- `agent-protocol` (Level 3): Splitty's ACP client behavior.

### Modified Capabilities
- `terminal-sessions`: a reader that keeps running without a streaming client; the shell-integration environment and arguments (`SPLITTY_SHELL_INTEGRATION`, `ZDOTDIR`, `ENV`, `--posix`), which exist today but were left out of the baseline; the semantic fields on `created` and `attached`.
- `mux-protocol`: new server-to-client event types on the shared connection (app state, agent messages).
- `workspace-model`: a pane kind for the native diff preview and possibly for agent UIs. The model already has `webview` and `viewer` roles that no op creates.
- `access-control`: only if Level 3 needs a new listener, transport or credential for agents. That would be a deliberate MODIFIED, never a side effect.

## Impact

- Code: the server PTY read path and OSC scanner, the notification engine, protocol events (and `docs/protocol.ts`), the home config (`[parsers]`), `shell-integration/` (fish), the frontend (app-state UI, diff pane, agent UI).
- Packaging: only the Windows WSL payload is checked to bundle `shell-integration/` (`windows-tauri.yml`). The Linux packages and the macOS bundle must ship it too, or Level 1 silently degrades to process polling in release builds.
- Bugs to fix first (findings doc): the lost PTY reader on re-attach, no reading while no client streams, two streams sharing one reader.
- App parsers break when an app changes its UI; see design.md for the mitigations.

Open questions for the design:
- Inject shell integration or only detect an existing one? Today zsh is always injected and bash when started plain; the scripts guard against double sourcing.
- How granular should app-parser events be? Is `WaitingForInput` enough, or are cursor position or the visible prompt text needed too?
- Should `process.idle` also come from app parsers? An agent waiting inside a running TUI never emits OSC 133 `D`.
- ACP: a dedicated pane widget, or embedded in the existing split tree?
- ACP transport: the roadmap says JSON-RPC over WebSocket. Check the current ACP spec; agents are often run as a subprocess speaking JSON-RPC over stdio, which would need no new listener.
- When to open the app-parser API to external plugins (after the internal API is stable).
