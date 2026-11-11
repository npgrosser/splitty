# Proposal

> Draft captured on 2026-11-12 from `native-default-desktop`. Not planned yet. Specs and tasks are written when this change is next. Related: `remote-window` (the web app's host windows over Windows SSH) and `wsl-install-shared`.

## Why

The native Windows app starts its backend in a WSL distribution and reaches every SSH host through that backend's hub. Without WSL it shows a failure dialog and exits. The legacy web app opens hosts over the Windows `ssh.exe` without WSL; the native app has no such path, so remote-only use on Windows needs the web app.

## What Changes

- Without a usable WSL distribution the native Windows app still opens a window and offers hosts: it connects with `splitty-remote` over the Windows `ssh.exe`, installs the server there from archives built into the executable, and shows remote Screens as it does today. Local terminals, templates and the local settings service are unavailable in that mode and are shown as such.
- The startup failure dialog offers this mode beside the hint to install WSL.

## Capabilities

### Modified Capabilities
- `client-connections`: Native Windows startup without a WSL distribution; a host stream that does not pass through an owned local backend.
- `remote-hosts`: the native app as a direct installer client on Windows.

## Impact

- `crates/splitty-client-core` (a host connection that is not opened through the owner helper; settings and catalogs that today come from the local service), `crates/splitty-gpui` (startup, a window without a local source), `packaging/windows` and the Windows CI job (the executable would carry the server archives, which it deliberately does not today).
- Open: where settings, the host directory and remembered Screens live without a local service (the Windows side, or the first connected host).

## Architecture impact

`architecture/client-core.c4` and `landscape.c4`: `splitty-client-core` uses `splitty-remote`'s tunnel and installer directly on Windows.
