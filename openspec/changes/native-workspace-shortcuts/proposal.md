# Proposal

## Why

The experimental native local workspace has clickable split, tab and close controls, but its terminal takes key events before a workspace shortcut can handle them. Add the established desktop shortcuts so the local screen is practical to navigate from the keyboard during the [native UI migration](../native-ui-gpui/proposal.md) in [roadmap 120](../../../roadmap/README.md).

## What Changes

- Handle Cmd on macOS and Alt elsewhere: modifier+T creates a tab, modifier+D splits right, modifier+Shift+D splits below, and modifier+W closes the focused pane.
- Handle modifier+Shift+Left/Right/Up/Down by focusing the nearest visible split group in that direction, using the web client's directional rule. Consume recognized workspace shortcuts before terminal input handling so they never reach the PTY. Keep ordinary terminal keys, Ctrl+C and clipboard shortcuts intact.
- Use rendered slot bounds and the existing service-owned screen tree for focus navigation; no second layout model or new mux operation is introduced.

Out of scope: shortcut customization, keyboard resizing, screen switching, other web shortcuts and tab cycling. Bracket-based tab cycling is deferred because AltGr is required for brackets on some host layouts and would make the proposed Alt chord unreliable.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None. The existing experimental native local screen contract already covers split, tab and close operations and window-local focus. This change adds keyboard access to those operations without changing protocol or client API behavior; `skip_specs: true` keeps that contract unchanged.

## Impact

`splitty-gpui` handles workspace shortcuts before terminal key dispatch and tracks visible slot bounds for directional focus. The service, client core, wire format and web client are unchanged.

## Architecture impact

Runtime component boundaries remain the same. The navigation tests construct workspace identities through the public client API; `architecture/gpui.c4` records that `gpui_workspace -> client_root` dependency, since the model also covers test code.

## Decisions

- Match the web client's Cmd/Alt modifier and directional 90-degree nearest-group rule, so the two desktop clients navigate splits consistently.
- Defer tab cycling until a portable chord is chosen; the web client has no tab-cycling chord and bracket keys are layout dependent on AltGr hosts.
