## ADDED Requirements

### Requirement: Open a cached remote screen deliberately
Choosing Open on a cached screen SHALL connect its exact host and open that
existing screen only after a current catalog confirms its identity. Reading,
filtering or selecting a host in the browser SHALL NOT connect it. Leaving the
initiating browser, cancelling the attempt or replacing its local owner SHALL
revoke an uncompleted open continuation. A missing screen SHALL NOT be recreated.

#### Scenario: Open an unattached screen from a cached host
- **WHEN** the user opens a cached screen that is not open in this window
- **THEN** the client connects that host, confirms the screen in its live catalog and attaches this window to the same screen
- **AND** other screens, clients and local forwarding preferences remain unchanged

#### Scenario: Stale cached screen
- **WHEN** the current host catalog no longer contains the selected cached screen
- **THEN** the browser reports that it is no longer available and creates no replacement

#### Scenario: Cancel before connection completes
- **WHEN** the user leaves the browser or cancels while a cached-screen open is waiting for connection
- **THEN** later connection/catalog results do not open or select that screen

### Requirement: Screen catalog membership is separate from local attachment
The host browser SHALL include the remote workspace's visible screens regardless
of whether this window has them open. Local attachment indicators and filters
SHALL describe this window only, not infer other clients' attachment. Cached
catalog observations SHALL be distinguished from a current live catalog.

#### Scenario: Detach the last open screen of a host
- **WHEN** the user detaches the last screen of a host from this window
- **THEN** its drawer group is removed, while the host and its screen catalog remain accessible
- **AND** the remote screen, other clients and explicit forwards continue independently

#### Scenario: Inspect screens not opened here
- **WHEN** the user filters a host's catalog to screens not open in this window
- **THEN** screens opened in this window are excluded without changing any attachments or connections

### Requirement: Close a host
The native client SHALL offer Close host for a host with screens open in the
window. It SHALL remove every reference to that host's screens from the window
in one step and release the host's connection when nothing else in the window
uses it. It SHALL NOT end or change anything on the host, and SHALL NOT stop
forwards that outlive a detached screen.

#### Scenario: Close a host with running work
- **WHEN** the user closes a host that has three screens open in the window
- **THEN** its group leaves the drawer, and the three screens and their processes keep running on the host

### Requirement: Native remembered screens follow the window
The native client SHALL keep the local service's remembered remote screens in
step with the window, one host at a time. A host with open screens is stored as
exactly those; a host whose last screen was detached or has ended is stored
with none; a closed host keeps its stored screens, marked `closed`; every host
keeps them unmarked when the app quits. A write SHALL keep the stored screens of
hosts it does not name.

#### Scenario: Closed host stays remembered
- **WHEN** the user closes a host that has three screens open in the window
- **THEN** the three screens stay in the remembered list marked closed, and the entries of other hosts are unchanged

#### Scenario: Detach instead of close
- **WHEN** the user detaches the last open screen of a host
- **THEN** its group leaves the drawer and no screens are remembered for that host

### Requirement: Reopen a closed host's screens
The native client SHALL NOT connect a host at start unless that host is chosen
for it (see Reconnect hosts at start). For a host without open screens it SHALL show how many screens
are remembered. Choosing that host to connect SHALL open the remembered screens
its current catalog still has, in stored order, and show the first. Screens the
catalog no longer has SHALL be forgotten, never recreated. Leaving the panel or
cancelling before the connection completes SHALL revoke the reopening.

#### Scenario: Come back to a closed host
- **WHEN** the user chooses a closed host in the Hosts panel after one of its three remembered screens has ended
- **THEN** the client connects the host, opens the two screens that still exist and shows the first
- **AND** it creates no screen for the one that ended

#### Scenario: Restart
- **WHEN** the app is quit with a host's screens open and started again, and the host is not chosen for reconnecting
- **THEN** no host is connected and the drawer shows local screens only
- **AND** choosing the host in the Hosts panel reopens the screens that were open

### Requirement: Reconnect hosts at start
The native client SHALL let the user choose per saved host whether it is
reconnected at start. The choice is kept in `remote.reconnectHosts`, a list of
targets that is empty by default and read once per start. At start the client
SHALL connect each listed host whose remembered screens are not marked closed
and open those its catalog still has. Local screens SHALL be usable meanwhile,
and the shown screen SHALL stay shown.

#### Scenario: Start with a chosen host
- **WHEN** the app was quit with two screens of `vm1` open, `vm1` is chosen for reconnecting, and the app is started
- **THEN** the local screens are usable at once, `vm1` is connected without a further action and its two screens are back in the drawer
- **AND** the screen that was shown is still shown

#### Scenario: Host that is not chosen
- **WHEN** the app was quit with screens of `vm1` and `prod` open and only `vm1` is chosen
- **THEN** only `vm1` is connected at start, and `prod` is offered with its screens in the Hosts panel

