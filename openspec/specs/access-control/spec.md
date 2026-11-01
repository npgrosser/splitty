# access-control Specification

## Purpose
Defines who may talk to a Splitty server and how that is enforced: the loopback-only listener, the Host, Origin and Fetch Metadata guards, the credentials (process token, bootstrap code, browser session, WebSocket ticket), the local control socket, and the protection of credential files. These are security invariants; other capabilities build on them and must not weaken them.

## Requirements

### Requirement: Loopback-Only Listener
The server SHALL listen only on loopback addresses. `splitty serve` and `splitty daemon start` MUST resolve the requested `--addr` (default `127.0.0.1:9800`, overridable via `SPLITTY_ADDR`) and refuse to start, exiting with status 2 before serving anything, if any resolved address is not loopback. The removed `--remote` flag MUST be refused the same way, with a message pointing to `splitty connect`.

#### Scenario: Routable bind address is refused
- **WHEN** the user runs `splitty serve --addr 0.0.0.0:8080`
- **THEN** the process exits with status 2 without accepting any connection
- **AND** the error explains that Splitty listens on loopback only

#### Scenario: Name resolving to a routable address is refused
- **WHEN** the requested address is a hostname that resolves to a loopback and a non-loopback address
- **THEN** the server refuses to start rather than binding only the loopback part

#### Scenario: Direct remote mode is refused
- **WHEN** the user passes `--remote` to `splitty serve` or `splitty daemon start`
- **THEN** the process exits with status 2 and suggests `splitty connect <user@host>`

### Requirement: Host Header Guard
The server SHALL reject with 403 every HTTP request on its TCP listener whose `Host` header, with any port removed, is not exactly `localhost`, `127.0.0.1` or `::1`; a missing or empty `Host` MUST be rejected as well. This guard defends against DNS rebinding and applies to every path, including `/health`; it does not replace the loopback-only listener.

#### Scenario: Foreign host name
- **WHEN** a request arrives with `Host: evil.example:9800`
- **THEN** the server responds 403 without evaluating any credential

#### Scenario: Loopback host with port
- **WHEN** a request arrives with `Host: 127.0.0.1:9800` or `Host: [::1]:9800`
- **THEN** the Host guard lets the request through to the next check

### Requirement: Origin Allowlist
The server SHALL accept a browser `Origin` only if it is exactly `http://127.0.0.1:<port>` or `http://localhost:<port>`, where `<port>` is the port actually bound (not the requested one, so `--addr 127.0.0.1:0` works). A release build MUST NOT accept any other origin and MUST NOT offer a runtime option to widen the list; development builds additionally accept the dev-server origins on port 1420. For every path except `/health`, a request carrying a non-allowed `Origin` MUST be rejected with 403. A `/ws` request without any `Origin` MUST be rejected with 403; an `/api` request without `Origin` is not rejected by this rule, because same-origin GETs and the CLI send none.

#### Scenario: Foreign origin with a valid credential
- **WHEN** a request to an `/api` path carries a valid credential and `Origin: https://evil.example`
- **THEN** the server responds 403

#### Scenario: Other loopback port is a foreign origin
- **WHEN** a request carries `Origin: http://127.0.0.1:<other port>`
- **THEN** the server responds 403, even though the page is same-site with Splitty

#### Scenario: WebSocket without Origin
- **WHEN** a `/ws` upgrade request carries no `Origin` header
- **THEN** the server responds 403

#### Scenario: CLI request without Origin
- **WHEN** the CLI calls an `/api` path with a valid bearer token and no `Origin` header
- **THEN** the Origin rule does not reject the request

### Requirement: Fetch Metadata Same-Origin Rule
For `/api` paths and `/ws`, the server SHALL reject with 403 any request whose `Sec-Fetch-Site` header is present and not `same-origin`. This MUST hold for `same-site`, `cross-site` and `none` (an address-bar navigation). A request without `Sec-Fetch-Site` is not rejected by this rule; the Origin rules still apply to it.

#### Scenario: Same-site request with the session cookie
- **WHEN** a page on another loopback port causes a GET to `/api/mux/status` carrying the session cookie and `Sec-Fetch-Site: same-site`
- **THEN** the server responds 403

#### Scenario: Address-bar navigation to the API
- **WHEN** a request to an `/api` path carries `Sec-Fetch-Site: none`
- **THEN** the server responds 403

#### Scenario: Same-origin request from the UI
- **WHEN** the UI calls an `/api` path with `Sec-Fetch-Site: same-origin`, its own `Origin` and a valid session cookie
- **THEN** the request is processed

### Requirement: Guards Precede Authentication
The server MUST apply the Host guard first, then the Origin and Fetch Metadata rules, and only then evaluate credentials. A request rejected by a guard SHALL receive 403 regardless of whether it carried a valid credential, so a foreign origin cannot learn whether a guessed credential is valid.

