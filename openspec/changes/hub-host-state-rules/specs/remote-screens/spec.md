## MODIFIED Requirements

### Requirement: Remote Screen Hub Endpoint
An explicit browser gateway SHALL accept `/ws/remote` upgrades with `ticket` and `target`, authenticated like `/ws`. An authorized native main client SHALL be able to open the equivalent host stream through its assigned service. For either client stream the hub SHALL:
1. validate the target as `splitty connect` does;
2. make sure the host has a server and a running daemon, as `splitty connect` does (see `cli`, Remote Connect Tunnel, and `remote-hosts`), without a TCP port hint and requiring native transport compatibility;
3. start `splitty remote-bridge --transport native-v1` on the host over SSH, using the installed server's absolute path when it installed one or found one installed;
4. send `remote.ready`;
5. relay every application message between the client stream and the host daemon unchanged, text as text and binary as binary, in order.

Before the relay the hub SHALL send only these application text messages:
- `{"type": "remote.progress", "step", "message"}`, with `step` one of `install`, `daemon`, `started` and `bridge`;
- exactly one `{"type": "remote.ready", "target", "daemon": {"version", "protocol", "keepsSessions", "started"}, "screens": [{"id", "name", "panes"}]}`, where `keepsSessions` is the handshake's (see `remote-hosts`, Remote Handshake);
- or `{"type": "remote.error", "code", "message", "hint"?}`, after which it SHALL close the socket. `code` is one of `invalid_target`, `host_key`, `auth`, `unreachable`, `unsupported_host`, `install`, `daemon`, `incompatible`, `migration_unavailable`, `bridge` and `internal`.

After `remote.ready`, the hub SHALL send no application message of its own; transport negotiation/liveness is outside this application stream. When the connection to the host ends, it SHALL close the WebSocket with code 4001 or the native stream with the equivalent host-lost reason. When the socket closes, it SHALL end its `ssh` process. Each socket SHALL use an SSH connection of its own. Once `remote.ready` is sent, the server SHALL record the target as a recent host (see `remote-hosts`, Known Hosts).

The ready message SHALL include `update: {available, keepsSessions, toVersion?}`, the hub's update plan for the host (see Host Update); `toVersion` is the version the host's daemon gets and SHALL be present whenever `available` is true. Every hub error SHALL include `retryable`; the client SHALL use this classification when present and retain legacy classification only for older hubs. After readiness the hub SHALL validate and persist the handshake directory without interpreting relayed frames. Invalid or unwritable cache data SHALL not fail an otherwise usable connection.

#### Scenario: Connect a host without Splitty
- **WHEN** a client opens `/ws/remote?target=vm1` with a valid ticket and `vm1` has no Splitty
- **THEN** it receives `remote.progress` messages for the install and the daemon start, then `remote.ready` with `started` true, then the host daemon's `mux.welcome`

#### Scenario: Unknown host key
- **WHEN** `ssh` to the target fails host key verification
- **THEN** the client receives `remote.error` with code `host_key` and a hint to run `ssh <target>` once in a terminal, and the socket closes

#### Scenario: Host connection lost
- **WHEN** the `ssh` process of a relaying socket ends
- **THEN** the server closes the socket with code 4001

#### Scenario: Client closes the socket
- **WHEN** the client closes a relaying socket
- **THEN** the server ends that socket's `ssh` process, and the host daemon and its sessions keep running

#### Scenario: Ready from a daemon that keeps its sessions
- **WHEN** a client opens `/ws/remote` for a host whose daemon keeps its sessions
- **THEN** its `remote.ready` carries `keepsSessions` true in `daemon`

#### Scenario: Ready from an older host that can be updated
- **WHEN** a client opens `/ws/remote` for a host whose daemon keeps its sessions and runs an older version that the hub can replace
- **THEN** its `remote.ready` carries `update` with `available` and `keepsSessions` true and `toVersion` the hub's own version

#### Scenario: Authoritative retry classification
- **WHEN** a reconnect fails and the hub reports `retryable: false`
- **THEN** the client stops automatic retrying even if an older client classified that error code as transient

### Requirement: Remembered Remote Screens
The server SHALL keep the client's list of open remote screens in `remote-screens.json` in its configuration directory, written atomically with mode 0600. The file holds `{"version": 1, "screens": [{"target", "screenId", "name"?, "position"?, "closed"?}]}`, each target valid (see `access-control`, Remote Access Through SSH Tunnel), each (target, screenId) once; `closed` marks the screens of a host that was closed in a window, offered again but not reopened at start. `GET /api/remote/screens` SHALL return the list, empty when the file is missing or unreadable. `PUT /api/remote/screens` SHALL replace it and SHALL reject a body that does not have that shape; it remains for older clients. Both routes have the authentication of the other `/api` routes and are not served on the control socket.

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
- **WHEN** the native window shows screens of `vm1` and a web window shows screens of `vm2`, and each writes its screens
- **THEN** the list holds the screens of both hosts, and a `closed` mark the native window wrote for `vm3` stays

