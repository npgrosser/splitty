# access-control Specification

## Purpose
Defines who may talk to a Splitty service and how that is enforced: the private native client endpoint, the owner-only control socket and its route set, the restricted sockets of agent grants, the keeper sockets, SSH as the only way to and from remote hosts, the admission of port forwards and reverse shares, and the protection of private files. The terminal service has no TCP listener and hands out no credential. These are security invariants; other capabilities build on them and must not weaken them.

## Requirements

### Requirement: No Reusable Credential In Output
The server and the CLI SHALL print no credential on stdout, stderr or in log lines; machine-readable startup lines (see `cli`) and the output of grant administration name sockets, never a secret. `splitty daemon start` MUST print no credential at all.

#### Scenario: Serve startup
- **WHEN** a user runs `splitty serve`
- **THEN** its output names the remaining ways and contains no credential, and no token or code is created

#### Scenario: Daemon startup
- **WHEN** `splitty daemon start` starts a daemon
- **THEN** its output describes the private client endpoint and contains no credential

### Requirement: Credential Files Are Owner-Only
On Unix, the Splitty config directory SHALL have mode 0700, and every writer MUST create it with that mode or repair an existing directory to it. The `daemon.json` file MUST have mode 0600 and be written atomically so it is never observable partially written or with a looser mode. An `auth-token` file left by an older version SHALL be neither read, repaired, rewritten nor removed, and no endpoint SHALL accept its value (provisional, tbd). `daemon.json` MUST NOT contain any credential (its contents are defined in `server-modes`), and a `token` key left by an older version MUST be ignored.

Authorized daemon start, restart and remote-handshake ensure SHALL prepare the config directory before discovery. They MAY tighten an owned final directory that has no group/world write bits through its verified descriptor. Symlinks, foreign-owned final directories and group/world-writable final directories SHALL be refused without chmod or daemon startup; shared ancestor permissions SHALL remain unchanged. Read-only discovery SHALL NOT repair directory modes.

#### Scenario: Directory left behind by an older version
- **WHEN** the config directory exists with mode 0755 and the server starts
- **THEN** the directory ends up with mode 0700

#### Scenario: Token file with loose permissions
- **WHEN** the config directory holds an `auth-token` with mode 0644 and a daemon starts
- **THEN** the file keeps its content and mode, and no request carrying its value is accepted anywhere

#### Scenario: Daemon info contents
- **WHEN** a daemon writes `daemon.json`
- **THEN** the file has mode 0600 and contains no token

#### Scenario: Read-only discovery of a loose config directory
- **WHEN** the owned config directory has mode 0755 and a remote handshake has no ensure option
- **THEN** it refuses unsafe discovery without changing the mode or starting a daemon

#### Scenario: Config repair preserves shared ancestors
- **WHEN** daemon start or an ensuring handshake finds an owned 0755 config directory below owned 0775 ancestors
- **THEN** only the config directory becomes 0700, and daemon discovery/start continues through the verified private path

#### Scenario: Writable final config directory
- **WHEN** daemon start, restart or an ensuring handshake encounters a final config directory with group or world write permission
- **THEN** it refuses before discovery/start and leaves that directory and its ancestors unchanged

### Requirement: Owner-Only Control Socket
Each server process SHALL offer a local control channel as a Unix socket. A daemon's socket SHALL be `daemon.sock` in its run directory: `$SPLITTY_CONFIG_DIR/run/` when `SPLITTY_CONFIG_DIR` names a configuration directory (see `server-modes`), otherwise `<user data dir>/splitty/run/`. Any other server's socket SHALL be `<pid>.sock` in `$SPLITTY_CONFIG_DIR/run/` when `SPLITTY_CONFIG_DIR` names a configuration directory, otherwise in `$XDG_RUNTIME_DIR/splitty/` (or `<user data dir>/splitty/run/` when `XDG_RUNTIME_DIR` is unset or relative). The socket's directory MUST have mode 0700 and the socket mode 0600. On Linux and macOS, the server MUST drop any connection whose peer user id differs from its own effective user id, and MUST also drop it when the peer credentials cannot be read. The channel requires no credential: filesystem permissions and the peer check are its authorization. The server MUST refuse to place the socket under `/mnt/` or `/media/`; failing to create the socket MUST NOT stop the server and MUST NOT cause any credential to be handed to panes instead. Embedded services and the daemon SHALL remove their socket file on graceful shutdown.

#### Scenario: Socket permissions
- **WHEN** a server has started its control channel
- **THEN** the socket directory has mode 0700 and the socket has mode 0600

#### Scenario: Connection from another user
- **WHEN** a process running as a different user connects to the socket on Linux or macOS
- **THEN** the server closes the connection before handling any request

#### Scenario: Unsupported location
- **WHEN** the resolved socket directory is under `/mnt/`
- **THEN** the server runs without a control socket and logs a warning

#### Scenario: Socket below a named configuration directory
- **WHEN** an embedded service runs with `SPLITTY_CONFIG_DIR=/Users/me/.config/splitty`
- **THEN** its control socket is `/Users/me/.config/splitty/run/<pid>.sock`
- **AND** a daemon with the same variable uses `/Users/me/.config/splitty/run/daemon.sock`

#### Scenario: The daemon's socket survives a restart
- **WHEN** a daemon restarts and adopts a shell that was started before
- **THEN** the new daemon's control socket has the path the shell's `SPLITTY_SOCK` names, and `splitty list-screens` run in that shell reaches the new daemon

