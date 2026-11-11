# Proposal

## Why

The native client covers the daily workflows of the web client. Releases still install the web shells (Tauri on Windows and macOS, Electron on Linux), and the native Windows executable only starts from a launcher that names a distribution, a backend path and a configuration directory.

## What Changes

- The native client becomes the desktop app a release installs: a per-user NSIS installer on Windows, a dmg on macOS (Apple Silicon) and deb, rpm and tar.gz packages on Linux x86_64.
- On Windows the native app starts without arguments: it uses the remembered WSL distribution or the default one, refuses unsupported or root-default distributions with a visible message, installs the Linux backend that ships beside the executable into the distribution when its version or checksum differs, and starts it as its owned embedded backend with the distribution's own configuration directory. The explicit arguments remain for isolated previews and tests.
- The web shells stay in the repository, in CI and in the release as legacy artifacts under a "Web" name; nothing of the web client is removed.

## Capabilities

### New Capabilities

### Modified Capabilities
- `client-connections`: the native Windows workspace starts without arguments by choosing a distribution and installing its backend; explicit arguments stay an override.

## Impact

`crates/splitty-gpui` (Windows startup, executable icon), `crates/splitty-client-core` (`WslLaunch` with an optional configuration directory), `packaging/`, `scripts/windows-installer.sh`, `.github/workflows/`, the release skill's platform reference, README and docs.

## Architecture impact

Two relationships in `architecture/gpui.c4`: `gpui_main` to the client core's `owned_service` (the prepared WSL launch target) and `gpui_workspace` to `gpui_startup` (the WSL distribution for editor links). No new crate or component.

## Decisions

- The Windows installer of the native app keeps the product name, install directory and uninstall entry of the earlier web app and removes that app's executable, so an update replaces it in place. The legacy web app installs as "Splitty Web" beside it (provisional, tbd).
- The backend archive ships beside the executable and is installed to the same per-version path and checksum marker the web app uses, so both apps of one version share an installed backend. Moving the installation to `splitty-remote`'s installer stays with `wsl-install-shared`.
- Without a remembered distribution the app uses the WSL default distribution, or the first supported one, and does not ask; it reads the choice the web app stored. A chooser in the native app is left for later (provisional, tbd).
- Several native app instances may run at once, each with its own backend; there is no single-instance handoff (provisional, tbd).
- macOS ships the native app for Apple Silicon only; Intel Macs use the legacy universal web dmg. Linux has no native AppImage; the legacy Electron AppImage remains (provisional, tbd).
- The native Linux package is named `splitty`, installs to `/opt/splitty` and starts as `splitty-gpui`; the `splitty` command ships beside the executable for the app's terminals and is not put on the system `PATH`. The legacy Electron packages keep the package name `splitty-electron` and move their desktop entry to `splitty-web.desktop`, so both install side by side (NDD-1, NDD-2; provisional, tbd).
- Release files of the legacy apps are renamed when collected (`Splitty-Web_…`, `splitty-web…`); only the Tauri Windows product is renamed in its configuration. The legacy macOS app stays `Splitty.app` with its identifier; the native macOS app has the identifier `com.splitty.native` (NDD-3, NDD-4; provisional, tbd).
- The Linux and macOS packages ship the static server of the release as the `splitty` command, with `shell-integration/`, beside the executable (NDD-5). The deb names its dependencies by package, the rpm by library (NDD-6; provisional, tbd).
