# remote-screens Specification

## Purpose
Defines remote screens: screens of a remote host shown next to local screens in one window, through the local server acting as a transport hub to the host's daemon. Covers the hub endpoint and its messages, the protocol check, remote screens as references, per-host connection state, collapse versus terminate, the reopen offer and the known-hosts API.

## Requirements

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

### Requirement: Remote Screen Protocol Check
Before it starts the bridge, the server SHALL require a supported native transport and compare the host daemon's protocol version from the handshake with its own. It SHALL accept the same version and the one before (see `remote-hosts`, Protocol Version). A missing or unsupported native transport SHALL also be incompatible. Otherwise it SHALL send `remote.error` with code `incompatible`, the daemon's `version`, `protocol` and `keepsSessions`, and its `screens` as in `remote.ready`, and close the socket. Ordinary connection SHALL NOT stop or replace the host's daemon. A positively eligible legacy daemon SHALL additionally report a structured migration offer; a blocked legacy daemon SHALL report `migration_unavailable` with its reason.

#### Scenario: Newer host
- **WHEN** the host daemon's protocol version is higher than the server's
- **THEN** the client receives `remote.error` with code `incompatible` naming the daemon's version, and the daemon keeps running

#### Scenario: What an update would end
- **WHEN** the host daemon is incompatible and runs two screens
- **THEN** the `remote.error` lists both screens with their numbers of panes

### Requirement: Known Hosts API
`GET /api/remote/hosts` SHALL return the known hosts of the user the server runs as (see `remote-hosts`, Known Hosts) as a JSON array of `{ "target", "source", "lastConnected"?, "alias"? }`, with the authentication of the other `/api` routes.

#### Scenario: Hosts for the picker
- **WHEN** the UI requests `/api/remote/hosts` and `vm1` is a recent host
- **THEN** the response lists `vm1` first with source `recent`

`GET /api/remote/host-preferences` SHALL return `{version:1,hosts:[{target,alias?,hidden?}]}`. `PUT` SHALL accept `{target,alias?:string|null,hidden?:false}`, require at least one field besides target, preserve omitted fields and return the stored row. `DELETE /api/remote/hosts` SHALL accept `{target}`, forget that host as described in `remote-hosts`, and return 204. These endpoints SHALL require the ordinary API authentication and origin checks and SHALL NOT be exposed on the control socket. Reading or changing these metadata endpoints SHALL NOT connect to a host.

#### Scenario: Invalid preference update
- **WHEN** an update contains an invalid target, overlong alias, controls or unknown fields
- **THEN** it is rejected without modifying saved metadata

#### Scenario: Unauthorized metadata change
- **WHEN** a preference or removal request is unauthenticated, cross-origin or made over the control socket
- **THEN** it is rejected without changing host metadata or contacting a host

### Requirement: Remote Screens In The Window
A client SHALL show local screens and remote screens grouped by their host in the screen list. Local screens SHALL come first; the host groups SHALL follow in the order the user arranged them (see `remote-hosts`, Host Group Order), and hosts the user did not arrange SHALL follow the arranged ones in the order their screens first appear. The user SHALL be able to move a host group, with its screens, above or below another host group by dragging its caption; the local screens SHALL NOT be movable as a group, and a screen SHALL NOT move to another host's group. It SHALL hold one hub connection per host with open screens or an explicit request to browse that host, and SHALL speak the mux and session protocols over it with that host's daemon. A remote screen SHALL be a reference to a screen of a host. Its layout and sessions are the host's, and every op on it SHALL go to that host. Local settings (theme, font, keybindings) SHALL apply to remote screens. A new tab or split in a remote screen SHALL send no local shell profile. A remote screen's group SHALL identify the host and its connection state. Screen switching and shortcut numbering SHALL follow the grouped display order.

#### Scenario: New screen on a host
- **WHEN** the user creates a screen on `vm1`
- **THEN** it appears under `vm1`, and shows connection progress until its first pane runs on `vm1`

#### Scenario: Split in a remote pane
- **WHEN** the user runs `splitty split` in a pane of a remote screen
- **THEN** the pane is split on the host, and the local workspace is unchanged

#### Scenario: Grouped navigation
- **WHEN** local screens and screens from multiple hosts are open
- **THEN** screen navigation and shortcut numbers follow the same host-group order shown in the drawer