### Requirement: Control Socket Route Set
The owner control socket SHALL route only these multiplexer endpoints: `GET /api/mux/state`, `GET /api/mux/screens`, `GET /api/mux/screen/{id}`, `POST /api/mux/op`, `GET /api/mux/neighbor`, `GET /api/mux/status` and `POST /api/session/{id}/send`, plus `POST /api/editor/open` for confirmed client editor requests, `POST /api/file/offer` for file offers the user saves in a window, `GET /api/viewer/comments` and `POST /api/viewer/comments/reply` for what the user handed over on viewer panes and agents' replies to it, `POST /api/viewer/review/wait` for waits for the user's review, and the job and local agent-administration endpoints specified in `control-api`. Every other path MUST answer 404 there, in particular configuration, shell profiles, themes, templates, stats, the hub's `/api/remote/*` operations, and the former credential-minting paths `/api/session`, `/api/bootstrap` and `/api/ws-ticket`. The separate restricted agent sockets SHALL expose only their fixed subset below. No local socket SHALL mint a credential.

#### Scenario: Multiplexer status over the socket
- **WHEN** a pane process sends `GET /api/mux/status` over the owner control socket without any credential
- **THEN** the server responds 200

#### Scenario: Privileged endpoint over the socket
- **WHEN** a pane process sends `POST /api/bootstrap` or `GET /api/home/settings` over any local control socket
- **THEN** the server responds 404

#### Scenario: Job administration over the owner socket
- **WHEN** the owning user creates an agent grant through the owner control socket
- **THEN** a local restricted socket is created and no credential is issued

#### Scenario: Editor request on the owner socket
- **WHEN** the owner socket receives `POST /api/editor/open`
- **THEN** it may request a local UI confirmation but cannot authorize a launch, choose a destination host, or mint credentials
- **AND** the same route on a restricted grant socket answers 404

#### Scenario: File offer on the owner socket
- **WHEN** the owner socket receives `POST /api/file/offer`
- **THEN** it may offer the file to the windows of the session, which save it only on the user's Save
- **AND** the same route on a restricted grant socket answers 404

#### Scenario: Comments on the owner socket
- **WHEN** the owner socket receives `GET /api/viewer/comments` or `POST /api/viewer/comments/reply`
- **THEN** it returns only what the user handed over on the viewer panes of the caller's screen, and adds at most an agent's reply
- **AND** the same routes and `POST /api/viewer/review/wait` on a restricted grant socket answer 404

### Requirement: Pane Environment Carries No Credential
When the server spawns a terminal session, it SHALL pass the owner control socket path as `SPLITTY_SOCK` for ordinary sessions, or the restricted grant socket as `SPLITTY_AGENT_SOCK` for an agent-owned managed job. An agent-owned job MUST NOT receive `SPLITTY_SOCK`, including an inherited value. The server MUST NOT set a credential or a network address of a server in the session's environment (in particular it sets neither `SPLITTY_TOKEN` nor `SPLITTY_SERVER`). Other variables given to sessions are defined in `terminal-sessions`.

#### Scenario: Spawned pane
- **WHEN** an ordinary pane's shell is spawned
- **THEN** its environment contains `SPLITTY_SOCK=<socket path>`
- **AND** the server has set no `SPLITTY_TOKEN` or `SPLITTY_SERVER` variable

#### Scenario: Agent-owned job environment
- **WHEN** an agent-owned job starts while the server inherited owner or agent socket variables
- **THEN** the job receives only its own grant's `SPLITTY_AGENT_SOCK` and pane/screen/session identifiers, and neither the owner socket nor another grant's socket from that inheritance

### Requirement: Remote Access Through SSH Tunnel
A remote service SHALL be reached through SSH with the system client and validated target, using an exec stream to its private client endpoint. A target MUST match `[user@]host[:port]` (bracketed IPv6 allowed); option-like targets, Unicode dash lookalikes, whitespace and controls MUST be rejected before SSH starts, by the hub with `remote.error` code `invalid_target`. No terminal-server TCP port or credential SHALL be required on the host. Command/stream connections SHALL disable configured local/remote/dynamic forwards, agent/X11 forwarding and PTYs. Deliberate forwarding connections SHALL retain their configured-forward checks and owned-process lifetime.

#### Scenario: Option-like target
- **WHEN** a client opens a host stream for `-oProxyCommand=x`
- **THEN** the hub answers `remote.error` with code `invalid_target` without running SSH

#### Scenario: Matching origin through the tunnel
- **WHEN** a remote daemon has only a private client endpoint
- **THEN** the hub reaches it through the SSH exec bridge, with no matching remote port and no origin or address to allow

#### Scenario: GatewayPorts in the SSH configuration
- **WHEN** SSH configuration sets `GatewayPorts yes`
- **THEN** desktop service traffic creates no TCP forward

#### Scenario: Port held while the tunnel is rebuilt
- **WHEN** the `ssh` of a hub port forward exits
- **THEN** the hub keeps the forward's local port reserved while it rebuilds the forward to the same validated target, and connects that port to no other destination

#### Scenario: SSH configuration adds network features
- **WHEN** user SSH configuration enables local, remote or dynamic forwarding, agent/X11 forwarding or a PTY
- **THEN** native command/bridge connections create none of those features, while explicitly requested forwarding retains its existing validation

### Requirement: Remote Installation Integrity
A server installed on a remote host by the hub SHALL come only from the client, over the SSH connection; the host MUST NOT download anything for it. Its SHA-256 checksum, computed by the client over the bytes it sends, MUST be verified on the host before the payload is unpacked or run, and the server MUST NOT be moved into place when the check fails. The install directories below `~/.local/share/splitty/remote/` MUST be created owner-only (mode 0700).

