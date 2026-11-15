## ADDED Requirements

### Requirement: Host Screen Directories
The local server SHALL persist the last observed screen directory per host separately from the remembered open-screen list. `GET /api/remote/catalog` SHALL return `{version:1,hosts:[{target,lastSeen,screens:[{id,name?,panes}]}]}`. `PUT /api/remote/catalog` SHALL accept `{target,screens}`, replace only that host's directory and return its stored entry with `lastSeen` in Unix seconds. A successful empty list SHALL replace a previous nonempty one. The catalog routes remain for older clients; current clients report through `hub.catalog.observe` and read `remote.directory.get`. These APIs SHALL neither connect nor send commands to a remote host.

Hidden hosts SHALL be excluded from catalog reads. An update for a hidden host SHALL return 409 without storing its observation.

The file `remote-catalog.json` SHALL be written atomically with mode 0600 in the private configuration directory. It SHALL keep at most 20 host entries, each with at most 200 screens. Targets SHALL be valid and canonical, screen ids unique per host with 1–256 characters, names at most 200 characters, and pane counts nonnegative integers. Invalid writes and unknown fields SHALL be rejected without modifying the saved directory. Missing or corrupt files SHALL read as empty. The routes are available only as the equivalent native operations of a local main connection (see `client-connections`, Enumerated Native Requests); no listener serves these paths, and the control socket answers 404 for them.

After successful host readiness the hub SHALL store the handshake screen directory through the same validation and hidden-host rules. A local client MAY report later directory observations using `hub.catalog.observe` with `{target,screens}`; admitted observations SHALL be processed in connection order and SHALL never connect a host. Persistence and invalidation SHALL be handled by the hub; legacy PUT remains available and follows the same rule.

An observation lists the host's screens as they are now. A hub announcing `hub-host-state-v1` SHALL accept a screen without `panes`: it SHALL keep the pane count it has stored for that screen, and SHALL leave a screen it has stored no count for out of the directory until an observation gives one. A client that knows pane counts SHALL send them.

While a host is connected and its live catalog of the current connection is complete, the native workspace SHALL report that catalog, without pane counts, whenever it differs from the last report for the connection: new screens, renames and screens that left the host, such as a screen terminated from the window once the host confirmed it, or one that closed with its last pane. A later connection to the host SHALL therefore not list such a screen while it is being established. A terminate the host has not confirmed or has refused, and a detached screen, SHALL leave the stored directory unchanged. Without `hub-host-state-v1` the native workspace SHALL report nothing.

#### Scenario: Host directories survive a local restart
- **WHEN** directories for `vm1` and `vm2` are saved and `vm1` is updated
- **THEN** a subsequent read after server reconstruction returns the updated `vm1` directory and the unchanged `vm2` directory

#### Scenario: Empty directory
- **WHEN** the host reports no screens after previously reporting two
- **THEN** the next saved directory for that host is empty

#### Scenario: Invalid or unauthorized update
- **WHEN** a request is made over the control socket or a connection that is not a local main connection, or contains duplicate screen ids
- **THEN** the update is rejected and no saved directory or remote host is changed

#### Scenario: Late observation after removal
- **WHEN** an already attached host publishes its screen list after that host was forgotten
- **THEN** it remains hidden and no cached directory is recreated

#### Scenario: Catalog without a window writer
- **WHEN** host readiness lists screens and the window sends no catalog PUT
- **THEN** the local cache stores those screens, including a successful empty directory

#### Scenario: Ordered observations
- **WHEN** one local connection reports a screen rename and then removal
- **THEN** the final cache reflects the removal and late hidden-host observations cannot recreate a forgotten directory

#### Scenario: A screen that left a connected host
- **WHEN** the native window terminates a screen on `vm1`, the host confirms it, and `vm1` later reconnects
- **THEN** the host's last observed screens do not include it while the connection is being established
- **AND** an unconfirmed or refused terminate and a detach leave the screen listed

