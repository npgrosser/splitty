## MODIFIED Requirements

### Requirement: Desktop App Hosting On macOS And Linux
On macOS and Linux the desktop app SHALL always start an owned embedded local service without a UI TCP listener. Its local workspace and jobs SHALL end when the app exits. The app SHALL NOT automatically connect to, start, restart or stop an independent local daemon, adopt its keepers or modify its discovery/workspace files. Existing, incompatible, unreachable, stale or corrupt daemon discovery SHALL not change desktop startup. Persistent local access remains explicit through the CLI or `serve --daemon`.

#### Scenario: Daemon available
- **WHEN** a compatible local daemon is already running
- **THEN** the app starts its own empty embedded service and leaves the daemon and its sessions untouched

#### Scenario: No daemon
- **WHEN** there is no daemon and no kept session
- **THEN** the app starts its embedded service without opening a UI TCP listener

#### Scenario: Independent daemon cannot admit clients
- **WHEN** a local daemon is incompatible or unreachable
- **THEN** normal embedded desktop startup still works without connecting to or updating that daemon

#### Scenario: Daemon does not issue a code
- **WHEN** an independent local daemon has no usable bootstrap endpoint
- **THEN** the desktop app still starts its own embedded service in-process without reading a browser token or requesting a code from the daemon

#### Scenario: Stale daemon file
- **WHEN** local daemon discovery is stale or corrupt
- **THEN** the app starts embedded and leaves the independent daemon files untouched

#### Scenario: Kept sessions after a daemon crash
- **WHEN** keepers remain after a daemon crash and the app starts
- **THEN** the app starts its own embedded workspace without adopting, restarting or ending those kept sessions

#### Scenario: App exit closes only its local sessions
- **WHEN** the desktop app exits while an independent local daemon and a remote daemon have sessions
- **THEN** its embedded local sessions and jobs end, while the independent daemon and remote sessions keep running

### Requirement: Desktop App Hosting On Windows
The native Windows app SHALL run its local embedded backend in the selected supported WSL 2 distribution through inherited pipes using `desktop-backend --transport native-v1 --backend embedded --owner`. It SHALL need no localhost forwarding and discover no local daemon. On exit it SHALL close input, wait up to 3 seconds, then terminate only its owned bridge if needed, never the distribution. Without a usable WSL distribution it SHALL report that it needs one and start no backend.

#### Scenario: App exit stops the backend
- **WHEN** the Windows app quits
- **THEN** pipe EOF shuts down its embedded sessions without requiring a TCP connection

#### Scenario: Backend does not stop in time
- **WHEN** the owned backend does not stop within 3 seconds
- **THEN** the app terminates its bridge only and leaves unrelated WSL work running

#### Scenario: Repeated start for the same distribution
- **WHEN** the app is started a second time while an instance runs with the same distribution
- **THEN** the second instance starts a backend of its own and leaves the first instance's backend and sessions running

#### Scenario: Remote only
- **WHEN** the Windows app starts on a machine without WSL
- **THEN** it reports that it needs a WSL distribution and starts neither a backend nor a host connection

#### Scenario: WSL localhost forwarding unavailable
- **WHEN** WSL localhost forwarding is disabled or unavailable
- **THEN** the local desktop's terminal, API and file traffic still works through the owned pipes

## REMOVED Requirements

### Requirement: Desktop Stdio Lifetime Binding
**Reason**: It specified the `--backend ssh` form of `desktop-backend` next to the embedded one. Only the Tauri shell had a use for an SSH helper process; the native app reaches hosts through its local service's hub and starts only the embedded form. The embedded guarantees move unchanged to Desktop Backend Child Lifetime, which also states that the removed SSH options are refused.
**Migration**: Launchers use `desktop-backend --transport native-v1 --backend embedded`, with `--owner` for one service across substreams; a host is opened through that service's hub.

### Requirement: Desktop App Command Search Path
**Reason**: Only the Tauri shell asked the user's login shell for its `PATH` when the desktop started it. The native app does not; the gap is recorded in `docs/issues/openspec-baseline-findings.md` (native app), and adopting the login shell's `PATH` in the native app is a change of its own.
**Migration**: Until then, start the native app from a terminal, or name the programs that `ssh` and its `ProxyCommand` need by absolute path in the SSH configuration.

## ADDED Requirements

### Requirement: Desktop Backend Child Lifetime
A child started with `desktop-backend --transport native-v1 --backend embedded` SHALL own an embedded service; with `--owner` it SHALL keep that service across independent substreams. Inherited stdin/stdout SHALL carry only its negotiated connection, with bounded stderr diagnostics. EOF SHALL stop its embedded sessions and jobs. Protocol bytes SHALL pass unchanged. Other `--backend` values, the former SSH options and the former `serve --desktop-stdio` SHALL be refused before a backend starts.

#### Scenario: Launcher exits
- **WHEN** the launcher closes the child's input pipe
- **THEN** the child stops accepting work and shuts down its embedded sessions

#### Scenario: Binary roundtrip
- **WHEN** terminal output includes NULs, non-UTF-8 bytes or split frame headers
- **THEN** those bytes are delivered unchanged and no log or bootstrap line contaminates stdout

#### Scenario: Legacy launcher
- **WHEN** a caller requests `serve --desktop-stdio`
- **THEN** it gets a usage error pointing to the native child entry point, without opening a listener

#### Scenario: Launcher requests a fresh code
- **WHEN** a launcher writes the old `bootstrap` line to the native child's input
- **THEN** negotiation fails as invalid input without printing a bootstrap line or opening HTTP

#### Scenario: SSH backend form refused
- **WHEN** a caller starts `desktop-backend --transport native-v1 --backend ssh --target vm1`, or adds `--remember-target` or `--update` to the embedded form
- **THEN** it exits with status 2 and writes nothing to stdout, without starting a backend or running `ssh`

#### Scenario: Owner carrier across reloads
- **WHEN** the launcher of `desktop-backend --transport native-v1 --backend embedded --owner` closes one substream and opens another
- **THEN** both reach the same embedded workspace and its jobs, and closing the launcher's pipes ends those sessions and jobs
