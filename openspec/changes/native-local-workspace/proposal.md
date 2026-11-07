# Proposal

## Why

The first usable native milestone needs a local screen with real terminal panes, splits and tabs rather than the fixed terminal/document fixture. This implements the initial local desktop slice of [the native migration](../native-ui-gpui/proposal.md) and [roadmap 120](../../../roadmap/README.md), on the selected Ghostty terminal foundation.

## What Changes

- Make the development executable open a local terminal workspace by default, while keeping the existing Markdown/CEF probe explicitly available.
- Render the authoritative service screen tree directly with native split containers, tab bars and resize handles. Support horizontal/vertical splits, new tabs, tab selection, pane close and divider resizing through existing mux operations.
- Extend the UI-free client with typed screen operations, snapshot reconciliation and generation-scoped multi-session routing. Keep terminal entities stable across snapshot updates.
- Exercise the complete flow against the real embedded service and native window. Keep it explicitly experimental until the larger migration has platform and product parity.

Out of scope: drag-and-drop pane relocation, scratchpad, `splitty open`, viewer/editor features, hosts, full settings, persistence migration from production workspaces and release cutover. Initial windows continue to own isolated service/workspace state; native Windows launcher integration remains outside this bounded milestone.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `client-connections`: extend the experimental native client workflow to render a service-owned local terminal screen and retain session/view identity through split/tab/resize changes.

## Impact

`splitty-client-core` gains a typed workspace model and screen operation surface. `splitty-gpui` gains native workspace presentation and a worker using that surface, with terminal views keyed by pane identity. The shared mux tree/reducer and protocol stay authoritative and unchanged. Architecture model, developer documentation and main spec record are updated; released clients are unchanged.
