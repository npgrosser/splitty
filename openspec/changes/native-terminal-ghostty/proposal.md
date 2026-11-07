# Proposal

## Why

The native development executable still uses the temporary Alacritty integration and repeatedly redraws and shapes terminal cells even when their contents have not changed. The parent [native UI migration](../native-ui-gpui/proposal.md) and [roadmap 120](../../../roadmap/README.md) select Ghostty as the first regular emulator; this increment implements that choice and removes the observed avoidable rendering work before expanding the workspace UI.

## What Changes

- Replace the development executable's terminal engine with pinned `libghostty-vt` 1.0.0, behind an owned, UI-independent Rust boundary with explicit handle and callback lifetimes.
- Preserve current terminal input, resize, scrollback, selection/copy, paste and replay behavior through the existing server-owned session path. Keep clipboard/platform effects under application policy.
- Separate engine state, native presentation and session wiring; cache terminal shaping and request rendering only when visible state changes. Retain a complete visible frame when GPUI requests a paint.
- Provide an explicit, reproducible native dependency setup and CI integration. Cargo does not silently download/run a toolchain.
- Record native smoke and optimized idle, input, output and scrolling measurements, including graphics backend and optional offscreen engine state.

Out of scope: service-driven splits/tabs, host migration, production cutover, removing the web clients, complete image/extended-input feature parity, synchronized-output support beyond the current probe and any runtime emulator/plugin selection. Existing experimental scope and platform limitations remain explicit.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None. This is an internal engine/presentation replacement and performance improvement. Existing `client-connections` Native Probe Data Paths, Isolated Native Probe Lifetime and Scripted Probe Isolation requirements remain unchanged. `skip_specs: true` avoids inventing library-specific behavior requirements.

## Impact

`splitty-gpui` gains an independent terminal engine module and native build integration; its terminal view and window/document invalidation change. Build setup, relevant CI jobs, the LikeC4 model and development/spike documentation are updated. Service, protocol, data formats and release selection remain unchanged.
