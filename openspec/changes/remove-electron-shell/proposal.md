# Proposal

## Why

The native app (`crates/splitty-gpui`) is the default desktop app on every platform and the only one releases publish (`native-default-desktop`). On Linux the Electron shell (`frontend-electron/`) was a second web shell next to the Tauri one; it is no longer released, yet it still costs a CI job, its own lockfile and dependency audits, Dependabot updates, a version file in every release, and Electron-only branches and conformance tests in the web frontend. Removing it leaves Linux with the native app, and the web frontend with one desktop shell (Tauri) besides the browser.

## What Changes

- Delete `frontend-electron/` (main process, preload, native pipe broker, packaged-asset protocol, smokes, lockfile) and `packaging/electron/`.
- CI and release: drop the `electron` job, the Electron type install before the frontend check (`ci.yml`, `server-payloads.yml`, `packaging/macos/build.sh`), the npm audit of the Electron shell and its Dependabot entry; `scripts/release.sh` no longer writes or checks `frontend-electron/package.json` and keeps checking `Cargo.toml`, `tauri.conf.json` and `Cargo.lock`.
- Makefile: drop `electron`, `install-electron`, `uninstall-electron` and `run-electron`.
- Web frontend: remove the Electron shell kind and every `__SPLITTY_ELECTRON__` branch (window controls, quit guard, drag regions, page zoom, platform detection, remote window bridge, editor dispatch, download reveal, packaged `splitty:` bootstrap) and its tests. The native transport codec and the `NativeAssignment` type, which the Tauri adapter shares, move from `frontend-electron/src/` into `frontend/src/lib/protocol/`.
- Rust: remove the opt-in Electron SSH smoke from `crates/splitty-server/tests/remote_connect.rs`; comments that named Electron describe the remaining clients.
- Specs: the Electron channel names leave the desktop shell API, editor dispatch and download reveal requirements; the remote payload requirement no longer names the Electron app.
- Docs, `SECURITY.md`, `CLAUDE.md`, the release skill and the architecture model describe the remaining shells.

Out of scope: the Tauri shell (it stays buildable with `make dev-tauri` and `make install-tauri` and is not released), the web frontend itself (served by `splitty serve` and `splitty connect`, and shown in Tauri), the `README.md`, and the Rust native transports Electron used together with others (`desktop-backend --transport native-v1 --owner`, used by the Windows WSL launchers; `splitty connect --json` and the hidden `splitty remote-hosts --json`, both specified in `cli`).

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `desktop-remote`: Remote Window Shell API, Remote Folder Editor Handoff and Local Folder Editor Handoff name only the Tauri commands and events.
- `access-control`: Explicit Remote Editor Dispatch loses its Electron child-frame clause (the Tauri proof is issued to the main frame only, and the no-frame content policy remains); Showing A Saved Download names only `reveal_download`.
- `remote-hosts`: Payload Sources no longer requires the released Electron app to run the static x86_64 server.

## Impact

- Removed: `frontend-electron/`, `packaging/electron/`, the CI job `electron`, Make targets, the Electron frontend adapter (`frontend/src/lib/protocol/electron_channel.ts`) and the frontend tests that imported Electron sources.
- Changed: `frontend/src/lib/{platform,window,zoom,file_offers,editor_requests,remote_editor,remoteWindows}.ts`, `frontend/src/App.svelte`, `frontend/src/lib/protocol/{bootstrap,native_socket,native_wire}.ts`, workflows, `scripts/release.sh`, `scripts/generate-icons.sh`, docs.
- No wire protocol, data format or CLI change. Existing Electron installs are not touched; development docs say how to remove one.

## Architecture impact

Removes the container `splitty.electron` and its relationships to `splitty.server` and `splitty.web` from `architecture/landscape.c4`; `docs/architecture.md` drops the Electron pipe adapter, listener inventory rows and the Electron variants of remote windows, downloads and editor handoff.

## Decisions

- **RES-1 (provisional, tbd):** `make uninstall-electron` goes with the other Electron targets; `docs/development.md` names the two files an earlier `make install-electron` left. Alternative: keep the uninstall target for a release or two.
- **RES-2 (provisional, tbd):** the hidden `splitty remote-hosts --json` and `splitty connect --json`, whose main consumer was the Electron shell, stay as specified in `cli` for scripts and launchers. Alternative: remove `remote-hosts` with a REMOVED delta in `cli`.
- **RES-3 (provisional, tbd):** provisional decisions in `openspec/open-decisions.md` that concerned only the Electron shell (`desktop-login-path` Tauri only, `remote-window` D10 and D16, the Linux deb/rpm packages of electron-builder, NDD-2) leave the file, since nothing is left to decide; points that mentioned Electron next to other shells keep their other content. Alternative: keep them, marked as moot.
- **RES-4 (provisional, tbd):** draft changes that mention Electron (`frontend-client-core`, `native-ui-gpui`, `remote-microphone`, `viewer-sandboxed-html`, `viewer-webview`) get a carried-over note instead of a rewrite; the completed but unarchived `native-client-transports` has the Electron channel names taken out of its delta specs, so that archiving it later does not bring them back. Alternative: rewrite the drafts now.
