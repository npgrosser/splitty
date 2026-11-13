# Proposal

## Why

Three rules about a host's state are decided by each window instead of by the hub that keeps that state:

- Host update: the hub reports `update: {available, keepsSessions}`, but the web window builds its own warning, counts the panes an update would end from `remote.ready` and its layouts and has a text for an update that ends sessions, which the hub never offers; the native window refuses that case. Both take the version the host gets from somewhere else (the web window from `/api/build`, which in front of a daemon is the gateway's version, the native window from its backend).
- Remembered remote Screens: the web window replaces the whole list with what it shows and drops the `closed` field, so each of its writes erases the Screens and `closed` marks of hosts it does not show; the native window reads the list, edits its hosts and writes it back in two requests. Last writer wins.
- Host directory: the web window reports each host's full list with pane counts and, for hubs without `hub-host-decisions-v1`, merges known hosts and the catalog itself and writes `PUT /api/remote/catalog`; the native window merges the stored directory with the live catalog and reports only removals, because the live catalog has no pane counts.

## What Changes

- Host update: `update` in `remote.ready` and in `remote.error` gains `toVersion`, the version the host's daemon gets, whenever `available` is true. The spec states that an available update keeps every session. Both windows offer an ordinary update only when `available` and `keepsSessions` are true, and name the hub's `toVersion`. The web window drops `updateImpact` and the warnings for an update that ends sessions.
- Remembered Screens: new operation `remoteScreens.updateHost` (`PUT /api/remote/screens/host`, `{target, screens: [{screenId, name?, position?}], closed?}`). The hub replaces only that host's entries, under its lock, in the host's place in the list; an empty list forgets the host; `closed` marks them as a closed host's. Both windows write only the hosts they show or showed, with this operation. The web window keeps `closed` entries and does not restore them. The whole-list `PUT /api/remote/screens` stays for older clients.
- Host directory: a screen in an observation (`hub.catalog.observe`, `PUT /api/remote/catalog`) may omit `panes`. The hub then keeps the stored count, or leaves the screen out while it has none. The native window reports the host's live list (new Screens, renames, removals) instead of a merge of its own. The web window drops its merge and its catalog writes for hubs without `hub-host-decisions-v1`.
- The local service announces `hub-host-state-v1` in `mux.welcome` on local main connections. It covers the per-host remembered write and the observations without pane counts.

## Capabilities

### New Capabilities
None.

### Modified Capabilities
- `remote-screens`: Remote Screen Hub Endpoint (`toVersion`), Host Update (an available update keeps every session; what the window offers and names), Remembered Remote Screens (per-host write, `closed`, older services), Host Screen Directory Cache (observations without pane counts, the native report, legacy routes, older hubs).

## Impact

`crates/splitty-core` (`native_api`: the new operation and the shared request types, the capability), `crates/splitty-server` (hub: `remembered`, `catalog`, `failure::UpdateAvailability`; dispatch, route, welcome), `crates/splitty-client-core` (`hub::remembered` writes per host, `native_workspace::host_catalog` reports the live list, `workspace::window::host_update` reads `toVersion`), `frontend/` (`lib/remote/update.ts`, `remembered.ts`, `host_directory.ts`, `Workspace.svelte`), `docs/protocol.ts`, `docs/native-operations.json`, `docs/architecture.md`, `docs/native-ui-handoff.md`.

## Architecture impact

None in `architecture/*.c4`: no new crate, top-level module or dependency between components. `docs/architecture.md` describes the per-host remembered write and the hub's observation rule.

## Decisions

- HHS-1: an ordinary update keeps every session, so the update plan is `available`, `keepsSessions` and `toVersion`, and no window shows Screens or panes that would end. With a hub that sends no `toVersion` the window names the local version as before (provisional, tbd).
- HHS-2: the remembered list is written per host. A window writes only hosts it shows or showed: the web window forgets a host whose last Screen leaves it and never writes `closed`; the native window keeps a host it closes as `closed`. Neither window restores `closed` entries at start. Against a local service without `hub-host-state-v1` neither window writes the list; both still read it and bring its Screens back (provisional, tbd).
- HHS-3: a screen an observation lists without `panes` keeps its stored count; one the hub has stored no count for is left out until an observation gives one, which the next ready connection does. The native window reports a ready host's live list whenever it changes and, without `hub-host-state-v1`, reports nothing (provisional, tbd).
- HHS-4: the web window keeps no host directory of its own for a hub without `hub-host-decisions-v1`: no last observed Screens and no recent hosts from it in the drawer; the host picker still lists known hosts. `GET`/`PUT /api/remote/catalog` (`catalog.get`, `catalog.update`) stay as legacy routes with the same rule: no current window calls them, but a browser gateway of an older release can front a newer daemon and forward its window's writes. `GET /api/remote/hosts` stays for the host picker (provisional, tbd).