#### Scenario: Host group moved
- **WHEN** the screen list shows local screens, then `vm1`, then `vm2`, and the user drags the caption of `vm2` above `vm1`
- **THEN** the list shows local screens, then `vm2` with its screens, then `vm1` with its screens
- **AND** the shortcut numbers of the screens follow that order
- **AND** the order is the same after the client restarts

### Requirement: Remote Screen Reconnect
When a host's connection drops after `remote.ready`, the client SHALL mark that host's screens as reconnecting, keep them from taking input, and reconnect with a delay that grows to 10 seconds, with a fresh ticket for every browser attempt or a freshly authorized native stream for a native client. A failed ticket request for a host SHALL NOT be reported as the local session's authentication error. No queued terminal input or mutation from the lost connection SHALL be replayed. After a successful reconnect, the client SHALL resync the host's screens and re-attach their sessions. When the connection fails with an error code that retrying cannot fix, the client SHALL stop retrying and show the error with its hint and a retry. When a host cannot be reached again, its screens SHALL be shown as offline with a retry.

#### Scenario: Network drop
- **WHEN** the connection to `vm1` drops and the host is reachable again 20 seconds later
- **THEN** `vm1`'s screens show "reconnecting…" and then their content again, with the output written in the meantime

### Requirement: Collapse And Terminate
Closing a remote screen SHALL detach it without a question: the screen leaves the open list, its panes are detached, and its sessions keep running on the host. Terminating a remote screen SHALL be a separate action that ends its panes and their processes on the host (`close_screen`), and the client SHALL ask for confirmation before the first terminate. Quitting the app or closing the window SHALL only detach remote screens, and remote sessions SHALL NOT count as running processes that block quitting.

#### Scenario: Collapse
- **WHEN** the user closes a remote screen with the default action
- **THEN** the screen leaves the open list, and its processes keep running on the host

#### Scenario: Terminate
- **WHEN** the user terminates a remote screen and confirms
- **THEN** the host ends the screen's panes and their processes

### Requirement: Reopen Offer
When the user creates a screen on a host whose `remote.ready` lists screens that the window does not show, the client SHALL first offer to open those screens, each with its name and number of panes, or to create a new screen. Choosing New Screen explicitly within a host directory SHALL count as choosing creation and SHALL skip that offer.

#### Scenario: Screens already running
- **WHEN** the user picks `vm1` through the general host picker and `vm1` runs the screens `api` and `infra`, which the window does not show
- **THEN** the client offers to open `api` and `infra`, and opening `api` shows it with its running panes

#### Scenario: Explicit creation in a host directory
- **WHEN** the user chooses New Screen within a host group which also lists unopened screens
- **THEN** a new screen is created on that host, and its existing screens remain unchanged

### Requirement: Remembered Remote Screens
The server SHALL keep the client's list of open remote screens in `remote-screens.json` in its configuration directory, written atomically with mode 0600. The file holds `{"version": 1, "screens": [{"target", "screenId", "name"?, "position"?}]}`, each target valid (see `access-control`, Remote Access Through SSH Tunnel), each (target, screenId) once. `GET /api/remote/screens` SHALL return the list, empty when the file is missing or unreadable. `PUT /api/remote/screens` SHALL replace it and SHALL reject a body that does not have that shape. Both routes have the authentication of the other `/api` routes and are not served on the control socket.

A client SHALL write the list after every change to its remote screens. On start, it SHALL restore the saved references and order, displayed within their host groups as described by Remote Screens In The Window. The saved position format SHALL remain compatible with existing mixed lists. It SHALL connect a remembered screen's host only when the screen becomes visible or when that host is connected anyway. A remembered screen that its host no longer has SHALL be shown as missing, with an action to remove it.

#### Scenario: Restart
- **WHEN** the window shows a remote screen of `vm1` and the app is restarted
- **THEN** the screen is back in its host group in the remembered relative order, and it connects when the user opens it

#### Scenario: Unreachable host at start
- **WHEN** a remembered screen's host is unreachable when the app starts
- **THEN** the local screens are usable at once, and the remote screen shows its host as offline only when it is opened

### Requirement: Screens On Hosts
The client SHALL retain host groups for recently connected hosts even when they have no open screens and are disconnected. It SHALL show the last observed names and pane counts of unopened screens, marking disconnected observations as last seen rather than live. Expanding a disconnected host SHALL connect and refresh its directory without creating a screen. Opening an existing screen SHALL add a reference to it; it SHALL NOT recreate a screen missing on the host. The group SHALL also offer creation of a new screen. Reading the directory at startup SHALL NOT connect its hosts.

