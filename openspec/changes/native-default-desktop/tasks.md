## 1. Windows startup

- [x] 1.1 `WslLaunch` takes an optional configuration directory; without one the backend keeps the distribution's default (unit, `owned_service.rs`).
- [x] 1.2 Startup without arguments on Windows: distribution choice, support checks, backend installation from the archive beside the executable, visible failure dialog (unit tests for list parsing, choice and command construction; the installation itself is checked in the installed app).
- [x] 1.3 The Windows executable carries the application icon and version resource.

## 2. Packages

- [x] 2.1 Windows: NSIS installer of the native app built by `packaging/windows` locally and in CI; replaces the earlier web app in place.
- [ ] 2.2 Linux: deb, rpm and tar.gz of the native app. macOS: dmg of the native app for Apple Silicon.
- [ ] 2.3 Legacy web shells keep building and are released under a "Web" name.

## 3. CI, release and docs

- [ ] 3.1 CI lints, tests and builds the native app as the default desktop app on the three platforms; the release workflow publishes the native packages and the legacy ones.
- [ ] 3.2 README, `docs/development.md`, the release skill's platform reference, `docs/native-ui-handoff.md` and the roadmap describe the native app as the default and the web shells as legacy.
- [ ] 3.3 Before merge run the full gate once.

## Verification record
- Windows startup: unit tests for the list, the remembered distribution, the checksum file, the inspection fields and the support rules; the two shell scripts run under `sh` with a temporary home (install, find by version and checksum, replace, nothing left after a broken archive). 133 native tests pass in an optimized MSVC build.
- In a real window on Windows: the executable with the archive in `resources\` beside it, started without arguments, installed the backend under the per-version path, started it with the distribution's default configuration and the SSH dialog program, and a normal close left no backend process.
- `native/bundle.ps1` built `Splitty_<version>_x64-setup.exe` with the NSIS of the Tauri CLI's cache. Not yet seen: the installer run over an installed web app.
