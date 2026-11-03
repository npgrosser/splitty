# Proposal

## Why

Desktop clients and SSH bridges currently depend on a loopback web server even when no browser needs one. Replacing that dependency with native IPC and private local connections must preserve the existing authorization boundaries, rather than turn an IPC bridge into an unrestricted network, filesystem or command proxy.

## What Changes

- Make the client service independent of HTTP/WebSocket. Preserve one implementation of workspace operations, session handling, settings and host management behind transport adapters.
- Load packaged UI assets in every desktop window. Use window-bound IPC for Tauri and Electron; connect the Tauri embedded service in-process and child backends through inherited pipes. Use private Unix sockets for independently running daemons.
- Give local desktop sessions the app's lifetime on every platform. Always start an owned embedded service; never auto-attach a local daemon or revive its keepers. Persistent local work remains an explicit CLI/browser workflow.
- Carry Windows/WSL traffic through the existing `wsl.exe` child pipes. Native Windows remote-only operation continues to work without WSL.
- Reach remote daemons through the existing system-SSH exec/stdio approach, ending in a private client socket on the host. No terminal-server port or browser credential is needed on either end of a desktop SSH connection.
- Keep browser access as an explicit, authenticated loopback gateway: `splitty serve` for an embedded workspace, `splitty serve --daemon` for an existing daemon, and `splitty connect` for a remote daemon. Each gateway serves local assets and owns its own browser authentication.
- Give browser gateways independent session cookies and exact origins derived from their actual canonical IPv4/IPv6 listener; reject unsupported bind aliases instead of serving a UI whose origin cannot authenticate.
- Keep the existing owner-control, restricted-agent and keeper sockets distinct. The new UI/session connection does not widen their operation sets, mint browser credentials through them, or claim to isolate mutually hostile processes of the same OS user.
- Bind every native handle to its window, document lifetime, backend and operation scope. Remote data cannot select another backend or invoke local native actions. Treat parent-owned Dockview popouts as part of the main UI's trust domain; separate remote windows have no opener/shared-script path to it. Bound framing, requests, queues and connection/worker admission; disconnects never silently replay terminal input or mutations.
- Preserve intentional development-port forwards and reverse forwards, including their opt-in restoration rules and hardened SSH invocation. Exclude Splitty's own browser listeners from reverse-forward destinations with reservations that remain effective after coordinator crashes until local forwarding has ended.
- **BREAKING:** daemons no longer start HTTP listeners; their discovery/status output describes private endpoints instead of a required port. Desktop child startup and remote bridge negotiation use a versioned native transport. Older clients/servers get an explicit compatibility result rather than an automatic TCP fallback.
- **BREAKING:** remote desktop windows display packaged UI rather than executable content supplied by the host. Their native transport authority is limited to the assigned host; this does not grant local editor, filesystem, host-management or window-administration commands.

## Capabilities

### New Capabilities

- `client-connections`: client-service requests and session streams with transport negotiation, connection ownership, bounded delivery, cancellation and consistent lifecycle across native and browser clients.

### Modified Capabilities

- `access-control`: explicit listener creation, private client endpoints, native window scope, browser-gateway credentials, SSH boundaries and protection of internal endpoints from forwarding.
- `server-modes`: listener-free embedded hosting and daemons, private discovery, child lifetime and explicit browser adapters.
- `cli`: daemon commands and output, explicit browser serving, browser connect/reconnect, attach transport and native child startup.
- `control-api`: transport availability, shared service behavior and status without a mandatory listening address.
- `mux-protocol`: the same real-time messages over native streams and WebSocket, transport-neutral welcome, liveness, reconnect and port discovery.
- `remote-hosts`: private-endpoint handshake, independently versioned transport capabilities and SSH bridge behavior.
- `remote-screens`: host setup and reconnection independent of WebSocket tickets and remote TCP port hints.
- `desktop-remote`: packaged remote-window UI, host-bound IPC and connection lifetime without a loopback tunnel.

## Impact

Touches the server composition roots, HTTP and WebSocket adapters, CLI discovery/attach, remote handshake/bridge/connect, frontend request/socket adapters, both desktop shells and their packaging. Windows/WSL startup, desktop navigation/CSP/capabilities, popouts, downloads and editor handoffs need explicit acceptance coverage.

The design builds on [persistent daemons](../../../roadmap/05-persistent-daemon.md) and [SSH access](../../../roadmap/05a-remote-connect.md). It accounts for the reverse-forward behavior already merged on `main` and the pending upload cancellation work. The `mobile-ssh-client` plan must consume the same transport contract when implemented; its SSH library, device keys and session-control changes are not implemented here. The `mcp-server` plan retains restricted control-API access rather than acquiring UI-socket authority.

Out of scope: new multi-user access, a public network listener, HTTP content proxying, SSH cryptography or key management, session-control ownership redesign, changes to keeper protocol v1, new mobile platforms, and a guaranteed performance improvement. Terminal, workspace, file-transfer and forwarding behavior stays the same except for the explicit transport, desktop lifetime and CLI compatibility changes above.

Implementation is phased, but the default cutover is one coordinated release after all supported desktop, browser and SSH paths pass the acceptance matrix. Architecture choices and numeric limits in the design are provisional until reviewed.