Commands sent to the host SHALL be constant scripts; every dynamic value SHALL be passed as an argument quoted for a POSIX shell, and versions and checksums MUST match an allowlist pattern before they are sent. A path the host reports, such as its home directory, MUST be absolute and free of control characters before the client uses it, and MUST be quoted wherever it is sent back.

#### Scenario: Hostile home directory
- **WHEN** a host reports a home directory containing quotes, `$(…)`, backquotes or a leading dash
- **THEN** the client either rejects it or uses it only quoted, and no part of it is executed on the host

#### Scenario: Checksum mismatch
- **WHEN** the payload is altered on its way to the host
- **THEN** the host-side check fails before the payload is unpacked, and nothing is installed

### Requirement: Remote Bridge Carries No Credential
`splitty remote-bridge --transport native-v1` SHALL resolve the daemon on the host and reach its private client endpoint using filesystem and peer-user checks. It SHALL read no credential file and obtain no credential. Its stdin/stdout SHALL carry only the negotiated client transport, without any credential. The SSH channel establishes the remote user; no terminal-service TCP connection is made.

#### Scenario: Nothing secret on the wire
- **WHEN** a client relays a session through the remote bridge
- **THEN** bridge traffic contains only transport control, permitted API messages and session/mux data, and no credential

### Requirement: Remote Screen Hub Access
Host connections SHALL require an authorized native main-service connection with host-management authority; no `/ws/remote` endpoint SHALL exist, and neither host streams nor `/api/remote/*` operations SHALL be served on the control socket. The hub SHALL run `ssh` only to a target that passed the target validation of Remote Access Through SSH Tunnel, non-interactively, without agent forwarding, and without remote forwards except the single remote forward of a reverse forward the user requested (see Hub Reverse Forwards). The hub SHALL send nothing to the host but the client's relayed messages and the commands of the install, the handshake and the bridge, `remote-relay prepare` and `remote-relay serve` for a reverse forward, and, for a host stream opened with the explicit update intent (see `remote-screens`, Host Update), instance-bound `daemon stop` or conditional `remote-migrate --expected <proof>` of its installed server. The host alone SHALL resolve and verify legacy owner sockets and process identity; renderer data SHALL NOT select the proof, PID, executable or configuration. The legacy migration command SHALL require the guards of Confirmed Legacy Daemon Migration and SHALL NOT be replayed after ambiguous completion. It SHALL forward no credential of the local service to the host.

#### Scenario: Missing ticket
- **WHEN** a process asks any Splitty socket or port for `/ws/remote?target=vm1`, with or without a ticket
- **THEN** the request is not served and no `ssh` process is started

#### Scenario: Control socket
- **WHEN** a process requests `/ws/remote?target=vm1` or `GET /api/remote/hosts` over the control socket
- **THEN** the request is not served

#### Scenario: Update stops only the daemon
- **WHEN** a client opens a host stream with the update intent for a host whose daemon runs another version
- **THEN** the only mutating maintenance command beyond installation and daemon startup is its installed server's instance-bound stop or explicitly confirmed conditional legacy migration; no shell-selected signal, session-ending flag or TCP fallback is used

#### Scenario: Remote forwards only for reverse forwards
- **WHEN** the hub opens screens of `vm1`, forwards a port of `vm1` and shares a local port with `vm1`
- **THEN** only the `ssh` of the shared port carries a remote forward, and it names a Unix socket on the host

#### Scenario: Native caller without hub authority
- **WHEN** a remote-window handle or restricted agent requests a local host connection
- **THEN** the request is rejected before any local SSH process starts

### Requirement: Hosts Cannot Reach Local Objects
A client showing screens of several servers SHALL resolve every screen, pane and session id, every attach, spawn notification and notification, and every server command only against the objects of the connection it arrived on. A host SHALL NOT be able to change, focus, attach to, send input to or close a local pane or another host's pane, whatever ids it sends. A server command from a host SHALL NOT paste the local clipboard. Names, titles and notifications from a host SHALL be shown as plain text, and a host's notification SHALL name the host.

#### Scenario: Colliding ids
- **WHEN** a host sends a snapshot, output and `pane.close` using the ids of a local screen, pane and session
- **THEN** no local screen, pane or session changes, and nothing is sent to the local server

#### Scenario: Paste from a host
- **WHEN** a host sends the server command `pane.paste`
- **THEN** the client does not read the clipboard and sends nothing to the host

### Requirement: Hub Port Forwards
A forward opened by the hub MUST default to an explicit `127.0.0.1` binding, regardless of SSH configuration. Only an explicit local `bind` preference or action MAY select a numeric LAN/VPN address or wildcard binding. Shared host catalog metadata MUST NOT select a local binding or start a forward. Changing the binding of an active forward MUST require stopping it first. Browser addresses MUST derive from the confirmed binding. The default IPv4 loopback and IPv4 wildcard bindings MUST use `localhost`. Its `ssh` MUST run non-interactively, with no agent forwarding, no remote forwards and no terminal, to a target that passed the target validation of Remote Access Through SSH Tunnel, and remote and explicit local ports MUST be integers from 1 to 65535.

A forward MUST NOT be opened without the user's forward action or remote loopback-link click, unless the user turned on `remote.autoForwardPorts`, or previously opted to remember an enabled mapping and now connects that host successfully. Saving preferences, starting the local server, connecting a client, reading state and observing ports MUST NOT by themselves authorize saved restoration. A disabled saved rule MUST override automatic forwarding. `/api/remote/forwards` and `/api/remote/forward-preferences` MUST NOT be served on the control socket.

