# Local Windows installer build

The installer contains two different binaries, so a complete build needs both
toolchains: WSL builds the static Linux backend and Windows builds the
application plus the NSIS installer. The local build script orchestrates both
and keeps their incremental caches.

`build.ps1` builds the native app by default: `splitty-gpui.exe`, packed by
`native/bundle.ps1` with `native/installer.nsi` as `Splitty.exe` with the
backend archive in `resources\`. `-App web` builds the legacy Tauri/WebView2
app with the Tauri bundler instead; it installs as "Splitty Web".

## Prerequisites

- Windows 10/11 x64 with WSL 2 and Ubuntu or Debian
- Visual Studio 2022 Build Tools with **Desktop development with C++** and a
  Windows 10/11 SDK
- Rust stable (MSVC), Git for Windows, Node.js 24, and pnpm 11 on Windows
- For the native app: NSIS 3 (`makensis.exe` on `PATH`, in its default install
  directory or in the Tauri CLI's cache) and the pinned libghostty-vt, either
  as `GHOSTTY_VT_DIR` or built by `scripts\bootstrap-libghostty-vt.py`
  (Python 3)
- Rust stable, `musl-tools`, and `file` inside the selected WSL distribution

Install the Linux packages in Ubuntu or Debian once:

```bash
sudo apt update
sudo apt install musl-tools file
```

Keep the checkout on a Windows drive such as `C:\Code\splitty`. Cargo's WSL
target cache is automatically stored in the Linux filesystem, while MSVC and
NSIS work from the native Windows path. A checkout below `\\wsl$` is not
supported by the Windows build tools.

The build includes servers for both x86_64 and aarch64 Linux hosts. Remote
windows can install them over Windows SSH without WSL at runtime; the WSL
backend also carries the aarch64 server for remote screens. These are the same
payloads included by the release workflow.

## Build from Windows

Run this from PowerShell in the repository:

```powershell
powershell.exe -ExecutionPolicy Bypass -File .\packaging\windows\build.ps1 -Distro Ubuntu
```

Omit `-Distro Ubuntu` to use the default WSL distribution. During quick local
iteration, `-SkipTests` skips the frontend check and desktop unit tests:

```powershell
powershell.exe -ExecutionPolicy Bypass -File .\packaging\windows\build.ps1 -SkipTests
```

The finished installer is written to `bundle\native\` (native app) or
`bundle\nsis\` (`-App web`) below:

```text
target\x86_64-pc-windows-msvc\release\
```

The native installer keeps the install directory, Start menu shortcut and
uninstall entry of the earlier web app and removes that app's executable, so
it updates an existing installation in place. The app installs the backend
archive into the WSL distribution on its first start.

## Start the same build from WSL

When the checkout is below `/mnt/c` (or another mounted Windows drive), run:

```bash
./packaging/windows/build-from-wsl.sh
```

This launches the native Windows PowerShell stage and automatically selects the
current WSL distribution. Building the complete MSVC/NSIS installer solely
inside the WSL Linux environment is intentionally not supported; only the
payload can be built there directly with `build-wsl-payload.sh`.
