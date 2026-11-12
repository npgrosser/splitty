## 1. Windows startup

- [x] 1.1 `WslLaunch` takes an optional configuration directory; without one the backend keeps the distribution's default (unit, `owned_service.rs`).
- [x] 1.2 Startup without arguments on Windows: distribution choice, support checks, backend installation from the archive beside the executable, visible failure dialog (unit tests for list parsing, choice and command construction; the installation itself is checked in the installed app).
- [x] 1.3 The Windows executable carries the application icon and version resource.
- [x] 1.4 With several installed distributions the first start asks and remembers the answer, and the settings show a WSL section that checks and remembers another one for the next start; tool distributions are left out (unit: candidate list, state file, section visibility and note).

## 2. Packages

- [x] 2.1 Windows: NSIS installer of the native app built by `packaging/windows` locally and in CI; replaces the earlier web app in place.
- [x] 2.2 Linux: deb, rpm and tar.gz of the native app. macOS: dmg of the native app for Apple Silicon.
- [x] 2.3 Legacy web shells keep building in CI and are not released (v0.3.0 was the last release with their packages).

## 3. CI, release and docs

- [x] 3.1 CI lints, tests and builds the native app as the default desktop app on the three platforms; the release workflow publishes the native packages and the legacy ones.
- [x] 3.2 README, `docs/development.md`, the release skill's platform reference, `docs/native-ui-handoff.md` and the roadmap describe the native app as the default and the web shells as legacy.
- [x] 3.3 Before merge run the full gate once.

## Verification record
- Windows startup: unit tests for the list, the remembered distribution, the checksum file, the inspection fields and the support rules; the two shell scripts run under `sh` with a temporary home (install, find by version and checksum, replace, nothing left after a broken archive). 133 native tests pass in an optimized MSVC build.
- In a real window on Windows: the executable with the archive in `resources\` beside it, started without arguments, installed the backend under the per-version path, started it with the distribution's default configuration and the SSH dialog program, and a normal close left no backend process.
- `native/bundle.ps1` built `Splitty_<version>_x64-setup.exe` with the NSIS of the Tauri CLI's cache. Not yet seen: the installer run over an installed web app.
- Linux packages: `packaging/linux/native/build.sh` run against a release build of `splitty-gpui` and `splitty`; `dpkg-deb -I/-c` and `rpm -qip/-qlp/-qRp` show the expected control data, files and requirements, with the rpmbuild of Ubuntu 22.04 (4.17) as well as a current one; `splitty-gpui --help` and `splitty --version` run from the unpacked tar.gz. The legacy Electron deb built with the new naming installs `splitty-web.desktop` and shares no path with the native deb. Not installed on a system here: CI installs the deb on Ubuntu 22.04.
- Workflows: `actionlint` (with shellcheck) passes on every workflow; the release's collect and publish steps were run against placeholder files named for `0.3.0-rc.1` and refuse a missing package or a duplicate name. `scripts/release.sh --check` still passes with the renamed legacy products.
- Only a CI or release run shows: the macOS script (bundle layout, ad-hoc signature and its verification, `hdiutil`), the native build and the terminal library on Ubuntu 22.04, the Windows jobs (native build without the remote payload variables, NSIS on the runner, `bundle.ps1`), and the Tauri installer under the product name "Splitty Web".
- Distribution choice: unit tests for the candidate list without tool distributions, the state file and the settings section; the optimized MSVC build and its tests pass. Not seen in a real window: the dialog on a first start and the WSL settings section, since the test system has one user distribution.