A selected local port MUST be reserved on its selected binding, including explicit and saved ports and automatically chosen candidates. Default IPv4 loopback and IPv4 wildcard bindings MUST additionally be checked on `::1` to ensure localhost browser access cannot reach an unrelated listener. A busy address or an unexplained failure to establish availability MUST NOT be treated as free; an unavailable IPv6 stack alone MUST NOT disable IPv4 forwarding. Rebuilds MUST retain the selected port. Forward URLs MUST contain no Splitty credential. Labels MUST NOT affect SSH arguments, destinations or URLs.

#### Scenario: GatewayPorts in the SSH configuration
- **WHEN** the user's SSH configuration sets `GatewayPorts yes` and the hub forwards port 3000 of a host
- **THEN** the forward listens on `127.0.0.1` only

#### Scenario: Explicit local network access
- **WHEN** a user selects a numeric LAN/VPN address or wildcard binding locally
- **THEN** the listener uses that address, replies expose the confirmed address and local port, and a rebuild retains both
- **AND** another client receiving the shared port name does not inherit network exposure

#### Scenario: Forwards in the SSH configuration
- **WHEN** the user's SSH configuration sets `RemoteForward` for `vm1` and the UI requests a forward of a port of `vm1`
- **THEN** the request is refused with a message naming the configured forward, and no `ssh` connects to `vm1` for it

#### Scenario: Control socket
- **WHEN** a process requests `/api/remote/forwards` or `/api/remote/forward-preferences` over the control socket
- **THEN** the request is not served

#### Scenario: Remembered authorization is limited
- **WHEN** the local server starts, a client connects, or a client reads or edits saved preferences without connecting a host or requesting a forward
- **THEN** no saved forward is opened, and connecting one host later authorizes only that host's enabled rules

#### Scenario: Strict loopback availability
- **WHEN** a selected port is occupied on IPv6 alone or its availability check fails for a reason other than an unavailable address family
- **THEN** it is not used as an available local port
- **AND** a genuinely unavailable IPv6 stack permits a port verified free on IPv4, and a saved-port rebuild never silently changes ports

#### Scenario: Development URL carries no Splitty credential
- **WHEN** the client opens or rewrites a saved forward's URL in the browser
- **THEN** the hostname is localhost, no Splitty credential is added, and the development service receives no cookie the browser holds for 127.0.0.1
- **AND** SSH carries the service's TCP bytes without injecting HTTP authentication or changing TLS or WebSocket traffic

### Requirement: Explicit Remote Editor Dispatch
Only a deliberate editor action in the trusted hub UI SHALL dispatch a local VS Code or Zed folder handoff for the selected local or SSH terminal, or an SFTP client handoff for the selected SSH terminal. The desktop app SHALL validate the fixed editor enum (`vscode`, `zed`, or `sftp` for remote folders only), an absolute directory, an optional restricted SSH target for remote folders, and an optional validated WSL distribution for local VS Code folders, and generate the fixed editor URI itself before it hands that URI to the operating system's handler; no arbitrary URI, executable, command string or shell interpolation SHALL be accepted. Terminal links, remote output, server commands and URL query parameters SHALL NOT invoke this dedicated path. Existing generic URL openers SHALL continue to admit only HTTP(S).

#### Scenario: Dedicated native command scope
- **WHEN** a server or host message, or terminal output, names an editor, a folder or an editor URI
- **THEN** no handoff occurs unless the user chooses the editor action in the window

#### Scenario: Hostile dispatch inputs
- **WHEN** the action receives an executable/URI as target, an option-like target, a relative directory, control characters, unsupported SSH syntax, or `sftp` for a local folder
- **THEN** dispatch rejects the request before opening anything

#### Scenario: Terminal link uses editor scheme
- **WHEN** a hub remote terminal link or Splitty's generic URL helper receives a `vscode:`, `zed:` or `sftp:` URI
- **THEN** the generic opener refuses it and no editor dispatch is invoked

### Requirement: Image Uploads Stay With Their Pane
A client SHALL send `save_image` only for the user's own paste or drop into a pane, never for a server command or a message from a server, and only to the server of that pane's connection. It SHALL paste the returned path only into that pane. A server SHALL store an uploaded image only under the name it chooses itself in its own paste directory; no part of the request names a file. The image bytes SHALL be relayed by the hub like any other session message and SHALL NOT be passed to a shell.

#### Scenario: A host asks for a paste
- **WHEN** a host sends the server command `pane.paste` while the local clipboard holds an image
- **THEN** the client sends no `save_image`, neither to the host nor to the local server

#### Scenario: Paste into a host's pane
- **WHEN** the user pastes an image into a pane of `vm1` while a local screen and a screen of `vm2` are open
- **THEN** only `vm1` receives `save_image`, and the path is pasted only into that pane

### Requirement: Terminal Clipboard Policy
The xterm client SHALL allow OSC 52 clipboard writes by default, capped at 64 KiB of UTF-8. `terminal.allowOsc52Write=false` SHALL disable writes and survive settings save/reload. Clipboard reads via OSC 52 SHALL always return empty without reading the clipboard. Successful writes SHALL show a brief notice without copied contents; failed writes SHALL show an error, not success.

#### Scenario: Copy from a terminal application
- **WHEN** an application emits a nonempty OSC 52 write within the size limit and writes are enabled
- **THEN** the client writes the text and announces success only after the write succeeds

#### Scenario: Clipboard opt-out
- **WHEN** a user disables clipboard writes and reloads settings
- **THEN** OSC 52 writes remain disabled

