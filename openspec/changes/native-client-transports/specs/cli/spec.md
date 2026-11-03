# Spec Delta

## MODIFIED Requirements

### Requirement: Single Executable Command Surface
The CLI SHALL be a single executable named `splitty` that provides the server commands `serve`, `daemon start`, `daemon stop`, `daemon status` and `connect`, and the control commands `split`, `new-screen`, `close-pane`, `close-screen`, `rename-screen`, `list-screens`, `list-panes`, `status`, `send-keys` and `issue-bootstrap`. It SHALL provide the internal native child entry point `desktop-backend --transport native-v1`. It SHALL additionally provide `run`, `jobs`, `inspect`, `logs`, `input`, `stop`, `forget` and `agent` with `guide`, `create`, `list`, `exec`, `revoke`, `assign` and `reclaim` subcommands.

#### Scenario: Version flag
- **WHEN** a user runs `splitty --version`
- **THEN** the CLI prints the Splitty version and exits with status 0

#### Scenario: Unknown subcommand
- **WHEN** a user runs `splitty` with a subcommand that does not exist
- **THEN** the CLI prints a usage error to stderr and exits with a non-zero status

### Requirement: Serve Command
Plain `splitty serve` SHALL start a server in embedded mode (see `server-modes`) in the foreground, listening on the address given by `--addr`, else by the `SPLITTY_ADDR` environment variable, else `127.0.0.1:9800`, and SHALL run until it receives Ctrl+C. With `--open` the CLI SHALL open the launch URL in the default browser; without it no browser is opened. The address MUST be a loopback address (see `access-control`), and the former `--desktop-stdio` flag SHALL be refused with guidance to the native child entry point. With `--daemon`, `serve` SHALL instead attach a browser gateway to the existing compatible daemon, without creating or owning a workspace; no daemon SHALL fail with an actionable error.

#### Scenario: Default address
- **WHEN** a user runs `splitty serve` with neither `--addr` nor `SPLITTY_ADDR`
- **THEN** the server listens on `127.0.0.1:9800`

#### Scenario: Ephemeral port
- **WHEN** a user runs `splitty serve --addr 127.0.0.1:0`
- **THEN** the server listens on a free loopback port
- **AND** the startup output reports the port actually bound, not `0`

#### Scenario: Open browser
- **WHEN** a user runs `splitty serve --open`
- **THEN** the CLI opens the launch URL in the default browser
- **AND** a failure to open the browser is logged without stopping the server

#### Scenario: Browser for an existing daemon
- **WHEN** a compatible daemon runs and the user invokes `splitty serve --daemon --open`
- **THEN** a foreground browser gateway opens that daemon's workspace, and stopping the command leaves its sessions running

#### Scenario: No daemon for browser attachment
- **WHEN** `splitty serve --daemon` finds no daemon
- **THEN** it fails without creating a workspace or leaving a listener

### Requirement: Machine-Readable Startup Output
On startup `splitty serve` SHALL write the lines `SPLITTY_PORT=<port>`, `SPLITTY_BOOTSTRAP=<code>` and `SPLITTY_URL=http://<host>:<port>/?bootstrap=<code>&desktop=1` to stdout, one per line and flushed individually, where `<code>` is a single-use bootstrap code (see `access-control`). A successful new `splitty daemon start` SHALL write `SPLITTY_CLIENT_SOCKET=<absolute path>` and `SPLITTY_INSTANCE=<instance>` instead of a port. Native child stdout SHALL contain protocol frames only. Which credentials may appear in this output is governed by `access-control`. Closing a reader of the text startup lines MUST NOT stop the service; a native child's connection closure follows Desktop Stdio Lifetime Binding.

#### Scenario: Serve handshake lines
- **WHEN** `splitty serve` has bound its listener
- **THEN** stdout contains `SPLITTY_PORT`, `SPLITTY_BOOTSTRAP` and `SPLITTY_URL` lines
- **AND** the bootstrap code differs from the process token

