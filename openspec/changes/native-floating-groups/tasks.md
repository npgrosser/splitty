# Tasks

## 1. Shared floating invariants

- [ ] 1.1 Share finite/positive floating-bound validation between direct float/move ops and template application; extend mux unit tests for invalid values, atomic rejection and valid offscreen/oversized bounds.
- [ ] 1.2 Preserve sibling layout and sessions when closing a pane in a nested float; extend reducer unit tests for stack removal, slot pruning, single-child collapse, last-pane removal and effect-free transfer out of a nested float.
- [ ] 1.3 Update the shared mux architecture description and workspace documentation; verify the architecture map and the unchanged touched wire shapes in docs/protocol.ts.

## 2. Client projection and captured floating actions

- [ ] 2.1 Extend canonical projection, focus, selection/cycling, pending new-tab focus, maximize and displayed-size membership across tiled/floating roots; extend client unit tests for hidden tabs, nested observations, Screen switches, maximize/restore and unchanged routes.
- [ ] 2.2 Add captured float/dock/bounds admission, one identified pending action and ordered settlement with no mutation retry; extend unit tests for stale identities/bounds, metadata tolerance, no-op and queue rejection, late failure, later-navigation cancellation and coherent-refresh interaction.
- [ ] 2.3 Permit existing Screen-transfer admission from floating tabs while retaining identity checks and root-only constraints on unsupported actions; extend unit/in-process tests for transfer, new tabs/profile inheritance, whole-group dock into populated/empty roots, continued output and joined shutdown.
- [ ] 2.4 Update client-core component descriptions and Client connections documentation; verify the architecture check includes any new direct dependency edge.

## 3. Native floating surfaces and gestures

- [ ] 3.1 Render canonical floating roots as occluding overlays with local stacking and reachable finite geometry, reusing tab/terminal views; add focused unit tests for projection/clamping, stack pruning and root-only action availability.
- [ ] 3.2 Add Float Tab, Dock and canonical-slot maximize controls plus captured move/resize previews; test gesture normalization, unchanged release, stale/cancelled gestures and viewport/zoom/window-deactivation cancellation without terminal input.
- [ ] 3.3 Integrate floating surfaces into geometry, keyboard focus, visible-size reporting, profile/transfer menus, search and close handling; verify focused native tests and update GPUI architecture/runbook documentation.
- [ ] 3.4 Add private real-window acceptance for overlap/pointer isolation, Float/NewTab/Move/Resize/Dock, focus/maximize, selection/search/history/process retention, background output, invalidated gestures and final cleanup. This smoke-level check is needed because model tests cannot verify hit occlusion, native capture, actual rendering or PTY dimensions.

## 4. Integration and acceptance

- [ ] 4.1 Run affected shared mux/client/native tests, targeted strict Clippy, formatting, architecture and OpenSpec validation; record platform and unchanged web behavior limits.
- [ ] 4.2 Freeze a release candidate, pass private floating acceptance and existing hardware workspace smoke, visually inspect screenshots, and update the preview with matching binary/evidence hashes.

Full workspace merge checks and CI remain deferred while the experimental branch develops. Before a later merge, run the full checks in CLAUDE.md once. After implementation acceptance, sync and archive this change through the normal completion workflow.
