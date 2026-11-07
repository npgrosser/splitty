## Why

The native workspace currently owns one local Screen. Multiple local Screens need one authoritative ordered catalog, while keeping terminal entities and sessions alive when the window shows a different Screen. This advances [native UI milestone 120](../../../roadmap/README.md) and the [parent native UI change](../native-ui-gpui/proposal.md).

## What Changes

- Create, switch, rename and close local Screens in the experimental native window; closing the last Screen leaves an explicit New screen state.
- Add a capability-gated, ordered workspace catalog request and response to the existing mux session stream. List membership, names, order and the initial active-screen hint come from one service snapshot.
- Separate connection-wide workspace coordination from each Screen's canonical snapshot and local tab/focus state. Preserve existing terminal entities, attachments and output across switches.
- Apply attachment and pending-pane admission limits across all Screens. Withdraw inactive Screen visibility and size claims without detaching its terminals.

Out of scope: persistence, remote Screens, reorder, cross-Screen drag, group drag, popouts, scratchpads, viewers, released-client cutover, and additional platform enablement. This increment uses focused native MVP validation; broad release and CI checks remain part of the later cutover.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `mux-protocol`: an ordered workspace catalog refresh, additive capability negotiation, and refresh support.
- `client-connections`: multiple local Screens on one native connection, local selection, global admission and retained terminal state.

Existing workspace lifecycle operations and terminal hide/resize semantics remain the underlying contracts.

## Impact

Touches shared protocol types and service session handling, the UI-free client connection/workspace model, and the native workspace worker/view. Updates `docs/protocol.ts`, native workspace architecture documentation and component descriptions. No worker per Screen, layout model or transport is introduced. The existing `anyhow` dependency is also declared by client-core when the native worker moves there.