#### Scenario: Clipboard read request
- **WHEN** a terminal program requests the clipboard via OSC 52
- **THEN** the client returns empty and never reads the local clipboard

#### Scenario: Oversized write
- **WHEN** a write exceeds 64 KiB of UTF-8
- **THEN** the clipboard is not changed

#### Scenario: Write failure
- **WHEN** the clipboard write fails
- **THEN** the client reports failure without displaying the copied contents

### Requirement: Restricted Local Agent Sockets
Each agent grant SHALL have a distinct Unix socket in the private runtime directory, with the owner socket's directory, mode and peer-user protections. The server SHALL derive the grant from the listener, never from a request-supplied owner ID, pane ID, label or working directory. A grant socket SHALL serve only job creation, listing, inspection, logs, input, stop and forget; all other paths SHALL return 404.

#### Scenario: Attempt to access an existing user terminal
- **WHEN** an agent socket receives a mux-state query, raw session-send request, WebSocket upgrade or grant-administration request
- **THEN** it returns 404 without reading a terminal, performing an operation or minting authority

#### Scenario: Different OS user
- **WHEN** another user attempts to connect to a grant socket
- **THEN** the same filesystem protection and platform peer checks as the owner socket deny the connection

### Requirement: Own Jobs Only
Every job operation on an agent socket SHALL check that the grant is live and currently owns the job. Lists SHALL contain only owned jobs; unknown and unowned IDs SHALL both return 404. The check SHALL cover output, metadata, input, stop, forget and replayed launch results. Supplying an owner or creator field in an agent launch SHALL be rejected rather than trusted.

#### Scenario: Agent guesses another job's identifiers
- **WHEN** agent A requests metadata, logs, input, stop or forget for agent B's or the user's job
- **THEN** each request returns 404 with no job data or effects

#### Scenario: Forged owner
- **WHEN** a launch through an agent socket supplies another agent's owner ID
- **THEN** the server rejects it before creating a job

### Requirement: Placement Is Not Terminal Access
A grant SHALL carry an optional administrator-selected screen/pane anchor for tab or split creation. An agent SHALL create next to that anchor or one of its own job panes, or create a new screen. Placement SHALL never convey access to the anchor's output, command, directory, profile or input. A missing or moved-out-of-screen anchor SHALL fail rather than selecting the globally active pane.

#### Scenario: Root shell as placement anchor
- **WHEN** a grant is anchored next to a user terminal running a root shell and starts a backend
- **THEN** only a new non-elevated job is started under the server's user, using its explicitly requested directory and program, and no anchor output or input is exposed

#### Scenario: Anchor disappears
- **WHEN** an agent's configured anchor has been closed
- **THEN** tab/split creation fails without changing another terminal and creating a new screen remains available

### Requirement: Revocation Applies To In-Flight Work
Revocation and ownership transfer SHALL be ordered with job actions. Before acknowledging either, the server SHALL prevent further actions and delivery of newly authorized output under the old grant, wake waiting reads, and reject queued operations that have not been authorized to execute. Already accepted process starts SHALL finish with truthful state and remain under trusted administration, not be duplicated or abandoned.

#### Scenario: Revoke during long poll and queued input
- **WHEN** trusted administration revokes a grant while it has a waiting log read and input waiting to execute
- **THEN** the waiting read ends without new output, the input is refused, and existing jobs keep running

### Requirement: Agent Socket Authority Is Local
Grant administration SHALL be available only on the owner control socket. Grant sockets and IDs SHALL survive agent-client exits but expire when their server process ends; stale sockets SHALL never be rebound to another grant. Documentation SHALL identify the unrestricted same-user owner socket and daemon-executed jobs as outside this scoped API boundary, and SHALL not claim OS or sudo isolation.

#### Scenario: Grant path used after server restart
- **WHEN** a client reconnects through a former grant socket after the server restarted
- **THEN** access fails without falling back to owner authority or reassigning that grant identity

### Requirement: Owner-Only Keeper Sockets
Each session keeper SHALL serve one Unix socket in the `keepers` directory below its daemon's run directory, under the owner control socket's rules: directory mode 0700, socket mode 0600, no location under `/mnt/` or `/media/`, and on Linux and macOS a dropped connection for a peer of another or unreadable user id, which its controllers check of the keeper likewise. A keeper SHALL open no network socket and hold no credential, and no session's environment SHALL name a keeper socket.

#### Scenario: Keeper socket permissions
- **WHEN** a daemon has started a session
- **THEN** the session's keeper socket lies in a directory of mode 0700 and has mode 0600

#### Scenario: Connection from another user
- **WHEN** a process running as a different user connects to a keeper socket on Linux or macOS
- **THEN** the keeper closes the connection before sending anything

#### Scenario: Pane environment
- **WHEN** a daemon spawns a pane's shell
- **THEN** no variable of the shell's environment names a keeper socket or the keepers directory

### Requirement: Saved Forward Preference Guards
Every forward-preference operation (`forwardPreferences.get`, `forwardPreferences.save`, `forwardPreferences.forget`, HTTP form `/api/remote/forward-preferences`) SHALL be available only as native operations of a local main connection (see `client-connections`, Enumerated Native Requests). Targets and ports SHALL be validated before any write or SSH invocation. Invalid requests SHALL leave saved and live state unchanged. They SHALL grant no access to control-socket or grant-socket callers, remote-window connections or host relays.

#### Scenario: Unauthenticated or cross-origin preference access
- **WHEN** a pane process sends `GET /api/remote/forward-preferences` over the owner control socket, or a remote-window connection or host relay requests `forwardPreferences.save`
- **THEN** the socket answers 404 and the connection's request is refused, before any metadata or forwarding side effect