#### Scenario: Daemon handshake line
- **WHEN** `splitty daemon start` has bound its private client endpoint
- **THEN** stdout contains its client socket and instance lines, no port line and no credential

### Requirement: Process Token Options
`splitty serve` (including `--daemon`) SHALL accept `--token <value>` to set the process token explicitly; without it the token SHALL be resolved as specified in `access-control`. `splitty serve --oneshot-token` SHALL use a freshly generated random token for this process only instead of resolving one. The gateway owned by `splitty connect` SHALL use an ephemeral token without reading a token file. Native daemon/desktop entry points SHALL reject `--token` rather than creating HTTP implicitly.

#### Scenario: One-shot token
- **WHEN** a user runs `splitty serve --oneshot-token`
- **THEN** the server uses a newly generated random token
- **AND** the token is not printed

### Requirement: Daemon Start Command
`splitty daemon start` SHALL securely bind its private client endpoint and acquire its instance lock before detaching, then return only once the daemon is serving. Bind/start failures SHALL exit with status 1 and a reason, naming `daemon.log` for detached-start failures. A registered live daemon SHALL be reported without replacement, including an explicit legacy indication when it lacks native transport. Non-Unix platforms SHALL fail with status 1. `--addr` and `--token` SHALL be usage errors pointing to `serve --daemon`; `SPLITTY_ADDR` SHALL not enable HTTP.

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
- **THEN** it exits with usage status 2 and explains explicit browser serving

#### Scenario: Port in use
- **WHEN** another process occupies port 9800 and `daemon start` is invoked without listener flags
- **THEN** a daemon can start on its private endpoint without attempting to bind the occupied TCP port

### Requirement: Daemon Stop and Status Commands
`splitty daemon stop` SHALL ask the running daemon to terminate gracefully, which leaves its sessions running in their keepers (see `session-keepers`), and wait up to about two seconds, then report either `Daemon stopped (pid=<pid>)` or that the daemon may still be shutting down. With `--end-sessions` it SHALL then end every kept session of the run directory, also when no daemon runs, wait up to 5 seconds for them and print `Ended <n> kept session(s)`. `splitty daemon status` SHALL print the daemon's PID, private client endpoint, start time and version. With `--json` and verified discovery, `splitty daemon status` SHALL instead print exactly one JSON object and exit with status 0, whether or not a daemon runs:
- `{"running": false}` when no daemon is running;
- otherwise `"running": true` with the daemon's `pid`, `clientSocket`, `socket`, `instance`, `transportVersion`, `startedAt` and `version` as its info file holds them, and no credential.

A running legacy record SHALL instead carry `running:true`, `legacy:true` and its known diagnostic fields, without fabricated native fields or automatic attachment to its recorded port.

When no daemon is running, both commands SHALL print `No daemon running` (`daemon status --json` prints its object instead), and SHALL remove a daemon info file that names a dead process only after taking the native instance lock and rereading the unchanged record. A live instance lock SHALL prevent stale cleanup or startup from unlinking a newer publication. Corrupt or unreadable discovery SHALL produce an explicit diagnostic and nonzero exit without deleting it or inferring absence. Without `--json`, `daemon status` SHALL then add the number of kept sessions, when there are any, and how to reach them.

#### Scenario: Stop a running daemon
- **WHEN** a daemon is running and the user runs `splitty daemon stop`
- **THEN** the daemon receives a termination request and the CLI reports that it stopped

#### Scenario: Stop keeps the sessions
- **WHEN** the user runs `splitty daemon stop` while a pane of the daemon runs a build
- **THEN** the build keeps running after the daemon has exited

#### Scenario: Stop and end the sessions
- **WHEN** the user runs `splitty daemon stop --end-sessions` while two panes of the daemon run shells
- **THEN** the daemon stops, both shells end, and the CLI prints `Ended 2 kept session(s)`