#### Scenario: Observation without pane counts
- **WHEN** the hub has stored `s1` with three panes for `vm1` and an observation lists `s1` renamed and a new `s2`, both without `panes`
- **THEN** the stored directory lists `s1` under its new name with three panes and does not list `s2` until an observation gives its pane count

## MODIFIED Requirements

### Requirement: Remote Screen Hub Endpoint
An authorized native main client SHALL be able to open a host stream for a `target` through its assigned local service; no `/ws/remote` endpoint SHALL exist. For each host stream the hub SHALL:
1. validate the target (see `access-control`, Remote Access Through SSH Tunnel);
2. make sure the host has a server and a running daemon: install it where needed (see `remote-hosts`), run the format-2 handshake with `--ensure-daemon` and no port hint over the system `ssh`, whose questions reach the user only through the local service (see `remote-hosts`, SSH Questions Of The Local Service), and request no TCP forwarding. A daemon of another version SHALL be used only when its transport and mux protocols are supported; it is replaced only as Host Update allows, and otherwise reported `incompatible` with update guidance and no HTTP fallback;
3. start `splitty remote-bridge --transport native-v1` on the host over SSH, using the installed server's absolute path when it installed one or found one installed;
4. send `remote.ready`;
5. relay every application message between the client stream and the host daemon unchanged, text as text and binary as binary, in order.

Preparation progress SHALL travel once per stream. The steps are `install`, `daemon`, `update`, `started` and `bridge`, in this order. On a native host stream the hub SHALL report them only as transport progress reports during preparation, before the stream is accepted; such reports MAY also carry a phase that is not a step, such as `connecting`. The hub SHALL send no `remote.progress` text.

Before the relay the hub SHALL send only these application text messages, after the host stream's acceptance:
- exactly one `{"type": "remote.ready", "target", "daemon": {"version", "protocol", "keepsSessions", "started"}, "screens": [{"id", "name", "panes"}]}`, where `keepsSessions` is the handshake's (see `remote-hosts`, Remote Handshake);
- or `{"type": "remote.error", "code", "message", "hint"?}`, after which it SHALL close the stream. `code` is one of `invalid_target`, `host_key`, `auth`, `unreachable`, `unsupported_host`, `install`, `daemon`, `incompatible`, `migration_unavailable`, `bridge` and `internal`.

After `remote.ready`, the hub SHALL send no application message of its own; transport negotiation/liveness is outside this application stream. When the connection to the host ends, it SHALL close the host stream with the host-lost reason. When the stream closes, it SHALL end its `ssh` process. Each host stream SHALL use an SSH connection of its own. Once `remote.ready` is sent, the server SHALL record the target as a recent host (see `remote-hosts`, Known Hosts).

The ready message SHALL include `update: {available, keepsSessions, toVersion?}`, the hub's update plan for the host (see Host Update); `toVersion` is the version the host's daemon gets and SHALL be present whenever `available` is true. Every hub error SHALL include `retryable`; the client SHALL use this classification when present and retain legacy classification only for older hubs. After readiness the hub SHALL validate and persist the handshake directory without interpreting relayed frames. Invalid or unwritable cache data SHALL not fail an otherwise usable connection.

#### Scenario: Connect a host without Splitty
- **WHEN** a native main client opens a host stream for `vm1` and `vm1` has no Splitty
- **THEN** it receives transport progress reports for the install and the daemon start, then `remote.ready` with `started` true, then the host daemon's `mux.welcome`

#### Scenario: Progress on a native host stream
- **WHEN** a native main client opens a host stream for a host without Splitty
- **THEN** it receives transport progress reports for the install and the daemon start during preparation, and after acceptance `remote.ready` is the first application message, followed by the host daemon's `mux.welcome`, without a `remote.progress` text

#### Scenario: Hub of the previous release
- **WHEN** a native client opens a host stream through a hub of the previous release, which repeats its progress as `remote.progress` text after acceptance
- **THEN** the reader ignores every `remote.progress` text before `remote.ready`, so each step is shown once, from the transport progress reports; no capability is involved

