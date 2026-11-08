# Proposal

## Why

The experimental native workspace can inspect known hosts but cannot use their running Screens. After `native-source-routing` is complete, the next useful increment is to deliberately connect a known host, open an existing Screen beside local work, and safely detach or reconnect it through the existing service-owned transport.

This advances the Hosts phase of [native-ui-gpui](../native-ui-gpui/proposal.md) and the native client work listed as item 120 in [the roadmap](../../../roadmap/README.md), while preserving the web client and its released behavior.

## What Changes

- Add explicit Connect, Cancel/Retry and live catalog browsing to existing native Hosts entries. Passive inspection remains passive; neither a catalog nor a remote active-Screen hint opens a Screen.
- Open selected existing remote Screens as window-local references, grouped beside local Screens. Use the same canonical Screen projection, source-qualified view keys and terminal interaction as local work.
- Share actual opened-Screen collection, reconciliation and admission logic in Rust. Separate local catalog/bootstrap policy from remote explicit-reference policy; do not copy the local all-catalog/initial-Screen policy into a second workspace model.
- Support default remote tab/split, selection, terminal input/output, resize, search and assessed pane close. Local appearance still applies; remote launches inherit from the remote source pane without local shell-profile identifiers.
- Make remote Screen close detach by default. Provide a separate assessed, confirmed Terminate action. Normal owned-window shutdown assesses local destructive impact and detaches remote Screens without ending remote sessions.
- Add bounded reconnect after previous readiness, immutable source/attempt ownership, input revocation and fresh replay on replacement. Retain and join setup/read/close work and report structured failures without inventing reachability or cleanup success.

**Out of scope:** creating remote Screens (the narrow already-open ready-group action is covered by successor `native-remote-screen-create`) or recreating missing/empty ones; remembering/restoring open references across application restart; typed host entry, host editing/removal/reordering, daemon updates/migration, port or reverse-forward UI, remote profile pickers/templates, cross-source or remote cross-Screen transfers, viewer feature expansion, remote-only application startup, a new SSH stack/helper supervisor, a web rewrite, packaging/releases and Windows builds. Existing enabled-forward restoration on deliberate host connection remains owned by the hub. Remote loopback-link forwarding is unavailable in this increment and must not accidentally open a same-port local service.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `remote-screens`: extend the experimental native subset from passive inspection to explicit known-host browse/open, source-scoped interaction, detach/terminate and bounded recovery. Existing released-client creation, persistence, host-order and forwarding contracts remain unchanged.

## Impact

`splitty-client-core` gains the shared source/window collection and per-host lifecycle using existing `ScreenModel`, command tokens, native clients and owned-service adapters. `splitty-gpui` renders that model through its existing Drawer, Hosts and terminal seams. Small transport/client accessors retain typed preparation/closure diagnostics; they do not change the wire format. The embedded adapter calls `Connector::host_admitted`; the owned adapter calls `Owner::open_context(Some(target), false, ...)`.

Implementation also updates the relevant client-core, GPUI, transport/server-native architecture descriptions and `docs/architecture.md`. No new crate, independent mux tree, server policy engine or persistence format is needed. Use `spec-driven` because this adds user-visible behavior and spans model, runtime, transport diagnostics and native presentation.

**Dependency:** complete and integrate `native-source-routing` first, including its GPUI stale-delivery/callback tests. This proposal does not overlap that implementation.
