# Proposal

## Why

The native workspace can open and operate existing remote Screens but cannot create a Screen in an already opened host group. The shared mux already owns creation and default naming; extending the UI-free client collection closes this gap without introducing another transport or workspace authority.

## What Changes

- Offer explicit New Screen for an already opened host group whose current connection and coherent catalog are ready. Capture the exact local owner, remote source/attempt and execution connection; selection and aliases cannot retarget the request.
- Admit at most one unresolved creation per host, reserve aggregate Screen/pane capacity before sending, and send the existing `create_screen` once with fresh Screen/Pane IDs and no local launch settings.
- Keep the newly requested reference separate from canonical Screen data. Confirm its effective state through the existing ordered catalog/snapshot collector, with bounded uncertainty and no mutation replay after disconnect.
- Follow a confirmed Screen only while the original navigation intent remains current. Detach and normal window close preserve host sessions, including a Screen whose creation outcome was not observed.
- Use the host's existing atomic default-name assignment and adopt its canonical name; no client-side copy of the naming policy.

This advances [Milestone 120](../../../roadmap/README.md), building on `native-remote-screens` and `native-drawer-host-ports`. Their unfinished acceptance obligations remain independent.

**Out of scope:** creating on disconnected, unopened or newly entered hosts; automatic Connect/Retry; general host-picker/reopen-offer UI; remote empty/missing Screen recreation; remote profiles/templates/transfers; remembered references; host-group persistence/reorder; a new mux operation/capability/receipt; Web client changes; packaging or platform provisioning. Existing Web creation, reconnect, reopen-offer and naming guarantees remain intact.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `remote-screens`: add a bounded experimental native creation workflow for an already opened ready group and an explicitly native default-naming rule, preserving other clients' guarantees.

## Impact

The implementation extends `splitty-client-core` connection serialization, the existing source/window collection and native worker. GPUI submits a captured typed intent and projects the shared pending/outcome state. The server remains authoritative for layout, session lifecycle, launch defaults and names; no new service, protocol or dependency is required. Correct the touched `CreateScreenOp` documentation in `docs/protocol.ts` to its existing Rust shape. Update the client-core/GPUI architecture descriptions and the native Remote Screens lifecycle documentation.