#### Scenario: Unknown host key
- **WHEN** `ssh` to the target fails host key verification
- **THEN** the client receives `remote.error` with code `host_key` and a hint to run `ssh <target>` once in a terminal, and the stream closes

#### Scenario: Host connection lost
- **WHEN** the `ssh` process of a relaying host stream ends
- **THEN** the hub closes the stream with the host-lost reason

#### Scenario: Client closes the socket
- **WHEN** the client closes a relaying host stream
- **THEN** the hub ends that stream's `ssh` process, and the host daemon and its sessions keep running

#### Scenario: Ready from a daemon that keeps its sessions
- **WHEN** a client opens a host stream for a host whose daemon keeps its sessions
- **THEN** its `remote.ready` carries `keepsSessions` true in `daemon`

#### Scenario: Ready from an older host that can be updated
- **WHEN** a client opens a host stream for a host whose daemon keeps its sessions and runs an older version that the hub can replace
- **THEN** its `remote.ready` carries `update` with `available` and `keepsSessions` true and `toVersion` the hub's own version

#### Scenario: Authoritative retry classification
- **WHEN** a reconnect fails and the hub reports `retryable: false`
- **THEN** the client stops automatic retrying even if an older client classified that error code as transient

#### Scenario: Host daemon cannot be started
- **WHEN** the host cannot start a daemon
- **THEN** the client receives `remote.error` with code `daemon` and the host's reason, and the hub's `ssh` process ends

#### Scenario: Former browser endpoint
- **WHEN** any process connects to a local service and asks for `/ws/remote?target=vm1`
- **THEN** no listener answers, and no `ssh` process starts

### Requirement: Known Hosts API
`GET /api/remote/hosts` SHALL return the known hosts of the user the server runs as (see `remote-hosts`, Known Hosts) as a JSON array of `{ "target", "source", "lastConnected"?, "alias"? }`, available only as the equivalent native operations of a local main connection (see `client-connections`, Enumerated Native Requests); no listener serves these paths, and the control socket answers 404 for them.

#### Scenario: Hosts for the picker
- **WHEN** the UI requests `/api/remote/hosts` and `vm1` is a recent host
- **THEN** the response lists `vm1` first with source `recent`

`GET /api/remote/host-preferences` SHALL return `{version:1,hosts:[{target,alias?,hidden?}]}`. `PUT` SHALL accept `{target,alias?:string|null,hidden?:false}`, require at least one field besides target, preserve omitted fields and return the stored row. `DELETE /api/remote/hosts` SHALL accept `{target}`, forget that host as described in `remote-hosts`, and return 204. These operations are available only as the equivalent native operations of a local main connection (see `client-connections`, Enumerated Native Requests); no listener serves these paths, and the control socket answers 404 for them. Reading or changing these metadata endpoints SHALL NOT connect to a host.

#### Scenario: Invalid preference update
- **WHEN** an update contains an invalid target, overlong alias, controls or unknown fields
- **THEN** it is rejected without modifying saved metadata

#### Scenario: Unauthorized metadata change
- **WHEN** a preference or removal request is made over the control socket, a grant socket or a connection that is not a local main connection
- **THEN** it is rejected without changing host metadata or contacting a host

### Requirement: Remote Screen Reconnect
When a host's connection drops after `remote.ready`, the client SHALL mark that host's screens as reconnecting, keep them from taking input, and reconnect with a delay that grows to 10 seconds, with a freshly authorized host stream for every attempt. A failed host-stream open SHALL NOT be reported as an error of the local connection. No queued terminal input or mutation from the lost connection SHALL be replayed. After a successful reconnect, the client SHALL resync the host's screens and re-attach their sessions. When the connection fails with an error code that retrying cannot fix, the client SHALL stop retrying and show the error with its hint and a retry. When a host cannot be reached again, its screens SHALL be shown as offline with a retry.

#### Scenario: Network drop
- **WHEN** the connection to `vm1` drops and the host is reachable again 20 seconds later
- **THEN** `vm1`'s screens show "reconnecting…" and then their content again, with the output written in the meantime