#### Scenario: Foreign origin probing a ticket
- **WHEN** a `/ws` request from a foreign origin carries a guessed ticket
- **THEN** the server responds 403 and the ticket is neither checked nor consumed

### Requirement: Accepted HTTP Credentials
Every request to a path under `/api` or `/proxy/` SHALL require a credential, except `/api/session` (which establishes one) and `/ws` (which uses a ticket). Exactly two credentials MUST be accepted: a valid `splitty-session` cookie, or `Authorization: Bearer <process token>`. A missing or invalid credential MUST yield 401. A token in the `?token=` query parameter and the former `splitty-token` cookie MUST NOT authenticate. Empty values MUST never authenticate, and the token comparison SHALL be constant-time.

#### Scenario: No credential
- **WHEN** a request to `/api/mux/state` carries no cookie and no `Authorization` header
- **THEN** the server responds 401

#### Scenario: Token in the query string
- **WHEN** a request to an `/api` path carries the correct process token only as `?token=<token>`
- **THEN** the server responds 401

#### Scenario: Legacy token cookie
- **WHEN** a request carries the correct process token only in a `splitty-token` cookie
- **THEN** the server responds 401

#### Scenario: Bearer token from the CLI
- **WHEN** a request carries `Authorization: Bearer <process token>` and passes the guards
- **THEN** the request is processed

### Requirement: Unauthenticated Surface
The server SHALL serve without any credential only: `/health` (also exempt from the Origin and Fetch Metadata rules), `POST /api/session`, the `/ws` handshake (which is ticket-gated instead), and the application shell with its static assets. Paths under `/api/` and `/proxy/` and the `/ws` prefix that have no handler MUST NOT be answered with the application shell; an authenticated caller MUST receive 404 for them. The removed web-content routes (`/proxy/{port}`, `/api/favicon`, `/api/frame-check`, `/api/webapps`) MUST answer 404 even with a valid credential.

#### Scenario: Health check
- **WHEN** a client sends `GET /health` with a loopback `Host` and no credential
- **THEN** the server responds 200 with `ok`

#### Scenario: Removed route with a valid credential
- **WHEN** an authenticated caller requests `/proxy/8080/` or `/api/favicon`
- **THEN** the server responds 404, not 200 with the application shell

#### Scenario: Client-side app route
- **WHEN** a browser requests an application route outside the server prefixes
- **THEN** the server responds 200 with the application shell

### Requirement: Process Token
The server SHALL authenticate non-browser callers with a process token of 128 random bits encoded as 32 hex characters. Unless overridden, the token MUST be read from `auth-token` in the Splitty config directory (`~/.config/splitty/auth-token` on Linux) and created there if missing, so it is stable across restarts. A non-empty `--token` flag MUST take precedence over the file; the `SPLITTY_TOKEN` environment variable MUST NOT be consulted. `splitty serve --oneshot-token` MUST use a fresh random token that is not persisted; if the token file cannot be read or created, the server MUST fall back to a random, non-persisted token. The process token MUST NOT be sent to the browser. Rotation SHALL be done by deleting the token file and restarting the server.

#### Scenario: First start creates the token file
- **WHEN** a server starts and no `auth-token` file exists
- **THEN** a 32-hex-character token is written to it with mode 0600
- **AND** the next server start reuses the same token

#### Scenario: Inherited SPLITTY_TOKEN is ignored
- **WHEN** a server is started with `SPLITTY_TOKEN` set in its environment and no `--token` flag
- **THEN** it uses the token from its own token file, not the environment value

#### Scenario: One-shot token
- **WHEN** the user runs `splitty serve --oneshot-token`
- **THEN** the server authenticates bearer requests with a random token that is not written to disk

### Requirement: Browser Session Establishment
`POST /api/session` SHALL exchange either a `bootstrap` code or the process `token` (JSON body) for a browser session; a non-empty `bootstrap` field MUST take precedence. On success the server MUST respond 200 with `Set-Cookie: splitty-session=<id>; Path=/; HttpOnly; SameSite=Strict`, where the session id is 128 random bits. On failure it MUST respond 401 and set no cookie. The endpoint is subject to the Host, Origin and Fetch Metadata guards. Sessions MUST NOT be persisted: they stay valid for the life of the server process and are all invalidated by a restart.

#### Scenario: Valid bootstrap code
- **WHEN** the UI posts `{"bootstrap": "<code>"}` with a valid, unexpired, unused code from its own origin
- **THEN** the server responds 200 with an `HttpOnly`, `SameSite=Strict` `splitty-session` cookie

#### Scenario: Manually entered process token
- **WHEN** the UI posts `{"token": "<process token>"}`
- **THEN** the server responds 200 with a session cookie

#### Scenario: Invalid credential
- **WHEN** the body carries an unknown code, a wrong token, or neither
- **THEN** the server responds 401 and sets no cookie

#### Scenario: Session after restart
- **WHEN** a browser presents a `splitty-session` cookie issued before the server restarted
- **THEN** `/api` requests with only that cookie receive 401

