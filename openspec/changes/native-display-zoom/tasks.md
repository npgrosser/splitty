# Tasks

## 1. Shared client intent and settings work

- [x] 1.1 Add pure zoom level/scope and thirteen-step intent policy; unit-test defaults, finite bounds, saturation/reset and parity with the established steps. Extend owned local settings jobs with a 32-intent bound, fair read/write scheduling, minimal patches, independent acknowledgements and no mutation retry; in-process/unit tests cover rapid ordered actions, refresh races, foreign owners, overflow, explicit uncertain replies, failures, unrelated fields and joined cancellation.
- [x] 1.2 Integrate identified zoom commands and outcomes into the native worker without consuming terminal events; process-isolated real-service tests verify persistence and input/output continuity, while owned-job tests cover close during pending settings work. Update the architecture model and settings-I/O concept documentation, verified by architecture checks.

## 2. Native geometry and interaction

- [x] 2.1 Centralize scaled terminal metrics across rendering, pointer/search/link/selection/cursor/IME and VT geometry; unit tests cover 50/125/300% and pixel-only geometry changes. Apply metrics to retained/new/hidden panes without resets or premature size claims, preserving search queries and rejecting old gestures; verify with existing engine and pointer tests plus the real-window task below.
- [x] 2.2 Apply both confirmed zoom scopes to native chrome and terminals, converting static chrome metrics to rems while preserving measured pixel geometry. Capture standard shortcuts before terminal/editors, surface outcomes, cancel stale pane drags and keep popups/toolbars usable at extreme zoom. Test shortcut matching and geometry-sensitive helpers; update native usage documentation and verify window/terminal scope behavior in the real-window task.

## 3. Acceptance

- [x] 3.1 Extend the private real-window test tooling for both scopes at 50/125/300%, rapid shortcut steps with exact PTY input exclusion, persisted restart, hidden-pane reactivation and preserved process/history/query state, scaled pointer/link/search geometry, and high-zoom popup/drag behavior. A real process/window is needed for persisted config reload, glyph bounds, layout/hit targets and platform events not established by in-process tests; inspect the resulting screenshots and record unsupported platform/IME coverage explicitly.
- [x] 3.2 Run affected client-core/native tests and all-target Clippy, architecture checks, formatting/diff checks and OpenSpec validation; verify an exact frozen release candidate under Wayland/X11 hardware workspace smoke and the targeted window acceptance, then update the preview. Full-workspace merge CI remains deferred outside this feature-branch evaluation.