#### Scenario: A closed host is not reopened
- **WHEN** the list holds `closed` screens of `vm1` and a window starts
- **THEN** the window does not bring them back or connect `vm1` for them

#### Scenario: Older local service
- **WHEN** a client's local service does not announce `hub-host-state-v1`
- **THEN** the client brings back the remembered screens and writes no change to the list

### Requirement: Host Update
`/ws/remote` SHALL accept `update=1`; an authorized native main host connection SHALL support the corresponding explicit intent. A client SHALL request it only after the user confirms the versions and keeper-session behavior. An older native daemon may be updated only with verified instance identity, session keepers and the matching installed replacement. An eligible legacy daemon SHALL use Confirmed Legacy Daemon Migration instead. A missing replacement, newer daemon, absent session keepers or blocked legacy migration SHALL stop nothing and return an actionable failure. No update SHALL deliberately end sessions.

The intent SHALL apply to one connection attempt and be consumed before a potentially mutating command; reconnect and ambiguous completion SHALL NOT replay it. Successful replacement SHALL report update progress and require a fresh handshake for the intended native daemon. A daemon already running the intended version SHALL remain running. Migration offers SHALL be structured metadata rather than inferred from message text; blocked legacy migration SHALL use `migration_unavailable` and SHALL NOT offer an ordinary update that cannot succeed.

The hub SHALL report ordinary update availability only when the daemon version has lower SemVer precedence than its own version, accepting one optional `v` prefix and ignoring build metadata. Unknown versions SHALL not imply an available update. Availability SHALL also require the verified instance, session preservation and matching prepared replacement required by the shared replacement plan. The window SHALL use this metadata, including the keeper behavior, rather than comparing versions itself. This metadata SHALL never bypass the replacement checks above.

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

### Requirement: Host Screen Directory Cache
The local server SHALL persist the last observed screen directory per host separately from the remembered open-screen list. `GET /api/remote/catalog` SHALL return `{version:1,hosts:[{target,lastSeen,screens:[{id,name?,panes}]}]}`. `PUT /api/remote/catalog` SHALL accept `{target,screens}`, replace only that host's directory and return its stored entry with `lastSeen` in Unix seconds. A successful empty list SHALL replace a previous nonempty one. The catalog routes remain for older clients; current clients report through `hub.catalog.observe` and read `remote.directory.get`. These APIs SHALL neither connect nor send commands to a remote host.

Hidden hosts SHALL be excluded from catalog reads. An update for a hidden host SHALL return 409 without storing its observation.

The file `remote-catalog.json` SHALL be written atomically with mode 0600 in the private configuration directory. It SHALL keep at most 20 host entries, each with at most 200 screens. Targets SHALL be valid and canonical, screen ids unique per host with 1–256 characters, names at most 200 characters, and pane counts nonnegative integers. Invalid writes and unknown fields SHALL be rejected without modifying the saved directory. Missing or corrupt files SHALL read as empty. The endpoints SHALL require the ordinary API authentication and origin checks and SHALL NOT be exposed on the control socket.

After successful host readiness the hub SHALL store the handshake screen directory through the same validation and hidden-host rules. A local client MAY report later directory observations using `hub.catalog.observe` with `{target,screens}`; admitted observations SHALL be processed in connection order and SHALL never connect a host. Persistence and invalidation SHALL be handled by the hub; legacy PUT remains available and follows the same rule.

An observation lists the host's screens as they are now. A hub announcing `hub-host-state-v1` SHALL accept a screen without `panes`: it SHALL keep the pane count it has stored for that screen, and SHALL leave a screen it has stored no count for out of the directory until an observation gives one. A client that knows pane counts SHALL send them.

While a host is connected and its live catalog of the current connection is complete, the native workspace SHALL report that catalog, without pane counts, whenever it differs from the last report for the connection: new screens, renames and screens that left the host, such as a screen terminated from the window once the host confirmed it, or one that closed with its last pane. A later connection to the host SHALL therefore not list such a screen while it is being established. A terminate the host has not confirmed or has refused, and a detached screen, SHALL leave the stored directory unchanged. Without `hub-host-state-v1` the native workspace SHALL report nothing.

A web window SHALL keep no host directory of its own: with a hub that does not announce `hub-host-decisions-v1` it shows no last observed screens and writes no catalog.

#### Scenario: Host directories survive a local restart
- **WHEN** directories for `vm1` and `vm2` are saved and `vm1` is updated
- **THEN** a subsequent read after server reconstruction returns the updated `vm1` directory and the unchanged `vm2` directory

#### Scenario: Empty directory
- **WHEN** the host reports no screens after previously reporting two
- **THEN** the next saved directory for that host is empty

#### Scenario: Invalid or unauthorized update
- **WHEN** a request is unauthenticated, cross-origin, made over the control socket, or contains duplicate screen ids
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

#### Scenario: Web window with an older hub
- **WHEN** a web window's local server does not announce `hub-host-decisions-v1`
- **THEN** the window shows no last observed screens and sends no catalog write

