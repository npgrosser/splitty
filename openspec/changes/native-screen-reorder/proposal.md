# Proposal

## Why

The native Drawer cannot rearrange Screens within a host group. Reusing the web client's full-list `reorder_screens` request would let a stale native gesture remove a concurrently created Screen from the server's ordered catalog, so native parity needs a bounded relative placement intent.

## What Changes

- Add capability-gated `move_screen` to place one existing ordered Screen before another or at the current end, atomically against current server state. Preserve legacy `reorder_screens`, including partial orders.
- Let native clients rearrange local Screens through that shared operation and observe the authoritative order through the existing ordered workspace catalog. Preserve selected identity, terminal state and sessions; do not replay failed or uncertain placement.
- Rearrange remote Screens only as this window's opened references within their existing host group. Preserve the arrangement through host catalog refresh and reconnect without sending remote order operations or connecting an offline host.
- Add native Drawer drag interaction over captured Screen identities, with cancellation, same-group boundaries, and a keyboard-accessible equivalent through the existing Screen menu.

Out of scope: changing legacy/web replacement-order semantics, moving Screens across hosts, rearranging host groups, adding native restart persistence, changing remote global order, new installers or transports, and Screen slide/motion implementation. The active `native-remote-screens` and `native-workspace-motion` acceptance obligations remain separate.

## Capabilities

### New Capabilities

None; extend the existing workspace and connection contracts.

### Modified Capabilities

- `workspace-model`: atomic relative Screen placement and native local admission/reconciliation without selection or session changes.
- `mux-protocol`: additive `screen-placement-v1` capability, existing ordered delivery, rejection and workspace-change propagation for `move_screen`.
- `remote-screens`: window-local native ordering of opened remote references within their host group.

## Impact

Shared mux operation/reducer and both existing server mux entry points; the application protocol capability and `docs/protocol.ts`; UI-free client connection, source/window workspace and native worker; GPUI Drawer interaction and existing tests. No new crate, transport or dependency is expected. Update the affected architecture component descriptions and the workspace/client connection concepts together.

This is a bounded native-parity increment of [native-ui-gpui](../native-ui-gpui/proposal.md) and the native evaluation direction in the [roadmap](../../../roadmap/README.md); it does not complete a separate numbered milestone or authorize cutover from the web client.
