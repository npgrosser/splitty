# Spec Delta

## MODIFIED Requirements

### Requirement: Loopback-Only Listener
Only an explicitly started browser gateway SHALL create a terminal-service TCP listener. `splitty serve` (including `--daemon`) MUST resolve its requested `--addr` (default `127.0.0.1:9800`, overridable via `SPLITTY_ADDR`) and refuse to start with status 2 before serving anything if any resolved address is not loopback. Supported browser binds SHALL be canonical `127.0.0.1` or `::1`; other loopback addresses SHALL fail with guidance to those addresses. `splitty connect` SHALL bind its browser gateway explicitly to `127.0.0.1`. Native desktop backends, daemons and SSH bridges SHALL create no terminal-service TCP listener, including on failure. The removed `--remote` flag MUST be refused the same way, with a message pointing to `splitty connect` or an explicit browser gateway.

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

#### Scenario: Unsupported loopback alias
- **WHEN** a browser gateway is asked to bind `127.0.0.2`
- **THEN** it fails before serving with guidance to a supported canonical loopback address

### Requirement: Origin Allowlist
A browser gateway SHALL accept an `Origin` only if it exactly names its canonical bound address (`http://127.0.0.1:<port>` or `http://[::1]:<port>`) or `http://localhost:<port>`, where `<port>` is the actual bound port, including for port 0. A hostname used to choose a bind SHALL not add another allowed origin. A release build MUST NOT accept any other origin and MUST NOT offer a runtime option to widen the list; development builds additionally accept the dev-server origins on port 1420. For every path except `/health`, a request carrying a non-allowed `Origin` MUST be rejected with 403. A `/ws` request without any `Origin` MUST be rejected with 403; an `/api` request without `Origin` is not rejected by this rule, because same-origin GETs and the CLI send none.

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

#### Scenario: IPv6 gateway origin
- **WHEN** a browser gateway binds `[::1]:0`
- **THEN** its launch URL, allowed HTTP origins and corresponding WebSocket CSP origins use the actual port and bracketed IPv6 address
- **AND** another address or port does not become allowed by supplying a valid credential

### Requirement: Accepted HTTP Credentials
Every request to a path under `/api` or `/proxy/` SHALL require a credential, except `/api/session` (which establishes one) and `/ws` (which uses a ticket). Exactly two credentials MUST be accepted: a valid `splitty-session-<instance>` cookie, or `Authorization: Bearer <process token>`. A missing or invalid credential MUST yield 401. A token in the `?token=` query parameter and the former `splitty-token` cookie MUST NOT authenticate. Empty values MUST never authenticate, and the token comparison SHALL be constant-time.

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

### Requirement: Browser Session Establishment
`POST /api/session` SHALL exchange either a `bootstrap` code or the process `token` (JSON body) for a browser session; a non-empty `bootstrap` field MUST take precedence. On success the gateway MUST respond 200 with `Set-Cookie: splitty-session-<instance>=<id>; Path=/; HttpOnly; SameSite=Strict`, where the session id is 128 random bits. On failure it MUST respond 401 and set no cookie. The endpoint is subject to the Host, Origin and Fetch Metadata guards. Sessions MUST NOT be persisted: they stay valid for the gateway's lifetime and are invalidated by its restart, independently of target daemon restarts.

#### Scenario: Valid bootstrap code
- **WHEN** the UI posts `{"bootstrap": "<code>"}` with a valid, unexpired, unused code from its own origin
- **THEN** the server responds 200 with an `HttpOnly`, `SameSite=Strict` `splitty-session-<instance>` cookie

#### Scenario: Manually entered process token
- **WHEN** the UI posts `{"token": "<process token>"}`
- **THEN** the server responds 200 with a session cookie

#### Scenario: Invalid credential
- **WHEN** the body carries an unknown code, a wrong token, or neither
- **THEN** the server responds 401 and sets no cookie

