# Proposal

## Why

The native workspace can create tabs but cannot select an existing shell profile. The web client already offers this choice, and the service owns profile discovery, availability, launch resolution and fallback; the native client should reuse those decisions.

## What Changes

- Read local shell profiles through the existing native API and offer an explicit profile choice for a new tab.
- Preserve service inheritance for ordinary tab and split actions; an explicit choice supplies only a profile identifier.
- Show service-reported fallback when a selected profile becomes unavailable before launch.
- Share the existing owned auxiliary API lifecycle between independent appearance and profile reads, retaining cancellation, allocation bounds and terminal event isolation.
- Keep failures visible and feature-local, without settings writes.

Out of scope: profile editing, remote profile selection, daemon adoption, workspace restoration, new-screen/split profile controls, Windows launcher work, web removal, WASM and a shared frontend rewrite. This is a bounded increment of the native evaluation; no numbered roadmap milestone defines it.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `settings`: experimental native local profile selection, refresh and failure isolation using the existing service-owned profile contract.

## Impact

Touches the UI-free native client, GPUI workspace controls, architecture map, native evaluation documentation and focused tests. Reuses `shellProfiles.get`, `new_tab.shellProfileId`, `settings.changed` and `mux.pane_spawned.shellProfileFallbackReason`; no new wire shape, dependency or persisted setting.