#### Scenario: Kept sessions without a daemon
- **WHEN** no daemon runs, two keepers do, and the user runs `splitty daemon status`
- **THEN** the CLI prints `No daemon running` and that 2 sessions are kept, to be adopted by `splitty daemon start` or shown with `splitty attach`

#### Scenario: Stale info file
- **WHEN** the daemon info file names a process that is no longer running and no live instance holds its lock
- **THEN** `splitty daemon status` prints `No daemon running (stale info file cleaned up)`
- **AND** the info file is removed

#### Scenario: Status races daemon publication
- **WHEN** status or startup observes a dead PID but a running or starting daemon holds the native instance lock
- **THEN** it does not remove or replace that daemon's discovery publication

#### Scenario: Corrupt discovery is not absence
- **WHEN** daemon status, stop or restart encounters corrupt or unreadable discovery
- **THEN** it exits nonzero with a diagnostic and preserves the record instead of inferring that no daemon exists

#### Scenario: Machine-readable status of a running daemon
- **WHEN** a daemon is running and the user runs `splitty daemon status --json`
- **THEN** the CLI prints one JSON object with `running` set to true and the daemon's pid, client and control socket paths, instance, transport version, start time and version, and exits with status 0

#### Scenario: Machine-readable status without a daemon
- **WHEN** no daemon is running and the user runs `splitty daemon status --json`
- **THEN** the CLI prints `{"running":false}` and exits with status 0

#### Scenario: Machine-readable status of a legacy daemon
- **WHEN** a live daemon has only legacy discovery metadata
- **THEN** JSON status marks it as legacy, reports known diagnostic fields and invents no private client endpoint

### Requirement: Issue Bootstrap Command
`splitty issue-bootstrap` SHALL obtain a single-use code only from an explicitly named browser gateway using `--addr` and the process token from `--token` or the configured token file. It SHALL print `SPLITTY_BOOTSTRAP=<code>` on success. It SHALL neither use native/control sockets nor infer a TCP endpoint from daemon discovery. Without `--addr` it SHALL fail with guidance to start `serve --daemon` or name an existing gateway.

#### Scenario: Mint for an explicit gateway
- **WHEN** a gateway runs and the CLI names its address with a valid token
- **THEN** the CLI prints one bootstrap line and exits with status 0

#### Scenario: Native daemon only
- **WHEN** the user runs `issue-bootstrap` without an explicit gateway address
- **THEN** it fails with actionable guidance and opens no listener or native credential path

#### Scenario: Mint a code for the local daemon
- **WHEN** a daemon and its explicit `serve --daemon` gateway run and `issue-bootstrap` names that gateway with a valid token
- **THEN** it prints one gateway bootstrap line without requesting a credential from the daemon's socket

#### Scenario: No daemon
- **WHEN** no daemon runs and `--addr` is not given
- **THEN** `issue-bootstrap` exits with status 1 and guidance to start or name an explicit browser gateway

### Requirement: Remote Connect Tunnel
After target validation, `splitty connect` SHALL use the existing remote installation and noninteractive system-SSH flow, then handshake format 2 with `--ensure-daemon` and no port hint. It SHALL establish a native exec bridge to the host's private daemon endpoint and create its own loopback browser gateway. A free gateway port SHALL not need to match any remote port. No daemon TCP forwarding SHALL be requested. Phase failures SHALL exit with status 1 and clean up owned resources.

A daemon of another version SHALL be reported. The existing automatic update is allowed only when it is older, keeps sessions and the answering installed server matches the client; then stop/start it without ending keepers and renegotiate. Otherwise leave it running and use it only when transport and mux protocols are supported; incompatibility SHALL fail with update guidance rather than HTTP fallback.

#### Scenario: No daemon on the remote
- **WHEN** the host has no daemon
- **THEN** the handshake starts a private-endpoint daemon and the gateway connects through SSH