#### Scenario: Session after restart
- **WHEN** a browser presents a `splitty-session-<instance>` cookie issued before its gateway restarted
- **THEN** `/api` requests with only that cookie receive 401

### Requirement: Process Token
Native services and clients SHALL neither read nor create a process token for their native transport; the following token rules apply only to browser gateways.

Each explicitly started browser gateway SHALL authenticate non-browser callers with a process token of 128 random bits encoded as 32 hex characters. For `serve`, unless overridden, the token MUST be read from `auth-token` in the Splitty config directory (`~/.config/splitty/auth-token` on Linux) and created there if missing, so it is stable across restarts. A non-empty `--token` flag of the browser gateway MUST take precedence over the file; the `SPLITTY_TOKEN` environment variable MUST NOT be consulted. `splitty serve --oneshot-token` and the gateway of `splitty connect` MUST use a fresh random token that is not persisted; if the token file cannot be read or created, `serve` MUST fall back to a random, non-persisted token. The process token MUST NOT be sent to the browser. Rotation of a persisted token SHALL be done by deleting the token file and restarting the gateway.

#### Scenario: First start creates the token file
- **WHEN** `splitty serve` starts without a token override and no `auth-token` file exists
- **THEN** a 32-hex-character token is written to it with mode 0600
- **AND** the next server start reuses the same token

#### Scenario: Inherited SPLITTY_TOKEN is ignored
- **WHEN** `splitty serve` is started with `SPLITTY_TOKEN` set in its environment and no token override
- **THEN** it uses the token from its own token file, not the environment value

#### Scenario: One-shot token
- **WHEN** the user runs `splitty serve --oneshot-token`
- **THEN** the server authenticates bearer requests with a random token that is not written to disk

### Requirement: Single-Use Bootstrap Codes
A browser gateway's bootstrap code SHALL be 128 random bits as 32 hex characters, valid for one redemption and at most 120 seconds. Any redemption attempt SHALL consume it, including after expiry. Codes SHALL not persist or equal the process token. Only the explicit gateway's startup and its authenticated TCP `POST /api/bootstrap` endpoint, including `issue-bootstrap`, SHALL issue them. Native startup, SSH bridges and all local sockets SHALL issue none.

#### Scenario: Replay
- **WHEN** a redeemed bootstrap code is posted again
- **THEN** the gateway returns 401

#### Scenario: Expired code
- **WHEN** a code is redeemed more than 120 seconds after issue
- **THEN** the gateway returns 401 and removes it

#### Scenario: Native launcher
- **WHEN** a desktop backend or SSH bridge starts
- **THEN** it negotiates a native connection and emits no bootstrap code

#### Scenario: Launcher requests a fresh code
- **WHEN** a launcher sends the former text `bootstrap` command to a native child connection
- **THEN** it is rejected as invalid transport input and issues no credential

### Requirement: Frontend Security Headers
Every browser-gateway response serving the application shell or a static asset SHALL carry: a `Content-Security-Policy` of `default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self' <gateway WebSocket origins>; frame-src 'none'; frame-ancestors 'none'; object-src 'none'; base-uri 'none'; form-action 'none'`, where the WebSocket origins correspond exactly to the bound-port HTTP origins in Origin Allowlist; `X-Content-Type-Options: nosniff`; `Referrer-Policy: no-referrer`; `Permissions-Policy: camera=(), microphone=(), geolocation=(), clipboard-read=()`; `Cross-Origin-Opener-Policy: same-origin`; and `Cross-Origin-Resource-Policy: same-origin`.

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
- **THEN** its output describes the private client endpoint and contains no credential