#### Scenario: Closed host stays closed
- **WHEN** the user closed `vm1` before quitting and starts the app with `vm1` chosen
- **THEN** `vm1` is not connected, and the Hosts panel still offers its screens

### Requirement: Failed reconnect at start
A host that cannot be connected at start SHALL leave the window with a notice
that stays until dismissed, and SHALL keep its remembered screens. Nothing SHALL
retry it in the background. Closing a Hosts panel SHALL NOT end a connection
that the start began.

#### Scenario: Host unreachable at start
- **WHEN** a host to reconnect cannot be reached at start
- **THEN** the window says so once, the host does not stay in the window, and its screens remain offered in the Hosts panel

## MODIFIED Requirements

### Requirement: Remembered Remote Screens
The server SHALL keep the client's list of open remote screens in `remote-screens.json` in its configuration directory, written atomically with mode 0600. The file holds `{"version": 1, "screens": [{"target", "screenId", "name"?, "position"?, "closed"?}]}`, each target valid (see `access-control`, Remote Access Through SSH Tunnel), each (target, screenId) once. `GET /api/remote/screens` SHALL return the list, empty when the file is missing or unreadable. `PUT /api/remote/screens` SHALL replace it and SHALL reject a body that does not have that shape. Both routes have the authentication of the other `/api` routes and are not served on the control socket.

The browser client SHALL write the list after every change to its remote screens. On start, it SHALL restore the saved references and order, displayed within their host groups as described by Remote Screens In The Window. The saved position format SHALL remain compatible with existing mixed lists. It SHALL connect a remembered screen's host only when the screen becomes visible or when that host is connected anyway. A remembered screen that its host no longer has SHALL be shown as missing, with an action to remove it. The native client SHALL use the list as described by Native remembered screens follow the window.

#### Scenario: Restart
- **WHEN** the browser window shows a remote screen of `vm1` and the app is restarted
- **THEN** the screen is back in its host group in the remembered relative order, and it connects when the user opens it

#### Scenario: Unreachable host at start
- **WHEN** a remembered screen's host is unreachable when the app starts
- **THEN** the local screens are usable at once, and the remote screen shows its host as offline only when it is opened

### Requirement: Saved Forward Preferences API
`/api/remote/forward-preferences` SHALL provide GET, PUT and DELETE for mappings keyed by validated target and remote port, using the wire shapes in `docs/protocol.ts`. GET SHALL return `{version:1,forwards:[{target,remote,local,label,enabled,lastError?}]}`, ordered by target and remote port. PUT SHALL merge only supplied fields for one mapping, including an optional boolean `enabled` for local restoration consent; DELETE SHALL forget it. These metadata operations SHALL neither connect a host nor open or rebuild a forward.

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


#### Scenario: Save restoration separately from forwarding
- **WHEN** a user saves `enabled:false` for a new or active mapping
- **THEN** restoration remains disabled durably, and the save neither starts nor stops a forward
- **AND** enabling restoration changes only the saved consent; restoration still requires a later successful explicit host connection

### Requirement: Stop And Forget Saved Forwards
Stop SHALL close the forward and persist `enabled:false` when a saved rule exists; an already inactive saved rule SHALL also be stoppable. Explicit successful Forward/Resume SHALL retain legacy re-enabling behavior unless `preserveRestore:true` is supplied. With that option, successful opening SHALL preserve the saved restoration choice. An explicit preference edit MAY independently enable or disable restoration without starting or stopping a tunnel. Native one-off forwarding and loopback-link opening SHALL preserve restoration; new native saved settings SHALL default restoration to off. Forget SHALL idempotently remove only the preference and its runtime error, leaving a running forward and its supervisor intact. The client SHALL explain that forgetting does not stop a live tunnel.

#### Scenario: Persistent Stop and explicit Resume
- **WHEN** the user stops an active or inactive saved mapping
- **THEN** DELETE active forwards returns 204, its live forward is gone and the saved rule is disabled even after a reconnect or restart
- **AND** an explicit successful POST resumes at the saved port and enables that rule again

#### Scenario: Forget a running or inactive mapping
- **WHEN** the user forgets a saved mapping, repeating the request if necessary
- **THEN** DELETE preferences returns 204, the preference is absent and any existing live forward continues on its current port
- **AND** the forgotten rule is not restored after restart; a later Stop of an unsaved absent forward retains the existing 404 response


#### Scenario: Native one-off forwarding preserves restoration
- **WHEN** a native client forwards a saved mapping with restoration disabled, including opening a terminal URL
- **THEN** the forward opens after verification while the saved restoration choice remains disabled
- **AND** stopping still closes the tunnel and durably disables restoration

