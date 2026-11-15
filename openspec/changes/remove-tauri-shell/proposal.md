# Proposal

## Why

The native app (`crates/splitty-gpui` on `crates/splitty-client-core`) has been the only released desktop app since v0.3.0. The Tauri shell around the web frontend (`crates/splitty-tauri`, package `splitty-gui`) is no longer released, but it still carries about 10,000 lines of Rust and JavaScript, its own CI jobs on three platforms, a Windows installer job, smoke harnesses, build targets and a large part of the security specs (packaged documents, remote windows, document proofs). Every change pays for it in checks (`--exclude splitty-gui`, the extra clippy and test runs) and in specs that describe a product nobody ships. It is the first of three removals of web parts: this change removes the shell, `remove-browser-access` then removes the web client, the browser gateway and the commands around it, and a later change removes protocol and server surface that only the web client used.

Roadmap: item 120 of `roadmap/README.md` ([`native-ui-gpui`](../native-ui-gpui/proposal.md), part of its step 5, the cutover). There is no `roadmap/NNN-*.md` milestone file for it.

## What Changes

- **BREAKING** Remove the crate `crates/splitty-tauri` (package `splitty-gui`) with its workspace membership, lockfile entries, Tauri configuration, permissions, build script, resources directory and the Tauri-only icon sizes.
- **BREAKING** Remove the separate per-host remote windows, the Tauri commands (`remote_hosts`, `remote_open`, `remote_cancel`, `open_remote_editor`, `open_local_editor`, `reveal_download`, the `wsl_*` and `native_*` commands) and the event `remote-progress`. The native app shows a host's screens in its own window; there is no replacement window.
- **BREAKING** Remove the SSH form of the internal `desktop-backend` command (`--backend ssh` with `--target`, `--remember-target`, `--update`) and the server and remote helpers only it used. `desktop-backend --transport native-v1 --backend embedded` (with and without `--owner`) stays: the native Windows app starts its WSL backend with `--owner`.
- Remove the Tauri jobs from CI (`tauri-linux`, the Tauri part of the macOS job), the Tauri installer job and its `web` input from the Windows installer workflow (renamed to `windows-installer.yml`, still called by `release.yml`), the `frontend-dist` artifact only that job used, the Tauri smoke harnesses in `scripts/testing/`, the legacy macOS packaging script, the Tauri path of `packaging/windows/build.ps1` and `scripts/windows-installer.sh`, and the Make targets `dev-tauri`, `install-tauri`, `uninstall-tauri`, `dmg`, `build/splitty-gui` and `build/Splitty.app`.
- Move what the native packages still take from the Tauri crate: the application icons (Linux packages, the Windows installer and the native executable's Windows resource) and the place where the local Windows build puts the WSL backend archive.
- Drop the checks that name the crate: `--exclude splitty-gui` and the Tauri check lines in `CLAUDE.md`, `openspec/config.yaml`, CI and the docs. The full check becomes `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`.
- Update specs, the architecture model, docs, the roadmap index, open decisions and draft changes that describe the shell as existing.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `desktop-remote`: the four remote-window requirements are removed; the remote and local folder editor handoffs lose their Tauri commands and describe the native app's in-process dispatch.
- `access-control`: Remote Window Isolation, Desktop Shell Command Scope, Showing A Saved Download and Packaged Desktop Content Policy are removed with explicit reasons; Explicit Remote Editor Dispatch keeps its validation rules without the shell's command grant.
- `server-modes`: Desktop Stdio Lifetime Binding loses the SSH backend form; the macOS/Linux and Windows hosting requirements describe the native app only; Desktop App Command Search Path is removed (only the shell met it).
- `client-connections`: Native Document Lifetime is removed; Bounded Delivery, Resource Admission and Feature Parity Without Hidden Listeners lose their WebView, invoke-body and popout clauses.
- `terminal-sessions`: Saving An Offered File loses the desktop shell's "show in folder" action.
- `remote-hosts`: Payload Sources names the released desktop packages instead of the Tauri apps.

## Impact

- Code: `crates/splitty-tauri/` (deleted), `Cargo.toml`, `Cargo.lock`, `crates/splitty-server/src/main.rs` and `src/native/helper.rs` (SSH helper), `crates/splitty-remote/src/tunnel/mod.rs` (helpers left without caller), `crates/splitty-gpui/build.rs` (icon path), comments in `splitty-core`, `splitty-server`, `splitty-transport` and `splitty-client-core` that name the shell or its remote windows; tests in `crates/splitty-server/tests/daemon_lifecycle.rs` and `src/main.rs`.
- CI and release: `.github/workflows/ci.yml`, `windows-tauri.yml` (renamed), `release.yml`, `server-payloads.yml`, `deny.toml` (comment), `.agents/skills/release/`.
- Build and packaging: `Makefile`, `packaging/macos/build.sh` (deleted), `packaging/linux/` (Tauri desktop entry, icon and `nostrip/`), `packaging/windows/`, `scripts/` (release, icons, Windows installer, Tauri smoke harnesses), `.gitignore`.
- Architecture model: `architecture/desktop.c4` (deleted), `landscape.c4`, `views.c4`, `README.md`.
- Docs and planning: `docs/architecture.md`, `docs/native-ui-handoff.md`, `docs/development.md`, `docs/networking.md`, `docs/remote.md`, `docs/cli.md`, `docs/gpui-spike.md`, `docs/protocol.ts` (comments), `SECURITY.md`, `docs/security/findings.md`, `docs/issues/openspec-baseline-findings.md`, `roadmap/README.md`, `openspec/open-decisions.md`, `CLAUDE.md`, `openspec/config.yaml` and nine draft changes.
- Users: Windows machines without WSL (remote-only use over `ssh.exe`) have no desktop app until the draft `native-windows-without-wsl` is implemented, also not from source (RTS-1). Intel Macs keep the released native `x64` disk image.

## Out of scope

- `frontend/` is not touched. It keeps building and testing; its Tauri platform code (`tauri_channel.ts`, `tauri_invoke.ts`, the shell bridge calls) and the `@tauri-apps/api` and `@tauri-apps/cli` dependencies become dead code and go with the whole directory in `remove-browser-access`.
- The browser gateway, `splitty serve` (including its hidden refusing flags `--desktop-stdio` and `--remote`), `splitty connect`, `issue-bootstrap`, the process token and `packaging/linux/splitty-serve.service`: `remove-browser-access`.
- Protocol messages and server routes that only the web client or the shell used (for example the `Role::Remote` relay role, which the host relays also use, stays): the later protocol follow-up.
- `desktop-backend --backend embedded` without `--owner`, which no released app starts any more: kept here, a candidate for the protocol follow-up (RTS-4).
- Moving the login-shell `PATH` adoption into the native app (RTS-3): a change of its own.
- A desktop app for Windows without WSL: `native-windows-without-wsl` (draft).
- Historical milestone documents in `roadmap/` (for example `roadmap/04-rust-backend.md`) stay as written; only the index `roadmap/README.md` is updated.
