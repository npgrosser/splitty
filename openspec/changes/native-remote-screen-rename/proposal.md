# Proposal

## Why

The native Drawer can rename local Screens but disables the same interaction for opened remote Screens. The host already implements `rename_screen`; extending captured native admission and the existing editor closes this everyday interaction gap without another naming or persistence owner.

## What Changes

- Rename an already opened, canonically observed remote Screen through its current owning connection, independently of the selected Screen.
- Capture the window owner, source connection and exact Screen identity when the editor is opened; revalidate before sending and discard obsolete work without connecting or replaying it.
- Reuse the existing inline editor and server operation. Preserve local rename behavior and use authoritative catalog observations for the resulting name.
- Keep an editor attached to its original source through unrelated selection/model updates; connection replacement, removal or shutdown cannot redirect its commit.

This advances [Milestone 120](../../../roadmap/README.md) and the native Drawer interaction parity work.

**Out of scope:** renaming unopened cached entries, missing/unconfirmed Screens or hosts; offline edits/replay; automatic Connect/Retry; new creation or persistence policy; changing the Web client; new wire operations, profiles, templates, server-side validation or protocol capabilities; broad editor/navigation refactoring.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `remote-screens`: captured native rename of one current opened remote Screen, preserving source ownership and canonical observation.

## Impact

Reuse `splitty-client-core` source rename validation, serializer and ordered observation, extending the existing remote command scope. Generalize the GPUI rename editor's captured identity/reconciliation and existing Drawer entrypoints. Update existing client-core/GPUI architecture descriptions and native lifecycle documentation. No server, transport, data-format or dependency change is required.
