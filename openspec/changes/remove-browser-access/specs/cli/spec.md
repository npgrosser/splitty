## ADDED Requirements

### Requirement: Removed Browser Commands
The former commands `serve`, `connect` and `issue-bootstrap` SHALL NOT start a server, a browser gateway or an SSH connection, read or write a credential, or create a workspace, socket or listener. Until the next minor release after the one that removes them, each SHALL remain a hidden command that accepts any arguments, prints one line naming `splitty daemon start`, the desktop app and `splitty attach` (also `ssh -t <host> splitty attach`), and exits with status 2; with `--json` that line SHALL be `{"code":"removed","error":<text>}` (provisional, tbd). `serve --desktop-stdio` SHALL instead name `desktop-backend --transport native-v1 --backend embedded`, as `server-modes`, Desktop Backend Child Lifetime, specifies.

#### Scenario: Former serve
- **WHEN** a user runs `splitty serve --addr 127.0.0.1:9800 --open`
- **THEN** the CLI prints the line naming the remaining ways and exits with status 2
- **AND** no listener, socket, workspace or browser is opened

#### Scenario: Former connect with JSON
- **WHEN** a user runs `splitty connect me@192.0.2.10 --json`
- **THEN** standard output holds exactly one JSON object with `code` `removed`, the CLI exits with status 2, and no `ssh` process starts

#### Scenario: Former issue-bootstrap
- **WHEN** a user runs `splitty issue-bootstrap`
- **THEN** the CLI exits with status 2 without reading or creating `auth-token` and without contacting a server

## MODIFIED Requirements

### Requirement: Single Executable Command Surface
The CLI SHALL be a single executable named `splitty` that provides the server commands `daemon start`, `daemon stop`, `daemon restart` and `daemon status`, the terminal command `attach`, and the control commands `split`, `new-screen`, `close-pane`, `close-screen`, `rename-screen`, `list-screens`, `list-panes`, `status` and `send-keys`. It SHALL provide no command that starts a browser gateway or mints a browser credential (see Removed Browser Commands). It SHALL provide the internal native child entry point `desktop-backend --transport native-v1`. It SHALL additionally provide `run`, `jobs`, `inspect`, `logs`, `input`, `stop`, `forget` and `agent` with `guide`, `create`, `list`, `exec`, `revoke`, `assign` and `reclaim` subcommands.

#### Scenario: Version flag
- **WHEN** a user runs `splitty --version`
- **THEN** the CLI prints the Splitty version and exits with status 0

#### Scenario: Unknown subcommand
- **WHEN** a user runs `splitty` with a subcommand that does not exist
- **THEN** the CLI prints a usage error to stderr and exits with a non-zero status

### Requirement: Machine-Readable Startup Output
A successful new `splitty daemon start` SHALL write `SPLITTY_CLIENT_SOCKET=<absolute path>` and `SPLITTY_INSTANCE=<instance>` to stdout, one per line and flushed individually, and no port, URL or credential. Native child stdout SHALL contain protocol frames only. Closing a reader of the text startup lines MUST NOT stop the service; a native child's connection closure follows Desktop Backend Child Lifetime.

#### Scenario: Serve handshake lines
- **WHEN** a user runs `splitty serve`
- **THEN** standard output holds no `SPLITTY_PORT`, `SPLITTY_BOOTSTRAP` or `SPLITTY_URL` line, and the command exits with status 2 as Removed Browser Commands specifies

#### Scenario: Daemon handshake line
- **WHEN** `splitty daemon start` has bound its private client endpoint
- **THEN** stdout contains its client socket and instance lines, no port line and no credential

### Requirement: Daemon Start Command
`splitty daemon start` SHALL securely bind its private client endpoint and acquire its instance lock before detaching, then return only once the daemon is serving. Bind/start failures SHALL exit with status 1 and a reason, naming `daemon.log` for detached-start failures. A registered live daemon SHALL be reported without replacement, including an explicit legacy indication when it lacks native transport. Non-Unix platforms SHALL fail with status 1. `--addr` and `--token` SHALL be usage errors with status 2; `SPLITTY_ADDR` SHALL not enable HTTP.

#### Scenario: Already running
- **WHEN** a daemon runs and `daemon start` is invoked
- **THEN** its PID and native endpoint or legacy status are reported and no second daemon starts

#### Scenario: Endpoint in use or unsafe
- **WHEN** the private client endpoint cannot be bound safely
- **THEN** the CLI reports the reason with status 1 without detaching or opening TCP

