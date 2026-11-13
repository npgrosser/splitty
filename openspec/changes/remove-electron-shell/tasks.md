# Tasks

## 1. Remove the shell and its build

- [x] 1.1 Delete `frontend-electron/` and `packaging/electron/`; drop the Electron targets from the Makefile (`electron`, `install-electron`, `uninstall-electron`, `run-electron`) and the Electron icon from `scripts/generate-icons.sh`.
- [x] 1.2 CI and release: remove the `electron` job and the Electron npm audit from `ci.yml`, the Electron type install from `ci.yml`, `server-payloads.yml` and `packaging/macos/build.sh`, the Electron lockfile from the payload cache keys, and the Dependabot entry; reword the release workflow's header.
- [x] 1.3 `scripts/release.sh`: no Electron `package.json`; `--check` keeps checking `Cargo.toml`, `tauri.conf.json` and `Cargo.lock` (run `scripts/release.sh --check v<current version>`).

## 2. Web frontend

- [x] 2.1 Move the native transport codec (`native_wire.ts`) and `NativeAssignment` into `frontend/src/lib/protocol/`, since the Tauri adapter shares them; delete `electron_channel.ts`.
- [x] 2.2 Remove the `electron` shell kind and the `__SPLITTY_ELECTRON__` branches in `platform.ts`, `window.ts`, `zoom.ts`, `file_offers.ts`, `editor_requests.ts`, `remote_editor.ts`, `remoteWindows.ts`, `protocol/bootstrap.ts` and `App.svelte`, and the `noDragRegion` action, which only Electron needed.
- [x] 2.3 Unit tests (vitest): delete the tests that imported Electron sources (`electron_*`, `native_broker`, `native_budget`, `native_helper`, `native_pipe`, `native_preload`, `electron_known_hosts`); move the Electron cases of `bootstrap`, `browser_assignment`, `platform`, `window`, `zoom`, `editor_requests`, `editor_target`, `context_menus` and `remoteWindows` tests to the Tauri bridge or drop them where Tauri already has the case. This covers the modified scenarios of Remote Window Shell API, Remote Folder Editor Handoff, Local Folder Editor Handoff and Showing A Saved Download on the frontend side.

## 3. Rust

- [x] 3.1 Remove the opt-in Electron SSH smoke from `crates/splitty-server/tests/remote_connect.rs`; reword the comments in `http/middleware.rs`, `splitty-remote/src/tunnel/mod.rs` and `splitty-tauri/src/client_ipc.rs`.
- [x] 3.2 Unit: the Tauri tests that cover the modified access-control scenarios stay as they are and keep passing (`capability.rs` `remote_windows_only_have_native_transport_commands_and_no_remote_origins`, `editor.rs` `bounds_untrusted_input_before_dispatch`, `downloads.rs` `a_failed_or_unrequested_download_is_not_shown`); the scenarios only lose their Electron clauses.

## 4. Docs, model and decisions

- [x] 4.1 Architecture model: remove the `electron` container and its two relationships from `architecture/landscape.c4`, and the mention in `architecture/README.md`; `docs/architecture.md` drops the Electron pipe adapter, listener inventory rows and Electron variants.
- [x] 4.2 Update `CLAUDE.md`, `SECURITY.md`, `docs/{cli,development,networking,remote,known-issues,native-ui-handoff,gpui-spike,protocol.ts}`, `docs/security/findings.md`, `docs/issues/openspec-baseline-findings.md`, `scripts/testing/README.md`, the release skill and `roadmap/README.md`.
- [x] 4.3 `openspec/open-decisions.md`: remove the Electron-only points, update the mixed ones and add RES-1 to RES-4; add carried-over notes to the drafts and take the Electron channel names out of `native-client-transports`' delta specs.

## 5. Checks

- [x] 5.1 Targeted: `pnpm test` of the changed frontend tests, `cargo test -p splitty-server --test remote_connect --no-run`, `scripts/release.sh --check`, the workflow YAML parses.
- [x] 5.2 Full check: `pnpm install --dir architecture` if needed; `cargo fmt --all --check`; `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`; `cargo test --workspace --exclude splitty-gui`; `cargo clippy -p splitty-gui --all-targets -- -D warnings` and `cargo test -p splitty-gui --lib` (the Tauri crate's comments changed); `pnpm check`, `pnpm test` and `pnpm build` in `frontend/`; `openspec validate --specs`.
