# Design

> Draft design carried over from roadmap/104-mcp-server.md on 2026-10-09; revisit against the specs before implementation.

## Context

See proposal.md for the motivation. What exists today:

- The control API (`control-api`) already covers most of what the MCP tools need: workspace and screen queries (`GET /api/mux/state`, `/api/mux/screens`, `/api/mux/screen/{id}`, `/api/mux/status`), layout ops with full effects and nudges to UI clients (`POST /api/mux/op`: `split_pane`, `close_pane`, `create_screen`, …), and input to a session (`POST /api/session/{id}/send`). It is served on the loopback HTTP listener (bearer token or session cookie) and on the owner-only control socket (no credential; route set fixed by `access-control`).
- Missing: reading a session's output, a global session list with live status/command/cwd, and starting a pane with a command (`split_pane` and `create_screen` take a working directory and shell profile, no command).
- Every session keeps a scrollback buffer (1 MiB by default, `terminal-sessions`), but it only fills while a client streams the session, and re-attaching currently loses the PTY reader (docs/issues/openspec-baseline-findings.md).
- Screen ownership and single-attach no longer exist (removed in `fa5e5b7`); any number of clients can read and modify every screen.

## Goals / Non-Goals

**Goals:**
- Agents can read screens, layouts, sessions and terminal output, and can start terminals, send input, and split/close panes and create screens, all visible to the human in the UI.
- The MCP layer stays thin: anything it needs that the server lacks is added to the control API first, so the CLI and scripts gain it too.
- No change to the credential model in `access-control`.

**Non-Goals:**
- ACP / agent UIs inside Splitty (terminal intelligence).
- Remote access paths other than the existing SSH tunnel model.

## Decisions

### Build in stages: control API first, MCP as a thin wrapper

1. **Foundation (largely done)**: the control API exposes state and operations. Still needed: an output-read endpoint (raw buffer) and, depending on the open questions, a session list and a `command` on pane creation. Useful without MCP too (scripting, CLI, monitoring) and validates the API surface before MCP sits on top.
2. **MCP server**: a thin MCP wrapper over the control API that exposes the resources and tools.
3. **Advanced, only if stage 2 proves itself**: virtual-terminal screen snapshots, output streaming through MCP subscriptions, agent-specific panes, terminal-intelligence events.

Alternative from the roadmap's diagram: an MCP endpoint inside the server process that calls the workspace store and PTY manager directly. Rejected for now: it duplicates the control API's transport and authorization work, and the original reason to avoid the WebSocket path (single-attach) is gone.

### MCP mapping

Resources (read):

| Resource | Backed by |
|---|---|
| `splitty://screens` | `GET /api/mux/screens` |
| `splitty://screens/{id}/layout` | `GET /api/mux/screen/{id}` |
| `splitty://sessions` | session tables in `GET /api/mux/state` today; needs live status/cwd (see Risks) |
| `splitty://sessions/{id}/content` | new output-read endpoint |

Tools (write):

| Tool | Backed by |
|---|---|
| `create_session` | `POST /api/mux/op` (`split_pane` / `create_screen`); a command needs a new op field |
| `send_input` | `POST /api/session/{id}/send` |
| `read_output` | new output-read endpoint |
| `split_pane`, `close_pane`, `create_screen` | `POST /api/mux/op` |
| `list_screens` | `GET /api/mux/screens` |
| `list_sessions` | see `splitty://sessions` |

Subscriptions (later): terminal output stream, session status changes (exit, exit code), title changes (cwd, foreground process).

### Terminal content: hybrid, raw buffer first

PTY output is a byte stream of escape sequences, not text.

- **Option A, raw buffer**: expose the scrollback buffer. Minimal effort; agents read ANSI surprisingly well. Good enough for the output of `cat`, `ls`, `make`; useless for full-screen TUIs.
- **Option B, server-side virtual terminal**: a headless terminal emulator per session keeps the screen state and returns a clean text snapshot. Needed for TUIs (vim, htop); costs memory per session and must follow the PTY size.
- **Chosen: hybrid.** `read_output(mode: "buffer")` returns the raw buffer (last N bytes or lines), `read_output(mode: "screen")` returns a virtual-terminal snapshot. Build A first, add B when use cases show up.

### Access modes

Two modes per MCP connection: **read-only** (resources only, no input, no layout changes) and **full control** (everything). Default is full control, because the agent is explicitly invited; read-only serves monitoring.

### Locating and authenticating against the server

The roadmap planned to authenticate with `SPLITTY_TOKEN`. That is not available: `access-control` forbids reading credentials from the environment and putting them into pane environments. The MCP server uses the same paths as the CLI (`cli`, Server Discovery Order): the owner-only control socket via `SPLITTY_SOCK` (inside a pane) or `daemon.json` (daemon), or an explicit address and bearer token over loopback TCP. A served SSE/HTTP transport would live behind the existing Host, Origin and Fetch Metadata guards.

## Risks / Trade-offs

- [Scrollback only fills while a client streams the session, and re-attach currently loses the PTY reader] → fix both before `read_output` ships, otherwise agents read stale or truncated output (baseline findings).
- [The workspace's session status never leaves `starting`, pane title and cwd are never updated] → `list_sessions` needs these fixed or must read live process info instead.
- [New endpoints over the control socket widen its route set] → explicit MODIFIED of `access-control` Control Socket Route Set; the socket must never gain credential-minting routes.
- [A read-only mode enforced only in the wrapper] → document it as a convenience; anyone with socket access can call the control API directly.
- [Virtual terminal memory and size sync] → create it on demand, not for every session.
- [Unclear daily use case] → keep stages 1 and 2 lean and see whether agents actually use it.

## Architecture impact

- Adds an MCP server component next to the WebSocket handler and HTTP API in the overview, as a client of the control API rather than of the internal store.
- The CLI Flow section of docs/architecture.md (already stale, see baseline findings) would be rewritten to describe the control API as the shared surface for CLI and MCP.
- A virtual-terminal component per session (stage 3) would add a module to the PTY layer in the module map.

## Open Questions

- Transport: stdio, SSE/HTTP, or both? (stdio fits the control socket and works over `ssh host …` for remote use; SSE/HTTP needs its own authorization.)
- Tool granularity: switch tab, floating panes, resize?
- Output cleanup: strip everything, strip formatting but keep structure, or raw?
- Session ownership: agent-owned sessions that end with the MCP connection, or screen-owned sessions that survive?
- Multiple agents: namespace isolation or shared access?
- Virtual terminal: per session or on demand?
