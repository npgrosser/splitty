# Proposal

## Why

The experimental native workspace receives notification events but does not turn them into unread state, so activity in another pane has no Drawer indicator. A bounded client policy can give the native window the same useful signal as the web client while preserving connection isolation and service-owned settings.

## What Changes

- Introduce a UI-free, window-local notification ledger in the Rust client core for existing process and generic notification events. It classifies, bounds, deduplicates, routes and records read state without retaining event transport buffers. Current-source fact admission remains separate from command-send readiness; badge association requires a proven session binding.
- Read the local service's effective notification enablement and per-type switches through the existing settings refresh path. Apply settings before admission and when authoritative settings change.
- Let the GPUI window supply its visible, focused, active-pane observation and render unread Screen counts and pane severity in the Drawer. Reading a pane clears its unread state locally.
- Preserve existing server events and the web client. This changes neither server notification generation nor workspace operations.

Out of scope: server storage or synchronization of read state, protocol changes, notification settings editing, toast or OS notifications, notification-center UI, terminal bell/OSC notification handling, removal of the TypeScript client, and any assumption that native replaces web.

This advances [Milestone 120, Native UI with GPUI](../../../roadmap/README.md) using the existing [Milestone 2 notification system](../../../roadmap/02-notifications.md). The roadmap is context; the event and settings contracts in the current code remain authoritative.

## Capabilities

### New Capabilities

- `client-notifications`: Window-local admission, unread lifecycle and source-scoped display of existing service notification facts.

### Modified Capabilities

None. Existing client-connection and access-control requirements already require source isolation, safe text and host attribution; this change adds a focused client behavior without changing those guarantees.

## Impact

The Rust client core gains notification policy and a typed view of effective notification settings. The GPUI workspace consumes existing source-scoped events and projects unread state in its Drawer. The server, wire format, browser client and desktop carriers retain their current contracts. The architecture model and architecture guide will record the new client-core component and its GPUI relationship.
