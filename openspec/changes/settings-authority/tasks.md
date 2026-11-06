# Tasks

## 1. Server metadata and validation

- [x] 1.1 Add server-owned defaults and field limits at `home.metadata` / `GET /api/home/settings/metadata` on native and browser adapters; extend existing catalog and HTTP in-process tests to verify shapes and authorization.
- [x] 1.2 Validate PATCH fields atomically, including editor rules, fractional sensitivities, numeric ranges and enums, and remove stale terminal fields; extend config unit and HTTP in-process tests for valid, invalid and legacy stored values.

## 2. Server change events and shell fallback

- [x] 2.1 Broadcast `settings.changed` after successful settings/profile writes through native and browser clients and document wire shape in `docs/protocol.ts`; extend existing client-session and HTTP in-process tests to verify delivery and absence on failure.
- [x] 2.2 Resolve missing, disabled and unavailable profile IDs on the server, report reason in `created`, and update protocol docs; extend existing shell-profile unit and session in-process tests for fallback and no usable default.

## 3. Window settings client

- [x] 3.1 Load server metadata, use its defaults and limits for reset and controls, and keep older-host behavior; extend frontend unit tests for metadata and compatibility.
- [x] 3.2 Apply `settings.changed` in each window, rollback rejected/failed optimistic writes, and expose field-specific failures; extend frontend unit tests for success, concurrent changes and rejected writes.
- [x] 3.3 Remove frontend shell default resolution for new local screens while retaining explicit profile choices and inherited choices; extend frontend unit tests for the resulting request shape.

## 4. Documentation and integration

- [x] 4.1 Update `docs/native-operations.json`, `docs/architecture.md` and relevant `architecture/*.c4` relationships for the implemented settings flow; verify the catalog and architecture checks.
- [x] 4.2 Run targeted Rust unit and in-process tests and targeted frontend tests of the touched modules, and validate the change with `openspec validate settings-authority --strict`.
- [ ] 4.3 Run the one full pre-merge check: `cargo fmt --all --check`, `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`, `cargo test --workspace --exclude splitty-gui`, `pnpm check`, `pnpm test`, `pnpm build` in `frontend/`, and `openspec validate --specs` (after `pnpm install --dir architecture` if needed).