### Requirement: Single-Use Bootstrap Codes
A bootstrap code SHALL be 128 random bits as 32 hex characters, valid for exactly one redemption and for at most 120 seconds after issue. Any redemption attempt MUST consume the code, including attempts on an expired code, and codes MUST NOT be persisted. Codes SHALL be issued only: once at startup of an embedded server (`splitty serve` or the desktop app's in-process server) for its launcher; by `POST /api/bootstrap` to an authenticated caller over TCP; by `splitty issue-bootstrap`, which calls that endpoint with the process token from `--token` or the token file; and, under `splitty serve --desktop-stdio`, in reply to a `bootstrap` line on stdin. A bootstrap code MUST never equal the process token.

#### Scenario: Replay
- **WHEN** a bootstrap code that was already exchanged is posted to `/api/session` again
- **THEN** the server responds 401

#### Scenario: Expired code
- **WHEN** a bootstrap code is redeemed more than 120 seconds after it was issued
- **THEN** the server responds 401 and the code is gone

#### Scenario: Launcher requests a fresh code
- **WHEN** the native launcher writes `bootstrap` followed by a newline to the stdin of `splitty serve --desktop-stdio`
- **THEN** the server writes `SPLITTY_BOOTSTRAP=<new code>` to stdout

### Requirement: WebSocket Ticket Authentication
The `/ws` upgrade SHALL be authorized only by a ticket passed as `?ticket=<ticket>`; the session cookie alone MUST NOT authorize it. Tickets SHALL be issued by `POST /api/ws-ticket`, which requires an authenticated caller and passes the guards. A ticket MUST be single-use and expire 10 seconds after issue. A missing, unknown, expired or reused ticket MUST yield 401. Independently of the request pipeline, the upgrade MUST itself require an allowed `Origin` and an acceptable `Sec-Fetch-Site`.

#### Scenario: Ticket without a session
- **WHEN** a caller with no credential posts to `/api/ws-ticket`
- **THEN** the server responds 401

#### Scenario: Ticket replay
- **WHEN** a ticket that already opened a WebSocket is used for a second handshake from the app's own origin
- **THEN** the server responds 401

#### Scenario: Valid handshake
- **WHEN** the UI opens `/ws?ticket=<fresh ticket>` from its own origin
- **THEN** the server upgrades the connection

### Requirement: No CORS Allowance
The server SHALL NOT emit `Access-Control-Allow-Origin` or other CORS allowance headers on any response, including preflights. The UI is served from the same origin as the API and needs none.

#### Scenario: Preflight from a foreign origin
- **WHEN** a foreign origin sends a CORS preflight to an `/api` path
- **THEN** the response carries no `Access-Control-Allow-Origin` header

### Requirement: Frontend Security Headers
Every response serving the application shell or a static asset SHALL carry: a `Content-Security-Policy` of `default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self' ws://127.0.0.1:<port> ws://localhost:<port>; frame-src 'none'; frame-ancestors 'none'; object-src 'none'; base-uri 'none'; form-action 'none'`, with `<port>` the bound port; `X-Content-Type-Options: nosniff`; `Referrer-Policy: no-referrer`; `Permissions-Policy: camera=(), microphone=(), geolocation=(), clipboard-read=()`; `Cross-Origin-Opener-Policy: same-origin`; and `Cross-Origin-Resource-Policy: same-origin`.

#### Scenario: App document
- **WHEN** a browser loads the application shell
- **THEN** the response carries the deny-by-default CSP with `frame-src 'none'` and `frame-ancestors 'none'` and all listed headers

#### Scenario: Static asset
- **WHEN** a browser loads a bundled script or stylesheet
- **THEN** the response carries the same security headers as the application shell

### Requirement: No Reusable Credential In Output
The server SHALL NOT print the process token, a session id or a ticket on stdout, stderr or in log lines. The only credential allowed in the machine-readable startup lines (see `cli`) is a single-use bootstrap code, and log lines MUST NOT contain the bootstrap code. `splitty daemon start` MUST print no credential at all.

#### Scenario: Serve startup
- **WHEN** `splitty serve` starts
- **THEN** its output contains a bootstrap code that differs from the process token and does not contain the process token

#### Scenario: Daemon startup
- **WHEN** `splitty daemon start` starts a daemon
- **THEN** its output contains the port and no credential

### Requirement: Credential Files Are Owner-Only
On Unix, the Splitty config directory SHALL have mode 0700, and every writer MUST create it with that mode or repair an existing directory to it. The `auth-token` and `daemon.json` files MUST have mode 0600, be written atomically so they are never observable partially written or with a looser mode, and an existing `auth-token` with a looser mode MUST be repaired to 0600 when it is read. `daemon.json` MUST NOT contain any credential (its contents are defined in `server-modes`), and a `token` key left by an older version MUST be ignored.

#### Scenario: Directory left behind by an older version
- **WHEN** the config directory exists with mode 0755 and the server starts
- **THEN** the directory ends up with mode 0700

#### Scenario: Token file with loose permissions
- **WHEN** an existing `auth-token` has mode 0644
- **THEN** the server keeps its value and changes its mode to 0600

#### Scenario: Daemon info contents
- **WHEN** a daemon writes `daemon.json`
- **THEN** the file has mode 0600 and contains no token

### Requirement: Owner-Only Control Socket
Each server process SHALL offer a local control channel as a Unix socket. A daemon's socket SHALL be `daemon.sock` in its run directory: `$SPLITTY_CONFIG_DIR/run/` when `SPLITTY_CONFIG_DIR` names a configuration directory (see `server-modes`), otherwise `<user data dir>/splitty/run/`. Any other server's socket SHALL be `<pid>.sock` in `$SPLITTY_CONFIG_DIR/run/` when `SPLITTY_CONFIG_DIR` names a configuration directory, otherwise in `$XDG_RUNTIME_DIR/splitty/` (or `<user data dir>/splitty/run/` when `XDG_RUNTIME_DIR` is unset or relative). The socket's directory MUST have mode 0700 and the socket mode 0600. On Linux and macOS, the server MUST drop any connection whose peer user id differs from its own effective user id, and MUST also drop it when the peer credentials cannot be read. The channel requires no credential and applies no Host or Origin check: filesystem permissions and the peer check are its authorization. The server MUST refuse to place the socket under `/mnt/` or `/media/`; failing to create the socket MUST NOT stop the server and MUST NOT cause any credential to be handed to panes instead. `splitty serve` and the daemon SHALL remove their socket file on graceful shutdown.

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
- **WHEN** `splitty serve` runs with `SPLITTY_CONFIG_DIR=/Users/me/.config/splitty`
- **THEN** its control socket is `/Users/me/.config/splitty/run/<pid>.sock`
- **AND** a daemon with the same variable uses `/Users/me/.config/splitty/run/daemon.sock`

#### Scenario: The daemon's socket survives a restart
- **WHEN** a daemon restarts and adopts a shell that was started before
- **THEN** the new daemon's control socket has the path the shell's `SPLITTY_SOCK` names, and `splitty list-screens` run in that shell reaches the new daemon

### Requirement: Control Socket Route Set
The owner control socket SHALL route only these multiplexer endpoints: `GET /api/mux/state`, `GET /api/mux/screens`, `GET /api/mux/screen/{id}`, `POST /api/mux/op`, `GET /api/mux/neighbor`, `GET /api/mux/status` and `POST /api/session/{id}/send`, plus `POST /api/editor/open` for confirmed client editor requests, `POST /api/file/offer` for file offers the user saves in a window, and the job and local agent-administration endpoints specified in `control-api`. Every other path MUST answer 404 there, in particular configuration, shell profiles, themes, templates, stats, and the credential-minting endpoints `/api/session`, `/api/bootstrap` and `/api/ws-ticket`. The separate restricted agent sockets SHALL expose only their fixed subset below. No local socket SHALL mint browser or network credentials.

#### Scenario: Multiplexer status over the socket
- **WHEN** a pane process sends `GET /api/mux/status` over the owner control socket without any credential
- **THEN** the server responds 200

#### Scenario: Privileged endpoint over the socket
- **WHEN** a pane process sends `POST /api/bootstrap` or `GET /api/home/settings` over any local control socket
- **THEN** the server responds 404

#### Scenario: Job administration over the owner socket
- **WHEN** the owning user creates an agent grant through the owner control socket
- **THEN** a local restricted socket is created and no bearer token or browser credential is issued

#### Scenario: Editor request on the owner socket
- **WHEN** the owner socket receives `POST /api/editor/open`
- **THEN** it may request a local UI confirmation but cannot authorize a launch, choose a destination host, or mint credentials
- **AND** the same route on a restricted grant socket answers 404

#### Scenario: File offer on the owner socket
- **WHEN** the owner socket receives `POST /api/file/offer`
- **THEN** it may offer the file to the windows of the session, which save it only on the user's Save
- **AND** the same route on a restricted grant socket answers 404

### Requirement: Pane Environment Carries No Credential
When the server spawns a terminal session, it SHALL pass the owner control socket path as `SPLITTY_SOCK` for ordinary sessions, or the restricted grant socket as `SPLITTY_AGENT_SOCK` for an agent-owned managed job. An agent-owned job MUST NOT receive `SPLITTY_SOCK`, including an inherited value. The server MUST NOT set the process token, a session id used as an authentication credential, a bootstrap code, a ticket or the server's HTTP address in the session's environment (in particular it sets neither `SPLITTY_TOKEN` nor `SPLITTY_SERVER`). Other variables given to sessions are defined in `terminal-sessions`.

#### Scenario: Spawned pane
- **WHEN** an ordinary pane's shell is spawned
- **THEN** its environment contains `SPLITTY_SOCK=<socket path>`
- **AND** the server has set no `SPLITTY_TOKEN` or `SPLITTY_SERVER` variable

#### Scenario: Agent-owned job environment
- **WHEN** an agent-owned job starts while the server inherited owner or agent socket variables
- **THEN** the job receives only its own grant's `SPLITTY_AGENT_SOCK` and pane/screen/session identifiers, and neither the owner socket nor another grant's socket from that inheritance

### Requirement: Browser UI Holds No Credential
The browser UI SHALL NOT store any credential in `localStorage`, in script-readable cookies or in request URLs; its only credential is the `HttpOnly` session cookie. After exchanging a `bootstrap` URL parameter, the UI MUST remove that parameter from the address bar. It MUST fetch a fresh WebSocket ticket for every connection attempt, including reconnects. Without a bootstrap code or a valid session, it SHALL ask for the process token and exchange it once through `POST /api/session`.

#### Scenario: Launch URL with a bootstrap code
- **WHEN** the UI loads with `?bootstrap=<code>`
- **THEN** it exchanges the code for a session cookie and replaces the URL with one that no longer contains `bootstrap`

#### Scenario: Reconnect
- **WHEN** the WebSocket drops and the UI reconnects
- **THEN** it requests a new ticket from `/api/ws-ticket` before the handshake

### Requirement: Remote Access Through SSH Tunnel
A remote server SHALL only be reached through an SSH tunnel opened by `splitty connect <target>` or by a desktop app's remote window (see `desktop-remote`), with the remote server still bound to loopback. The target MUST be validated as `[user@]host[:port]` (bracketed IPv6 allowed) before `ssh` is invoked; a target starting with a dash (including Unicode dash lookalikes) or containing whitespace or control characters MUST be rejected, by `splitty connect` with exit status 2.

The local end of the forward MUST be bound to `127.0.0.1` explicitly, whatever the user's SSH configuration says, and MUST use the same loopback port as the remote daemon, so that the browser's origin matches the remote server's allowlist. The remote server MUST NOT widen its allowlist for tunnels. The forward MUST be verified before any credential is requested. When the tunnel drops, the client MUST reserve the local port again as soon as it notices, and hold it until a rebuilt forward takes it over, so that no other local process can take the port and receive the session's requests.

Only a single-use bootstrap code, minted on the remote with `splitty issue-bootstrap`, crosses the link; the remote process token MUST NOT.

#### Scenario: Option-like target
- **WHEN** the user runs `splitty connect -oProxyCommand=x`
- **THEN** `splitty connect` exits with status 2 without running `ssh`

#### Scenario: Matching origin through the tunnel
- **WHEN** the remote daemon listens on port 9800 and the local port 9800 is free
- **THEN** the tunnel forwards `127.0.0.1:9800` to the remote `127.0.0.1:9800`
- **AND** the printed `SPLITTY_URL` carries a single-use bootstrap code accepted by the remote server's origin check

#### Scenario: GatewayPorts in the SSH configuration
- **WHEN** the user's SSH configuration sets `GatewayPorts yes` and the user runs `splitty connect`
- **THEN** the forward listens on `127.0.0.1` only

#### Scenario: Port held while the tunnel is rebuilt
- **WHEN** the tunnel has dropped and the client waits for its next attempt
- **THEN** no other local process can bind the local port

### Requirement: Remote Installation Integrity
A server installed on a remote host by `splitty connect` SHALL come only from the client, over the SSH connection; the host MUST NOT download anything for it. Its SHA-256 checksum, computed by the client over the bytes it sends, MUST be verified on the host before the payload is unpacked or run, and the server MUST NOT be moved into place when the check fails. The install directories below `~/.local/share/splitty/remote/` MUST be created owner-only (mode 0700).

Commands sent to the host SHALL be constant scripts; every dynamic value SHALL be passed as an argument quoted for a POSIX shell, and versions and checksums MUST match an allowlist pattern before they are sent. A path the host reports, such as its home directory, MUST be absolute and free of control characters before the client uses it, and MUST be quoted wherever it is sent back.

#### Scenario: Hostile home directory
- **WHEN** a host reports a home directory containing quotes, `$(…)`, backquotes or a leading dash
- **THEN** the client either rejects it or uses it only quoted, and no part of it is executed on the host

#### Scenario: Checksum mismatch
- **WHEN** the payload is altered on its way to the host
- **THEN** the host-side check fails before the payload is unpacked, and nothing is installed

### Requirement: Remote Bridge Carries No Credential
`splitty remote-bridge` SHALL authenticate to the daemon only on the host itself: with the process token from the owner-only token file of the user it runs as, it obtains a single-use WebSocket ticket and opens the WebSocket with the daemon's own origin. No token, ticket, bootstrap code or session cookie SHALL be read from or written to the bridge's standard input or output; the SSH connection that carries the bridge is its only authentication. The bridge SHALL reach the daemon over loopback only.

#### Scenario: Nothing secret on the wire
- **WHEN** a client relays a session through `splitty remote-bridge`
- **THEN** the bytes on the bridge's standard input and output contain only the WebSocket messages of the session and mux protocols, and no credential

### Requirement: Forwarded Ports Bound To Loopback
Every port forward of `splitty connect --forward` MUST be bound to `127.0.0.1` on the PC explicitly, whatever the user's SSH configuration says, and MUST be held reserved while the tunnel is rebuilt, like the daemon's port. The CLI MUST announce a forward only as `http://localhost:<port>`, never as `http://127.0.0.1:<port>`, so that the browser does not send it the session cookie of Splitty servers, which belongs to the host `127.0.0.1`.

#### Scenario: GatewayPorts in the SSH configuration
- **WHEN** the user's SSH configuration sets `GatewayPorts yes` and the user runs `splitty connect vm1 --forward 3000`
- **THEN** the forward of port 3000 listens on `127.0.0.1` only

#### Scenario: No session cookie for the dev server
- **WHEN** the user opens a forward's announced URL in the browser that also holds a Splitty session for `127.0.0.1`
- **THEN** the forwarded server receives no Splitty session cookie

### Requirement: Remote Window Isolation
A remote window (see `desktop-remote`) shows UI code delivered by the host and SHALL be treated as untrusted:
- It SHALL be able to call no native command of the shell.
- It SHALL have a cookie store of its own that is not persisted and that no other window uses.
- It SHALL navigate only within its tunnel origin `http://127.0.0.1:<port>`. Other `http` and `https` URLs SHALL open in the system browser; every other target SHALL be blocked, including the shell's bundled pages.
- It SHALL NOT open new windows.
- Of the browser permissions, it SHALL be granted only notifications and clipboard writes, and only for its tunnel origin.

#### Scenario: Host UI calls a native command
- **WHEN** the page in a remote window calls a shell command, such as `remote_open` or a window command
- **THEN** the call is rejected

#### Scenario: Separate session cookies
- **WHEN** a remote window for `vm1` holds a session cookie
- **THEN** no other window and no browser profile sends that cookie, and the remote window sends no cookie of another window

#### Scenario: Navigation away
- **WHEN** the page in a remote window navigates to `https://example.com` or to the shell's bundled setup page
- **THEN** the remote window stays on its tunnel origin; the web URL opens in the system browser and the bundled page is blocked

### Requirement: Desktop Shell Command Scope
A desktop shell's native commands SHALL be callable only:
- from its own bundled pages;
- from its main window while it shows the origin of the shell's own backend, `http://127.0.0.1:<port>` with the exact port.

A page on any other loopback port SHALL be able to call none of them, even in the main window.

#### Scenario: Another loopback port
- **WHEN** a page on `http://127.0.0.1:<other port>` ends up in the Tauri app's main window
- **THEN** its calls of native commands are rejected

### Requirement: Remote Screen Hub Access
`/ws/remote` SHALL require the same origin check, same-origin fetch rule and single-use WebSocket ticket as `/ws`, and `/ws/remote` and `/api/remote/*` SHALL NOT be served on the control socket. The hub SHALL run `ssh` only to a target that passed the target validation of Remote Access Through SSH Tunnel, non-interactively, without agent forwarding, and without remote forwards except the single remote forward of a reverse forward the user requested (see Hub Reverse Forwards). The hub SHALL send nothing to the host but the client's relayed messages and the commands of the install, the handshake and the bridge, `remote-relay prepare` and `remote-relay serve` for a reverse forward, and, for a socket opened with `update=1` (see `remote-screens`, Host Update), `daemon stop` of its installed server. In particular, it SHALL NOT forward the local session cookie, a ticket or the process token.

#### Scenario: Missing ticket
- **WHEN** a client opens `/ws/remote?target=vm1` without a valid ticket
- **THEN** the upgrade is refused and no `ssh` process is started

#### Scenario: Control socket
- **WHEN** a process requests `/ws/remote` or `/api/remote/hosts` over the control socket
- **THEN** the request is not served

#### Scenario: Update stops only the daemon
- **WHEN** a client opens `/ws/remote` with `update=1` for a host whose daemon runs another version
- **THEN** the only command the hub sends beyond the install, the handshake and the bridge is `daemon stop` of its installed server

#### Scenario: Remote forwards only for reverse forwards
- **WHEN** the hub opens screens of `vm1`, forwards a port of `vm1` and shares a local port with `vm1`
- **THEN** only the `ssh` of the shared port carries a remote forward, and it names a Unix socket on the host

### Requirement: Hosts Cannot Reach Local Objects
A client showing screens of several servers SHALL resolve every screen, pane and session id, every attach, spawn notification and notification, and every server command only against the objects of the connection it arrived on. A host SHALL NOT be able to change, focus, attach to, send input to or close a local pane or another host's pane, whatever ids it sends. A server command from a host SHALL NOT paste the local clipboard. Names, titles and notifications from a host SHALL be shown as plain text, and a host's notification SHALL name the host.

#### Scenario: Colliding ids
- **WHEN** a host sends a snapshot, output and `pane.close` using the ids of a local screen, pane and session
- **THEN** no local screen, pane or session changes, and nothing is sent to the local server

#### Scenario: Paste from a host
- **WHEN** a host sends the server command `pane.paste`
- **THEN** the client does not read the clipboard and sends nothing to the host

### Requirement: Hub Port Forwards
A forward opened by the hub MUST be bound to `127.0.0.1` explicitly, whatever the user's SSH configuration says. It MUST be announced only as `http://localhost:<port>`, never as `http://127.0.0.1:<port>`, so that the browser does not send it Splitty's session cookie. Its `ssh` MUST run non-interactively, with no agent forwarding, no remote forwards and no terminal, to a target that passed the target validation of Remote Access Through SSH Tunnel, and remote and explicit local ports MUST be integers from 1 to 65535.

A forward MUST NOT be opened without the user's forward action or remote loopback-link click, unless the user turned on `remote.autoForwardPorts`, or previously opted to remember an enabled mapping and now connects that host successfully. Saving preferences, starting the local server, authenticating, reading state and observing ports MUST NOT by themselves authorize saved restoration. A disabled saved rule MUST override automatic forwarding. `/api/remote/forwards` and `/api/remote/forward-preferences` MUST NOT be served on the control socket.

A selected local port MUST be checked for availability on both `127.0.0.1` and `::1`, including explicit and saved ports and automatically chosen candidates. A busy address or an unexplained failure to establish availability MUST NOT be treated as free; an unavailable IPv6 stack alone MUST NOT disable IPv4 forwarding. Rebuilds MUST retain the selected port. Forward URLs MUST contain no Splitty token, bootstrap code, ticket or session credential. Labels MUST NOT affect SSH arguments, destinations or URLs.

#### Scenario: GatewayPorts in the SSH configuration
- **WHEN** the user's SSH configuration sets `GatewayPorts yes` and the hub forwards port 3000 of a host
- **THEN** the forward listens on `127.0.0.1` only

#### Scenario: Forwards in the SSH configuration
- **WHEN** the user's SSH configuration sets `RemoteForward` for `vm1` and the UI requests a forward of a port of `vm1`
- **THEN** the request is refused with a message naming the configured forward, and no `ssh` connects to `vm1` for it

#### Scenario: Control socket
- **WHEN** a process requests `/api/remote/forwards` or `/api/remote/forward-preferences` over the control socket
- **THEN** the request is not served

#### Scenario: Remembered authorization is limited
- **WHEN** the local server starts, a client signs in or reads or edits saved preferences without connecting a host or requesting a forward
- **THEN** no saved forward is opened, and connecting one host later authorizes only that host's enabled rules

#### Scenario: Strict loopback availability
- **WHEN** a selected port is occupied on IPv6 alone or its availability check fails for a reason other than an unavailable address family
- **THEN** it is not used as an available local port
- **AND** a genuinely unavailable IPv6 stack permits a port verified free on IPv4, and a saved-port rebuild never silently changes ports

#### Scenario: Development URL carries no Splitty credential
- **WHEN** the client opens or rewrites a saved forward's URL in the browser
- **THEN** the hostname is localhost, no Splitty credential is added, and the development service receives no Splitty cookie belonging to 127.0.0.1
- **AND** SSH carries the service's TCP bytes without injecting HTTP authentication or changing TLS or WebSocket traffic

### Requirement: Explicit Remote Editor Dispatch
Only a deliberate editor action in the trusted hub UI SHALL dispatch a local VS Code or Zed folder handoff for the selected local or SSH terminal, or an SFTP client handoff for the selected SSH terminal. Native editor dispatch SHALL be granted solely to the main local UI on the shell's exact backend origin, not bundled setup pages, remote windows or popouts. Electron SHALL additionally reject child-frame calls. Tauri relies on the existing no-frame content policy; its native grant is scoped by window and exact origin. It SHALL validate the fixed editor enum (`vscode`, `zed`, or `sftp` for remote folders only), an absolute directory, an optional restricted SSH target for remote folders, and an optional validated WSL distribution for local VS Code folders and generate the fixed editor URI itself; no arbitrary URI, executable, command string or shell interpolation SHALL be accepted. Terminal links, remote output, server commands and URL query parameters SHALL NOT invoke this dedicated path. Existing generic URL openers SHALL continue to admit only HTTP(S); remote windows SHALL retain their isolation.

#### Scenario: Dedicated native command scope
- **WHEN** a remote window, another loopback origin or popout (or, in Electron, a child frame) attempts editor dispatch
- **THEN** the request is rejected before any native handoff

#### Scenario: Hostile dispatch inputs
- **WHEN** a caller supplies an executable/URI as target, an option-like target, a relative directory, control characters, unsupported SSH syntax, or `sftp` for a local folder
- **THEN** native dispatch rejects the request before opening anything

#### Scenario: Terminal link uses editor scheme
- **WHEN** a hub remote terminal link, Splitty's generic URL helper or a native navigation guard receives a `vscode:`, `zed:` or `sftp:` URI
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
Grant administration SHALL be available only on the owner control socket. Grant sockets and IDs SHALL survive agent-client exits but expire when their server process ends; stale sockets SHALL never be rebound to another grant. Documentation SHALL identify the unrestricted same-user owner socket, token files and daemon-executed jobs as outside this scoped API boundary, and SHALL not claim OS or sudo isolation.

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
Every `/api/remote/forward-preferences` method SHALL require ordinary API authentication after the Host, Origin and Fetch Metadata guards. Targets and ports SHALL be validated before any write or SSH invocation. Invalid requests SHALL leave saved and live state unchanged. The new endpoint SHALL grant no access to local control-socket callers or to remote-window native commands.

#### Scenario: Unauthenticated or cross-origin preference access
- **WHEN** GET, PUT or DELETE lacks valid authentication or fails the Host, Origin or Fetch Metadata guard
- **THEN** it returns the existing 401 or 403 response as appropriate, before any metadata or forwarding side effect
- **AND** an otherwise authenticated page on another local development port has no exception to those guards

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
A client SHALL send `save_image` and `upload` only for the user's own drop or paste into a pane or a scratchpad, only to the server of that pane's connection, and SHALL paste or insert the returned paths only there. A server SHALL store an upload only under the directory and the sanitized name it chooses itself; `name` SHALL NOT select a directory. A client SHALL read an offered file only after the user chose Save for it, only from the connection that offered it, and SHALL save it only as a download of the window under its sanitized name. File bytes SHALL be relayed by the hub like any other session message and SHALL NOT be passed to a shell.

#### Scenario: Upload name with a path
- **WHEN** an upload names `../../.ssh/authorized_keys`
- **THEN** the server stores it as `authorized_keys` inside a new `upload-` directory of its paste directory

#### Scenario: Offer without Save
- **WHEN** a host offers a file and the user does nothing
- **THEN** the window reads nothing and the offer expires

### Requirement: Showing A Saved Download
A desktop shell SHALL show a file in the system file manager only for a download of its main window that it saved itself and that finished, named by the page with that download's `blob:` URL (`reveal_download` in the Tauri app, `download:reveal` in the Electron app, both under Desktop Shell Command Scope). It SHALL NOT take a path or a file name from the page, SHALL NOT tell the page where it saved the file, and SHALL refuse every other URL, an unfinished, failed or cancelled download, and a download beyond the most recent 32.

#### Scenario: A finished download
- **WHEN** the main window saved a download from its `blob:` URL and asks the shell to show that URL
- **THEN** the shell opens the file manager on the saved file and answers the page without its path

#### Scenario: A URL the shell did not save
- **WHEN** the page asks the shell to show a URL that is not one of its finished downloads
- **THEN** the shell refuses and shows nothing

### Requirement: Hub Reverse Forwards
A reverse forward SHALL make `localhost:<local>` of this machine reachable on the host, and SHALL NOT make any other local address reachable: the API takes only a local port, and the forward's `ssh` SHALL name `localhost:<local>` as its destination. The local server's own HTTP port SHALL be refused with 409 `local_port_reserved`.

The remote end of the forward MUST be a Unix socket in an owner-only directory of the host's server, and the host side MUST listen for programs only through `splitty remote-relay serve` (see `remote-hosts`, Remote Relay), bound to `127.0.0.1` and, where available, `::1`. The relay MUST admit a TCP connection only when the connecting socket belongs to the user the relay runs as, and MUST close every other connection without passing it on, including one whose owner it cannot determine. A host where the owner cannot be determined (no `/proc/net/tcp`, such as macOS) SHALL be refused with code `unsupported_host` before anything listens there.

A reverse forward MUST NOT be opened without the user's explicit request, unless the user previously remembered it as an enabled rule and now connects its host successfully. Detected ports, link clicks in panes, `remote.autoForwardPorts`, messages from a host and remote windows SHALL NOT open one. The forward's `ssh` MUST run non-interactively, with no agent forwarding and no terminal, to a validated target; a host whose SSH configuration adds forwards SHALL be refused as for forwards. Ports MUST be integers from 1 to 65535. `/api/remote/reverse-forwards` and `/api/remote/reverse-forward-preferences` MUST NOT be served on the control socket and SHALL have the authentication and guards of the other `/api` routes; reverse-forward preferences SHALL be stored owner-only like forward preferences.

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
- **WHEN** a request asks to share the port of the local server itself
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