#### Scenario: Daemon fails to start
- **WHEN** the detached daemon fails before readiness
- **THEN** the CLI reports `Failed to start daemon: <reason>` and exits with status 1

#### Scenario: Old listener flag
- **WHEN** `daemon start --addr 127.0.0.1:9800` is invoked
- **THEN** it exits with usage status 2 without starting a daemon or binding a TCP port

#### Scenario: Port in use
- **WHEN** another process occupies port 9800 and `daemon start` is invoked without listener flags
- **THEN** a daemon can start on its private endpoint without attempting to bind the occupied TCP port

### Requirement: Server Discovery Order
When a non-empty `SPLITTY_AGENT_SOCK` is set, control commands SHALL use only that socket. Otherwise control commands SHALL locate the server in this order: (1) the control socket named by a non-empty `SPLITTY_SOCK`; (2) the control socket recorded in the daemon info file (`~/.config/splitty/daemon.json` on Linux) of a running daemon. Control commands SHALL reach a server only through these local sockets, never over TCP; `--addr` and `--token` SHALL be usage errors with status 2. The CLI MUST NOT read a credential from the environment. Unreachable or revoked scoped sockets MUST NOT fall back to owner discovery.

#### Scenario: Inside a Splitty pane
- **WHEN** a control command runs in an ordinary shell spawned by Splitty, with no scoped socket
- **THEN** the CLI talks to the server over the control socket named by `SPLITTY_SOCK`

#### Scenario: Outside a pane with a running daemon
- **WHEN** both socket variables are unset and a daemon is running
- **THEN** the CLI talks to the daemon over the control socket recorded in its info file

#### Scenario: Explicit target
- **WHEN** a control command is given `--addr 127.0.0.1:9800` and `--token <token>`
- **THEN** the CLI exits with usage status 2 before contacting a server, whether or not a daemon runs

#### Scenario: Token in the environment
- **WHEN** `SPLITTY_TOKEN` or `SPLITTY_SERVER` is set
- **THEN** the CLI ignores both variables

#### Scenario: Scoped discovery fails closed
- **WHEN** `SPLITTY_AGENT_SOCK` is set but unavailable while a normal daemon is discoverable
- **THEN** the command fails and sends no request to the daemon's owner socket

#### Scenario: Explicit target under a grant
- **WHEN** a scoped invocation supplies `--addr` and `--token`
- **THEN** the CLI exits with usage status 2 without contacting any server

#### Scenario: Incomplete explicit target
- **WHEN** only `--addr` or only `--token` is given
- **THEN** the CLI exits with usage status 2 before contacting a server

### Requirement: No Server Found
When discovery finds no server, a control command SHALL print `Error: no running Splitty server found (tried SPLITTY_SOCK, daemon.json)` to stderr and exit with status 1. An embedded server writes no daemon info file, so it SHALL be reachable by control commands only from its own panes, through `SPLITTY_SOCK`.

#### Scenario: Nothing running
- **WHEN** `SPLITTY_SOCK` is unset and no daemon is running
- **THEN** `splitty list-screens` prints the no-server error and exits with status 1

### Requirement: Scoped Administrative Commands Are Refused
With `SPLITTY_AGENT_SOCK` set, the CLI SHALL reject owner administration, terminal attachment and server lifecycle/remote-launch entry points, including `attach`, `daemon`, `remote-handshake`, `remote-bridge` and `desktop-backend`, before file access or process launch. This CLI guard SHALL not be described as an OS sandbox.

#### Scenario: Scoped caller tries credential or daemon commands
- **WHEN** a scoped invocation runs `issue-bootstrap` or `daemon stop`
- **THEN** the CLI fails without reading any file, contacting the owner socket or signalling the daemon

#### Scenario: Scoped caller tries to attach to a terminal
- **WHEN** a scoped invocation runs `splitty attach <pane-id>` while a daemon is running
- **THEN** the CLI fails without contacting any server

### Requirement: Attach Server And Credential
`splitty attach` SHALL use the daemon recorded in the daemon info file, never `SPLITTY_SOCK`, and SHALL verify and negotiate its private client endpoint and use its native session stream, without a credential or TCP. `--addr` and `--token` SHALL be usage errors with status 2. It SHALL print no credential. Without a daemon it SHALL use the keepers of this machine (Attach Without A Daemon), and without those fail with a command error naming both ways.

#### Scenario: Daemon on this machine
- **WHEN** a daemon runs and the user runs `splitty attach <pane-id>` outside any pane
- **THEN** the CLI streams the session over the verified private daemon endpoint without a token or ticket
- **AND** no token or ticket appears in its output

