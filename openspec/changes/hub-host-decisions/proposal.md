# Proposal

> Planned on 2026-11-06. Part 4 of the sequence in `frontend-client-core`, independent of parts 1 to 3. Related: `native-client-transports`; roadmap 112 in `roadmap/README.md`.

## Why

The local server is the hub for remote hosts, but several decisions about hosts are made in the window. Some of them repeat what the hub or `splitty-remote` already knows, and some contradict it:

- **Updates.** The window treats any other version on a host as an available update. A host running a newer Splitty is offered "Update…", although the remote installer never replaces a newer daemon. The Hosts panel also always says an update ends the host's screens, even for a daemon that keeps its sessions; only the confirmation dialog states it correctly.
- **Retries.** The window re-encodes which hub errors are worth retrying (`retryable` in `remote_socket.ts`).
- **Host directory.** The window merges recent, configured and alias hosts, the screen catalog, the hosts of port forwards and the host order into host groups. `hosts::known` in `splitty-remote` already dedups and prefers recent hosts.
- **Catalog.** The window writes the host screen catalog, with coalescing, revisions, keepalive and late-read guards. These exist only because the window is the writer. The hub already holds the screens from the handshake at `remote.ready`.
- **Ports.** Forwards are polled every 5 seconds, with reconciliation against an error baseline, because there is no change feed.

## What Changes

- **Update availability from the hub.** `remote.ready` (and `incompatible`) carries `update: {available, keepsSessions}`, where available means the host's daemon is older. The hub refuses to downgrade a newer daemon. The window shows what the hub says.
- **Retry classification from the hub.** `remote.error` carries `retryable`.
- **Host directory read.** A local endpoint returns the merged host directory: recent, configured and alias hosts with hidden ones filtered out, catalog `lastSeen` and screens, the hosts of saved and active forwards, ordered by the host order. The window adds only open screens, live connection state and first-appearance order.
- **Catalog written by the hub.** The hub persists the handshake directory at readiness. Later window observations use an ordered message on the local connection; the window has no persistence queue, keepalive writer or revision reconciliation. The hub continues relaying host frames without interpreting them. Existing catalog HTTP writes remain for older clients.
- **Forwards change feed.** Changes to forwards are pushed on the local connection, which replaces the window's polling.

## Capabilities

To be decided when the change is next. Likely candidates:

### Modified Capabilities

- `remote-screens`: Host Update (offer only for an older daemon, no downgrade); Host Screen Directory Cache (the hub writes it); Detected Ports In The Window (change feed instead of polling).
- Existing `remote-hosts` known-host and order rules are reused by the merged read. Separate desktop windows keep the supervisor's existing update policy; no new authority or desktop-shell interface is needed.

## Impact

- **Server:** the hub (ready and error frames, catalog writes, the forwards feed) and a new remote directory route.
- **Remote:** `hosts`, and `older` / `update_plan` made available to the hub.
- **Frontend:** `remote/update.ts`, `remote/on_hosts.ts`, `remote/host_directory.ts` (catalog writes removed), `remote/port_forwarding.ts` (polling removed), the Hosts panel.
- **Docs:** `docs/protocol.ts`.
- **Both transports:** new requests are added to the native operation catalog (`docs/native-operations.json`) as well as the HTTP routes, and new messages travel over the WebSocket and native connections alike (`native-client-transports`).

Out of scope, staying in the window on purpose (`frontend-client-core` design, "Sequence"):

- automatic port forwarding, which would make the hub parse relayed, untrusted host data;
- remembered remote screens (window-side per `remote-screens`);
- the reconnect loop, since the window treats `remote.*` frames from a ready host as forged;
- the SSH target grammar the window repeats for input checks.

## Carried-over design notes

- 2026-11-04, `frontend-client-core` inventory:
  - `updateNote` compares with `!==` (`remote/update.ts` about line 94) and drives the Hosts panel's `olderVersion` (`HostsPanel.svelte` about lines 143–146). `HostsPanel.svelte` about line 216 states the update consequence without `keepsSessions`. The hub stops a differing daemon (`hub/mod.rs` about line 544), while `splitty_remote`'s `update_plan` / `older` never replaces a newer one (`tunnel/supervisor.rs` about lines 147–157).
  - Duplicated host merging: `on_hosts.ts` (about lines 34–63), `host_directory.ts` (about lines 191–197). `hosts.rs` has `known` (about lines 77–126).
  - Polling and baseline: `port_forwarding.ts` (about lines 184–198 and 521–530).
  - `parseSavedForwards` rejects a whole list above 32 per host (`ports.ts` about lines 83–90). Raising the server's limit would blank saved forwards in older windows; settle this when the feed is designed.
  - SSH target grammar repeated in `host_directory.ts` (about lines 275–302) and `host_picker.ts` (about lines 28–33) against `SshTarget::parse`.