#### Scenario: Daemon cannot be started
- **WHEN** the host cannot start a daemon
- **THEN** the command reports the host's reason and exits with status 1

#### Scenario: Splitty missing on the remote
- **WHEN** no payload is available and noninteractive SSH cannot find Splitty
- **THEN** the error explains installing Splitty or using a client carrying the host's payload

#### Scenario: Splitty too old on the remote
- **WHEN** the answering binary does not support the required handshake/transport and cannot be updated under the established rule
- **THEN** the command reports incompatibility without TCP fallback

#### Scenario: Local port busy
- **WHEN** local port 9800 is busy
- **THEN** the gateway selects a free local port without changing the host daemon

#### Scenario: SSH needs interaction
- **WHEN** SSH needs a password or host-key prompt
- **THEN** it fails without prompting, adding the existing fingerprint-check guidance for an unknown key

#### Scenario: Older daemon that keeps its sessions
- **WHEN** an older daemon keeps sessions and the client's installed payload is available
- **THEN** the established update flow restarts it with keepers preserved and negotiates native transport

#### Scenario: Newer daemon, or one whose sessions would end
- **WHEN** a daemon is newer or cannot preserve sessions
- **THEN** it is left running and either used compatibly or reported incompatible without a replacement workspace

### Requirement: Remote Connect Session Handoff
After establishing the SSH backend and its own browser listener, `splitty connect` SHALL issue a local single-use bootstrap code and print `SPLITTY_URL=http://127.0.0.1:<port>/?bootstrap=<code>&host=<target>`, percent-encoding the target. It SHALL open that URL unless `--no-open` and keep the gateway and bridge alive until interrupted or reconnect fails permanently. No browser credential SHALL cross SSH. Ctrl+C or SIGTERM SHALL close owned listeners/processes, detach from the daemon and exit with status 0.

#### Scenario: Successful connect
- **WHEN** native SSH negotiation and local gateway startup succeed
- **THEN** the CLI prints the local session URL, opens it and reports how to disconnect

#### Scenario: No browser
- **WHEN** `connect` runs with `--no-open`
- **THEN** it prints the URL without opening a browser

#### Scenario: Gateway startup fails
- **WHEN** the gateway cannot bind or create its browser session
- **THEN** it reports the failure and closes its bridge without stopping host sessions

#### Scenario: Remote cannot mint a code
- **WHEN** the host has a compatible native daemon but no browser credential endpoint
- **THEN** `connect` issues its local gateway's bootstrap code without invoking remote `issue-bootstrap`

#### Scenario: Disconnect
- **WHEN** the user interrupts an active connection
- **THEN** the gateway and its owned SSH processes stop, host sessions survive and the command exits with status 0

### Requirement: Remote Connect Reconnect
When the SSH bridge ends, `connect` SHALL keep its browser listener and authentication state, disable backend operations and reconnect at once, then after 1, 2, 4, 8 and at most 10 seconds between attempts. It SHALL reinstall a missing server and start a missing compatible daemon through the existing flow. A recovered daemon SHALL receive a new native connection and resync without replayed mutations or a changed browser origin. After 10 minutes without success the command SHALL clean up and exit with status 1; interruption SHALL exit with status 0.

#### Scenario: Tunnel drops while the daemon keeps running
- **WHEN** SSH drops while the daemon remains alive
- **THEN** the gateway stays bound, reconnects and accepts its existing browser session without a new URL

#### Scenario: Daemon restarted while the tunnel was down
- **WHEN** the daemon restarted
- **THEN** the gateway negotiates the new instance and resynchronizes existing sessions without a new browser URL or stale input

#### Scenario: Host rebooted while the tunnel was down
- **WHEN** the host returns without a daemon
- **THEN** the handshake starts one and the gateway reconnects at its original local origin

#### Scenario: Host stays unreachable
- **WHEN** reconnect has failed for 10 minutes
- **THEN** the command reports failure, closes its gateway and exits with status 1

