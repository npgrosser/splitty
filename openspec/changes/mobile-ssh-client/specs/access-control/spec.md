# Spec Delta

## MODIFIED Requirements

### Requirement: Remote Access Through SSH Tunnel
A remote server SHALL only be reached over authenticated SSH: through a tunnel opened by `splitty connect <target>` or a desktop remote window (see `desktop-remote`), or through `splitty remote-bridge` carried by the remote-screen hub or native mobile client. The remote server SHALL remain bound to loopback. The forwarding rules below apply to tunnel clients; bridge clients follow Remote Bridge Carries No Credential and create no browser-facing local forward. The target MUST be validated as `[user@]host[:port]` (bracketed IPv6 allowed) before `ssh` is invoked; a target starting with a dash (including Unicode dash lookalikes) or containing whitespace or control characters MUST be rejected, by `splitty connect` with exit status 2.

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

#### Scenario: Mobile bridge connection
- **WHEN** the native mobile client opens a verified SSH exec channel to `splitty remote-bridge`
- **THEN** the bridge reaches the daemon on host loopback with its existing origin and authentication guards
- **AND** no daemon credential or browser-facing local forward is required on the phone


### Requirement: Terminal Clipboard Policy
The xterm client SHALL allow OSC 52 clipboard writes by default, capped at 64 KiB of UTF-8. `terminal.allowOsc52Write=false` SHALL disable writes and survive settings save/reload. Clipboard reads via OSC 52 SHALL never read the clipboard. For a server advertising `session-control-v1`, only the controlling connection SHALL return an empty reply; observers SHALL send no reply. With an older server the client SHALL return an empty reply as before. Successful writes SHALL show a brief notice without copied contents; failed writes SHALL show an error, not success.

#### Scenario: Copy from a terminal application
- **WHEN** an application emits a nonempty OSC 52 write within the size limit and writes are enabled
- **THEN** the client writes the text and announces success only after the write succeeds

#### Scenario: Clipboard opt-out
- **WHEN** a user disables clipboard writes and reloads settings
- **THEN** OSC 52 writes remain disabled

#### Scenario: Clipboard read request
- **WHEN** a terminal program requests the clipboard via OSC 52
- **THEN** the controlling client returns empty, observers send no reply, and no client reads the local clipboard

#### Scenario: Oversized write
- **WHEN** a write exceeds 64 KiB of UTF-8
- **THEN** the clipboard is not changed

#### Scenario: Write failure
- **WHEN** the clipboard write fails
- **THEN** the client reports failure without displaying the copied contents

## ADDED Requirements

### Requirement: Mobile Host Verification
The mobile client SHALL verify and pin SSH host keys by host and port before user authentication or remote commands. An unknown key SHALL require explicit verification; a changed pinned key MUST stop connection until the user deliberately replaces the verified pin. Reconnect MUST NOT silently trust a new key or downgrade verification.

#### Scenario: First connection
- **WHEN** a saved destination has no pinned host key
- **THEN** no user authentication or remote command occurs until the user verifies the presented fingerprint

#### Scenario: Changed host key on reconnect
- **WHEN** a reconnect presents a key different from the stored pin
- **THEN** the connection is blocked with a host-verification error and the stored pin is not overwritten

### Requirement: Mobile SSH Key Custody
The mobile client SHALL generate device-specific SSH keys and protect private material using OS secure storage or OS-keystore-backed encryption. Private keys MUST NOT enter the UI runtime, logs, URLs, ordinary profile exports or automatic app backups. Explicit local key deletion SHALL remove that stored credential; it MUST NOT claim to revoke server authorization or an established connection.

#### Scenario: Export a host profile
- **WHEN** a profile is exported or diagnostic information is recorded
- **THEN** neither contains private-key material and public-key provisioning remains possible

#### Scenario: Delete a device key
- **WHEN** the user explicitly deletes a device key
- **THEN** future authentication cannot use it and the client distinguishes local deletion from host-side revocation

### Requirement: Mobile Native Command Isolation
Only the app's bundled top-level UI SHALL invoke the mobile native API. The API SHALL validate connection handles and typed actions, and MUST NOT expose arbitrary local execution or private-key extraction. Host-provided pages, terminal output, links and server commands MUST NOT gain native privileges or trigger key access or a new SSH connection. SSH agent, X11 and remote port forwarding SHALL be disabled.

#### Scenario: Host attempts a native action
- **WHEN** a host sends an executable link, HTML-like title, server command or content requesting a native connection or key export
- **THEN** it is treated as untrusted data and no privileged native action occurs

#### Scenario: Untrusted page calls native API
- **WHEN** a remote page or child frame attempts a mobile native command
- **THEN** the call is rejected before key access, connection or process execution
