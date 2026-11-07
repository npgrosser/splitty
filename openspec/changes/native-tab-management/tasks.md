# Tasks

## 1. Shared exact placement

- [x] 1.1 Add `move_tab` validation and pure reduction; extend mux unit/store tests for first/middle/end, implicit and explicit active tabs, stable identities/session records, current nonuniform source proportions, atomic invalid source/destination/anchor/self-anchor rejection, unrelated concurrent state and unchanged legacy append/activate.
- [x] 1.2 Advertise `tab-placement-v1` and align docs/protocol.ts and frontend protocol types; extend protocol unit and existing server in-process propagation/rejection tests for accepted and rejected placement, stream ordering and no process effects. Reuse existing transport-equivalence and authorization tests for unchanged Layout Operations scenarios.
- [x] 1.3 Update core architecture description and layout operation documentation; verify the model still permits no I/O in reduction and has no new dependency.

## 2. UI-free client intent

- [x] 2.1 Implement captured exact placement admission, capability/no-op guards and connection-wide ordered settlement; extend client unit tests for missing capability, stale/cancelled targets, metadata-only changes, before/end confirmation, earlier catalog/snapshot, late failures, malformed verification, removed Screen and queue-pressure retries.
- [x] 2.2 Preserve same-group selection/maximization and cancel deferred cross-group focus on later navigation; add captured focused-group clamped TabStep commands. Extend unit/in-process tests for selection, Screen/slot/owner isolation, stale worker selection, confirmed movement and session/output continuity.
- [x] 2.3 Update client architecture descriptions and docs/architecture.md for intent ownership and ordered settlement; verify no GPUI types enter client policy.

## 3. Native interaction

- [x] 3.1 Add scrollable fixed-height tab strips, pinned add action, header insertion targets/marker and selected-tab reveal; extend GPUI geometry/unit tests for clipping, before/after/end, no-op and header/body target separation.
- [x] 3.2 Wire Mod+PageUp/PageDown with editor/dialog priority and captured worker validation; extend shortcut tests for both directions, modifiers and blocked input.
- [x] 3.3 Extend existing gpui-screens.py smoke coverage for real pointer first/middle/end and cross-group insertion, overflow reachability, clamped focused-group keys, maximize/selection/session retention and Escape cancellation. Use the existing owned control socket to assert authoritative order; real GPUI geometry, scroll and event dispatch cannot be established in-process. Keep exact stale-event races covered in-process if real timing is unstable.
- [x] 3.4 Update native usage/limitations and GPUI architecture descriptions; verify documented gestures against the built window.

## 4. Integration

- [x] 4.1 Run targeted mux/protocol/server/client/GPUI tests, touched-package Clippy, frontend type checks, architecture tests, formatting and strict OpenSpec validation; repair relevant regressions. Broad workspace CI remains deferred for this native milestone.
- [x] 4.2 Freeze the validated release binary, run owned real-window and Wayland/X11 hardware smoke checks, record binary hashes/cleanup evidence for preview integration after the checks pass.
- [x] 4.3 Review the final diff for integration on the experimental native branch; verify main and the web runtime remain unchanged.

Finalize by committing the verified implementation, syncing/archiving the completed change, and pushing the experimental native branch.
