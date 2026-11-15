## ADDED Requirements

### Requirement: Host Metadata API
`PUT /api/remote/host-preferences` SHALL accept `{target,alias?:string|null,hidden?:false}`, require at least one field besides target, preserve omitted fields and return the stored row. `DELETE /api/remote/hosts` SHALL accept `{target}`, forget that host as described in `remote-hosts`, and return 204. Windows read the known hosts with their aliases and hidden state through `remote.directory.get` (see Merged Host Directory). These operations are available only as the equivalent native operations of a local main connection (see `client-connections`, Enumerated Native Requests); no listener serves these paths, and the control socket answers 404 for them. Reading or changing host metadata SHALL NOT connect to a host.

#### Scenario: Invalid preference update
- **WHEN** an update contains an invalid target, overlong alias, controls or unknown fields
- **THEN** it is rejected without modifying saved metadata

#### Scenario: Unauthorized metadata change
- **WHEN** a preference or removal request is made over the control socket, a grant socket or a connection that is not a local main connection
- **THEN** it is rejected without changing host metadata or contacting a host

#### Scenario: Removed host reads
- **WHEN** a native main client requests `host.list`, `hostPreferences.get` or `hostOrder.get`
- **THEN** the request is refused as an unsupported operation and the connection stays open

## MODIFIED Requirements

### Requirement: Host Group Order API
`PUT /api/remote/host-order` SHALL accept `{order:[target,…]}`, replace the stored order and return it as `{version:1,order:[target,…]}`, storing a repeated target once; an invalid target, a value that is not a list of strings, more than 256 targets or an unknown field SHALL be rejected without a change. Windows read the stored order through `remote.directory.get` (see Merged Host Directory). The update is available only as the equivalent native operation of a local main connection (see `client-connections`, Enumerated Native Requests); no listener serves this path, the control socket answers 404 for it, and it SHALL NOT connect to a host.

#### Scenario: Order replaced
- **WHEN** the UI sends `PUT /api/remote/host-order` with `{"order":["vm2","vm1","vm2"]}`
- **THEN** the response is `{"version":1,"order":["vm2","vm1"]}` and a later `remote.directory.get` carries the order `vm2`, `vm1`

#### Scenario: Invalid order
- **WHEN** the UI sends an order containing ` vm1` with surrounding whitespace
- **THEN** the request is rejected with 400 and the stored order is unchanged

### Requirement: Host Screen Directories
The local server SHALL persist the last observed screen directory per host separately from the remembered open-screen list, as `{version:1,hosts:[{target,lastSeen,screens:[{id,name?,panes}]}]}` with `lastSeen` in Unix seconds. An observation SHALL replace only that host's directory; a successful empty list SHALL replace a previous nonempty one. Clients report observations through `hub.catalog.observe` and read the directory with `remote.directory.get` (see Merged Host Directory); no operation reads or writes the stored directory as such. Neither SHALL connect or send commands to a remote host.

Hidden hosts SHALL be excluded from directory reads. An observation for a hidden host SHALL be refused without storing it.

The file `remote-catalog.json` SHALL be written atomically with mode 0600 in the private configuration directory. It SHALL keep at most 20 host entries, each with at most 200 screens. Targets SHALL be valid and canonical, screen ids unique per host with 1–256 characters, names at most 200 characters, and pane counts nonnegative integers. Invalid observations and unknown fields SHALL be rejected without modifying the saved directory. Missing or corrupt files SHALL read as empty. Observations are accepted only from a local main connection (see `client-connections`, Enumerated Native Requests).

After successful host readiness the hub SHALL store the handshake screen directory through the same validation and hidden-host rules. A local client MAY report later directory observations using `hub.catalog.observe` with `{target,screens}`; admitted observations SHALL be processed in connection order and SHALL never connect a host. Persistence and invalidation SHALL be handled by the hub.

An observation lists the host's screens as they are now. A hub announcing `hub-host-state-v1` SHALL accept a screen without `panes`: it SHALL keep the pane count it has stored for that screen, and SHALL leave a screen it has stored no count for out of the directory until an observation gives one. A client that knows pane counts SHALL send them.

While a host is connected and its live catalog of the current connection is complete, the native workspace SHALL report that catalog, without pane counts, whenever it differs from the last report for the connection: new screens, renames and screens that left the host, such as a screen terminated from the window once the host confirmed it, or one that closed with its last pane. A later connection to the host SHALL therefore not list such a screen while it is being established. A terminate the host has not confirmed or has refused, and a detached screen, SHALL leave the stored directory unchanged. Without `hub-host-state-v1` the native workspace SHALL report nothing.

#### Scenario: Host directories survive a local restart
- **WHEN** directories for `vm1` and `vm2` are observed and `vm1` is observed again with other screens
- **THEN** `remote.directory.get` after server reconstruction returns the updated `vm1` directory and the unchanged `vm2` directory

#### Scenario: Empty directory
- **WHEN** the host reports no screens after previously reporting two
- **THEN** the next saved directory for that host is empty

#### Scenario: Invalid or unauthorized update
- **WHEN** an observation arrives over the control socket or a connection that is not a local main connection, or contains duplicate screen ids
- **THEN** the observation is rejected and no saved directory or remote host is changed

#### Scenario: Late observation after removal
- **WHEN** an already attached host publishes its screen list after that host was forgotten
- **THEN** it remains hidden and no cached directory is recreated

#### Scenario: Catalog without a window writer
- **WHEN** host readiness lists screens and the window sends no observation
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

## REMOVED Requirements

### Requirement: Known Hosts API
**Reason**: `GET /api/remote/hosts` and `GET /api/remote/host-preferences` were read only by the web client; windows read hosts, aliases and hidden state from `remote.directory.get`. The writes move to Host Metadata API.
**Migration**: Read `remote.directory.get`.
