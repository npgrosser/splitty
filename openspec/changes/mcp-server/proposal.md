# Proposal

> Draft migrated from roadmap/104-mcp-server.md on 2026-10-09. Not planned yet. Specs and tasks are written when this change is next. Depends on: the existing `control-api` (the foundation the roadmap called "Stufe 1" is largely in place) and on fixing the session output bugs listed in docs/issues/openspec-baseline-findings.md (scrollback only fills while a client streams; re-attach loses the PTY reader). Related: `cli-control` shares the same API surface; `terminal-intelligence` is complementary; `multi-attach` may share its role model.

## Why

When an AI agent (Claude Code, Cursor, …) starts subprocesses today, they run invisibly: the human cannot see or intervene, and the agent cannot see the terminals the human already has open. Copy-paste is the only bridge. An MCP server lets agents use Splitty as a tool, so agent and human share one workspace: the agent reads the human's terminals, starts its own terminals visibly in panes, and arranges the layout, while the human watches and can step in.

## What Changes

- **MCP server for Splitty**: agents connect over MCP and get read access through resources and control through tools. A thin wrapper over the control API, not a new path into the server's internals (see design.md).
- **Resources (read)**: the list of screens; a screen's layout (panes, splits, tabs); the list of sessions with status, command and working directory; a session's terminal content.
- **Tools (write)**: start a terminal (optionally with command and working directory), send input to a session, read a session's output, split a pane, close a pane, create a screen, list screens, list sessions.
- **Reading terminal output over the control API**: the one foundation piece that does not exist yet. The first version returns the raw scrollback buffer (escape sequences included); a server-side virtual terminal that returns a clean text snapshot of the visible screen comes later, only if TUI use cases show up.
- **Access modes per MCP connection**: read-only (resources only) and full control (default, since the agent is explicitly invited).
- **Later, only if the first stages prove useful**: output streaming and session/title change notifications via MCP subscriptions, screen snapshots from a virtual terminal, agent-specific panes, and semantic events from terminal intelligence instead of raw output.

Out of scope: ACP (Agent Communication Protocol) and agent UIs inside Splitty — that is the terminal intelligence change. MCP gives the agent hands (control), ACP gives Splitty eyes (understanding). Remote MCP access beyond what the existing SSH tunnel model allows is out of scope as well.

## Capabilities

To be decided when the change is next. Likely candidates:

### New Capabilities

- `mcp-server`: the MCP surface — resources, tools, access modes, transport, how an MCP server locates and authenticates against a Splitty server.

### Modified Capabilities

- `control-api`: an endpoint to read a session's output (raw scrollback, later a screen snapshot); possibly a global session list with status, command and working directory, and starting a pane with a command.
- `access-control`: if new endpoints must be reachable over the control socket, its **Control Socket Route Set** requirement is extended (a deliberate MODIFIED); any read-only mode that is meant as a security boundary needs a requirement here.
- `workspace-model`: only if agent-created sessions get a different lifetime than other sessions (see Impact, session ownership); `split_pane` and `create_screen` currently take a working directory and shell profile, but no command.
- `terminal-sessions`: if output reading needs guarantees the session layer does not give today (output captured while no client streams).

## Impact

- **Server**: a new read path for session output; possibly a headless terminal emulator per session later (memory per session, must follow PTY size).
- **CLI / binary**: the MCP server would ship with the `splitty` binary or as a wrapper next to it; how it is launched is open.
- **Data quality**: the session table in the workspace never leaves status `starting`, and pane title and working directory are not updated after creation (baseline findings). `list_sessions` with status and cwd needs those fixed or another source (the live process info the server sends as `title` events).
- **Already in place** (roadmap "Stufe 1"): listing screens and workspace state, applying layout ops (split, close, create screen) with nudges to UI clients, sending input to a session (`control-api`).

Changes against the roadmap doc:
- The roadmap argued against implementing the MCP server as a WebSocket client because of single-attach. Single-attach and screen ownership were removed in March 2026; every client can already modify every screen. The reason for building on the control API is now reuse and the credential model, not attach conflicts.
- The roadmap proposed authenticating with `SPLITTY_TOKEN`. That conflicts with `access-control`: the server never consults `SPLITTY_TOKEN` and never puts a credential into a pane's environment. An MCP server must use the existing paths instead: the owner-only control socket (found like the CLI finds it, via `SPLITTY_SOCK` or `daemon.json`) or an explicit bearer token over loopback TCP.

Open questions for the design:
- Transport: stdio (simple, local), SSE/HTTP (served by the Splitty server, needs its own authorization within the `access-control` guards), or both?
- Tool granularity: are the tools above enough, or are finer operations needed (switch tab, floating panes, resize)?
- Output format: how much ANSI cleanup does the server do versus the agent — strip everything, strip formatting but keep structure, or raw?
- Session ownership: does a session the agent creates belong to the agent (ends with the MCP connection) or to the screen (survives)? Today every session belongs to the workspace and outlives its clients.
- Multiple agents at once: namespace isolation or shared access? Concurrent clients are already allowed; layout-only ops sent over the WebSocket are not propagated to other clients (baseline findings), ops via the control API are.
- Virtual terminal: worth the overhead for every session, or only on demand when an agent asks for a screen snapshot?
- Read-only mode: where is it enforced? The control socket has no per-client permissions; a mode enforced only in the MCP wrapper is a convenience, not a security boundary.
- How realistic is the daily use case? The individual capabilities are clear, a scenario where agents use it every day is not yet sharp. Keep the first stages lean to find out.