#### Scenario: Invalid saved mapping
- **WHEN** a preference write has an invalid SSH target, invalid port, wrong field type, unknown field or label longer than 200 Unicode characters
- **THEN** it returns 400 without modifying a preference or starting SSH
- **AND** labels are displayed as bounded plain text with control and bidirectional formatting characters removed

### Requirement: Bounded Private Forward Preferences
Saved preferences SHALL contain at most 32 entries per target and 256 in total, without silently evicting existing rules. Exceeding either limit SHALL yield 409 `preference_limit`. The private configuration directory and `remote-forwards.json` SHALL have modes 0700 and 0600 on Unix, with atomic replacement. The file and runtime failure metadata SHALL contain no Splitty credentials.

#### Scenario: Preference bounds
- **WHEN** a request would exceed the per-host or total preference limit
- **THEN** it returns 409 `preference_limit` without dropping an existing rule, while an edit to an existing entry within those bounds still succeeds

#### Scenario: Private durable metadata
- **WHEN** a mapping is saved
- **THEN** its file is replaced atomically with mode 0600 inside a directory with mode 0700 on Unix
- **AND** neither it nor the runtime error exposed by GET contains a Splitty credential

### Requirement: File Transfers Need The User
A client SHALL send `save_image` and `upload` only for the user's own drop or paste into a pane or a scratchpad, or for a review the user sends to a terminal pane as a file (see `viewer-annotations`), only to the server of that pane's connection, and SHALL paste or insert the returned paths only there. A server SHALL store an upload under its temporary directory by default, or under the existing absolute directory explicitly selected by the user. In both cases the server SHALL sanitize the filename; `name` SHALL NOT select a directory. Durable uploads SHALL never overwrite files or follow an existing destination symlink. A client SHALL read an offered file only after the user chose Save for it, only from the connection that offered it, and SHALL save it only as a download of the window under its sanitized name. File bytes SHALL be relayed by the hub like any other session message and SHALL NOT be passed to a shell.

#### Scenario: Upload name with a path
- **WHEN** an upload names `../../.ssh/authorized_keys`
- **THEN** the server stores it as `authorized_keys` inside a new `upload-` directory of its paste directory

#### Scenario: Offer without Save
- **WHEN** a host offers a file and the user does nothing
- **THEN** the window reads nothing and the offer expires

#### Scenario: A review sent as a file
- **WHEN** the user sends a review to a terminal pane as a file
- **THEN** the window uploads it to the server of that pane's connection under its temporary directory and inserts the returned path only in that pane

### Requirement: Hub Reverse Forwards
A reverse forward SHALL make `localhost:<local>` of this machine reachable on the host, and SHALL NOT make any other local address reachable: the API takes only a local port, and the forward's `ssh` SHALL name `localhost:<local>` as its destination. A port that an internal endpoint holds (see Internal Endpoints Cannot Be Shared) SHALL be refused with 409 `local_port_reserved`.

The remote end of the forward MUST be a Unix socket in an owner-only directory of the host's server, and the host side MUST listen for programs only through `splitty remote-relay serve` (see `remote-hosts`, Remote Relay), bound to `127.0.0.1` and, where available, `::1`. The relay MUST admit a TCP connection only when the connecting socket belongs to the user the relay runs as, and MUST close every other connection without passing it on, including one whose owner it cannot determine. A host where the owner cannot be determined (no `/proc/net/tcp`, such as macOS) SHALL be refused with code `unsupported_host` before anything listens there.

A reverse forward MUST NOT be opened without the user's explicit request, unless the user previously remembered it as an enabled rule and now connects its host successfully. Detected ports, link clicks in panes, `remote.autoForwardPorts`, messages from a host and remote windows SHALL NOT open one. The forward's `ssh` MUST run non-interactively, with no agent forwarding and no terminal, to a validated target; a host whose SSH configuration adds forwards SHALL be refused as for forwards. Ports MUST be integers from 1 to 65535. `/api/remote/reverse-forwards` and `/api/remote/reverse-forward-preferences` MUST NOT be served on the control socket and SHALL be available only as native operations of a local main connection (see `client-connections`, Enumerated Native Requests); reverse-forward preferences SHALL be stored owner-only like forward preferences.

#### Scenario: Another user on the host
- **WHEN** the user shares local port 11434 with `vm1` as port 11434, and another user of `vm1` connects to `127.0.0.1:11434` there
- **THEN** the connection is closed and nothing reaches this machine

#### Scenario: The user's own program on the host
- **WHEN** a program running as the user on `vm1` connects to `localhost:11434` there
- **THEN** it reaches the service on `localhost:11434` of this machine

#### Scenario: GatewayPorts on the host
- **WHEN** the host's `sshd_config` sets `GatewayPorts yes` and the user shares a local port with it
- **THEN** nothing listens for the shared port on an address of the host other than loopback

#### Scenario: Own port refused
- **WHEN** a request asks to share a port that a browser gateway of an older Splitty version of this user holds
- **THEN** it is refused with 409 `local_port_reserved` and no `ssh` is started

#### Scenario: No automatic reverse forward
- **WHEN** a host has detected ports, `remote.autoForwardPorts` is on, and a remote pane prints a `localhost` link that the user clicks
- **THEN** no reverse forward is opened

#### Scenario: macOS host
- **WHEN** the user shares a local port with a host whose kernel is `Darwin`
- **THEN** the request is refused with code `unsupported_host` and no remote forward is requested