#### Scenario: Find a collapsed screen
- **WHEN** the user detaches a screen on `vm1` while another screen there remains open
- **THEN** the detached screen is listed in `vm1`'s group with its name and pane count, and opening it attaches to its existing panes

#### Scenario: Detach the last screen
- **WHEN** the user detaches the last open screen on `vm1` and its connection is released
- **THEN** `vm1` remains in the host list with its last observed screens, and expanding it refreshes them without creating a screen

#### Scenario: Browse without opening a screen
- **WHEN** the user expands `vm1` with no open screen there
- **THEN** its connection lists the host's screens, opening one attaches to its running panes, and folding the group without opening any screen releases the connection

#### Scenario: Restart without connecting
- **WHEN** the client restarts with saved host directories and no active remote screen
- **THEN** host groups and their last observed screens remain available without opening SSH connections

#### Scenario: A cached screen no longer exists
- **WHEN** the user opens a cached screen which the host has since removed
- **THEN** the client reports the screen as missing and does not recreate it

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

### Requirement: Port Forwards Through The Hub
The server SHALL offer forwards from a local port to a port of a host:
- `POST /api/remote/forwards` with `{"target", "port"}` opens one and answers `{"target", "remote", "local", "url", "state"}` once the forward is verified;
- `GET /api/remote/forwards` lists all active forwards with their `state`, `up` or `down`; saved inactive entries SHALL NOT appear in this array;
- `DELETE /api/remote/forwards` with `{"target", "port"}` stops one, and disables its saved preference if present.

The routes have the authentication of the other `/api` routes. A forward SHALL run as an `ssh -N` of its own to the validated target, from local `127.0.0.1:<local>` to `localhost:<port>` on the host. For a new unsaved forward without an explicit local port, the local port SHALL be the host's port when it is free on `127.0.0.1` and `::1`, and any free loopback port otherwise. The `url` SHALL be `http://localhost:<local>`. A forward SHALL live until it is stopped or the server exits, independently of host-screen connections. When its `ssh` exits, the server SHALL keep the local port reserved and rebuild the forward on it with growing waits up to 10 seconds, reporting it as `down` meanwhile. A second request for the same target and port SHALL return the existing forward, unless it explicitly requests a different local port. More than 32 active or opening forwards for one host SHALL be refused with 409 and code `forward_limit`.

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

### Requirement: Detected Ports In The Window
For each connected host, the client SHALL list the ports of its last `ports.changed`. Each port shows its process, and either its local address with "Open in browser" and "Stop" when it is forwarded, or "Forward" when it is not. Saved preferences and active forwards SHALL remain available independently of discovery; an absent discovery entry SHALL NOT imply that a manually entered service is not listening. The client SHALL join entries by target and remote port and distinguish a paused saved rule from a failed restore and from an active forward being rebuilt. A port that appears on a connected host SHALL be offered in the host's port list, not announced in a notice. A URL whose host is `localhost`, `127.0.0.1`, `0.0.0.0` or `[::1]`, opened from a remote pane's output, SHALL open the forwarded local address with the same scheme, path, query and fragment. When that port is not forwarded yet, the client SHALL forward it first, since the click counts as the user's consent, including explicit resumption of a paused saved rule. A saved-port conflict SHALL be shown without opening an unrelated local service or silently remapping the URL. Local panes SHALL keep opening such URLs as they are.

#### Scenario: Link in a remote pane
- **WHEN** a remote pane on `vm1` prints `http://localhost:5173/app` and the user opens it, with port 5173 not forwarded
- **THEN** the client forwards port 5173 of `vm1` and opens `http://localhost:<local>/app` in the browser

#### Scenario: New port notice
- **WHEN** `vite` on `vm1` starts listening on port 5173 while the window is connected to `vm1`
- **THEN** no notice appears, and `vm1`'s port list offers 5173 with `vite` to forward and open

#### Scenario: Saved ports without discovery
- **WHEN** saved frontend and API mappings exist but the host has sent no discovery list or no longer lists those ports
- **THEN** both mappings remain available with their saved local ports and preferences, joined with any active forwards without duplicates

