# Tasks

## 1. Server

- [ ] 1.1 Template file names derived from the name in the template store; save if absent atomic (complete temporary file, created only when no file exists), overwrite by replacement; temporary files with unique names
- [ ] 1.2 `POST /api/templates` and native `template.create` (`{"template", "overwrite"}`; 200 `slug`/`replaced`, 409 `slug`, 400); `template-names-v1` in `mux.welcome`; `docs/native-operations.json`
- [ ] 1.3 `scratchpadVersion` in session records, raised by every accepted `set_scratchpad`; optional `ifVersion` compare-and-set in the reducer; `scratchpad-versions-v1` in `mux.welcome`; the control API keeps the version out with the text
- [ ] 1.4 A blank or whitespace-only command is no command in template application and in a session's start
- [ ] 1.5 Tests (unit): file name derivation (`v1.2`, runs, case, cap, refusal); store save if absent, refusal of an existing file, overwrite, concurrent saves of one new name; scratchpad version rise, `ifVersion` accepted and refused without change, wire names; blank template command launches without command and restore mode; blank session command builds no startup arguments and types nothing
- [ ] 1.6 Tests (in-process): `POST /api/templates` and `template.create` give equal answers (save, 409, overwrite, 400); the welcome lists both capabilities; a refused `ifVersion` yields `mux.op_failed`

## 2. Web client

- [ ] 2.1 Template dialog saves by name with `POST /api/templates`, asks on 409 and saves the same name again with `overwrite`; its own file-name code removed
- [ ] 2.2 Scratchpads: `ifVersion` from the snapshots' `scratchpadVersion` with `scratchpad-versions-v1`, snapshots below the own accepted save ignored, versions forgotten on welcome; two-second window only without the capability
- [ ] 2.3 Tests (vitest): template save request and overwrite flow; scratchpad stale snapshot, refused race, waiting edit, older server

## 3. Native client

- [ ] 3.1 Client core: template save by name through `template.create`, `OverwriteRequired` from 409 with the server's file name, capability flag `template-names-v1` from the welcome and refusal without it; `set_scratchpad` with `ifVersion`; flag `scratchpad-versions-v1`
- [ ] 3.2 Template dialog without the file name field; overwrite confirmation for the captured name
- [ ] 3.3 Scratchpad editor: base version from snapshots, own save expected at base plus one, older snapshots ignored, versions forgotten on a new connection; settle window only without the capability
- [ ] 3.4 Tests (unit): template save request shape, 409 and older-service refusal (client core); scratchpad op carries `ifVersion`; the dialog's save sends the name without a file name (view tests)

## 4. Docs

- [ ] 4.1 `docs/protocol.ts`: `POST /api/templates`/`template.create`, `scratchpadVersion`, `ifVersion`, both capabilities
- [ ] 4.2 `docs/architecture.md` and `docs/native-ui-handoff.md` where they describe template saving and scratchpad saves; `openspec/open-decisions.md` bullet for SSR-1 to SSR-6

## 5. Checks

- [ ] 5.1 Targeted tests of what the change touched
- [ ] 5.2 Full check: `cargo fmt --all --check`, `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`, `cargo test --workspace --exclude splitty-gui`, `cargo test -p splitty-gpui --features ui-test-support`, in `frontend/` `pnpm check`, `pnpm test` and `pnpm build`, `openspec validate --specs`
