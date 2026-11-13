# Tasks

## 1. Shared types and hub

- [ ] 1.1 `splitty_core::native_api`: `remoteScreens.updateHost` (`PUT /api/remote/screens/host`) in the catalog; the request types `remembered::HostScreens` and `directory::ObservedScreen` (optional `panes`); the `hub-host-state-v1` constant
- [ ] 1.2 `hub::remembered`: replace one host's entries in place under the hub's lock, empty forgets, `closed` on each entry, 200 at most; route and dispatch; welcome capability next to `hub-host-decisions-v1`
- [ ] 1.3 `hub::catalog`: observations whose screens omit `panes` keep the stored count and leave out screens without one; `hub.catalog.observe` and `PUT /api/remote/catalog` share it
- [ ] 1.4 `failure::UpdateAvailability`: `toVersion` whenever `available`, in `remote.ready` and `incompatible` errors
- [ ] 1.5 Tests (unit, `hub::remembered`, `hub::catalog`, `hub::failure`): in-place replacement, forget, `closed`, invalid bodies; pane counts kept and missing ones left out; `toVersion`
- [ ] 1.6 Tests (in-process, router and native operations): `PUT /api/remote/screens/host` keeps other hosts' entries and `closed` marks; `docs/native-operations.json` lists the operation; the capability on local main connections (conformance); `remote.ready` carries `toVersion`

## 2. Native client

- [ ] 2.1 `hub::remembered`: write each edited host with `remoteScreens.updateHost` (no read before a write); nothing written without `hub-host-state-v1`
- [ ] 2.2 `native_workspace::host_catalog`: report a ready host's live catalog without pane counts whenever it changes; nothing without `hub-host-state-v1`
- [ ] 2.3 `workspace::window::host_update`: take `toVersion` from the hub's `update` as the replacement version
- [ ] 2.4 Tests (unit): the per-host write and the read-only mode; the live report with additions, renames and removals; `toVersion` from ready and failure

## 3. Web client

- [ ] 3.1 `lib/remote/update.ts`: offer only with `available` and `keepsSessions`; name `toVersion`; drop `updateImpact` and the texts for an update that ends sessions; `Workspace.svelte` and `HostPickerDialog.svelte` follow
- [ ] 3.2 `lib/remote/remembered.ts`: keep `closed`, skip it on restore, write per host with `PUT /api/remote/screens/host` only for hosts the window shows or showed, nothing without `hub-host-state-v1`
- [ ] 3.3 `lib/remote/host_directory.ts`: remove the merge, reads and catalog writes for hubs without `hub-host-decisions-v1`
- [ ] 3.4 vitest: `update.test.ts`, `remembered.test.ts`, `host_directory.test.ts`

## 4. Docs

- [ ] 4.1 `docs/protocol.ts`, `docs/native-operations.json`, `docs/architecture.md`, `docs/native-ui-handoff.md`
- [ ] 4.2 `openspec/open-decisions.md`: HHS-1 to HHS-4

## 5. Checks

- [ ] 5.1 Targeted tests of the touched modules
- [ ] 5.2 Full check as in CLAUDE.md, including `cargo test -p splitty-gpui --features ui-test-support` and `pnpm check`, `pnpm test`, `pnpm build` in `frontend/`