#### Scenario: First list and explicit paused link
- **WHEN** a reconnected host sends its initial port list containing a paused saved mapping
- **THEN** it is listed without being resumed automatically
- **AND** a later explicit link click resumes using the saved local port, preserving the link's scheme, path, query and fragment, or reports the saved-port conflict without opening that URL

### Requirement: Automatic Port Forwarding
The setting `remote.autoForwardPorts` SHALL be off by default. When it is on, the client SHALL forward every port that appears on a connected host without a click and without a notice, except a disabled saved mapping. Automatic requests SHALL use `automatic: true`, preserve saved local ports and respect the server's `forward_paused` refusal. They SHALL NOT create saved rules, resume a stopped rule or silently remap a saved-port conflict.

#### Scenario: Default
- **WHEN** a port appears on a connected host and the setting has its default value
- **THEN** no forward is opened until the user asks for one

#### Scenario: Automatic forwarding respects a pause
- **WHEN** automatic forwarding is enabled and a port reappears after its saved mapping was stopped, including a request from a window with stale state
- **THEN** the saved mapping remains disabled and no forward is opened for it automatically

#### Scenario: Automatic forwarding uses saved port
- **WHEN** automatic forwarding attempts an enabled saved mapping whose local port is occupied
- **THEN** the attempt reports `local_port_conflict` and preserves the mapping, while an unrelated new unsaved port can still be forwarded without being remembered

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

### Requirement: Screen Default Names Per Host
The client SHALL assign new default screen names as `Screen N`, using the smallest unused positive integer independently for local screens and for each remote host. Known unopened screens on that same host SHALL reserve their names. Existing server, restored and user-selected names SHALL remain unchanged. A pending default name that collides with a newly discovered screen on its host SHALL be adjusted before creation; names on another host SHALL NOT affect it.

#### Scenario: Independent numbering
- **WHEN** local screens already use Screen 1 through Screen 3 and a new screen is created on an otherwise empty remote host
- **THEN** the new remote screen is named Screen 1

#### Scenario: Existing unopened screen
- **WHEN** a host already has an unopened Screen 1 and a new screen is created there
- **THEN** the new screen takes the smallest available number on that host

#### Scenario: Existing names survive
- **WHEN** a workspace restores a screen named Default or a user-selected name
- **THEN** that name is preserved

### Requirement: Host Group Order API
`GET /api/remote/host-order` SHALL return `{version:1,order:[target,…]}`. `PUT` SHALL accept `{order:[target,…]}`, replace the stored order and return it, storing a repeated target once; an invalid target, a value that is not a list of strings, more than 256 targets or an unknown field SHALL be rejected without a change. Both SHALL require the ordinary API authentication and origin checks, SHALL NOT be on the control socket and SHALL NOT connect to a host.

#### Scenario: Order replaced
- **WHEN** the UI sends `PUT /api/remote/host-order` with `{"order":["vm2","vm1","vm2"]}`
- **THEN** the response and a later `GET` return `{"version":1,"order":["vm2","vm1"]}`

#### Scenario: Invalid order
- **WHEN** the UI sends an order containing ` vm1` with surrounding whitespace
- **THEN** the request is rejected with 400 and the stored order is unchanged

### Requirement: Saved Forward Preferences API
`/api/remote/forward-preferences` SHALL provide GET, PUT and DELETE for mappings keyed by validated target and remote port, using the wire shapes in `docs/protocol.ts`. GET SHALL return `{version:1,forwards:[{target,remote,local,label,enabled,lastError?}]}`, ordered by target and remote port. PUT SHALL merge only supplied fields for one mapping; DELETE SHALL forget it. These metadata operations SHALL neither connect a host nor open or rebuild a forward.

#### Scenario: Save a manual mapping
- **WHEN** PUT receives `{target:"vm1",port:5173,local:15173,label:"Frontend"}` without a running forward or discovery
- **THEN** it returns the saved entry with remote 5173, local 15173 and enabled true, and starts no SSH process
- **AND** an omitted local port on creation defaults to the remote port, and an omitted label serializes as an empty string

#### Scenario: Remember an active mapping
- **WHEN** PUT creates a preference for an active forward and omits local
- **THEN** it saves the forward's actual local port and label, and keeps the live tunnel unchanged
- **AND** an explicit local different from that active port is rejected with 409 `active_mapping_conflict`

