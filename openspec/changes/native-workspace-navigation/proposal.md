# Proposal

## Why

The native workspace already supports real local Screens, split groups and tabs, but common navigation still requires pointer use and a terminal cannot temporarily occupy the workspace. Extend the local desktop portion of [milestone 120](../../../roadmap/README.md) using the web client's existing maximize and Screen shortcuts as the reference.

## What Changes

- Maximize and restore the focused canonical tab group through a visible control and the existing Mod+M convention (Cmd on macOS, Alt elsewhere).
- Keep maximize state local to this window and Screen. Hidden terminals continue consuming output; visibility and size claims follow the displayed group without changing session ownership or the authoritative layout.
- Preserve tab selection and group identity during maximize. Reconcile removal or movement safely, and restore the full layout when a split or pane move needs to reveal another group.
- Add Ctrl+Shift+Space for a new local Screen and Mod+Shift+1–9 for direct Screen selection, retaining terminal/editor key routing and handling an empty workspace. On Linux, shifted-symbol selection is limited to verified keyboard layouts because the pinned framework does not expose physical key codes; unsupported symbols pass through.

Out of scope: floating windows, popouts, persistent maximize state, remote Screens, settings controls, new layout protocol operations, a second docking model, browser changes, platform release parity and broad CI.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `client-connections`: native local viewport maximization preserves layout and session identity, with visibility scoped to the displayed group. Screen navigation reuses the existing catalog and local selection contract.

## Impact

Touches the existing client workspace model, native workspace worker, GPUI workspace and focused native interaction driver. The service remains the layout and PTY owner. No new dependency, wire message or stored setting is needed. Update the existing architecture descriptions and native usage/evidence documentation alongside targeted unit, in-process and real-window checks.