### Requirement: Remote Access Through SSH Tunnel
A remote service SHALL be reached through SSH with the system client and validated target, using an exec stream to its private client endpoint. A target MUST match `[user@]host[:port]` (bracketed IPv6 allowed); option-like targets, Unicode dash lookalikes, whitespace and controls MUST be rejected before SSH starts, by `connect` with status 2. No terminal-server TCP port or credential SHALL be required on the host. An explicit browser connection SHALL terminate browser authentication at its local gateway. Command/stream connections SHALL disable configured local/remote/dynamic forwards, agent/X11 forwarding and PTYs. Deliberate forwarding connections SHALL retain their configured-forward checks and owned-process lifetime.

#### Scenario: Option-like target
- **WHEN** the user runs `splitty connect -oProxyCommand=x`
- **THEN** it exits with status 2 without running SSH

#### Scenario: Matching origin through the tunnel
- **WHEN** a remote daemon has only a private client endpoint
- **THEN** an explicit browser gateway uses its own local origin and requires no matching remote port or widened origin allowlist

#### Scenario: GatewayPorts in the SSH configuration
- **WHEN** SSH configuration sets `GatewayPorts yes`
- **THEN** desktop service traffic creates no TCP forward and a browser gateway still listens on `127.0.0.1` only

#### Scenario: Port held while the tunnel is rebuilt
- **WHEN** the gateway's SSH connection drops
- **THEN** the gateway keeps its listening socket bound and authenticates no operation to a different backend

#### Scenario: SSH configuration adds network features
- **WHEN** user SSH configuration enables local, remote or dynamic forwarding, agent/X11 forwarding or a PTY
- **THEN** native command/bridge connections create none of those features, while explicitly requested forwarding retains its existing validation

### Requirement: Remote Bridge Carries No Credential
`splitty remote-bridge --transport native-v1` SHALL resolve the daemon on the host and reach its private client endpoint using filesystem and peer-user checks. It SHALL neither read a token file nor obtain browser credentials. Its stdin/stdout SHALL carry only the negotiated client transport, without token, bootstrap code, browser cookie or WebSocket ticket. The SSH channel establishes the remote user; no terminal-service TCP connection is made.

#### Scenario: Nothing secret on the wire
- **WHEN** a client relays a session through the remote bridge
- **THEN** bridge traffic contains only transport control, permitted API messages and session/mux data, and no browser credential

### Requirement: Remote Window Isolation
A separate remote window SHALL load packaged application code and hold only a native connection bound to its assigned host. It SHALL have no local service, other-host, local editor, file-reveal, reverse-forward or native administration authority. Its storage SHALL be private, ephemeral and unshared. It SHALL have no opener, shared-JavaScript or named-window reference path into main/setup/popout or other remote contexts. Navigation SHALL remain within its assigned packaged document; external HTTP(S) links go to the system browser, other targets and new windows are blocked. Host-provided executable UI content SHALL never be loaded. Browser permissions SHALL be limited to notifications and clipboard writes for its assigned document.

#### Scenario: Remote window calls a local command
- **WHEN** a remote window invokes a local editor, host-management or general shell command
- **THEN** the shell rejects it before any local effect

#### Scenario: Host UI calls a native command
- **WHEN** a host-provided page attempts a shell command or native transport call
- **THEN** it receives no native authority even if its URL or arguments impersonate the packaged remote document

#### Scenario: Separate storage
- **WHEN** two remote windows use different hosts
- **THEN** neither window receives the other's connection handles, storage or credentials

#### Scenario: Separate session cookies
- **WHEN** a remote window's ephemeral cookie store contains a cookie
- **THEN** no other window or browser profile sends it, and the remote window receives none of their cookies
- **AND** native transport does not use that cookie as a credential

#### Scenario: Navigation away
- **WHEN** a remote window attempts to navigate to an external URL or the bundled setup page
- **THEN** it stays on its assigned document, only HTTP(S) links can open externally, and existing transport handles cannot survive a document replacement

#### Scenario: Host supplies application code
- **WHEN** a host response contains an HTML document, script URL or another backend target
- **THEN** it is treated as untrusted data and grants no code execution or connection authority

