# Design

## Context

The existing native client already supports multiple attachments and charged session-scoped output. The fixed probe creates one terminal and a document. The core exposes the shared `ScreenState`, `LayoutNode` and `ScreenOp`; originating mux operations require an explicit resync because the server does not nudge their sender. The Ghostty terminal view is being integrated separately.

## Goals / Non-Goals

Deliver one local screen with working splits, tabs, close and divider resizing on the existing native service. Keep the development workspace isolated and terminal views stable. Defer drag-and-drop, scratchpad, viewers, hosts, production packaging and the native Windows launcher.

## Decisions

- Use the shared Rust mux tree as the only layout model. GPUI recursively renders leaf/split/tabs nodes with native flex sizing; no independent docking framework/tree. Layout edits wait for authoritative snapshots instead of introducing optimistic state in this increment.
- A UI-free `LocalWorkspace` owns the current typed snapshot, pending client-generated pane IDs, focused pane/window-local active tabs and session-to-pane routing. `Client` sends typed operations over its existing bounded native connection and explicitly requests resync afterward. After a user changes tabs, a server announcing `tab-select-v1` receives `select_tab` for changed multi-tab groups, retaining the choice for a rebuilt window without nudging other windows. Match spawn notifications against known/pending pane IDs since they carry no screen ID. All updates stay scoped to the connection generation.
- A GPUI workspace module owns terminal entities keyed by pane ID, subscribes to their input/resize commands, and draws the current service tree. Reconcile by identity: unchanged panes keep their engine, scrollback and focus; new sessions attach once; removed/replaced sessions revoke their route and view. Hidden tab panes do not steal focus or contribute visible resize claims.
- Keep worker/service lifecycle explicit. A background worker owns the client and embedded service, sends bounded charged events, and cancels/joins on window close. The UI does not parse remote bytes on a separate authority path. Preserve current Windows explicit-unavailable behavior until its native launcher exists.
- Add an explicit legacy probe switch; retain existing `--smoke` behavior and add a local-workspace smoke path that exercises distinct terminal sessions, splits, tabs, selection, resize and close. Native smoke verifies presentation/interaction and supplements real-service in-process state/routing tests.

## Architecture impact

Add `client_workspace` under `splitty.client_core` and shared mux dependencies in `architecture/client-core.c4`. Add GPUI `workspace` with dependencies on `terminal` and public client-core API; register it from `gpui_main`. The terminal engine remains independent of workspace/client-core. The native client concept in `docs/architecture.md` records snapshot-driven rendering, identity reconciliation and the retained isolated lifetime.

## Risks / Trade-offs

- Out-of-order spawn/snapshot arrival: track intended IDs and reconcile both messages idempotently; session routes never infer authority from another screen.
- Origin does not receive a nudge: explicitly resync after mutations and surface `mux.op_failed`; never silently keep speculative layout.
- Tab display and focus are window-local: preserve local active-pane choice when valid, fall back to a visible pane after closure, and send capability-gated `select_tab` only after user selection. The server persists that tab choice without nudging other windows.
- Closing the final pane leaves the service screen empty. A new terminal recreates that screen under its existing ID and name rather than creating a second screen.
- Rapid divider gestures: show geometry from authoritative state, coalesce bounded resize intent where useful and commit valid ratios rather than creating another tree.
- Native event handling cannot be established by reducer tests: use the existing window smoke harness plus an interaction test on the actual native window.

## Provisional decisions

- NL-1 (2026-11-07, provisional, tbd): await authoritative snapshots for layout edits, with no optimistic reducer in the initial local workspace.
- NL-2 (2026-11-07, provisional, tbd): keep the first native local workspace isolated per app run, preserving the probe's cleanup contract until production persistence/ownership migration is planned.
