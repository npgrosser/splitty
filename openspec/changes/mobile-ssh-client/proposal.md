# Proposal

## Why

Splitty needs a usable smartphone client for checking running work, continuing an agent conversation and operating existing terminals away from a desktop. A native, remote-only app over SSH fits the private native daemon endpoint and avoids introducing a network-facing web gateway and a second login system.

## What Changes

- Add an Android app with bundled mobile UI, native SSH, saved host profiles and device-specific protected keys. Reuse the existing `splitty remote-bridge` protocol; the phone runs no local Splitty server or shells.
- Present a screen's terminals individually on phones, including tabs and floating panes, without rewriting its desktop layout. Navigation, scroll position and presentation settings belong to the device. Large-tablet dual-terminal presentation is follow-up work.
- Support direct terminal input with special keys and a separate multiline composer with explicit paste/submit actions. Handle software keyboards, composition, selection, clipboard and hardware keyboards.
- Add server-enforced per-session control handoff so desktop and phone do not compete over input or PTY size. Viewing does not take control; explicit takeover transfers it.
- **BREAKING for legacy terminal writers on upgraded servers:** session-protocol input and resize require current control ownership. Updated desktop/web clients participate too; older clients receive an actionable rejection rather than bypassing ownership. The mobile app refuses hosts lacking the required capability. Existing owner-authorized control APIs retain their explicit automation semantics.
- Restore views after suspension and network changes without replaying uncertain input or ending host processes. Start with a real-device prototype before completing the client.

Out of scope: iOS (deferred to a later change), a public web gateway or hosted relay, local mobile shells, general multi-user permissions, shared simultaneous typing, a general layout-concurrency redesign, a full SSH-config interpreter/agent forwarding/jump-host UI, automatic server installation or destructive upgrades from the phone, arbitrary file transfer, port-forward previews, push notifications, app-store publication, and tablet split editing. A compatible daemon is installed on the host beforehand; reachability through LAN, VPN or an accessible SSH endpoint is a prerequisite.

## Capabilities

### New Capabilities

- `mobile-client`: remote-only mobile connection lifecycle, device-local presentation, input composition and reconnect behavior.

### Modified Capabilities

- `access-control`: explicitly admit the native mobile SSH bridge path and define mobile key custody, host verification and native-command isolation without widening server listeners, origins or credential exposure.
- `terminal-sessions`: per-session control claim/release, input and size arbitration, observer-safe attach/replay and single-responder terminal queries.
- `mux-protocol`: advertise session-control support and report control release on disconnect while retaining connection-independent session lifetime and shared screen access.

## Impact

- Native mobile shell and SSH adapter; existing `splitty-remote` framing, validated commands and handshake; shared frontend connections/session logic and a mobile workspace renderer alongside Dockview.
- Core session dispatch and server connection lifecycle; updated desktop/web input, resize and attach behavior. Linux/macOS and Windows-with-WSL remain supported; mobile does not link a local PTY backend.
- Additive control messages/capability advertisement plus deliberate input/resize compatibility changes in `docs/protocol.ts`, Rust messages and frontend types. No daemon credential travels to the phone.
- Architecture model, architecture/security documentation, mobile build instructions and unit/in-process/device-smoke acceptance coverage.
- Related draft: [multi-attach](../multi-attach/proposal.md). This change owns the minimum session handoff needed for one user's devices; general ordered layout replication and collaborative roles stay with that draft. Reconcile overlapping deltas with the active [cli-control](../cli-control/proposal.md) change before implementation. No dedicated current mobile roadmap milestone exists.

## Carried-over design notes

- 2026-10-17, maintainer discussion: native mobile app, remote-only, direct SSH and reuse of the session protocol were accepted; the earlier PWA/gateway approach was superseded. The maintainer subsequently narrowed this change to Android first; iOS is deferred.
- 2026-10-17, `multi-attach` review: its claim that a second reader necessarily ends the first is stale against current continuous-output capture and `terminal-sessions` guarantees. Reuse that implementation; the unresolved mobile needs are control, size arbitration and independent presentation.
- 2026-10-17, `cli-control` review: its pending work touches session dispatch, lifecycle, wire types and architecture. Mobile observers are a coordination mode for the same owner, not agent grants or a security sandbox.