#### Scenario: Parent bridge through a window reference
- **WHEN** a remote window tries opener access, named-window reuse or a shared reference to reach the main window's bridge
- **THEN** no such reference or native authority is available, independently of cookie/storage isolation

### Requirement: Desktop Shell Command Scope
Native commands SHALL be admitted only for an authorized packaged caller, document lifetime and assigned role; payloads, queries and bundled origin alone SHALL not grant main authority. Main, setup and remote contexts SHALL have distinct allowlists. Popouts and child frames SHALL receive no independent direct native grant. Parent-owned same-origin Dockview popouts SHALL be documented as sharing main script authority, not as containing compromised popout code.

#### Scenario: Another loopback port
- **WHEN** a page on `http://127.0.0.1:<port>` attempts a native command
- **THEN** the shell rejects it even if the page is in the main window

#### Scenario: Forged main role
- **WHEN** setup, a remote window, a popout or a child frame supplies a main-window label or copied handle in a direct native invocation
- **THEN** it cannot obtain a direct main-window grant

#### Scenario: Shared-trust popout
- **WHEN** a Dockview popout retains same-origin references to its trusted parent
- **THEN** native calls executed by the parent are authorized as parent calls, and documentation/tests make no claim to isolate a compromised popout from that parent
- **AND** the popout opens no independent service connection or native grant

### Requirement: Remote Screen Hub Access
On a browser gateway, `/ws/remote` SHALL require the same origin check, same-origin fetch rule and single-use WebSocket ticket as `/ws`; native host connections SHALL require an authorized main-service connection with host-management authority, and `/ws/remote` and `/api/remote/*` SHALL NOT be served on the control socket. The hub SHALL run `ssh` only to a target that passed the target validation of Remote Access Through SSH Tunnel, non-interactively, without agent forwarding, and without remote forwards except the single remote forward of a reverse forward the user requested (see Hub Reverse Forwards). The hub SHALL send nothing to the host but the client's relayed messages and the commands of the install, the handshake and the bridge, `remote-relay prepare` and `remote-relay serve` for a reverse forward, and, for a browser socket opened with `update=1` or the corresponding explicit native update intent (see `remote-screens`, Host Update), instance-bound `daemon stop` or conditional `remote-migrate --expected <proof>` of its installed server. The host alone SHALL resolve and verify legacy owner sockets and process identity; renderer data SHALL NOT select the proof, PID, executable or configuration. The legacy migration command SHALL require the guards of Confirmed Legacy Daemon Migration and SHALL NOT be replayed after ambiguous completion. In particular, it SHALL NOT forward the local session cookie, a ticket or the process token.

#### Scenario: Missing ticket
- **WHEN** a client opens `/ws/remote?target=vm1` without a valid ticket
- **THEN** the upgrade is refused and no `ssh` process is started

#### Scenario: Control socket
- **WHEN** a process requests `/ws/remote` or `/api/remote/hosts` over the control socket
- **THEN** the request is not served

#### Scenario: Update stops only the daemon
- **WHEN** a client opens `/ws/remote` with `update=1` for a host whose daemon runs another version
- **THEN** the only mutating maintenance command beyond installation and daemon startup is its installed server's instance-bound stop or explicitly confirmed conditional legacy migration; no shell-selected signal, session-ending flag or TCP fallback is used

#### Scenario: Remote forwards only for reverse forwards
- **WHEN** the hub opens screens of `vm1`, forwards a port of `vm1` and shares a local port with `vm1`
- **THEN** only the `ssh` of the shared port carries a remote forward, and it names a Unix socket on the host

#### Scenario: Native caller without hub authority
- **WHEN** a remote-window handle or restricted agent requests a local host connection
- **THEN** the request is rejected before any local SSH process starts