#### Scenario: Edit without resuming
- **WHEN** PUT supplies only a label for an existing paused saved mapping
- **THEN** the local port and disabled state are preserved and no forward starts
- **AND** omitted label preserves its value, while an empty string clears it

### Requirement: Durable Forward Preferences
The local server SHALL save `{version:1,forwards:[{target,remote,local,label,enabled}]}` in `remote-forwards.json` in its private configuration directory with atomic, owner-only writes. Successful mutations SHALL survive reconstruction and preserve unrelated entries. Missing, unreadable or invalid documents SHALL yield an empty list without starting SSH or rewriting the file. Runtime failures SHALL NOT be persisted.

#### Scenario: Restart and independent updates
- **WHEN** two windows save different mappings, including frontend and API ports on the same host, and the local server is reconstructed
- **THEN** both mappings, labels, local ports and enabled states remain saved, with no forward opened merely by construction or reads

#### Scenario: Missing or corrupt preferences
- **WHEN** the file is absent, unreadable, malformed, has an unsupported version or fails saved-data validation
- **THEN** preferences read as empty, no host connects, and reading does not replace the file

#### Scenario: Write failure
- **WHEN** a preference mutation, durable Stop or successful-open preference update cannot be written
- **THEN** the request returns 500 `storage_failure`, previous saved state remains and the client does not claim the change succeeded
- **AND** an uncommitted newly opened forward is cleaned up, while Stop does not claim a persistent pause or remove the existing forward before saving the disabled state

### Requirement: Lazy Saved Forward Restoration
After successfully sending `remote.ready` for a user-requested host connection, the hub SHALL attempt that target's enabled saved mappings without waiting for discovery or blocking screen relay. Repeated connections SHALL reuse active or opening forwards. Startup, authentication, reads and preference edits SHALL never trigger restoration. Disabled rules SHALL remain stopped across reconnects and server restarts.

#### Scenario: Connect only one saved host
- **WHEN** the user opens a screen or explicitly browses `vm1`, reaching `remote.ready`, and enabled mappings also exist for `vm2`
- **THEN** enabled mappings for `vm1` are restored at their saved ports without waiting for detected listeners
- **AND** `vm2` stays unconnected and disabled mappings stay stopped

#### Scenario: Failed or unauthorized connection
- **WHEN** a host connection is rejected or fails before `remote.ready`
- **THEN** no saved forwards are restored for it and local screens remain usable

#### Scenario: Multiple windows connect
- **WHEN** multiple windows connect the same host and request the same saved mapping concurrently
- **THEN** at most one opening attempt or supervised forward exists for that mapping at a time, and other mappings can progress independently

### Requirement: Saved Port Conflicts And Retry
Saved mappings SHALL retain their local port on restore and retry. A port occupied on `127.0.0.1` or `::1` SHALL yield `local_port_conflict`; uncertain availability SHALL NOT count as free. An initial restore failure SHALL leave the saved rule available with runtime `lastError` in GET preferences. A later successful host connection or explicit Forward/Resume SHALL retry; reads SHALL NOT. Only an explicit successful alternate-port action SHALL change the saved port.

#### Scenario: IPv4 or IPv6 collision
- **WHEN** another listener owns a saved port on either IPv4 or IPv6 loopback during restore
- **THEN** that mapping stays inactive with `lastError.code: "local_port_conflict"`, its saved port stays unchanged, the other listener is untouched and other saved ports can restore

#### Scenario: Choose an alternate or retry the original
- **WHEN** the user requests a different numeric local port explicitly, or retries after the original port becomes free
- **THEN** success opens exactly the selected port and updates the saved local port and enabled state
- **AND** failure preserves the previous saved port and enabled state

#### Scenario: Query failures without hub messages
- **WHEN** an enabled saved mapping fails SSH validation or forwarding after its host is ready
- **THEN** GET preferences exposes the failure's code, message and optional hint, GET active forwards omits the failed initial opening, and the hub sends no extra messages after `remote.ready`
- **AND** polling does not retry, a subsequent successful connection or explicit retry can retry, and successful opening clears the runtime error

### Requirement: Stop And Forget Saved Forwards
Stop SHALL close the forward and persist `enabled:false` when a saved rule exists; an already inactive saved rule SHALL also be stoppable. Only an explicit successful Forward/Resume, including a loopback-link click, SHALL re-enable it. Forget SHALL idempotently remove only the preference and its runtime error, leaving a running forward and its supervisor intact. The client SHALL explain that forgetting does not stop a live tunnel.

