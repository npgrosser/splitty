# Proposal

## Why

The experimental native workspace can display saved local appearance but has no usable settings editor. A first native Settings view should let people change the preferences that the current workspace actually honors, while keeping the local service as the single settings authority and preserving terminal usability during failures.

## What Changes

- Add a native Settings destination with focused Appearance, Terminal, and Workspace groups, current values, supported choices, reset actions, pending/error feedback, and keyboard access. It edits the local service even while a remote Screen is selected.
- Expose saved UI theme mode and choices, UI font size and family, display zoom and scope; a separate terminal palette choice using the same service theme catalog; and workspace Drawer visibility and width. The Terminal group also exposes the already effective close-on-exit policy.
- Apply the selected terminal palette to existing and future native terminals independently of the UI theme. `default` continues to inherit the active UI theme. Existing terminal sessions, history, and native font metrics survive palette changes.
- Project the service-resolved UI theme into GPUI Kit's semantic theme roles so native controls stay readable across supported light and dark themes. Kit's two built-in defaults provide fallback colors in this increment; selecting additional native Kit presets remains a separate design choice.
- Extend the owned local API stream for typed metadata/theme-list reads and bounded single-field settings updates. Use the existing `home.get`, `home.metadata`, `home.update`, `theme.get`, and `theme.list` operations and `settings.changed` reconciliation.

Out of scope: a full copy of the web settings panel, web-only renderer and xterm controls, shell-profile editing, remote-host settings editing, custom theme authoring, a new storage format, and release cutover. No new protocol operation is proposed.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `settings`: native editing, independent terminal palette behavior, and service-owned reconciliation for the supported subset.

## Impact

The change affects `splitty-client-core` local settings ownership, `splitty-gpui` Settings presentation and theme projection, and focused tests. The existing server config, theme resolver, native operation catalog, and persisted `config.toml` remain authoritative. The native UI milestone is [roadmap item 120](../../../roadmap/README.md); the broad `native-ui-gpui` draft remains the migration context.
