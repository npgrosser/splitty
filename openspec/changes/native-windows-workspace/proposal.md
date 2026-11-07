# Proposal

## Why

The experimental native workspace currently starts only on Unix; its Windows entry point reports an unwired WSL launcher. A native Windows window is needed to evaluate input and rendering independently of WSLg while retaining the existing WSL terminal backend and shared client architecture. This advances [native workspace evaluation](../../../roadmap/README.md#als-nächstes).

## What Changes

- Run the existing workspace, settings and template orchestration on Windows using an app-owned WSL helper with independent session and API streams.
- Share the existing typed API catalog and request transaction logic across embedded and owned connections without duplicating their validation, cancellation or accounting rules.
- Accept explicit development startup configuration for an existing WSL distribution and backend executable; provide a matching native executable and convenient local launcher.
- Report observed helper cleanup failures accurately and retain joined ownership of the backend.
- Build the pinned Ghostty library and optimized GPUI executable for Windows, then verify real terminal input, layouts, API operations, native graphics and shutdown.

Out of scope: WSL installation/provisioning or distribution switching UI, installers/releases/CI, native Windows shells, remote host UI, Drawer development, replacing the released clients and claims of full Windows feature or latency parity.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `client-connections`: explicit Windows development workspace startup using the existing owned-backend contract, shared session/API behavior and truthful cleanup evidence.

## Impact

Pure API catalog in splitty-core, reusable API request transactions in splitty-transport, server-client delegation, owned-connection API/cleanup in splitty-remote, portable native client service wrappers, GPUI startup arguments, architecture model, development documentation and local build/test tooling. Native wire shapes and released desktop launchers remain compatible. The Windows binary still uses WSL for its server, PTYs and shells.
