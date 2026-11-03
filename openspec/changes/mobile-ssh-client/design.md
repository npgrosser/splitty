# Design

## Context

See [proposal.md](proposal.md) for motivation and scope. The maintainer selected a native remote-only mobile app over SSH on 2026-10-17 and subsequently limited this change to Android. iOS implementation, signing and device acceptance are deferred.

Observed implementation:
- `splitty-remote::ssh` launches a system OpenSSH process. `bridge` already separates text/binary framing from child-process plumbing; the framing is reusable, spawning a desktop `ssh` process is not a mobile transport.
- `splitty-server::host::bridge` verifies the host-local private daemon endpoint and relays native transport v1 over stdio. The phone needs no process token, browser cookie or exposed daemon port.
- `ServerConnection` consumes `ConnSocket`; session routing and attach buffering are already separate from transport. `Workspace.svelte`, `ScreenBinding`, pane mounting and commands still assume Dockview and a local server for some HTTP services.
- `TerminalSession.wire()` sends resize on emulator changes and a two-resize redraw nudge after attach. The dispatcher currently accepts input and resize from any connection. Output fan-out and continuous capture already exist.
- The desktop Cargo manifest gates server dependencies with `not(windows)`, which would include mobile targets. A mobile entry point must not accidentally compile/embed the local server.
- Baseline `mux-protocol` does not guarantee propagation of every layout-only edit. The mobile overview must refresh authoritative snapshots when opened/resumed; this change does not claim to finish the broader `multi-attach` redesign.

## Goals / Non-Goals

**Goals:** reuse the existing session protocol and frontend logic, keep SSH credentials in native code, make input and terminal dimensions predictable across one owner's devices, and validate the uncertain mobile interactions on hardware early.

**Non-Goals:** see proposal. In particular, an observer is not a less-privileged account. The authenticated owner can take control, close a session or use existing automation APIs. General collaborator authorization is a different capability.

## Decisions

### Accepted product direction

Bundle the UI in the app; run a native SSH exec channel without a remote PTY to `splitty remote-bridge`. Host output is terminal data, never app code. The phone depends on the `native-client-transports` format-2 discovery and native transport v1 contract (`splitty-transport`, `docs/native-transport-v1.json`), including service identity, deadlines, delivery credit and bounded buffers. It uses the direct remote bridge, which rejects local-hub host-open intent. Browser authentication and gateways are not part of this path. A public gateway/PWA was considered and superseded by the maintainer's SSH choice.

The phone shows one terminal, with a host/screen/pane picker and a special-key row. Long text has an explicit composer. Existing desktop splits are projected into a stable list, including floating panes; changing the viewport or local selection emits no layout, activation or destructive teardown ops. A terminal created on mobile is a real server-owned pane, created through existing mux ops and attached after spawn confirmation. Default placement is a tab in an existing slot (or a new screen when requested), with no projection-driven split rewriting.

### M1 — Mobile runtime and transport (provisional, tbd)

Use a separate mobile Tauri composition root sharing frontend modules and reusable Rust remote protocol code. Avoid making the current desktop composition root conditionally serve three unrelated lifecycles. Prototype the current xterm renderer first; do not promise both renderer backends on mobile.

Implement native SSH via an embedded maintained library behind a transport interface. Select the exact dependency in the first prototype based on Android builds, host-key verification, exec-channel backpressure, cancellation, key signing integration, license and supported cryptography. Do not implement SSH cryptography. Separate bridge framing from the current process-specific sender/receiver and reuse validated constant remote command construction. Keep native IPC fixed to saved profile/connection handles and typed actions; do not expose arbitrary process execution or private-key export to JS.

Mobile uses a `ConnSocket` adapter for native events, not a local TCP proxy or a page loaded from the host. Audit every initialization path for local HTTP/config assumptions. Use bundled defaults and device-local appearance settings; host-specific settings services outside session/mux control are not necessary for this release.

### M2 — First-release connection and key scope (provisional, tbd)