#### Scenario: Interrupted while reconnecting
- **WHEN** the user interrupts a retry wait
- **THEN** owned resources are released and the command exits with status 0

### Requirement: Remote Connect Machine-Readable Output
With `--json`, `splitty connect` SHALL print on standard output only JSON objects, one per line, each with an `event` field, instead of its text output:
- `progress`, with `step` (`install`, `daemon`, `started` or `tunnel`) and `message`;
- `notice`, with `message`, for example when the daemon runs another version, or when the target could not be recorded as a recent host or the browser could not be opened;
- `ready`, with `url` (the session URL of Remote Connect Session Handoff) and `port`, once the native bridge and local gateway are ready; `port` names the gateway;
- `forward`, with `remote`, `local` and `url`, one per forward of `--forward` after `ready` (Remote Connect Port Forwards);
- `lost`, with `message`, when the tunnel dropped;
- `retry`, with `message` and `retryIn` (seconds), after a failed attempt to rebuild it;
- `restored`, when the backend is reachable again, including a restarted daemon;
- `failed`, with `message` and, where the text output gives an instruction, `hint`, right before the command exits with status 1, or with status 2 for a target or a `--forward` list it rejects;
- `closed`, right before the command exits with status 0.

Backend restarts SHALL NOT emit a new `url` event or rotate the gateway's browser session. Everything else SHALL behave as without `--json`: the steps, the reconnect rules, the browser unless `--no-open`, and the exit status. Diagnostics of `ssh` itself MAY still appear on standard error.

#### Scenario: Connected
- **WHEN** the user runs `splitty connect vm1 --json --no-open` and the tunnel is established
- **THEN** standard output carries `progress` events, then one `ready` event whose `url` carries a single-use bootstrap code, and no line that is not a JSON object

#### Scenario: Unknown host key
- **WHEN** `splitty connect vm1 --json` fails because `ssh` reports a failed host key verification
- **THEN** the last line on standard output is a `failed` event whose `hint` tells the user to run `ssh vm1` once in a terminal and check the fingerprint
- **AND** the command exits with status 1

#### Scenario: Interrupted
- **WHEN** the user ends `splitty connect vm1 --json` with Ctrl+C or SIGTERM while the tunnel is active
- **THEN** the last line on standard output is a `closed` event and the command exits with status 0

#### Scenario: Rejected forward list
- **WHEN** the user runs `splitty connect vm1 --json --forward 3000,3000`
- **THEN** the only line on standard output is a `failed` event and the command exits with status 2 without running `ssh`

### Requirement: Remote Connect Port Forwards
`splitty connect` SHALL accept `--forward <list>`, repeatable, where the list is comma-separated and each entry is `PORT` or `LOCAL:REMOTE`, ports from 1 to 65535. An entry that does not parse, a remote port named twice, or a local port named twice SHALL be rejected with exit status 2 before `ssh` runs.

For each entry, the CLI SHALL forward local `127.0.0.1:<local>` to `localhost:<remote>` on the host, in a separately supervised forward-only `ssh` process, independent of the native daemon bridge:
- the local port is `LOCAL` when given, and the remote port otherwise;
- when that local port is not free, it is any free loopback port.

The CLI SHALL hold every local port reserved until `ssh` takes it over, and SHALL rebuild the forwards on the same local ports whenever it rebuilds the tunnel (Remote Connect Reconnect). Once the forwards and browser gateway are verified, it SHALL print one line per forward, `Forwarding http://localhost:<local> -> <target>:<remote>`. When the local port differs from the one asked for, the line SHALL say so.

With `--json`, each forward SHALL instead be one event `{"event": "forward", "remote": <port>, "local": <port>, "url": "http://localhost:<local>"}`, after the `ready` event. A forward whose remote port has no listener on the host SHALL NOT fail the command; connections to it fail until a server listens there.