#### Scenario: Control socket
- **WHEN** a process requests `/api/remote/reverse-forwards` or `/api/remote/reverse-forward-preferences` over the control socket
- **THEN** the request is not served

### Requirement: Private Native Client Endpoint
The daemon's client endpoint SHALL be distinct from control, grant and keeper sockets, with a 0700 parent directory and 0600 socket. Both peers SHALL verify the owning user on Linux/macOS. Unsafe owners, symlinks, non-sockets, unsupported filesystems, unreadable credentials and mismatched live instances SHALL be rejected without TCP fallback. No client endpoint path SHALL be injected into pane environments. The presence of group-write permission on an otherwise accepted owned ancestor SHALL NOT alone reject startup or connection. Private-directory identity SHALL bind sensitive filesystem operations so ancestor replacement cannot redirect writes, locks or cleanup. Shared ancestor permissions SHALL remain unchanged; neither a group name nor a current membership list grants an exception to ownership, peer or instance checks.

#### Scenario: Unsafe endpoint
- **WHEN** the endpoint or its private directory is a symlink, has an unsafe owner or cannot provide peer credentials
- **THEN** the connection fails before dispatch and no alternative listener opens

#### Scenario: Existing group-writable ancestors
- **WHEN** a valid private endpoint directory lies below existing owned group-writable ancestors
- **THEN** startup and connection preserve those permissions while binding sensitive operations to the verified private directory

#### Scenario: Ancestor replaced during an operation
- **WHEN** a previously verified path is renamed or replaced during endpoint or daemon publication work
- **THEN** no filesystem mutation follows the replacement and a changed configured identity fails admission rather than granting authority to it

#### Scenario: Another user
- **WHEN** another OS user reaches the endpoint
- **THEN** the connection closes without application data

#### Scenario: Live endpoint exists
- **WHEN** another live daemon already owns the endpoint
- **THEN** startup neither unlinks its socket nor starts a second workspace

### Requirement: No Authority Expansion Through Native Transport
The native client endpoint SHALL expose only its enumerated UI/session operations. Control and grant sockets SHALL retain their existing route sets, and grant administration SHALL remain owner-control-only. No native entry point SHALL mint a credential. A failed scoped command SHALL never rediscover an owner, client or keeper endpoint.

#### Scenario: Cross-surface escalation
- **WHEN** a grant or control socket receives a UI-stream request, or a native UI connection requests grant administration or a credential
- **THEN** it is refused without granting authority

#### Scenario: Scoped discovery failure
- **WHEN** a grant socket is revoked or unreachable while a native daemon is available
- **THEN** the command fails without contacting that daemon's owner or client endpoint

### Requirement: Internal Endpoints Cannot Be Shared
This version binds no browser-gateway port. Splitty SHALL still reject reverse forwarding, with `local_port_reserved`, of any port that an active browser gateway of an older Splitty version of the same user holds, as the shared exposure registry records it, including gateways for remote targets and other configurations (provisional, tbd). Listener reservation and sharing admission SHALL be coordinated, following Forwarding Reservation Lifetime, so neither a race, coordinator crash nor saved restoration can share a newly bound internal listener. Private client, control and keeper sockets SHALL not be accepted as forwarding destinations.

#### Scenario: Gateway races sharing
- **WHEN** an older version's browser gateway and a reverse-forward request concurrently select the same local port
- **THEN** at most one is admitted and the gateway is never exposed through that forward

#### Scenario: Saved share now names a gateway
- **WHEN** a saved reverse-forward rule targets a port now owned by an active browser gateway of an older Splitty version
- **THEN** restoration reports `local_port_reserved` without connecting that listener to the host

#### Scenario: Shared loopback across runtimes
- **WHEN** a Windows/WSL configuration exposes one runtime's older-version gateway to another runtime's sharing destination
- **THEN** gateway reservations protect that destination too, or sharing fails as unsupported before the forward starts

### Requirement: Forwarding Reservation Lifetime
A reverse-share reservation SHALL be durable before exposure and cover actual local forwarding workers by process generation/network context. Release SHALL require locally verified termination; coordinator death, remote cleanup/heartbeat and a PID alone SHALL not suffice. A different immutable process birth identity MAY prove termination of the saved generation only within the same verified machine, boot and PID namespace; an effective-UID change alone SHALL NOT suffice. Ambiguous spawn/publication or cleanup SHALL keep the port reserved. Recovery SHALL not kill an unrelated process by PID alone.

#### Scenario: Coordinator dies while forwarding lives
- **WHEN** a coordinator is killed and a hostile SSH peer keeps the local forwarding worker alive
- **THEN** no other share of this user is admitted on the reserved destination until local forwarding termination is verified, regardless of remote EOF or heartbeat reports

#### Scenario: Crash during worker publication
- **WHEN** startup crashes after reserving a port and before a spawned worker's identity is fully recorded
- **THEN** recovery treats the reservation as uncertain and admits no other share on that port

#### Scenario: Unverified process identity or failed cleanup
- **WHEN** recovery cannot verify the saved worker generation or that cleanup ended forwarding
- **THEN** it neither kills the unrelated process nor releases the reservation based on coordinator death or elapsed time

#### Scenario: Proven process generation replacement
- **WHEN** the same verified machine, boot and PID namespace reports a different immutable birth identity for the saved worker PID
- **THEN** recovery may release that worker generation without signaling the replacement process
- **AND** a changed effective UID without a birth-identity change does not authorize release

