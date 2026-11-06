# Design

## Context

See proposal.md for the drift caused by direct template spawning. The mux already supports ordered effects and target-preserving spawn failures. ScreenViews queues snapshots until mount, so template creation needs no animation-frame race.

## Goals / Non-Goals

Use one intent for replacement and creation, with pure layout conversion and server-owned launch effects. Retain the YAML format, terminal-only templates, and per-application command confirmation. Variables, viewer templates and CLI template commands are outside scope.

## Decisions

- `apply_template` carries `template`, `paneIds`, optional `root`, and `createScreen`; its `screenId` is the mux envelope target. Pure mux conversion validates references, IDs, split ratios and floating slots before replacing state. IDs omitted by non-window internal callers are generated; window IDs are preserved. Replacing a screen keeps its name/order and increments its revision, kills its old sessions and spawns each new terminal once. A new screen is created directly without an intermediate pane.
- The pure frontend projection uses the same template traversal and normalized sizes, then passes a ScreenState to the existing snapshot renderer. It creates no session IDs. The renderer waits for pane_spawned; MuxClient caches early results for views mounted later. Failures resync and never direct-spawn a replacement session.
- The target backend resolves requested shell profile IDs under the settings-authority contract, including a visible fallback reason for unavailable selections; the window forwards saved IDs unchanged and omits absent ones. Directory joining is lexical in the pure conversion; the backend validates launch directories. Restore mode is stored in the session and carried by SpawnSession.
- Extraction is authenticated `POST /api/templates/extract/screen` / native `template.extract`, with screenId, name and optional root. The matching session read `mux.extract_template` / `mux.template_extracted` enables browser multi-host extraction without a generic per-host API; both call the same effect-runner observation and pure conversion. It snapshots mux layout then enriches terminal launch metadata with live foreground command and cwd. Viewer-only/empty layouts fail explicitly. Viewer panes are pruned and remaining split sizes renormalized; floating coordinates come directly from mux state.
- `apply_template` is accepted only on authenticated window session connections; the generic HTTP/native mux API rejects it, including owner control callers. Existing command confirmation remains in the window; cancelling sends no op, layout-only removes commands and restore modes. Restricted agent sockets cannot reach session operations.
- Legacy web panes are pruned with a visible window warning; an all-web template fails without replacing state. Floating split trees are rejected because the workspace supports floating slots only. Duplicate references and unreferenced terminal definitions fail, ensuring every accepted pane is visible exactly once.

## Architecture impact

`splitty.core.mux` gains a relationship to `splitty.core.template` for the template model and validation, with conversion and extraction in mux/templates.rs (pure, no PTY access). `splitty.core.effect_runner` carries restore mode. `splitty.server.client_session` admits and dispatches the op; `splitty.core.effect_runner` observes live process metadata for extraction; both `splitty.server.service` and `splitty.server.client_session` use it. The effect runner gains a template model relationship for its result. Existing `http` and native catalog expose extraction. Update the mux and templates concept sections in docs/architecture.md and wire documentation/catalog. No new crate or top-level module.

## Risks / Trade-offs

- A session may fail after state acceptance → report its pane ID and resync; successful siblings remain, no client retries a spawn.
- A new screen may mount after spawn results arrive → retain per-connection results until the waiter consumes them, with a bounded cache.
- Concurrent replacement → last accepted op wins; all IDs are checked workspace-wide and each replacement cleans up sessions it supersedes.

## Migration Plan

Advertise `server-templates-v1`. Windows disable application/extraction without it and offer host update. Existing stored templates need no format migration. Rollback disables the new path by removing capability support.

## Provisional decisions

- ST-1 (2026-11-06, provisional, tbd): window-session-only apply; generic mux API rejects it even without commands, avoiding a second authority path.
- ST-2 (2026-11-06, provisional, tbd): floating splits and duplicate/unreferenced terminal definitions fail validation. Empty extraction reports an error.
