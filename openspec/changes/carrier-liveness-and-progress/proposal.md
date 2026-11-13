# Proposal

## Why

Browsers no longer reach the shared session loop: every carrier of a session is native, and the browser gateway speaks the native protocol to the service. Two leftovers of the in-process WebSocket router remain.

- The session loop still runs a WebSocket-style heartbeat: it queues a ping every 15 seconds, ends the session after 30 seconds without input, and sweeps expired editor requests and file offers on the same tick. The native carrier answers each of these pings itself, so the heartbeat never decides anything; the native transport's own ping and the gateway's browser ping are the liveness that works.
- Host preparation progress travels twice on a native host stream: as transport progress reports during preparation and, after the stream is accepted, once more as up to eight `remote.progress` text messages before `remote.ready`. The desktop client ignores the repeat, the gateway drops it by step order, the Tauri web view adapter matches it away, and the hub writes the step name in two ways.

## What Changes

- The session loop has no heartbeat and no ping or pong session message. Liveness belongs to each carrier: the native transport (ping every 15 seconds, ended when no matching pong arrived for 30 seconds at a check) and the browser gateway (WebSocket ping every 15 seconds, closed 30 seconds after the last matching pong). Ending a carrier releases the session as before.
- Editor requests and file offers expire by their own deadlines only; the session loop no longer sweeps them.
- A native host stream carries preparation progress once, as transport progress reports. After acceptance the hub's first application message is `remote.ready`. The gateway keeps translating the transport reports into browser `remote.progress` messages, one per step in order.
- The step names are defined once in the hub, and the gateway orders steps by that definition.
- Readers stay tolerant of a hub of the previous release: the desktop client, the gateway and the Tauri web view adapter ignore `remote.progress` text before `remote.ready`.
- Session-loop comments that still speak of WebSocket connections describe the session.

Out of scope: the transport's ping/pong frames and the gateway's browser heartbeat (unchanged); the progress of the remote-window helper and of `splitty connect`, which use their own step set; typed wire definitions for hub messages (separate cleanup).

## Capabilities

### New Capabilities

### Modified Capabilities
- `mux-protocol`: Heartbeat and Liveness states liveness per carrier with its bound, and that a disconnect by liveness releases the session.
- `remote-screens`: Remote Screen Hub Endpoint states that progress travels once per stream (transport reports on a native host stream, `remote.progress` text on a browser socket) and how readers treat an older hub's repeat.

## Impact

- `crates/splitty-server`: `client_session` (handler loop, session message type, sender, comments), `native` carrier loop, `hub` (native host stream, step names), `http::gateway` (step order and prelude filter), tests.
- `crates/splitty-client-core`: comment only on the tolerated older-hub prelude.
- `frontend/src/lib/remote/native_hub.ts` (Tauri web view adapter) and its test.
- `docs/protocol.ts`, `docs/architecture.md`.
- Wire: a native host stream no longer carries `remote.progress` text after acceptance. Browser sockets are unchanged. No capability: the readers of every supported release already ignore or drop that text.

## Architecture impact

None: no component or relationship of architecture/*.c4 changes; docs/architecture.md's native adapter and hub paragraphs state where liveness and progress live.

## Decisions

- **`carrier-liveness-and-progress`, CLP-1 to CLP-3** (2026-11-13, provisional, tbd): CLP-1 liveness is the carrier's alone, with the bounds the carriers already had (native: ended within 45 seconds of silence, at the first 15-second check 30 seconds after the last matching pong; browser: closed 30 seconds after the last matching pong); the session loop keeps no second timer of its own. Alternative: a session-level idle timeout on top. CLP-2 editor requests and file offers expire only through the deadline each waiting request already holds, which fires at the deadline itself instead of at the next 15-second sweep. CLP-3 older peers are handled on the reading side, without a capability: a hub of the previous release still repeats its progress as text, and the desktop client, the gateway and the Tauri web view adapter ignore every `remote.progress` text before `remote.ready`; a newer reader against an older hub therefore shows each step once from the transport reports. Alternative: a capability in `remote.ready`, which would arrive after the repeat it is meant to announce.
