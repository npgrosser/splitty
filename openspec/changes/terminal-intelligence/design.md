# Design

> Draft design carried over from roadmap/06-terminal-intelligence.md on 2026-10-09; revisit against the specs before implementation.

## Context

See proposal.md for motivation and scope. Current state that shapes the approach:

- **Level 1 pipeline (implemented):** an OSC scanner taps the PTY output in the server's read path and recognizes OSC 0/2 titles and OSC 133 `A`/`B`/`C`/`D`. It updates a per-session semantic state (phase, phase start, last exit code, command count) and forwards each boundary as an `osc133` event. The state is sent as a snapshot with `created`/`attached`. The notification engine turns `D` into `process.idle`. The design and its rejected alternatives are in `docs/osc-133-semantic-prompts.md`.
- **Foreground process detection (implemented):** about once per second the server resolves the session's foreground process (process group leader) and reports name, working directory, command line and whether it is the base shell in `title` events (see `terminal-sessions`, Session Metadata Updates). This drives `process.finished` and is the app detector Level 2 needs.
- **Constraint: scanning only happens while a client streams.** The scanner lives in the per-stream read path, and nothing reads the PTY otherwise (see the findings doc). Server-side intelligence that "survives disconnects" therefore requires a reader that keeps running independently of client streams. Level 2 and the Level 1 completion depend on this.
- **Constraint: no frames.** Webview panes are gone and the CSP forbids frames (`access-control`, Frontend Security Headers). Any new UI for Level 2/3 is native to the Splitty frontend.

## Goals / Non-Goals

**Goals:**
- One server-side parse result per session, shared by all clients and kept while no client is connected.
- Each level usable on its own; Level 2 and 3 feed the same UI where they overlap (diff preview).
- Parsers that fail quietly: when unsure, emit nothing.

**Non-Goals:**
- Full terminal emulation on the server (cursor, cell grid, attributes).
- A stable external plugin API in the first iteration.

## Decisions

### Parse on the server, not in the client renderer
The raw byte stream is cleaner to parse than a rendered terminal buffer; one result serves every client (web, desktop, future ones); parsing can be stateful across many writes; and it keeps working while no client is attached. *Alternative:* parsing in xterm.js. Rejected because the renderer does not expose its parsed state usefully and sees nothing while the window is closed. Raw bytes still reach the client unchanged, so a client may additionally parse on its own.

### A byte-stream tap, not a headless emulator
The scanner is a small state machine with constant memory, extended per sequence. *Alternative:* a full emulator (screen state per session). Rejected as orders of magnitude more code and memory for no Level 1 benefit. The roadmap names a VT parser (`vte`) and plain-text extraction (stripping escape sequences) as the core for Level 2 pattern matching; whether app parsers need them, or the existing scanner plus plain-text extraction suffices, is decided when Level 2 starts.

### Events on the wire, state on connect
Implemented for Level 1 and kept for app state: clients get lightweight transition events while connected and the aggregated state as a snapshot on (re)attach. *Alternative:* replaying an event log. Rejected because the history is unbounded and silently wrong when truncated, while the aggregated state is O(1) and always current.

### App detection by foreground process name
Level 2 selects the parser from the foreground process name already tracked by process polling. When the foreground process changes, the active parser is reset and replaced (shell → vim → shell).

### Parser selection: a glob on the binary name, configured globally
Built-in parsers are enabled by default; users switch off what bothers them. Matching is a glob on the binary name, nothing else: whoever starts `claude` wants the Claude parser, regardless of directory or environment. Configuration lives in the home config:

```toml
[parsers]
claude-code = { enabled = true,  match = "claude*" }
vim         = { enabled = true,  match = "{vim,nvim}" }
htop        = { enabled = false, match = "htop" }
```

Per-session overrides are conceivable but not in the first iteration.

### An internal parser boundary shaped like a future plugin API
Each parser answers whether it matches a process name, is fed output bytes chunk by chunk, is polled for semantic events, and is reset when the process changes. Parsers live in the Splitty codebase for now; the boundary is kept narrow so it can later become an external plugin interface (dynamic library or WASM).

### Delivery through notification events
All levels report through the notification events (drawer badges, toasts, desktop notifications) instead of inventing a separate channel. Raw `osc133` events stay standard-faithful (A/B/C/D); what counts as "idle" is decided by the notification engine and the client.

### Diff preview as a native, read-only pane
When the Claude Code parser recognizes an edit block (file, context lines, additions, removals), Splitty reconstructs the proposed version and shows a side-by-side diff against the current file. Before the edit is accepted it diffs the proposal against the file on disk; after acceptance the parser still knows the previous state, so the diff stays available. Read-only, no editor. The roadmap planned a Monaco diff view in a webview pane; it is now a native pane (see Context). With Level 3 the same pane takes its data from structured agent messages instead.

### Level 3: Splitty as ACP client
Agents act as ACP servers and Splitty as the client, giving bidirectional, structured interaction (diffs, tool approvals, conversation) for any ACP-speaking agent. Reference implementation named by the roadmap: `zed-industries/claude-agent-acp`. Level 2 is passive and fragile observation; Level 3 replaces it where an agent supports ACP.

## Risks / Trade-offs

- [App parsers break when the app changes its UI] → tolerant pattern matching (regular expressions, not exact strings), parsers versioned per app, and no event when the parser is unsure.
- [Sessions without integration (fish today, SSH, tmux) stay in an unknown phase] → process polling remains the universal fallback; OSC 133 is used when present.
- [Reading the PTY without a client changes session behavior (scrollback now fills while nobody watches, a chatty process no longer blocks)] → specify it in `terminal-sessions` as part of the reader fix.
- [Parsers see all terminal output, including secrets typed or printed] → parsed content stays in memory and is only sent to the session's clients like the output itself.
- [A Monaco-based diff view must run under the current CSP (`script-src 'self'`, no frames)] → verify its worker setup against the CSP or pick a lighter diff renderer.
- [Shell injection side effects (`ZDOTDIR`, bash POSIX mode, `HISTFILE`)] → existing behavior; keep the restore logic in the scripts covered by tests when fish is added.

## Architecture impact

`docs/architecture.md` does not describe the PTY, OSC or notification pipeline at all. This change adds:
- a "Terminal intelligence" section: scanner tap → semantic state → `osc133` events and notifications; foreground-process detector → parser registry → app-state events; later the ACP client;
- module map entries for the OSC scanner, the notification engine, the parser registry and the ACP client;
- a change to the PTY read model: one continuous server-side reader per session that feeds scrollback, scanners and parsers, with client streams as consumers.

## Open Questions

- Event granularity for app parsers (see proposal.md).
- When to open the parser boundary to external plugins.
