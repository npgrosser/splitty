# Spec Delta

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


The ready message SHALL include `update: {available, keepsSessions}`. Every hub error SHALL include `retryable`; the client SHALL use this classification when present and retain legacy classification only for older hubs. After readiness the hub SHALL validate and persist the handshake directory without interpreting relayed frames. Invalid or unwritable cache data SHALL not fail an otherwise usable connection.

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

#### Scenario: Authoritative retry classification
- **WHEN** a reconnect fails and the hub reports `retryable: false`
- **THEN** the client stops automatic retrying even if an older client classified that error code as transient


### Requirement: Host Update
`/ws/remote` SHALL accept `update=1`; an authorized native main host connection SHALL support the corresponding explicit intent. A client SHALL request it only after the user confirms the versions and keeper-session behavior. An older native daemon may be updated only with verified instance identity, session keepers and the matching installed replacement. An eligible legacy daemon SHALL use Confirmed Legacy Daemon Migration instead. A missing replacement, newer daemon, absent session keepers or blocked legacy migration SHALL stop nothing and return an actionable failure. No update SHALL deliberately end sessions.

The intent SHALL apply to one connection attempt and be consumed before a potentially mutating command; reconnect and ambiguous completion SHALL NOT replay it. Successful replacement SHALL report update progress and require a fresh handshake for the intended native daemon. A daemon already running the intended version SHALL remain running. Migration offers SHALL be structured metadata rather than inferred from message text; blocked legacy migration SHALL use `migration_unavailable` and SHALL NOT offer an ordinary update that cannot succeed.


The hub SHALL report ordinary update availability only when the daemon version has lower SemVer precedence than its own version, accepting one optional `v` prefix and ignoring build metadata. Unknown versions SHALL not imply an available update. Availability SHALL also require the verified instance, session preservation and matching prepared replacement required by the shared replacement plan. The window SHALL use this metadata, including the keeper behavior, rather than comparing versions itself. This metadata SHALL never bypass the replacement checks above.

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


### Requirement: Host Screen Directory Cache
The local server SHALL persist the last observed screen directory per host separately from the remembered open-screen list. `GET /api/remote/catalog` SHALL return `{version:1,hosts:[{target,lastSeen,screens:[{id,name?,panes}]}]}`. `PUT /api/remote/catalog` SHALL accept `{target,screens}`, replace only that host's directory and return its stored entry with `lastSeen` in Unix seconds. A successful empty list SHALL replace a previous nonempty one. These APIs SHALL neither connect nor send commands to a remote host.

Hidden hosts SHALL be excluded from catalog reads. An update for a hidden host SHALL return 409 without storing its observation.

The file `remote-catalog.json` SHALL be written atomically with mode 0600 in the private configuration directory. It SHALL keep at most 20 host entries, each with at most 200 screens. Targets SHALL be valid and canonical, screen ids unique per host with 1–256 characters, names at most 200 characters, and pane counts nonnegative integers. Invalid writes and unknown fields SHALL be rejected without modifying the saved directory. Missing or corrupt files SHALL read as empty. The endpoints SHALL require the ordinary API authentication and origin checks and SHALL NOT be exposed on the control socket.


After successful host readiness the hub SHALL store the handshake screen directory through the same validation and hidden-host rules. A local client MAY report later directory observations using `hub.catalog.observe` with `{target,screens}`; admitted observations SHALL be processed in connection order and SHALL never connect a host. Persistence and invalidation SHALL be handled by the hub; legacy PUT remains available.

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


## ADDED Requirements

### Requirement: Merged Host Directory
`GET /api/remote/directory` and native `remote.directory.get` SHALL return a deduplicated directory and saved host order using the shapes in `docs/protocol.ts`. It SHALL combine known hosts, aliases, cached screens, and hosts of saved or active forwards and reverse forwards, excluding hidden hosts and applying saved group order. Reads SHALL not connect hosts. Native remote-window and restricted callers SHALL not gain this operation.

#### Scenario: Sources merged without connection
- **WHEN** one host appears in SSH configuration, cached screens and saved forwards and another appears only in reverse forwards
- **THEN** both visible hosts appear once, with aliases and cached screen data preserved, in saved group order, and no SSH process starts

#### Scenario: Forgotten host still has a tunnel
- **WHEN** a hidden host retains an active forward
- **THEN** the passive directory excludes it and the forward keeps running

#### Scenario: Restricted caller
- **WHEN** a remote-window native connection or owner/grant control socket requests the merged local directory
- **THEN** the request is rejected and no metadata is exposed

### Requirement: Hub Changes On The Local Connection
A hub supporting `hub-host-decisions-v1` SHALL notify local clients with `hub.changed` after host metadata, active or saved forwards, restoration errors, or supervised up/down state changes. Notifications MAY coalesce, but a subscribed client SHALL be able to refresh the final state. They SHALL not be injected into host relays. Clients SHALL refresh passively on notification and reconnect, without polling a supporting hub or starting tunnels.

#### Scenario: Another window edits a mapping
- **WHEN** one window saves, pauses or forgets a mapping
- **THEN** other subscribed local windows refresh the resulting state without waiting for a poll

#### Scenario: A forward loses its SSH process
- **WHEN** a forward or reverse forward becomes down and is subsequently rebuilt
- **THEN** local clients are notified of changes and passive reads expose the current state

#### Scenario: Notification during a local mutation
- **WHEN** an invalidation arrives while the window is changing a mapping
- **THEN** a refresh after the mutation reads the latest state and no stale response resurrects removed data

#### Scenario: Reconnect and old hubs
- **WHEN** a client reconnects to the local hub
- **THEN** it refreshes current metadata and forwards; polling is used only if the hub lacks the capability