#### Scenario: Forward a dev server
- **WHEN** the user runs `splitty connect vm1 --forward 5173` and a dev server listens on port 5173 on `vm1`
- **THEN** `http://localhost:5173` on the PC reaches that dev server, and the CLI prints `Forwarding http://localhost:5173 -> vm1:5173`

#### Scenario: Local port busy
- **WHEN** the user forwards port 3000 and a local process already listens on `127.0.0.1:3000`
- **THEN** the CLI forwards another free local port to `vm1`'s port 3000 and names that port in its output

#### Scenario: Forwards survive a reconnect
- **WHEN** the tunnel drops and is rebuilt
- **THEN** every forward works again on the same local port

#### Scenario: Invalid entry
- **WHEN** the user runs `splitty connect vm1 --forward 70000`
- **THEN** the CLI exits with status 2 without running `ssh`

### Requirement: Scoped Administrative Commands Are Refused
With `SPLITTY_AGENT_SOCK` set, the CLI SHALL reject owner administration, credential minting, terminal attachment and server lifecycle/remote-launch entry points, including `issue-bootstrap`, `attach`, `serve`, `daemon`, `connect`, `remote-handshake`, `remote-bridge` and `desktop-backend`, before file access or process launch. This CLI guard SHALL not be described as an OS sandbox.

#### Scenario: Scoped caller tries credential or daemon commands
- **WHEN** a scoped invocation runs `issue-bootstrap` or `daemon stop`
- **THEN** the CLI fails without reading the token file, contacting the owner socket or signalling the daemon

#### Scenario: Scoped caller tries to attach to a terminal
- **WHEN** a scoped invocation runs `splitty attach <pane-id>` while a daemon is running
- **THEN** the CLI fails without reading the token file or contacting any server

### Requirement: Attach Server And Credential
`splitty attach` SHALL use the server given by both `--addr` and `--token`, otherwise the daemon recorded in the daemon info file, never `SPLITTY_SOCK`. For a daemon it SHALL verify and negotiate the private client endpoint and use its native session stream, without reading a token file or opening TCP. An explicit `--addr` with `--token` continues to target an explicitly running browser gateway using its guarded WebSocket. It SHALL print no credential. Without either server it SHALL use the keepers of this machine (Attach Without A Daemon), and without those fail with a command error naming both ways.

#### Scenario: Daemon on this machine
- **WHEN** a daemon runs and the user runs `splitty attach <pane-id>` outside any pane
- **THEN** the CLI streams the session over the verified private daemon endpoint without a token or ticket
- **AND** no token or ticket appears in its output

#### Scenario: Inside a pane of an embedded server
- **WHEN** `splitty attach` runs in a pane whose `SPLITTY_SOCK` names an embedded server while no daemon and no keeper runs
- **THEN** the CLI prints `Error: no running Splitty daemon found (splitty attach uses daemon.json, or --addr with --token)` and exits with status 1

#### Scenario: Explicit server
- **WHEN** the user runs `splitty attach --addr 127.0.0.1:9900 --token <token> <pane-id>`
- **THEN** the CLI resolves the target and streams the session from the server on `127.0.0.1:9900`, regardless of `daemon.json`

### Requirement: Daemon Restart Command
`splitty daemon restart` SHALL stop the running daemon as `daemon stop` does, wait until it has exited, and start a daemon of the invoking binary at the stopped daemon's private endpoint, as `daemon start` does; without a running daemon it SHALL start one as `daemon start` would. With `--end-sessions` it SHALL end the kept sessions before the start. A failed start SHALL be reported as `daemon start` reports it.

#### Scenario: Update without ending sessions
- **WHEN** a daemon runs a pane with `sleep 1000` and the user runs `splitty daemon restart`
- **THEN** a new daemon process serves the same private endpoint with a new instance identity, and the pane's session still runs the same `sleep` process

#### Scenario: Restart without a daemon
- **WHEN** no daemon runs and the user runs `splitty daemon restart`
- **THEN** a daemon starts as with `splitty daemon start`