Support saved hostname/address, port, username and a device-generated key, plus manual public-key provisioning on the host. Store private material encrypted with an Android Keystore-held wrapping key; use non-exportable native signing where the chosen algorithm/library supports it. Do not claim hardware-backed storage for every key type. Default keys are device-local and excluded from ordinary app exports/backups; deletion is explicit. Explain that deleting a local key/profile does not revoke an already-authorized host key or end an established SSH session.

Unknown host fingerprints require deliberate verification before user authentication; changed pins fail closed. Pin by host and port. QR provisioning, key import, biometric unlock, password authentication, jump hosts, and complete desktop SSH configuration compatibility are follow-ups. The native API must leave room for them without weakening verification.

A compatible Splitty daemon is already installed/running. Probe its version/capabilities and present a clear missing/outdated-server result. No automatic daemon restart or host installation. Saved profiles support several hosts, with one actively connected host initially. Switching hosts detaches and isolates all IDs/drafts; it never transfers input to a new destination. Existing remote screens behind a desktop hub are not silently imported as direct connections.

### M3 — Session control and compatibility (provisional, tbd)

The server advertises `session-control-v1` in `mux.welcome`. New session messages `control.claim`, `control.release`, and `control.state` describe per-session ownership, current PTY dimensions and an opaque generation. Wire shapes are documented in `docs/protocol.ts` during implementation. A claim carries the generation last observed and an explicit takeover intent; stale competing claims fail instead of stealing control in sequence. Initial control state is delivered on attach, before a client can write.

Input and resize carry the current control generation and are accepted only from its controlling connection. Serialize validation plus write/resize against takeover so a revoked write cannot race through after the handoff acknowledgement. An accepted input is not a promise that the target program executed a command. Do not automatically resend input on ambiguous delivery.

An unowned terminal can be claimed by the first deliberate edit. An occupied terminal requires explicit takeover. Desktop/web clients are updated in the same change; merely focusing/rebuilding a panel cannot reclaim control. A claim changes ownership, not the layout. Only the controller sends live terminal-query replies; observers suppress them as well as input and resize. Owner changes are ordered and carry fresh generations, preventing delayed writes after release/reconnect/ABA ownership changes.

Release, detach of the controller, session end and connection cleanup invalidate ownership. A controller suspended without cleanup expires through existing transport liveness; explicit takeover need not wait for it. The last PTY size remains until a new controller supplies one. Ownership is ephemeral and never persisted with workspace state.

This deliberately replaces unrestricted session-protocol writers on a new server. Old clients cannot claim control and receive a clear upgrade-required error on input/resize; metadata and output remain readable. New desktop clients retain their old behavior when connected to an old server; mobile refuses that host because it cannot ensure safe handoff. Owner-authorized control APIs such as `/api/session/{id}/send` retain their automation semantics and are not an observer permission boundary. Coordinate with `cli-control` rather than changing its grants.

### M4 — Rendering, input and resume (provisional, tbd)

Keep each observer emulator at the server-reported columns/rows, independent of its viewport. Pan or scale the display; fit only when controlling. Remove observer attach-time resize nudges. Bounded raw replay cannot always reconstruct an arbitrary full-screen TUI, especially after buffer truncation or historical resizes: do not advertise a complete screen snapshot. Clearly identify limited restored history; explicit takeover may request a redraw through controller resize. A full terminal-state snapshot is follow-up work if the prototype shows it is necessary for the agreed use case.

Provide direct input and an explicit composer, with distinct insert-text and send-Enter actions. Direct input disables smart replacements where possible and uses composition/input events rather than synthetic keydown alone. Composer editing/dictation never writes to the PTY until requested; use the emulator paste path and bracketed paste when supported. Preserve multiline contents without promising that all target programs treat pasted newlines safely. Special keys must respect active terminal modes. Clipboard/selection and hardware keyboards share the same routing/control checks.

Handle visual viewport changes, safe areas, rotation and keyboard animation without near-zero PTY fits. Navigation does not automatically summon the keyboard. Keep drafts keyed by saved host identity and session, in app memory for the first release; preserve across view switches and transient reconnects, but do not promise persistence after OS process eviction. Never restore a draft into a replacement session.

