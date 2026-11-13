# Proposal

## Why

`remote.autoForwardPorts` runs in each window, not in the hub that owns the forwards: the web window and the native window each follow the host's `ports.changed` and send automatic forward requests, with different skip rules (the web window skips active forwards and paused saved rules, the native window forwards that are up, being rebuilt or paused). Two windows on one host both fire. `remote.reconnectHosts` means "connect these hosts at start and bring back their Screens" in the native window, while the web window ignores it and connects a host only when one of its Screens becomes active.

## What Changes

- The hub reads the `ports.changed` that its relays (`/ws/remote` and native host streams) carry from a host and, with `remote.autoForwardPorts` on, forwards a port that appears itself. Each relay's first list is a baseline; what was forwarded is kept per host while one of its relays runs, so the port is forwarded once for all windows and once per process. One skip rule, in the hub: a port is not forwarded when it has a forward (opening, up or being rebuilt), its saved rule is paused, or an open of it is scheduled already. Requests are automatic: saved local ports exact, nothing remembered.
- The local service announces `hub-auto-forward-v1` in `mux.welcome` on local main connections.
- Both clients delete their automatic forwarding (`native_workspace/auto_forward.rs`, `ForwardRequest::automatic`; the web `PortForwarding` auto path and `appearedPorts`) and only show the forwards the hub reports.
- `remote.reconnectHosts` has one meaning: a window connects the listed hosts at its start when it brings back remembered Screens of them; other hosts connect when the user asks (web: when one of their Screens is shown). The web window now follows the list; the native window already does.

Out of scope: the remembered-Screens format and its `closed` entries, the host directory, the host update plan, connecting hosts without a window.

## Capabilities

### New Capabilities
None.

### Modified Capabilities
- `remote-screens`: Automatic Port Forwarding moves to the hub, with its capability and the older-service behavior; Remembered Remote Screens connects the hosts of `remote.reconnectHosts` at start.

## Impact

`crates/splitty-server` (hub: `auto_forward`, `Forwards::open_automatically`, settings handed to the hub, welcome capability), `crates/splitty-remote/tests/native_bridge.py` (test fixture emits daemon events), `crates/splitty-client-core` (auto-forward removed), `frontend/` (auto-forward removed, `hostsToReconnect`, `remote.reconnectHosts` in the settings type), `docs/protocol.ts`, `docs/architecture.md`, `docs/remote.md`, `docs/native-ui-handoff.md`, `architecture/server.c4`.

## Architecture impact

`hub` component description and its relations to `splitty.core.config`, `splitty.transport.codec` and from `client_session` name automatic forwarding; no new crate, top-level module or dependency between components. `docs/architecture.md` describes `hub::auto_forward`.

## Decisions

- HAF-1: the hub follows `ports.changed` per relay and keeps what it forwarded per host while one of the host's relays runs. With no window connected to a host there is no relay and nothing is forwarded; the hub opens no host connection of its own for discovery (provisional, tbd).
- HAF-2: a local service without `hub-auto-forward-v1` forwards nothing automatically; the clients keep no fallback and the setting stays editable, taking effect once the service is updated (provisional, tbd).
- HAF-3: `remote.reconnectHosts` stays a window's choice at its start, not the hub's at service start: a hub-held connection has no window to show the Screens in or to answer SSH questions, and service start stays passive (access-control, saved restoration). The web window connects the listed hosts of the Screens it restores (provisional, tbd).
- HAF-4: the hub reads a relayed text frame only when it begins `{"type":"ports.changed"`, as a daemon serializes it, and only up to 256 KiB and 256 ports; every other relayed message stays unread.