### Requirement: Explicit Remote Editor Dispatch
Only a deliberate editor action in the trusted hub UI SHALL dispatch a local VS Code or Zed folder handoff for the selected local or SSH terminal, or an SFTP client handoff for the selected SSH terminal. Native editor dispatch SHALL be granted solely to the main packaged UI on the shell's assigned application origin, not bundled setup pages, remote windows or direct popout invocations; parent-owned popouts share main script authority as specified by Desktop Shell Command Scope. Electron SHALL additionally reject child-frame calls. The native grant SHALL be scoped by window, actual caller, exact packaged origin and document generation; the no-frame content policy remains. It SHALL validate the fixed editor enum (`vscode`, `zed`, or `sftp` for remote folders only), an absolute directory, an optional restricted SSH target for remote folders, and an optional validated WSL distribution for local VS Code folders and generate the fixed editor URI itself; no arbitrary URI, executable, command string or shell interpolation SHALL be accepted. Terminal links, remote output, server commands and URL query parameters SHALL NOT invoke this dedicated path. Existing generic URL openers SHALL continue to admit only HTTP(S); remote windows SHALL retain their isolation.

#### Scenario: Dedicated native command scope
- **WHEN** a remote window, a web origin or popout (or, in Electron, a child frame) directly invokes native editor dispatch
- **THEN** the request is rejected before any native handoff

#### Scenario: Hostile dispatch inputs
- **WHEN** a caller supplies an executable/URI as target, an option-like target, a relative directory, control characters, unsupported SSH syntax, or `sftp` for a local folder
- **THEN** native dispatch rejects the request before opening anything

#### Scenario: Terminal link uses editor scheme
- **WHEN** a hub remote terminal link, Splitty's generic URL helper or a native navigation guard receives a `vscode:`, `zed:` or `sftp:` URI
- **THEN** the generic opener refuses it and no editor dispatch is invoked

## ADDED Requirements

### Requirement: Private Native Client Endpoint
The daemon's client endpoint SHALL be distinct from control, grant and keeper sockets, with a 0700 parent directory and 0600 socket. Both peers SHALL verify the owning user on Linux/macOS. Unsafe owners, symlinks, non-sockets, unsupported filesystems, unreadable credentials and mismatched live instances SHALL be rejected without TCP fallback. No client endpoint path SHALL be injected into pane environments.

#### Scenario: Unsafe endpoint
- **WHEN** the endpoint or its private directory is a symlink, has an unsafe owner or cannot provide peer credentials
- **THEN** the connection fails before dispatch and no alternative listener opens

#### Scenario: Another user
- **WHEN** another OS user reaches the endpoint
- **THEN** the connection closes without application data

#### Scenario: Live endpoint exists
- **WHEN** another live daemon already owns the endpoint
- **THEN** startup neither unlinks its socket nor starts a second workspace

### Requirement: No Authority Expansion Through Native Transport
The native client endpoint SHALL expose only its enumerated UI/session operations. Control and grant sockets SHALL retain their existing route sets, and grant administration SHALL remain owner-control-only. No native entry point SHALL mint browser credentials. A failed scoped command SHALL never rediscover an owner, client or keeper endpoint.

#### Scenario: Cross-surface escalation
- **WHEN** a grant or control socket receives a UI-stream request, or a native UI connection requests grant administration or browser credentials
- **THEN** it is refused without granting authority

#### Scenario: Scoped discovery failure
- **WHEN** a grant socket is revoked or unreachable while a native daemon is available
- **THEN** the command fails without contacting that daemon's owner or client endpoint

### Requirement: Browser Gateway Credential Isolation
Each gateway SHALL keep its own bootstrap, ticket and in-memory session store, with the cookie name `splitty-session-<instance>`, HttpOnly, host-only and SameSite=Strict. Process Token governs bearer tokens, including intentional reuse of the same configured owner token by `serve` gateways. The backend SHALL be fixed by the native invocation. Browser credentials SHALL not cross into the native service or SSH channel, and browser input SHALL not replace the gateway's assigned backend.

