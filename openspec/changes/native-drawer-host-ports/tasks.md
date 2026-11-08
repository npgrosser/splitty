# Tasks

## 1. Exact passive host navigation

- [ ] 1.1 Add a small captured host-group target with current local owner, source ID and canonical target, valid while the same group has an opened reference. Extend existing Hosts unit/production-view tests for colliding aliases, offline/reconnect readiness independence, wrong local owner, last-reference removal and source/target replacement.
- [ ] 1.2 Extract the existing HostRow projection narrowly and add exact Host/Ports section navigation through its current row identity and existing scroll handle, including when Hosts is already open. Extend existing production-view in-process tests for reorder, colliding aliases, form/modal guards, target disappearance and passive-only commands; verify no Connect/BrowseHost, screen creation or forwarding mutation is submitted.
- [ ] 1.3 Turn the existing Drawer host caption into an accessible Kit action using that captured target. Extend Drawer production-view UI integration tests for pointer and keyboard navigation to the intended host while another source is selected, and stale-owner/group rejection without effects.

## 2. Drawer port projection and popup authority

- [ ] 2.1 Project at most twelve existing target-filtered mapping rows plus management/status entries. Extend existing Ports unit/production-view fixtures for pending disablement, Up/Rebuilding/Paused/Saved/Failed states, total versus displayed count, saved-port wording, absent/partial/empty discovery and read failures; verify inspection performs no mutation and no UI-derived URL becomes authority.
- [ ] 2.2 Establish one fresh popup context on the actual open-change interaction, keep the content builder read-only, and revalidate/consume captured context, mapping action class and local-port choice on activation. Extend real Kit-menu production-view UI integration tests for correct target despite selection/alias collisions, changed rows, duplicate input, owner replacement, dismissal/replacement and group removal followed by reopening under the same source; verify old callbacks cannot revive.
- [ ] 2.3 Preserve standard popup anchoring, bounded viewport geometry, keyboard activation, Escape/trigger focus and unpinned Drawer reveal/dismissal behavior. Extend existing Drawer production-view tests for menu bounds, pointer travel to its popup, cancellation and zero forwarding commands from hover/render/open/dismiss alone.

## 3. Shared forwarding and group-scoped browser intent

- [ ] 3.1 Extend the existing single submission/result path with an explicit Drawer origin and existing ensure-forward-and-open request, retaining the 64-feedback admission guard and exact local owner/key/request identity. Extend existing Ports in-process production-view fixtures for Up and inactive/saved mappings, actual local-port choice, no Remember escalation, duplicate/capacity refusal, conflict/failure/uncertainty and verified one-shot browser dispatch.
- [ ] 3.2 Reconcile admitted group origins independently from popup/destination/visibility and remote execution readiness; revoke them on final-reference loss, source disposal/target replacement, Stop/cancel or owner/window close. Extend the existing revocable-result fixtures for reconnect, selection and Settings/Hosts navigation, autohide/dismissal after admission, same-source group recreation, late and duplicate results, full cancellation queue and complete cleanup without implicitly stopping a service-owned forward.
- [ ] 3.3 Surface pending and failed Drawer outcomes through existing feedback/notice presentation outside Hosts. Extend production-view tests to verify rejected or obsolete actions remain honest, no success is inferred from passive refresh, and a later explicit new choice can recover without automatic replay.

## 4. Documentation and integration evidence

- [ ] 4.1 Update existing workspace descriptions/relationships in `architecture/gpui.c4` and the Drawer/Hosts/Ports/browser-origin account in `docs/architecture.md`; verify all architecture checks pass and the Core retains the sole forwarding policy with no new server/protocol dependency.
- [ ] 4.2 Run focused GPUI Hosts/Ports/Drawer tests, strict touched-crate Clippy, formatting, architecture checks and strict change validation; verify they pass and record their scope.
- [ ] 4.3 Use the existing owned remote-screen/outbound-port window fixture and opener spy for one real Linux window: inspect the Drawer menu, dismiss/reopen, navigate to the exact management section and choose a port through actual input. Verify the spy sees only the acknowledged/verified fixture endpoint, inspection has no connection or mutation side effect, viewport/focus behavior is usable, and normal owned cleanup completes. Reuse existing automation; no new server-process test framework or external host is required.
- [ ] 4.4 Before later main integration, run once: `cargo fmt --all --check`, `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`, `cargo test --workspace --exclude splitty-gui` (install architecture dependencies with `pnpm install --dir architecture` if needed), and `openspec validate --specs`. Full workspace CI remains deferred until that checkpoint.