### Requirement: Remembered Remote Screens
The server SHALL keep the client's list of open remote screens in `remote-screens.json` in its configuration directory, written atomically with mode 0600. The file holds `{"version": 1, "screens": [{"target", "screenId", "name"?, "position"?, "closed"?}]}`, each target valid (see `access-control`, Remote Access Through SSH Tunnel), each (target, screenId) once; `closed` marks the screens of a host that was closed in a window, offered again but not reopened at start. `GET /api/remote/screens` SHALL return the list, empty when the file is missing or unreadable. `PUT /api/remote/screens` SHALL replace it and SHALL reject a body that does not have that shape; it remains for older clients. Both are available only as the equivalent native operations of a local main connection (see `client-connections`, Enumerated Native Requests); no listener serves these paths, and the control socket answers 404 for them.

`PUT /api/remote/screens/host` (native `remoteScreens.updateHost`) SHALL take `{"target", "screens": [{"screenId", "name"?, "position"?}], "closed"?}` and replace only that host's entries, in the place the host's first entry had in the list (a new host at the end), with `closed` on each entry when the body sets it. An empty `screens` SHALL forget the host. The hub SHALL apply the replacement to the list it holds under its own lock, so that writes for different hosts never undo each other, keep at most 200 entries, reject a body that does not have the shape without changing the list, and return the stored list. A local service supporting it SHALL announce `hub-host-state-v1` in `mux.welcome` on local main connections.

A client SHALL write, after every change to its remote screens, the entries of each host whose screens it shows or showed, with `remoteScreens.updateHost`, and SHALL NOT replace the entries of other hosts. A host whose last screen leaves the window SHALL be forgotten, unless the window closed the host, which keeps its screens as `closed`. Without `hub-host-state-v1` a client SHALL NOT write the list; it still reads it and brings its screens back. On start, it SHALL restore the saved references and order, except `closed` entries, displayed within their host groups as described by Remote Screens In The Window. The saved position format SHALL remain compatible with existing mixed lists. It SHALL connect a remembered screen's host only when the screen becomes visible, when that host is connected anyway, or, once at the window's start, when the setting `remote.reconnectHosts` lists the host. A remembered screen that its host no longer has SHALL be shown as missing, with an action to remove it.

#### Scenario: Restart
- **WHEN** the window shows a remote screen of `vm1` and the app is restarted
- **THEN** the screen is back in its host group in the remembered relative order, and it connects when the user opens it

#### Scenario: Unreachable host at start
- **WHEN** a remembered screen's host is unreachable when the app starts
- **THEN** the local screens are usable at once, and the remote screen shows its host as offline only when it is opened

#### Scenario: Host reconnected at start
- **WHEN** `remote.reconnectHosts` lists `vm1` but not `vm2`, and a window starts with remembered screens of both
- **THEN** the window connects `vm1` at once and brings back its screens, while `vm2` stays unconnected until the user opens one of its screens

#### Scenario: Two windows with different hosts
- **WHEN** two clients of one local service show screens of `vm1` and of `vm2`, and each writes its screens
- **THEN** the list holds the screens of both hosts, and a `closed` mark the native window wrote for `vm3` stays

#### Scenario: A closed host is not reopened
- **WHEN** the list holds `closed` screens of `vm1` and a window starts
- **THEN** the window does not bring them back or connect `vm1` for them

#### Scenario: Older local service
- **WHEN** a client's local service does not announce `hub-host-state-v1`
- **THEN** the client brings back the remembered screens and writes no change to the list

### Requirement: Host Update
An authorized native main host connection SHALL support an explicit update intent. A client SHALL request it only after the user confirms the versions and keeper-session behavior. An older native daemon may be updated only with verified instance identity, session keepers and the matching installed replacement. An eligible legacy daemon SHALL use Confirmed Legacy Daemon Migration instead. A missing replacement, newer daemon, absent session keepers or blocked legacy migration SHALL stop nothing and return an actionable failure. No update SHALL deliberately end sessions.