### Requirement: Viewer Documents Stay With Their Connection
A client SHALL read a viewer pane's document only over the connection whose screen holds the pane, and SHALL show it only in that pane. A server SHALL read for a viewer pane only the file its record names, only to answer `viewer.read` on the connection that sent it, and SHALL never write, create or delete a file for a viewer pane. Every client connection of a server MAY open viewer panes and read their documents, the authority it already has by starting a terminal on that server. The control API, and with it the processes in panes, MAY open viewer panes but SHALL NOT read a document's content or a `text` document's text, except the passages that annotations the user handed over quote (see `viewer-annotations`, Agents Read And Reply To Handed-Over Threads). A restricted agent grant socket SHALL do neither.

#### Scenario: A host names a local path
- **WHEN** a host's snapshot holds a viewer pane naming `/home/me/.ssh/id_ed25519` and the window shows that host's screen next to local screens
- **THEN** the window reads the document only over the host's connection, the local server reads nothing, and the host answers from its own file system

#### Scenario: A process in a pane reads the workspace
- **WHEN** a viewer pane shows a terminal selection and a process in a pane sends `GET /api/mux/state` over the owner control socket
- **THEN** the answer holds the viewer pane without the selected text

#### Scenario: A handed-over quote
- **WHEN** the user handed over a comment on a passage of a terminal selection shown in a viewer pane, and a process in a pane sends `GET /api/viewer/comments` over the owner control socket
- **THEN** the answer holds the comment with its quoted passage, and neither the rest of the selected text nor annotations the user has not handed over

#### Scenario: Grant socket
- **WHEN** an agent grant socket receives `POST /api/mux/op` with `open_viewer`
- **THEN** it answers 404 and no pane appears

### Requirement: Rendered Documents Are Inert
A client SHALL render a Markdown or HTML document without executing any script, event handler attribute or `javascript:` URL in it, without frames, objects, embedded documents or forms, and without loading anything over the network or from Splitty's own service: images SHALL show only from `data:` URLs. The rendered document SHALL stay inside its pane: its styles SHALL apply only within the rendered document, and nothing it positions SHALL cover anything outside the pane. A link SHALL open an `http:` or `https:` URL through the external opener, a relative link the named file in a viewer pane, and a link to an anchor the position within the document; any other link SHALL do nothing.

#### Scenario: A script in an HTML report
- **WHEN** a viewer pane renders an HTML file holding `<script>`, an `onerror` attribute and a `javascript:` link
- **THEN** no script runs and the link does nothing when clicked

#### Scenario: An image from the server's origin
- **WHEN** a Markdown document holds `![x](/api/mux/state)` or `![x](http://127.0.0.1:9800/api/mux/state)`
- **THEN** the window sends no request for it and shows the image's alt text

#### Scenario: A load hidden in a style
- **WHEN** an HTML document's style sheet or `style` attribute holds `background: var(--x, url(/api/mux/state))` or a `url()` spelled with CSS escapes inside `var()`
- **THEN** the window sends no request for it

#### Scenario: An overlay
- **WHEN** an HTML document holds an element with `position: fixed` covering the whole viewport
- **THEN** that element stays inside the viewer pane, and the rest of the window remains usable

#### Scenario: A link to a website
- **WHEN** the user clicks an `https:` link in a rendered document
- **THEN** the system browser opens it, and the window does not navigate

### Requirement: Template Apply Authority
Template application SHALL be accepted only through authenticated window session connections. The generic `POST /api/mux/op` and native `mux.op` API SHALL reject `apply_template`, including owner control requests. Restricted agent sockets SHALL expose neither application nor extraction.

#### Scenario: Non-window template application
- **WHEN** an owner control or restricted agent caller attempts template application or extraction
- **THEN** application is denied and extraction is unavailable on those sockets

### Requirement: JSON Request Bodies
An API endpoint that takes a JSON body SHALL accept the request only when its `Content-Type` is `application/json` or an `application/*+json` type, with or without parameters, on the control socket. Otherwise the server SHALL answer 415 before reading the body and without changing any state.

#### Scenario: Text body with valid credentials
- **WHEN** a pane process posts a well-formed operation to `/api/mux/op` over the control socket with `Content-Type: text/plain`
- **THEN** the server responds 415 and the workspace state is unchanged

#### Scenario: Missing content type on the control socket
- **WHEN** a pane process posts a JSON body to `/api/session/{id}/send` over the control socket without a `Content-Type`
- **THEN** the server responds 415 and nothing is written to the session

#### Scenario: JSON with a charset
- **WHEN** a client sends a body endpoint its JSON with `Content-Type: application/json; charset=utf-8`
- **THEN** the request is processed

### Requirement: No Terminal-Service TCP Listener
No server, daemon, embedded service, desktop backend, SSH bridge or keeper SHALL create a TCP listener for terminal, control, API or file traffic, including on failure. The only TCP listeners Splitty opens SHALL be the forwards and reverse-share relays the user requested (see Hub Port Forwards and Hub Reverse Forwards), bound to loopback. The removed `--remote` flag of `splitty daemon start` and the removed `--addr` and `--token` options SHALL be refused with status 2 before anything starts.

#### Scenario: Daemon and embedded service
- **WHEN** a daemon runs and the desktop app runs its embedded service, each with open screens
- **THEN** neither process holds a listening TCP socket other than a forward the user requested

#### Scenario: Direct remote mode is refused
- **WHEN** the user passes `--remote` to `splitty daemon start`
- **THEN** the process exits with status 2 without starting a daemon

#### Scenario: Former listener address
- **WHEN** the user runs `splitty daemon start --addr 0.0.0.0:8080`
- **THEN** the process exits with status 2 without opening a socket
