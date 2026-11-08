# Proposal

## Why

The native Drawer, workspace and split-drop feedback change position abruptly even though the established web workspace moves these surfaces together. Native presentation should communicate navigation and pinning without changing terminal or service ownership.

## What Changes

- Animate the unpinned Drawer overlay and the pinned workspace reservation with coordinated 220 ms motion. The unpinned overlay does not reserve workspace width; pinning and its rollback preserve the existing identified settings outcome.
- Animate Screen and terminal hover colors over 80 ms, and animate split-drop highlight bounds for the corresponding 70 ms within-group or 300 ms viewport-anchor treatment. Use the actual native drop target geometry.
- Add a subsequent vertical Screen transition matching the 200 ms web direction by Drawer order. The current Screen remains the only interactive and visible terminal route; at most one outgoing Screen is presentation-only. A shared terminal painter feasibility gate precedes this task.
- Honor reduced motion and make interrupted or reversed transitions continue from their currently displayed position.

Out of scope: Screen reorder, pane disclosure animation, new layout operations, protocol changes, settings writes beyond existing explicit pin actions, and a new general animation framework.

## Capabilities

### New Capabilities

None. This changes native presentation only.

### Modified Capabilities

None. Existing workspace, terminal visibility and Drawer settings contracts remain in force; `.openspec.yaml` sets `skip_specs: true`.

## Impact

The GPUI workspace Drawer and split-drop rendering, terminal presentation seam for the later Screen slide, existing native UI tests, and native workspace documentation. GPUI Kit's existing transition API supplies timing, reversal and reduced-motion behavior; no new dependency or service API is needed.

## Architecture impact

Update the existing `gpui_workspace` and `terminal` descriptions in `architecture/gpui.c4` and the native presentation section of `docs/architecture.md` when the Screen slide lands; no component or dependency relationship changes. The server remains the only layout authority and the selected route remains the only active terminal route.

## Decisions

- The pinned workspace inset and unpinned Drawer overlay use separate transition channels because an unpinned reveal must not move adjacent content. Their targets change together on an admitted pin action and its identified outcome.
- **NWM-1 (2026-11-08, provisional, tbd):** Screen slide retains at most one outgoing presentation and reuses the retained terminal painter through an explicit presentation-only mode. It must not use a second terminal renderer or platform screenshot capture; the pinned Linux GPUI backend has no runtime image-capture implementation.
- The native split-drop overlay uses its own measured rectangles. The web's 70 ms and 300 ms values refer to two different Dockview drop targets, so they are applied only to corresponding native targets.