On suspension/disconnect, disable sending and discard unsent direct-input queues. On resume, re-establish verified SSH with bounded backoff, renegotiate capabilities, get snapshots and reattach only the viewed session. Re-enter as observer and require a new claim. Show errors separately for unreachable SSH, rejected key, changed fingerprint, absent daemon and incompatible protocol. A phone's localhost is not a host preview URL; such links must explain the unavailable forwarding path rather than open an unrelated phone service.

## Architecture impact

- `architecture/landscape.c4` and `views.c4`: add a mobile composition-root container and its relationships to frontend, remote protocol and device key storage; no dependency on a local server/PTY backend.
- `architecture/remote.c4`: split reusable framing/command preparation from process transport; add the native SSH implementation and exact module relationships after selecting the library. Preserve desktop system-SSH behavior.
- `architecture/core.c4`: extend `splitty.core.protocol` with session control coordination and backend size access. Session control is connection state outside the pure mux reducer; no mux-to-PTY dependency is introduced.
- `architecture/server.c4`: update `ws` lifecycle/control notification wiring and the host bridge integration. Update `desktop.c4` only for changed shared composition relationships; mobile capabilities are separate from desktop remote-window permissions.
- `docs/architecture.md`: update Client Autonomy (device-local selection), Client connections/Connection Lifecycle (native transport), Attaching to sessions/Session Output (controller and replay limits), Remote Hosts (mobile keys), and add the Mobile Client composition section. Update `docs/protocol.ts` and both wire type definitions together.

## Risks / Trade-offs

- Software-keyboard/WebView behavior varies → real Android-device acceptance before completing the product UI; include IME, umlauts, swipe typing and dictation.
- Legacy writers fail against upgraded servers → capability negotiation, explicit errors and a coordinated desktop/server rollout; never silently fall back on mobile.
- SSH library/secure-storage mismatch → test actual signing and device builds first; do not equate encrypted key storage with non-exportable hardware keys.
- Full-screen replay may be incomplete → measure shell, agent, vim and htop cases; disclose bounded history, no observer-induced redraw, controller-only redraw.
- Server-only layout edits can be missed by baseline notifications → resync on overview open/resume and before acting on stale selections; general live layout replication remains `multi-attach` work.
- SSH cannot wake a host or bypass NAT → document a reachable SSH endpoint/private VPN and host uptime as setup prerequisites.
- Mobile OS suspension breaks connections → rely on host sessions and reconnect, never a promise of permanent background SSH. Push is separate future infrastructure.

## Migration Plan

1. Build a one-host device prototype and record dependency/platform decisions and acceptance evidence in this change.
2. Reconcile overlapping `cli-control` deltas; implement the control protocol and updated desktop/web clients together. Align the old `multi-attach` draft with the delivered handoff scope when implementation lands.
3. Ship mobile only against hosts advertising the feature; unsupported hosts receive setup/upgrade guidance without writes or session restarts.
4. No persisted workspace migration is needed. Rollback removes the app/new client; rolling back a daemon requires its normal controlled restart and loses ephemeral control state. Do not silently restart a host to repair compatibility.

## Provisional decisions

All dated 2026-10-17 and open to maintainer feedback:
- **M1:** separate Tauri mobile root, xterm-first prototype and embedded SSH adapter; alternative: fully native UI or another shell.
- **M2:** generated device key/manual provisioning, preinstalled daemon and one active host initially; alternative: import/passwords/QR/automatic installation and concurrent hosts in the first release.
- **M3:** connection-scoped controller with generation checks and explicit legacy-writer rejection; alternative: per-session opt-in compatibility mode with weaker mixed-client guarantees.
- **M4:** memory-only drafts, bounded replay with honest limitations, and explicit re-claim after resume; alternatives: encrypted persistent drafts and full terminal snapshots.

## Open Questions

- Exact embedded SSH dependency and key algorithm, selected by the M1 prototype criteria without changing the transport/security contract.
- Minimum Android version and supported keyboard combinations, recorded from real-device results before release packaging.
- Exact mobile spacing and optional tablet dual-view threshold, deferred until the single-terminal flow works; tablet dual-view is outside this change.
