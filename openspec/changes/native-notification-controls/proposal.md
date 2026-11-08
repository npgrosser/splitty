# Proposal

## Why

The native workspace now shows unread activity, but its Settings editor cannot change notification enablement or event categories. Extending the editor should reuse service-owned settings and the bounded local mutation path while keeping notification reads independent from appearance and theme resolution.

## What Changes

- Offer native controls for `notifications.enabled`, `processFinished`, `processExited` and `processCrashed`, with current effective values and service-provided reset defaults.
- Retain fixed-local ownership when a remote Screen is selected, submit only the selected field, and reconcile confirmed, rejected, uncertain and superseded outcomes without replaying a write.
- Keep notification settings and mutation completion independent of appearance/theme failures, using the existing local auxiliary stream and mutation queue.
- Move shared editor contracts out of the appearance-specific module so settings domains can share bounded mutation machinery without taking ownership of each other's projections.

This advances [Milestone 120](../../../roadmap/README.md) and complements `native-notification-badges`. Out of scope: toast or OS notification delivery or controls, remote settings editing, new settings storage or protocol operations, terminal font preferences, broad settings parity, and changes to the web editor or notification policy.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `settings`: Extend the experimental native local editor and mutation reconciliation to independently observed notification switches and their reset defaults.

## Impact

The shared Rust client settings contracts, local settings scheduler and native settings presentation change. The existing service `home.get`, `home.metadata`, `home.update` and `settings.changed` contracts remain unchanged. The architecture model records the shared settings contract boundary and the independent notification projection; the server and web implementation remain intact.