The intent SHALL apply to one connection attempt and be consumed before a potentially mutating command; reconnect and ambiguous completion SHALL NOT replay it. Successful replacement SHALL report update progress and require a fresh handshake for the intended native daemon. A daemon already running the intended version SHALL remain running. Migration offers SHALL be structured metadata rather than inferred from message text; blocked legacy migration SHALL use `migration_unavailable` and SHALL NOT offer an ordinary update that cannot succeed.

The hub SHALL report ordinary update availability only when the daemon version has lower SemVer precedence than its own version, accepting one optional `v` prefix, comparing numeric prerelease identifiers numerically and ignoring build metadata; an invalid version SHALL NOT authorize a replacement. Unknown versions SHALL not imply an available update. Availability SHALL also require the verified instance, session preservation and matching prepared replacement required by the shared replacement plan. The window SHALL use this metadata, including the keeper behavior, rather than comparing versions itself. This metadata SHALL never bypass the replacement checks above.

The `update` metadata in `remote.ready` and in an `incompatible` `remote.error` SHALL be the whole plan for an ordinary update: `available`, `keepsSessions`, and `toVersion` when available. An available update SHALL keep every session of the host, so it ends no screen and no pane; the connection drops for a moment. A window SHALL offer an ordinary update only when `available` and `keepsSessions` are both true, SHALL name `toVersion` as the version the host gets, and SHALL NOT count or name screens or panes that end. With a hub that sends no `toVersion` the window SHALL name its local server's version instead.

#### Scenario: Update a compatible older host
- **WHEN** a user confirms updating an older native daemon with keepers and a matching replacement
- **THEN** the instance-bound update preserves keeper sessions and connects to the freshly verified replacement

#### Scenario: Nothing to update
- **WHEN** a confirmed update finds the host already running the intended version
- **THEN** it connects without stopping that daemon

#### Scenario: Eligible legacy migration
- **WHEN** a host failure includes a positive migration offer and the user confirms it
- **THEN** the next connection consumes the migration intent once and preserves keeper-managed sessions through the conditional migration

#### Scenario: Migration unavailable
- **WHEN** legacy process identity or keeper compatibility cannot be verified
- **THEN** the UI explains the blocked reason and does not present a working update action or end sessions

#### Scenario: Newer or unparseable host version
- **WHEN** the daemon version is newer, equal in precedence, or unparseable
- **THEN** update availability is false and the window offers no ordinary update

#### Scenario: Confirming an update
- **WHEN** the hub reports `update` with `available` and `keepsSessions` true and `toVersion` `0.4.0` for `vm1`
- **THEN** the window asks to update `vm1` to `0.4.0`, says that its sessions keep running and names no screens or panes that end

#### Scenario: Availability without kept sessions
- **WHEN** a hub reports `available` true with `keepsSessions` false
- **THEN** the window offers no ordinary update

#### Scenario: Prerelease progression
- **WHEN** a host daemon runs `0.2.0-dev.9` and the hub runs `0.2.0-dev.10`, with all other update requirements met
- **THEN** the hub reports an available update

#### Scenario: Stable release follows its prereleases
- **WHEN** a host daemon runs `0.2.0-dev.1` and the hub runs `0.2.0`, with all other update requirements met
- **THEN** the hub reports an available update
- **AND** a hub of `0.2.0-dev.1` reports none for a daemon of `0.2.0`

#### Scenario: Build metadata only
- **WHEN** the hub's and the daemon's versions differ only in build metadata
- **THEN** the hub reports no available update

### Requirement: Port Forwards Through The Hub
The server SHALL offer forwards from a local port to a port of a host:
- `POST /api/remote/forwards` with `{"target", "port"}` opens one and answers `{"target", "remote", "local", "url", "state"}` once the forward is verified;
- `GET /api/remote/forwards` lists all active forwards with their `state`, `up` or `down`; saved inactive entries SHALL NOT appear in this array;
- `DELETE /api/remote/forwards` with `{"target", "port"}` stops one, and disables its saved preference if present.

