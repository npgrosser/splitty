# Proposal

## Why

The experimental native workspace can browse and open existing remote Screens, but it cannot enter an SSH target or manage the displayed host names. Complete these ordinary host workflows using the existing local hub authority before adding the larger Ports surface. This advances [native workspace milestone 120](../../../roadmap/README.md#als-nächstes).

## What Changes

- Add an explicit Connect flow for a known or entered SSH target, with an optional display name, bounded validation and visible preparation/connection outcomes.
- Add display-name editing/clearing and Remove from list through the existing local host preference APIs. Removing metadata preserves open work and independent forwards.
- Extend the existing owned hub API stream with typed, identified mutations and authoritative refresh, without replay after uncertain completion.
- Withdraw pending connection continuations when leaving Hosts, so delayed metadata completion cannot initiate unwanted SSH work.
- Retain visibility of an explicitly requested connection even when its target is not yet in the saved directory, distinguishing current work from saved metadata.

Out of scope: an offline saved-target format, editing SSH configuration or credentials, host updates, new remote Screen creation, persistent open references, host reordering, port/reverse-forward controls, and release cutover. The existing remote runtime remains the connection/session owner. This increment does not close the broader remote acceptance matrix.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `remote-screens`: native target entry, metadata edits/removal, local-owner mutation outcomes and cancellation of pending connection demand.

## Impact

Extend `splitty-client-core` hub scheduling and native workspace commands, and add retained GPUI host forms and row actions. Reuse `hostPreferences.update`, `host.forget`, `remote.directory.get`, `hub.changed` and the existing host transport. No new wire operation, storage format, dependency or transport implementation is required. Reconcile the active `native-remote-screens` draft's earlier host-editing exclusion with this follow-up.
