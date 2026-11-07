# Proposal

## Why

The native workspace cannot reorder tabs inside a group, and overflowing tabs are clipped instead of remaining reachable. Completing tab management advances the local native workspace in [roadmap item 120](../../../roadmap/README.md) while keeping layout authority in the shared service.

## What Changes

- Add a precise, capability-gated `move_tab` operation for placement before another tab or at the end, retaining existing `move_pane` behavior.
- Preserve canonical group IDs, sessions and selection for same-group reorder; place cross-group tabs at an exact position while preserving surviving source proportions.
- Add native tab-strip insertion targets, overflow scrolling and focused-group previous/next navigation.
- Validate captured gestures, settle admitted moves from ordered authoritative state and preserve editor/modal input priority.
- Leave floating/popout groups, cross-Screen moves, tab hiding settings, browser Dockview migration, arbitrary shortcut configuration and platform release certification outside this increment.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workspace-model`: exact tab placement and native admission/settlement semantics.
- `mux-protocol`: advertise and gate `tab-placement-v1` without changing existing operation meanings.

## Impact

The existing mux operations, validation and reducer gain a pure single-pane operation; native client models/runtime and GPUI translate gestures into it. Update the existing protocol documentation and frontend operation type union so both clients describe the shared wire contract. The browser keeps its working arrangement path. No new dependency or native emulator change is needed. Targeted tests and real-window checks cover this native increment; broad CI and release/platform certification remain later work.