#### Scenario: Two gateways
- **WHEN** a browser uses two gateways on different local ports
- **THEN** their cookies do not replace one another and each gateway rejects the other's session, bootstrap and ticket

#### Scenario: Target substitution
- **WHEN** a browser request, query or backend response names a different URL, socket or host
- **THEN** the gateway cannot be redirected to that endpoint

### Requirement: Packaged Desktop Content Policy
Desktop assets SHALL be limited to an exact packaged-asset/entry-document allowlist. Resource handling SHALL reject wrong authorities, malformed/encoded traversal and unknown paths before resolution, with no filesystem, network or directory-listing fallback. Production policy SHALL allow only required packaged/IPC resources, with no wildcard loopback or network scripts. Native data SHALL not grant executable content, navigation or generic OS access.

#### Scenario: Remote script and wildcard connection
- **WHEN** a desktop document attempts to load script from a host or connect directly to an arbitrary loopback service
- **THEN** the operation is blocked while its assigned native connection remains usable

#### Scenario: Asset path escape
- **WHEN** a packaged resource request uses traversal, encoded separators, a wrong authority or an unknown path
- **THEN** it returns no file or directory information outside the declared asset allowlist and performs no network fetch

### Requirement: Internal Endpoints Cannot Be Shared
Splitty SHALL reject reverse forwarding of any of its active browser-gateway ports with `local_port_reserved`, including gateways for remote targets and other configurations of the same user. Listener reservation and sharing admission SHALL be coordinated, following Forwarding Reservation Lifetime, so neither a race, coordinator crash nor saved restoration can share a newly bound internal listener. Private client, control and keeper sockets SHALL not be accepted as forwarding destinations.

#### Scenario: Gateway races sharing
- **WHEN** a browser gateway and a reverse-forward request concurrently select the same local port
- **THEN** at most one is admitted and the gateway is never exposed through that forward

#### Scenario: Saved share now names a gateway
- **WHEN** a saved reverse-forward rule targets a port now owned by any active Splitty browser gateway
- **THEN** restoration reports `local_port_reserved` without connecting that listener to the host

#### Scenario: Shared loopback across runtimes
- **WHEN** a Windows/WSL configuration exposes one runtime's gateway to another runtime's sharing destination
- **THEN** gateway reservations protect that destination too, or sharing fails as unsupported before the forward starts

### Requirement: Forwarding Reservation Lifetime
A reverse-share reservation SHALL be durable before exposure and cover actual local forwarding workers by process generation/network context. Release SHALL require locally verified termination; coordinator death, remote cleanup/heartbeat and a PID alone SHALL not suffice. A different immutable process birth identity MAY prove termination of the saved generation only within the same verified machine, boot and PID namespace; an effective-UID change alone SHALL NOT suffice. Ambiguous spawn/publication or cleanup SHALL keep the port reserved. Recovery SHALL not kill an unrelated process by PID alone; a conflicting gateway SHALL fail or choose a different ephemeral port.

#### Scenario: Coordinator dies while forwarding lives
- **WHEN** a coordinator is killed and a hostile SSH peer keeps the local forwarding worker alive
- **THEN** a gateway cannot bind the reserved destination until local forwarding termination is verified, regardless of remote EOF or heartbeat reports

#### Scenario: Crash during worker publication
- **WHEN** startup crashes after reserving a port and before a spawned worker's identity is fully recorded
- **THEN** recovery treats the reservation as uncertain and does not expose a new gateway on that port

#### Scenario: Unverified process identity or failed cleanup
- **WHEN** recovery cannot verify the saved worker generation or that cleanup ended forwarding
- **THEN** it neither kills the unrelated process nor releases the reservation based on coordinator death or elapsed time

#### Scenario: Proven process generation replacement
- **WHEN** the same verified machine, boot and PID namespace reports a different immutable birth identity for the saved worker PID
- **THEN** recovery may release that worker generation without signaling the replacement process
- **AND** a changed effective UID without a birth-identity change does not authorize release
