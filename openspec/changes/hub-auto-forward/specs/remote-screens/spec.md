## MODIFIED Requirements

### Requirement: Remembered Remote Screens
The server SHALL keep the client's list of open remote screens in `remote-screens.json` in its configuration directory, written atomically with mode 0600. The file holds `{"version": 1, "screens": [{"target", "screenId", "name"?, "position"?}]}`, each target valid (see `access-control`, Remote Access Through SSH Tunnel), each (target, screenId) once. `GET /api/remote/screens` SHALL return the list, empty when the file is missing or unreadable. `PUT /api/remote/screens` SHALL replace it and SHALL reject a body that does not have that shape. Both routes have the authentication of the other `/api` routes and are not served on the control socket.

A client SHALL write the list after every change to its remote screens. On start, it SHALL restore the saved references and order, displayed within their host groups as described by Remote Screens In The Window. The saved position format SHALL remain compatible with existing mixed lists. It SHALL connect a remembered screen's host only when the screen becomes visible, when that host is connected anyway, or, once at the window's start, when the setting `remote.reconnectHosts` lists the host. A remembered screen that its host no longer has SHALL be shown as missing, with an action to remove it.

#### Scenario: Restart
- **WHEN** the window shows a remote screen of `vm1` and the app is restarted
- **THEN** the screen is back in its host group in the remembered relative order, and it connects when the user opens it

#### Scenario: Unreachable host at start
- **WHEN** a remembered screen's host is unreachable when the app starts
- **THEN** the local screens are usable at once, and the remote screen shows its host as offline only when it is opened

#### Scenario: Host reconnected at start
- **WHEN** `remote.reconnectHosts` lists `vm1` but not `vm2`, and a window starts with remembered screens of both
- **THEN** the window connects `vm1` at once and brings back its screens, while `vm2` stays unconnected until the user opens one of its screens

### Requirement: Automatic Port Forwarding
The setting `remote.autoForwardPorts` SHALL be off by default. When it is on, the hub SHALL forward every port that appears in a host's `ports.changed` on a connection it relays, without a click and without a notice. A connection's first list, including the first after a reconnect, SHALL count as already listening; lists SHALL be followed while the setting is off, so that turning it on forwards only what appears afterwards. A port SHALL NOT be forwarded automatically when it has a forward (opening, up or being rebuilt), when its saved mapping is disabled, or when it was forwarded automatically for the same process while the host stayed connected through at least one relay; several windows connected to one host SHALL cause at most one forward. Automatic forwards SHALL preserve saved local ports and SHALL NOT create saved rules, resume a stopped rule or silently remap a saved-port conflict. They SHALL appear in the active forwards and `hub.changed` like any other. With no client connection to a host there is no discovery, and nothing SHALL be forwarded for it.

A local service that forwards automatically SHALL announce `hub-auto-forward-v1` in `mux.welcome` on its local main connections. Clients SHALL NOT send automatic forward requests of their own; they only show the forwards the hub reports. `POST /api/remote/forwards` SHALL keep accepting `automatic: true` from older clients.

#### Scenario: Default
- **WHEN** a port appears on a connected host and the setting has its default value
- **THEN** no forward is opened until the user asks for one

#### Scenario: A port appears with the setting on
- **WHEN** the setting is on, a window is connected to `vm1` and `vite` starts listening on port 5173 there
- **THEN** the hub forwards 5173 without a notice and the window lists it as forwarded

#### Scenario: Two windows on one host
- **WHEN** two windows are connected to `vm1` and a port appears there with the setting on
- **THEN** exactly one forward of that port is opened

#### Scenario: Stopped by the user
- **WHEN** the user stops an automatic forward and the same program listens on the port again while the host stays connected
- **THEN** it is not forwarded again; another program on that port is

#### Scenario: Automatic forwarding respects a pause
- **WHEN** automatic forwarding is enabled and a port reappears after its saved mapping was stopped, including a request from a window with stale state
- **THEN** the saved mapping remains disabled and no forward is opened for it automatically

#### Scenario: Automatic forwarding uses saved port
- **WHEN** automatic forwarding attempts an enabled saved mapping whose local port is occupied
- **THEN** the attempt reports `local_port_conflict` and preserves the mapping, while an unrelated new unsaved port can still be forwarded without being remembered

#### Scenario: Older local service
- **WHEN** a client's local service does not announce `hub-auto-forward-v1`
- **THEN** no port is forwarded automatically, the setting stays editable, and the client sends no automatic request in its place