#### Scenario: Persistent Stop and explicit Resume
- **WHEN** the user stops an active or inactive saved mapping
- **THEN** DELETE active forwards returns 204, its live forward is gone and the saved rule is disabled even after a reconnect or restart
- **AND** an explicit successful POST resumes at the saved port and enables that rule again

#### Scenario: Forget a running or inactive mapping
- **WHEN** the user forgets a saved mapping, repeating the request if necessary
- **THEN** DELETE preferences returns 204, the preference is absent and any existing live forward continues on its current port
- **AND** the forgotten rule is not restored after restart; a later Stop of an unsaved absent forward retains the existing 404 response

### Requirement: Ordered Forward Mutations
Mutations SHALL have one server-defined order per target and remote port. Completed Stop or Forget SHALL NOT be undone by earlier opening work, queued restoration, stale completion or rollback. Independent mappings SHALL NOT wait for another mapping's SSH operation. Clients SHALL refresh active and saved state across windows and ignore responses superseded by a newer mutation or read.

#### Scenario: Stop races restoration
- **WHEN** Stop overlaps an opening or queued restore and returns successfully
- **THEN** the saved rule is disabled, no earlier work can leave its forward running afterward, and later automatic attempts cannot resume it

#### Scenario: Forget races restoration
- **WHEN** Forget overlaps an opening or queued restore
- **THEN** a forward ordered before Forget may remain live, but after Forget succeeds no earlier completion or rollback recreates the preference and a restore ordered afterward skips the absent rule

#### Scenario: Retry and unrelated windows
- **WHEN** one mapping's SSH attempt is slow while another window edits a different mapping, or repeats a completed save, Stop, Forget or open
- **THEN** the different mapping can progress, repeated requests do not create duplicate entries or forwards, and successful changes become visible in other windows without stale responses resurrecting removed entries

### Requirement: Manual Forwarding And Remember Consent
The client SHALL support entering a target, remote port and optional explicit local port without discovery or a live host screen. Remember SHALL be opt-in, including for manual entries, detected-port actions and terminal-link forwards. Saved labels SHALL be bounded plain text. Multiple mappings per host SHALL remain independent, and service protocol, TLS, WebSockets and application authentication SHALL pass through unchanged.

#### Scenario: Manual frontend and API
- **WHEN** the user manually forwards frontend and API ports that discovery does not list and chooses Remember for them
- **THEN** both mappings work and are saved independently with their actual local ports, ready for lazy restoration after a restart
- **AND** forwarding without Remember creates no preference

#### Scenario: Manual save while disconnected
- **WHEN** the user saves a manual mapping without asking to start its forward
- **THEN** it is listed as saved without any SSH connection and can later be started explicitly or restored when the host connects

### Requirement: Reverse Forwards Through The Hub
The server SHALL offer reverse forwards, from a port of a host to a port of this machine (shapes in `docs/protocol.ts`):
- `POST /api/remote/reverse-forwards` with `{"target", "port", "local"}` opens one, with `port` the host's port and `local` the port here, and answers `{"target", "remote", "local", "state"}` once the host's relay reported that it listens;
- `GET /api/remote/reverse-forwards` lists all active reverse forwards with their `state`, `up` or `down`; saved inactive rules SHALL NOT appear in it;
- `DELETE /api/remote/reverse-forwards` with `{"target", "port"}` stops one, and disables its saved rule if present; it answers 404 only for an absent forward without a saved rule.

A reverse forward SHALL live until it is stopped or the server exits, independently of host-screen connections. When its `ssh` exits, the server SHALL rebuild it with the same ports and growing waits up to 10 seconds, reporting it as `down` meanwhile. A second request for the same target and port SHALL return the existing reverse forward when `local` matches, and 409 `active_mapping_conflict` otherwise. A port in use on the host SHALL yield 409 `host_port_conflict`. More than 32 active or opening reverse forwards for one host SHALL be refused with 409 `forward_limit`; forwards and reverse forwards are counted separately. POST SHALL NOT accept `automatic`, and SHALL NOT create a saved rule; an explicit successful POST SHALL enable an existing saved rule and save its `local`.

#### Scenario: Share a local model server
- **WHEN** the UI requests a reverse forward of local port 11434 to port 11434 on `vm1`, and a service listens on `localhost:11434` here
- **THEN** the answer reports it `up`, and a program of the user on `vm1` reaches that service at `localhost:11434`