#### Scenario: Inside a pane of an embedded server
- **WHEN** `splitty attach` runs in a pane whose `SPLITTY_SOCK` names an embedded server while no daemon and no keeper runs
- **THEN** the CLI prints `Error: no running Splitty daemon found (splitty attach uses daemon.json, or the keepers of this machine)` and exits with status 1

#### Scenario: Explicit server
- **WHEN** the user runs `splitty attach --addr 127.0.0.1:9900 --token <token> <pane-id>`
- **THEN** the CLI exits with status 2 before contacting a server

#### Scenario: On a host over SSH
- **WHEN** the user runs `ssh -t me@192.0.2.10 splitty attach` and the host's daemon runs
- **THEN** the CLI on the host shows the pick list of that daemon's panes and attaches over its private endpoint

### Requirement: Attach Without A Daemon
Without a running daemon, `splitty attach` SHALL list and resolve its targets among the live keepers of this machine (see `session-keepers`) and control the chosen keeper directly, without taking it over; a keeper another client controls SHALL fail with a command error. When a daemon takes the keeper over, the command SHALL continue through the daemon's session stream without showing the buffered output again.

#### Scenario: Attach while the daemon is stopped
- **WHEN** the daemon was stopped with `splitty daemon stop` and the user runs `splitty attach <pane-id>`
- **THEN** the terminal shows the session's output, and a line typed there reaches the process

#### Scenario: The daemon starts while attached
- **WHEN** a terminal is attached directly to a keeper and `splitty daemon start` runs
- **THEN** the terminal keeps showing the session without repeating its output, and typing keeps reaching the process

#### Scenario: Controlled by another terminal
- **WHEN** no daemon runs and another terminal already controls the target's keeper
- **THEN** the CLI prints an error that another client controls the session and exits with status 1

## REMOVED Requirements

### Requirement: Serve Command
**Reason**: Browser access is removed: no command starts an embedded server with a TCP listener, and no gateway attaches to a daemon. An embedded server is now only the desktop app's own (see `server-modes`); persistent local work is `splitty daemon start`.

**Migration**: `splitty serve` and `serve --daemon` answer as Removed Browser Commands specifies. Use `splitty daemon start` for a persistent local workspace, the desktop app to work in it, and `splitty attach` for a terminal without the app.

### Requirement: Process Token Options
**Reason**: No process token exists any more (see `access-control`), so there is nothing for `--token` or `--oneshot-token` to set.

**Migration**: Drop `--token` and `--oneshot-token`; the commands they belonged to are removed.

### Requirement: Issue Bootstrap Command
**Reason**: Bootstrap codes existed only to sign a browser in to a gateway; with the gateway removed there is nothing to issue a code for.

**Migration**: `splitty issue-bootstrap` answers as Removed Browser Commands specifies; no replacement.

### Requirement: Remote Connect Tunnel
**Reason**: `splitty connect` is removed with the browser gateway it served. The host preparation it shared (installation, the format-2 handshake with `--ensure-daemon`, version checks, no TCP forwarding) is the hub's, specified in `remote-screens` (Remote Screen Hub Endpoint, Host Update) and `remote-hosts`.

**Migration**: Open the host in the desktop app, which prepares it through the hub, or run `ssh -t <host> splitty attach` for a terminal on the host.

### Requirement: Remote Connect Session Handoff
**Reason**: The session URL and its bootstrap code were the browser's way into `connect`'s local gateway, which is removed.

**Migration**: See Remote Connect Tunnel.

### Requirement: Remote Connect Reconnect
**Reason**: The gateway that kept its listener and browser session across SSH reconnects is removed; reconnecting a host is the client's (see `remote-screens`, Remote Screen Reconnect).

**Migration**: See Remote Connect Tunnel.

### Requirement: Remote Connect Recent Hosts
**Reason**: With `connect` removed, a host becomes recent when the hub sends `remote.ready` for it (see `remote-screens`, Remote Screen Hub Endpoint).

**Migration**: None; the hub records recent hosts.

### Requirement: Remote Connect Machine-Readable Output
**Reason**: The event stream of `connect --json` described the removed command.

**Migration**: Scripts that parsed `connect --json` have no replacement; `splitty remote-hosts --json` stays (Known Hosts Command).

### Requirement: Remote Connect Port Forwards
**Reason**: `connect --forward` belonged to the removed command. Forwarding host ports stays in the hub (see `remote-screens` and `access-control`, Hub Port Forwards).

**Migration**: Forward ports from the desktop app's host port list, or with `ssh -L 127.0.0.1:<local>:localhost:<remote> <host>`.