The routes are available only as the equivalent native operations of a local main connection (see `client-connections`, Enumerated Native Requests); no listener serves these paths, and the control socket answers 404 for them. A forward SHALL run as an `ssh -N` of its own to the validated target, from local `127.0.0.1:<local>` to `localhost:<port>` on the host. For a new unsaved forward without an explicit local port, the local port SHALL be the host's port when it is free on `127.0.0.1` and `::1`, and any free loopback port otherwise. The `url` SHALL be `http://localhost:<local>`. A forward SHALL live until it is stopped or the server exits, independently of host-screen connections. When its `ssh` exits, the server SHALL keep the local port reserved and rebuild the forward on it with growing waits up to 10 seconds, reporting it as `down` meanwhile. A second request for the same target and port SHALL return the existing forward, unless it explicitly requests a different local port. More than 32 active or opening forwards for one host SHALL be refused with 409 and code `forward_limit`.

POST SHALL also accept optional `local` and `automatic`. An explicit `local`, and otherwise a saved mapping's `local`, SHALL be used exactly, with 409 `local_port_conflict` if unavailable; neither SHALL silently fall back. An existing forward requested with a different `local` SHALL yield 409 `active_mapping_conflict` without changing it. An explicit successful POST SHALL enable an existing saved rule and save a successfully selected alternate port. POST SHALL NOT create a saved rule by itself. `automatic: true` SHALL neither enable a disabled rule nor select an alternate; it SHALL return 409 `forward_paused` for a disabled rule, and combining it with `local` SHALL be invalid. Existing requests with neither optional field SHALL remain explicit actions.

#### Scenario: Forward a dev server
- **WHEN** the UI requests a forward of port 5173 on `vm1`, and a dev server listens there
- **THEN** the answer names `http://localhost:5173` when that local port is free, and requests to it reach the dev server

#### Scenario: Forward rebuilt
- **WHEN** the `ssh` of a forward is killed
- **THEN** the forward is rebuilt on the same local port

#### Scenario: Legacy caller and active list
- **WHEN** an existing caller posts only target and port for an unsaved mapping whose same-numbered local port is occupied
- **THEN** it receives an active forward on a free alternate port, repeated POST returns that forward, and GET retains its existing array and up/down shape
- **AND** no saved preference is created and inactive preferences never enter that array

#### Scenario: Explicit local port and live mismatch
- **WHEN** POST explicitly requests a local port that is occupied on either loopback family, or a different local port for an already active mapping
- **THEN** it returns 409 with `local_port_conflict` or `active_mapping_conflict` respectively, without moving a forward or changing a preference

#### Scenario: Capacity and detachment
- **WHEN** a host already has 32 active or opening forwards and its last screen is detached
- **THEN** those forwards remain, requesting an existing one succeeds, and opening a 33rd yields 409 `forward_limit`
- **AND** the limit does not prevent a forward for another host

### Requirement: Host Group Order API
`GET /api/remote/host-order` SHALL return `{version:1,order:[target,…]}`. `PUT` SHALL accept `{order:[target,…]}`, replace the stored order and return it, storing a repeated target once; an invalid target, a value that is not a list of strings, more than 256 targets or an unknown field SHALL be rejected without a change. Both are available only as the equivalent native operations of a local main connection (see `client-connections`, Enumerated Native Requests); no listener serves these paths, and the control socket answers 404 for them, and neither SHALL connect to a host.

#### Scenario: Order replaced
- **WHEN** the UI sends `PUT /api/remote/host-order` with `{"order":["vm2","vm1","vm2"]}`
- **THEN** the response and a later `GET` return `{"version":1,"order":["vm2","vm1"]}`

#### Scenario: Invalid order
- **WHEN** the UI sends an order containing ` vm1` with surrounding whitespace
- **THEN** the request is rejected with 400 and the stored order is unchanged

## REMOVED Requirements

### Requirement: Host Screen Directory Cache
**Reason**: Restated as Host Screen Directories without the web window, which is deleted; the stored directory and its rules are unchanged.