#### Scenario: Reverse forward rebuilt
- **WHEN** the `ssh` of a reverse forward is killed
- **THEN** it is reported `down` and rebuilt with the same ports

#### Scenario: Port taken on the host
- **WHEN** port 8080 of `vm1` is in use there and the UI requests a reverse forward to it
- **THEN** the answer is 409 with `host_port_conflict`, and no reverse forward remains

### Requirement: Saved Reverse Forwards
`/api/remote/reverse-forward-preferences` SHALL provide GET, PUT and DELETE for rules keyed by validated target and host port, separate from forward preferences: GET answers `{"version": 1, "forwards": [...]}` with `target`, `remote`, `local`, `enabled` and a runtime `lastError` when the last restore failed; PUT `{"target", "port", "local"?}` creates an enabled rule (with `local` from the request, else the active reverse forward's, else `port`) or changes `local`; DELETE removes a rule and leaves an active reverse forward running. These operations SHALL NOT contact a host or start anything. Rules SHALL persist in `remote-reverse-forwards.json`, at most 32 per host and 256 in total (409 `preference_limit` beyond).

When a host's screen connection reaches `remote.ready`, the server SHALL open the enabled rules of that host that are not active, in the background, recording a failure as `lastError`. Stop SHALL disable a saved rule durably; only an explicit POST enables it again. A restore SHALL never enable a disabled rule.

#### Scenario: Remembered share restored
- **WHEN** a rule for `vm1` port 11434 is saved and enabled, the server restarts and the user opens a screen of `vm1`
- **THEN** the reverse forward is opened after `remote.ready`, without a further request

#### Scenario: Stopped share stays paused
- **WHEN** the user stops a saved reverse forward and later connects its host again
- **THEN** it is not restored until the user resumes it explicitly

### Requirement: Sharing Local Ports In The Window
The window SHALL offer sharing a local port with a host only as the user's own action for a chosen host, with the local port and the host's port (by default the same number), and SHALL list each host's reverse forwards and saved rules with Stop, Resume and Forget, apart from that host's forwarded ports. Wherever the user starts sharing, the window SHALL state that programs running as the user on that host can use the local service, and SHALL warn more strongly for ports of administrative services (22, 2375, 2376, 3389, 5900, 6443 and 10250). Resuming a saved rule and restoring it on connect SHALL NOT ask again. Nothing a host sends, and no link in a pane, SHALL offer or start a reverse forward.

#### Scenario: Share from the Hosts panel
- **WHEN** the user shares local port 11434 with `vm1` and checks Remember
- **THEN** the window posts the reverse forward and then saves the rule, and lists it under `vm1` as shared

#### Scenario: Administrative port
- **WHEN** the user enters local port 2375 to share
- **THEN** the window shows the stronger warning before the user can share it

### Requirement: Native Host Management Surface
An authorized native main client SHALL have equivalents of the existing host, preference, order, catalog, remembered-screen and forwarding API operations through its assigned service. Reads and metadata edits SHALL retain their existing absence of connection side effects. Remote-window and restricted-agent connections SHALL not gain local hub or forwarding authority.

The experimental native workspace SHALL provide passive inspection of the assigned local service's merged host directory. Cached Screens SHALL remain observations rather than open-screen references or evidence of current host reachability. Inspection and refresh SHALL neither contact a remote host nor mutate local or remote state. A failed refresh SHALL preserve the last valid directory from the same local connection and report its failure, distinguishing it from a successfully empty directory. Results superseded by a refresh or local connection replacement SHALL not replace the current directory. Directory reads SHALL not interrupt terminal event delivery, and closing their owner SHALL cancel pending reads and complete owned cleanup.

The experimental native workspace SHALL also provide explicit target entry, display-name editing and removal from its saved host list through its assigned local service. A display name SHALL NOT change the SSH target. Removing host metadata SHALL preserve open Screens, sessions and independently owned forwards. Current connection work MAY remain visible separately from the saved directory.

#### Scenario: Native host metadata
- **WHEN** a native main window reads or edits its saved host metadata without requesting a connection
- **THEN** the same validated metadata is returned or saved and no SSH connection or listener is started

#### Scenario: Native saved forwarding
- **WHEN** a native main window deliberately connects a host
- **THEN** only that host's enabled saved forwards/reverse forwards may restore, with the existing Stop, Forget, ordering and endpoint-exclusion rules

#### Scenario: Inspect cached native hosts
- **WHEN** the experimental native workspace inspects a merged directory containing aliases and previously observed Screens
- **THEN** those observations remain available without connecting, creating or attaching a Screen, restoring a forward, or claiming that a host is currently reachable

#### Scenario: Refresh fails after a successful observation
- **WHEN** a native directory refresh fails or returns an invalid or unadmittable response after a valid result from the same local connection
- **THEN** the last valid directory remains available with a refresh failure and local terminals continue processing their events
- **AND** a first-read failure is not presented as an empty directory, while a later successful empty response replaces the old observations

#### Scenario: Directory invalidated during a read
- **WHEN** `hub.changed` arrives while the native workspace is reading the directory
- **THEN** it performs a subsequent passive refresh and a superseded result does not overwrite the current directory
- **AND** repeated notifications do not require an unbounded queue of reads

#### Scenario: Local directory owner replaced
- **WHEN** the local service connection is replaced while a directory read or its presentation delivery is pending
- **THEN** the old connection's result is ignored, its retained directory is not attributed to the replacement, and the replacement reads its own directory when ready

#### Scenario: Close while a native directory read is pending
- **WHEN** the native workspace completes an accepted owner shutdown while directory setup or a read is pending
- **THEN** the pending directory work is cancelled and its owned stream cleanup completes without publishing a later directory result
- **AND** no remote host or saved forward is started by shutdown

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

### Requirement: Native Entered Host Connection
The experimental native workspace SHALL accept a deliberately entered valid SSH target in addition to its known directory entries. It SHALL validate the exact target before effects, reveal a previously hidden target through the local host preferences API, and connect only after a confirmed preparation result while the original browse request remains active. An optional display name SHALL be metadata only. This action SHALL NOT promise an offline bookmark for an unnamed target before successful connection.

#### Scenario: Enter a new target or restore a hidden target
- **WHEN** a user submits a valid SSH target that is absent from the accepted directory or was previously forgotten
- **THEN** the native client deliberately reveals that exact target and starts its browse connection only after the local service confirms preparation
- **AND** existing target validation, source admission, session ownership and saved-forward restoration rules remain in effect

#### Scenario: Leave during preparation
- **WHEN** the user cancels the pending connection request, leaves Hosts, or closes the window before preparation completes
- **THEN** late preparation completion cannot start a new host connection
- **AND** already submitted metadata work is reconciled without repeating it, while existing open Screens retain their connection demand

#### Scenario: Invalid target or preparation failure
- **WHEN** the target or display name is invalid, preparation is rejected, or its completion is uncertain
- **THEN** no host connection starts from that request, its failure remains visible, and a failed unlisted connection is not represented as a saved or reachable host

### Requirement: Native Host Metadata Mutation Ownership
Native host metadata actions SHALL carry their assigned local owner, exact target and request identity, use bounded admission, and submit each admitted action at most once. Directory refresh and mutation outcomes SHALL remain distinct. A rejected, uncertain or stale result SHALL NOT become a claimed success; fresh reads and hub.changed SHALL reconcile effective metadata without replaying a mutation or a connection continuation.

#### Scenario: Rename or clear a display name
- **WHEN** a user changes or clears the display name of a host
- **THEN** the local service receives only that target metadata change, the confirmed directory reflects it, and neither SSH target identity nor open work changes

#### Scenario: Remove metadata while work remains open
- **WHEN** a user removes a host with an open Screen or independent forward from the saved list
- **THEN** its saved metadata is forgotten through host.forget while its running Screen, sessions and forwards continue
- **AND** later cache observations do not silently restore the forgotten directory entry

#### Scenario: Duplicate, uncertain or obsolete mutation
- **WHEN** an action duplicates pending work, loses its reply, or arrives from an obsolete local owner
- **THEN** it cannot create overlapping duplicate effects or alter another owner, uncertainty remains explicit, and a subsequent refresh does not claim the lost request succeeded

#### Scenario: Mutation during directory refresh and shutdown
- **WHEN** hub.changed overlaps a mutation or shutdown starts while API work is pending
- **THEN** stale directory data cannot replace the latest confirmed view, terminal delivery continues, and shutdown joins the owned work without replay or late connection creation
