# Proposal

> Draft captured on 2026-10-12 from `remote-install` D7 and `remote-supervisor`. Not planned yet. Specs and tasks are written when this change is next. Needs a way to run the Windows app's tests on Windows (CI or a Windows machine). Related: `remote-window` (the Windows app links `splitty-remote` since then).

## Why

The Windows app installs its WSL backend with code of its own (`crates/splitty-tauri/src/windows_wsl.rs`): it unpacks a bundled archive into the distribution, verifies its checksum and starts it. `splitty-remote` does the same for SSH hosts: a probe, an upload over stdin, a checksum check on the target, versioned directories, cleanup of unused versions. Two installers drift, and the WSL one has no version directories and no cleanup.

## What Changes

- The WSL backend is installed by `splitty-remote`'s installer, run over `wsl.exe -d <distribution> -- sh -c …` instead of `ssh`, into the same `~/.local/share/splitty/remote/<version>/` layout.
- The bundled WSL archive is replaced by the x86_64 payload built into the app (`remote-window` D8), so the installer has one payload source.
- `windows_wsl.rs` keeps the distribution handling and the lifecycle binding.

## Capabilities

### Modified Capabilities
- `server-modes`: Desktop App Hosting On Windows (where the backend is installed).
- `remote-hosts`: the installer also runs over WSL.

## Impact

- `crates/splitty-tauri/src/windows_wsl.rs`, `crates/splitty-remote` (an executor for `wsl.exe`), `tauri.windows.conf.json` (no bundled resource), `windows-tauri.yml`.
- Can be type-checked on Linux (`x86_64-pc-windows-msvc`, stub `llvm-rc`), but needs Windows to test.

## Architecture impact

`architecture/desktop.c4`: `windows_wsl` uses `splitty-remote`'s `install` and `ssh`.
