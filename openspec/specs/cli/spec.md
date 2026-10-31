# cli Specification

## Purpose
The `splitty` command-line interface starts and manages Splitty servers, connects to remote daemons over SSH, and lets scripts and shells inside panes control a running server (split, close, list, send keys) without handling credentials themselves.

## Requirements

### Requirement: Single Executable Command Surface
The CLI SHALL be a single executable named `splitty` that provides the server commands `serve`, `daemon start`, `daemon stop`, `daemon status` and `connect`, and the control commands `split`, `new-screen`, `close-pane`, `close-screen`, `rename-screen`, `list-screens`, `list-panes`, `status`, `send-keys` and `issue-bootstrap`. It SHALL additionally provide `run`, `jobs`, `inspect`, `logs`, `input`, `stop`, `forget` and `agent` with `guide`, `create`, `list`, `exec`, `revoke`, `assign` and `reclaim` subcommands.

#### Scenario: Version flag
- **WHEN** a user runs `splitty --version`
- **THEN** the CLI prints the Splitty version and exits with status 0

#### Scenario: Unknown subcommand
- **WHEN** a user runs `splitty` with a subcommand that does not exist
- **THEN** the CLI prints a usage error to stderr and exits with a non-zero status

### Requirement: Serve Command
`splitty serve` SHALL start a server in embedded mode (see `server-modes`) in the foreground, listening on the address given by `--addr`, else by the `SPLITTY_ADDR` environment variable, else `127.0.0.1:9800`, and SHALL run until it receives Ctrl+C. With `--open` the CLI SHALL open the launch URL in the default browser; without it no browser is opened. The address MUST be a loopback address (see `access-control`), and the hidden `--desktop-stdio` flag binds the server's lifetime to its launcher (see `server-modes`).

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

### Requirement: Machine-Readable Startup Output
On startup `splitty serve` SHALL write the lines `SPLITTY_PORT=<port>`, `SPLITTY_BOOTSTRAP=<code>` and `SPLITTY_URL=http://<host>:<port>/?bootstrap=<code>&desktop=1` to stdout, one per line and flushed individually, where `<code>` is a single-use bootstrap code (see `access-control`). `splitty daemon start` SHALL write only `SPLITTY_PORT=<port>`. Which credentials may appear in this output is governed by `access-control`. A reader that closes stdout early MUST NOT cause the server to exit.

#### Scenario: Serve handshake lines
- **WHEN** `splitty serve` has bound its listener
- **THEN** stdout contains `SPLITTY_PORT`, `SPLITTY_BOOTSTRAP` and `SPLITTY_URL` lines
- **AND** the bootstrap code differs from the process token

#### Scenario: Daemon handshake line
- **WHEN** `splitty daemon start` has bound its listener
- **THEN** stdout contains a `SPLITTY_PORT=<port>` line and no credential

### Requirement: Process Token Options
`splitty serve` and `splitty daemon start` SHALL accept `--token <value>` to set the process token explicitly; without it the token SHALL be resolved as specified in `access-control`. `splitty serve --oneshot-token` SHALL use a freshly generated random token for this process only instead of resolving one.

#### Scenario: One-shot token
- **WHEN** a user runs `splitty serve --oneshot-token`
- **THEN** the server uses a newly generated random token
- **AND** the token is not printed

### Requirement: Daemon Start Command
`splitty daemon start` SHALL bind its listener in the foreground, so that a busy port fails immediately with exit status 1, and then detach a background daemon (see `server-modes`) and return once that daemon is serving. If the daemon fails to start, the command SHALL print `Failed to start daemon: <reason>` to stderr, naming `daemon.log` for details, and exit with status 1. If a daemon is already registered (see `server-modes`, Single Daemon Instance), the command SHALL print `Daemon already running (pid=<pid>, port=<port>)` and exit with status 0. On platforms without Unix process support the command SHALL fail with exit status 1.

#### Scenario: Already running
- **WHEN** a daemon is running and the user runs `splitty daemon start`
- **THEN** the CLI prints `Daemon already running (pid=<pid>, port=<port>)`
- **AND** no second daemon is started

#### Scenario: Port in use
- **WHEN** the requested address is already bound by another process
- **THEN** the CLI prints `Failed to bind <addr>: <reason>` to stderr and exits with status 1

#### Scenario: Daemon fails to start
- **WHEN** the detached daemon fails before it is serving
- **THEN** the CLI prints `Failed to start daemon: <reason>` to stderr and exits with status 1

### Requirement: Daemon Stop and Status Commands
`splitty daemon stop` SHALL ask the running daemon to terminate gracefully, which leaves its sessions running in their keepers (see `session-keepers`), and wait up to about two seconds, then report either `Daemon stopped (pid=<pid>)` or that the daemon may still be shutting down. With `--end-sessions` it SHALL then end every kept session of the run directory, also when no daemon runs, wait up to 5 seconds for them and print `Ended <n> kept session(s)`. `splitty daemon status` SHALL print the daemon's PID, port, start time and version. With `--json`, `splitty daemon status` SHALL instead print exactly one JSON object and exit with status 0, whether or not a daemon runs:
- `{"running": false}` when no daemon is running;
- otherwise `"running": true` with the daemon's `pid`, `port`, `socket`, `startedAt` and `version` as its info file holds them, and no credential.

When no daemon is running, both commands SHALL print `No daemon running` (`daemon status --json` prints its object instead), and SHALL remove a daemon info file that names a dead process. Without `--json`, `daemon status` SHALL then add the number of kept sessions, when there are any, and how to reach them.

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
- **WHEN** the daemon info file names a process that is no longer running
- **THEN** `splitty daemon status` prints `No daemon running (stale info file cleaned up)`
- **AND** the info file is removed

#### Scenario: Machine-readable status of a running daemon
- **WHEN** a daemon is running and the user runs `splitty daemon status --json`
- **THEN** the CLI prints one JSON object with `running` set to true and the daemon's pid, port, control socket path, start time and version, and exits with status 0

#### Scenario: Machine-readable status without a daemon
- **WHEN** no daemon is running and the user runs `splitty daemon status --json`
- **THEN** the CLI prints `{"running":false}` and exits with status 0

### Requirement: Server Discovery Order
When a non-empty `SPLITTY_AGENT_SOCK` is set, control commands SHALL use only that socket; explicit `--addr` or `--token` SHALL be rejected rather than overriding scoped discovery. Otherwise control commands SHALL locate the server in this order: (1) `--addr` with `--token`, both non-empty, over TCP with a bearer token; (2) the control socket named by a non-empty `SPLITTY_SOCK`; (3) the control socket recorded in the daemon info file (`~/.config/splitty/daemon.json` on Linux) of a running daemon. A lone or empty explicit address/token SHALL be a usage error, never silently ignored. The CLI MUST NOT read a credential from the environment. Unreachable or revoked scoped sockets MUST NOT fall back to owner discovery.

#### Scenario: Inside a Splitty pane
- **WHEN** a control command runs in an ordinary shell spawned by Splitty, with no scoped socket or `--addr`/`--token`
- **THEN** the CLI talks to the server over the control socket named by `SPLITTY_SOCK`

#### Scenario: Outside a pane with a running daemon
- **WHEN** both socket variables are unset and a daemon is running
- **THEN** the CLI talks to the daemon over the control socket recorded in its info file

#### Scenario: Explicit target
- **WHEN** both `--addr <host:port>` and `--token <token>` are given without a scoped socket
- **THEN** the CLI sends its requests to that address over TCP with `Authorization: Bearer <token>`, regardless of `SPLITTY_SOCK` or the daemon info file

#### Scenario: Token in the environment
- **WHEN** `SPLITTY_TOKEN` or `SPLITTY_SERVER` is set and no explicit target is given
- **THEN** the CLI ignores both variables

#### Scenario: Scoped discovery fails closed
- **WHEN** `SPLITTY_AGENT_SOCK` is set but unavailable while a normal daemon is discoverable
- **THEN** the command fails and sends no request to the daemon's owner socket

#### Scenario: Explicit target under a grant
- **WHEN** a scoped invocation supplies `--addr` and `--token`
- **THEN** the CLI exits with usage status 2 without contacting the TCP target

#### Scenario: Incomplete explicit target
- **WHEN** only `--addr` or only `--token` is given
- **THEN** the CLI exits with usage status 2 before contacting a server

### Requirement: No Server Found
When discovery finds no server, a control command SHALL print `Error: no running Splitty server found (tried SPLITTY_SOCK, daemon.json)` to stderr and exit with status 1. A server started with `splitty serve` writes no daemon info file, so outside its own panes it SHALL be reachable only through explicit `--addr` and `--token`.

#### Scenario: Nothing running
- **WHEN** `SPLITTY_SOCK` is unset, no daemon is running and no explicit target is given
- **THEN** `splitty list-screens` prints the no-server error and exits with status 1

### Requirement: Implicit Pane and Screen Targeting
Control commands that act on a pane or screen SHALL use `--pane` and `--screen` when given, else `SPLITTY_PANE_ID` and `SPLITTY_SCREEN_ID` from the environment. If neither flag nor variable is present, the command SHALL fail with a command error (see Control Command Error Reporting).

#### Scenario: Current pane by default
- **WHEN** a user runs `splitty split` inside a Splitty pane
- **THEN** the pane and screen the shell runs in are split

#### Scenario: Explicit target overrides environment
- **WHEN** a user runs `splitty close-pane --pane <id> --screen <sid>` inside a pane
- **THEN** the named pane is closed, not the pane the shell runs in

#### Scenario: No target available
- **WHEN** `splitty close-screen` runs with no `--screen` and no `SPLITTY_SCREEN_ID`
- **THEN** the CLI reports the server's error and exits with status 1

### Requirement: Layout and Screen Commands
`split`, `close-pane`, `new-screen [--name <name>]`, `close-screen` and `rename-screen <name>` SHALL each submit one multiplexer operation through the mux operation endpoint (see `control-api`), whose effect is specified in `workspace-model`, and print a one-line confirmation on success. `split` SHALL request a `vertical` split (new pane below, stacked top to bottom; see `workspace-model`, Layout Tree Shape) by default and a `horizontal` split (new pane to the right, side by side) with `-H`/`--horizontal`.

#### Scenario: Default split
- **WHEN** a user runs `splitty split` inside a pane
- **THEN** the CLI requests a `vertical` split of that pane and prints `Split <pane-id> (vertical)`

#### Scenario: Horizontal split
- **WHEN** a user runs `splitty split -H`
- **THEN** the CLI requests a `horizontal` split and prints `Split <pane-id> (horizontal)`

#### Scenario: Named screen
- **WHEN** a user runs `splitty new-screen --name dev`
- **THEN** the CLI requests a new screen named `dev` and prints `Screen created (dev)`

### Requirement: Query Commands
`list-screens`, `list-panes` and `status` SHALL print human-readable output by default and the server's JSON response, pretty-printed, with `--json`. Human output of `list-screens` and `list-panes` is one tab-separated line per screen or pane, using `-` for missing values.

#### Scenario: List screens
- **WHEN** a user runs `splitty list-screens` against a server with two screens
- **THEN** two lines are printed, each with id, name and pane count
- **AND** the active screen's id is followed by ` *`

#### Scenario: JSON output
- **WHEN** a user runs `splitty list-panes --json`
- **THEN** the CLI prints the target screen's panes as pretty-printed JSON

#### Scenario: List panes
- **WHEN** a user runs `splitty list-panes` inside a pane
- **THEN** each pane of that screen is printed as `<id>`, title, cwd and `session=<session-id>`

#### Scenario: Status from inside a pane
- **WHEN** a user runs `splitty status` inside a Splitty pane
- **THEN** the output shows the control socket path as the server and `env` as the discovery source
- **AND** it shows the server mode, version, and the number of screens and sessions

### Requirement: Send Keys Command
`splitty send-keys <keys>` SHALL look up the session of the target pane and send `<keys>` to it as terminal input, after translating the escape sequences `\n`, `\t` and `\r` into newline, tab and carriage return. It SHALL print nothing on success.

#### Scenario: Run a command in the current pane
- **WHEN** a user runs `splitty send-keys "ls -la\n"` inside a pane
- **THEN** `ls -la` followed by a newline is written to that pane's session

#### Scenario: Pane without a session
- **WHEN** the target pane has no session
- **THEN** the CLI prints `Error: API error (404): no session for pane <id>` and exits with status 1

### Requirement: Control Command Error Reporting
Control commands SHALL exit with status 0 on success. On any failure they SHALL print a single `Error: <message>` line to stderr and exit with status 1, where a non-success HTTP response carrying a JSON `error` field is reported as `API error (<status>): <error>` and a transport failure as `request failed: <reason>`.

#### Scenario: Server rejects an operation
- **WHEN** the server answers a control command with status 400 and `{"error": "invalid op: …"}`
- **THEN** the CLI prints `Error: API error (400): invalid op: …` and exits with status 1

#### Scenario: Socket unreachable
- **WHEN** `SPLITTY_SOCK` names a socket that no server listens on
- **THEN** the CLI prints an `Error: request failed: could not reach <socket>: …` line and exits with status 1

### Requirement: Issue Bootstrap Command
`splitty issue-bootstrap` SHALL obtain a new single-use bootstrap code from a running server and print it as `SPLITTY_BOOTSTRAP=<code>`. It SHALL reach the server over TCP at `--addr`, else at `127.0.0.1:<port>` of the running daemon, authenticating with `--token`, else with the token from the token file. It SHALL NOT use the control socket or `SPLITTY_SOCK`, because minting a browser session is not available there (see `access-control`).

#### Scenario: Mint a code for the local daemon
- **WHEN** a daemon is running and the user runs `splitty issue-bootstrap`
- **THEN** the CLI prints exactly one `SPLITTY_BOOTSTRAP=<code>` line and exits with status 0

#### Scenario: No daemon
- **WHEN** no daemon is running and `--addr` is not given
- **THEN** the CLI prints the no-server error and exits with status 1

### Requirement: Remote Connect Tunnel
After validating the target (see `access-control`), `splitty connect` SHALL make sure the host has a server (see `remote-hosts`, Server Installation) and then ask the host for its daemon with the handshake (`remote-handshake --json --ensure-daemon`, see `remote-hosts`), run over non-interactive SSH with the installed server's absolute path when it installed one or found one installed, and with `splitty` from the host's `PATH` otherwise. The host thereby resolves its own configuration directory. When no daemon runs, the handshake SHALL start one on a port that the client found free on `127.0.0.1` and holds reserved meanwhile. The CLI SHALL then forward local `127.0.0.1:<port>` to the remote `127.0.0.1:<port>`, using the remote daemon's own port number on both ends and binding the local end to `127.0.0.1` explicitly, and verify within 15 seconds that its own `ssh` process holds the forward. When the daemon runs another version than the client, the CLI SHALL say so, and SHALL update it only when it is older than the client, keeps its sessions (`keepsSessions`) and the host's answering server is the client's version: then it SHALL stop it with `daemon stop`, run the handshake again and use the daemon that answers. Otherwise it SHALL keep using it. Every failure in this phase SHALL exit with status 1. The single-use code of Remote Connect Session Handoff SHALL be obtained with the same server.

#### Scenario: No daemon on the remote
- **WHEN** no daemon runs on the host
- **THEN** the handshake starts one on a port that is free on both ends, and the CLI connects to it

#### Scenario: Daemon cannot be started
- **WHEN** no daemon runs on the host and starting one fails
- **THEN** the CLI prints the reason the host reported and exits with status 1

#### Scenario: Splitty missing on the remote
- **WHEN** the client has no payload for the host and the remote command exits with status 127, because `splitty` is not in the `PATH` of non-interactive SSH sessions on the host
- **THEN** the CLI reports that `splitty` is not installed on the host or not in that `PATH`, and that a client with a server for the host's architecture would install it, and exits with status 1

#### Scenario: Splitty too old on the remote
- **WHEN** the host's `splitty` does not know `remote-handshake`
- **THEN** the CLI reports that the `splitty` on the host is too old for this client, and exits with status 1

#### Scenario: Local port busy
- **WHEN** the remote daemon listens on port 9800 and a local process already listens on `127.0.0.1:9800`
- **THEN** the CLI reports that local port 9800 is not available, explains that both ports must match, and suggests starting the remote daemon with `--addr 127.0.0.1:<port>`
- **AND** it exits with status 1 without opening a tunnel

#### Scenario: SSH needs interaction
- **WHEN** the SSH connection would require an interactive password or host-key prompt
- **THEN** `ssh` fails instead of prompting and the CLI exits with status 1
- **AND** when `ssh` reports a failed host key verification, the CLI adds an instruction to run `ssh <target>` once in a terminal and check the fingerprint

#### Scenario: Older daemon that keeps its sessions
- **WHEN** a client of version 0.2.0 connects to a host whose daemon of version 0.1.0 keeps its sessions, and the client installed its own server there
- **THEN** the CLI reports the update, the host's daemon is replaced by one of version 0.2.0, the processes in its panes keep running, and the CLI connects to the new daemon

#### Scenario: Newer daemon, or one whose sessions would end
- **WHEN** the host's daemon is newer than the client, or older without keeping its sessions
- **THEN** the CLI says so and keeps using that daemon, which keeps running

### Requirement: Remote Connect Session Handoff
After the tunnel is verified, `splitty connect` SHALL obtain a single-use bootstrap code by running `splitty issue-bootstrap` on the remote host and accept it only if it is 32 hexadecimal characters. It SHALL print `SPLITTY_URL=http://127.0.0.1:<port>/?bootstrap=<code>&host=<target>`, with the target percent-encoded, which the web UI shows as the name of the remote host. It SHALL open that URL in the default browser unless `--no-open` is given, and keep the tunnel open, rebuilding it when it drops (Remote Connect Reconnect), until the user ends the command. No long-lived credential SHALL cross the SSH link. The command SHALL exit with status 0 only when the user ends it with Ctrl+C or SIGTERM.

#### Scenario: Successful connect
- **WHEN** a daemon runs on the remote host and the tunnel is established
- **THEN** the CLI prints a `SPLITTY_URL` line carrying the bootstrap code and the target, opens it in the browser and prints `SSH tunnel active. Press Ctrl+C to disconnect.`

#### Scenario: No browser
- **WHEN** the user runs `splitty connect <target> --no-open`
- **THEN** the CLI prints the `SPLITTY_URL` line and opens no browser

#### Scenario: Remote cannot mint a code
- **WHEN** `splitty issue-bootstrap` fails on the remote host or returns a malformed code
- **THEN** the CLI terminates the tunnel, prints `Failed to obtain a session from the remote host: …` and exits with status 1

#### Scenario: Disconnect
- **WHEN** the user presses Ctrl+C while the tunnel is active
- **THEN** the CLI prints `Closing tunnel...`, ends its `ssh` process and exits with status 0

### Requirement: Remote Connect Reconnect
When the tunnel's `ssh` process exits while `splitty connect` is running, the CLI SHALL rebuild the tunnel instead of exiting. It SHALL print that the tunnel was lost, reserve the local port again (see `access-control`), and ask the host for its daemon again with the handshake: at once, then after waits of 1, 2, 4 and 8 seconds, then every 10 seconds, printing each failed attempt. When no daemon runs any more, the handshake SHALL start one on the previous port; when the installed server is gone, as after a rebuilt container, the CLI SHALL install it again first.
- When the same daemon answers on the same port (same pid and start time), the CLI SHALL re-open the forward, obtain no new code and print `SSH tunnel restored.`. The browser's session keeps working.
- When a different daemon answers, or the daemon's port changed, the CLI SHALL re-open the forward on the daemon's port, obtain a new single-use code as in Remote Connect Session Handoff, and print a new `SPLITTY_URL` line, opening it in the default browser unless `--no-open` was given.
- When the tunnel could not be rebuilt for 10 minutes, the CLI SHALL print that it gives up and exit with status 1.
- Ctrl+C or SIGTERM SHALL end the command at any time with status 0.

#### Scenario: Tunnel drops while the daemon keeps running
- **WHEN** the `ssh` process of an active tunnel exits and the same daemon still runs on the host
- **THEN** the CLI re-opens the forward on the same port and prints `SSH tunnel restored.` without a new `SPLITTY_URL`
- **AND** requests with the session cookie the browser already holds succeed again

#### Scenario: Daemon restarted while the tunnel was down
- **WHEN** the tunnel drops and the host's daemon has been restarted in the meantime
- **THEN** the CLI re-opens the forward on the daemon's port and prints a new `SPLITTY_URL` line with a fresh single-use code

#### Scenario: Host rebooted while the tunnel was down
- **WHEN** the tunnel drops because the host rebooted, and no daemon runs after the reboot
- **THEN** the CLI starts the daemon again on the previous port, re-opens the forward and prints a new `SPLITTY_URL` line

#### Scenario: Host stays unreachable
- **WHEN** the tunnel drops and the host cannot be reached again
- **THEN** the CLI retries with waits growing to 10 seconds, and after 10 minutes without success prints that it gives up and exits with status 1

#### Scenario: Interrupted while reconnecting
- **WHEN** the user presses Ctrl+C while the CLI waits for its next attempt
- **THEN** the CLI exits with status 0

### Requirement: Remote Connect Recent Hosts
Once `splitty connect` has obtained its first session URL, it SHALL record its target as the most recent host (see `remote-hosts`, Known Hosts). A failure to record SHALL NOT fail or end the command.

#### Scenario: Target recorded
- **WHEN** `splitty connect vm1` has printed its first session URL
- **THEN** `vm1` is the first entry of the recent hosts

#### Scenario: Failed connect
- **WHEN** `splitty connect vm1` fails before it obtains a session URL
- **THEN** the recent hosts are unchanged

### Requirement: Remote Connect Machine-Readable Output
With `--json`, `splitty connect` SHALL print on standard output only JSON objects, one per line, each with an `event` field, instead of its text output:
- `progress`, with `step` (`install`, `daemon`, `started` or `tunnel`) and `message`;
- `notice`, with `message`, for example when the daemon runs another version, or when the target could not be recorded as a recent host or the browser could not be opened;
- `ready`, with `url` (the session URL of Remote Connect Session Handoff) and `port`, once the tunnel is verified and the first URL obtained;
- `forward`, with `remote`, `local` and `url`, one per forward of `--forward` after `ready` (Remote Connect Port Forwards);
- `lost`, with `message`, when the tunnel dropped;
- `retry`, with `message` and `retryIn` (seconds), after a failed attempt to rebuild it;
- `restored`, when the same daemon is reachable again;
- `url`, with `url`, when a restarted or moved daemon needs a new session;
- `failed`, with `message` and, where the text output gives an instruction, `hint`, right before the command exits with status 1, or with status 2 for a target or a `--forward` list it rejects;
- `closed`, right before the command exits with status 0.

Everything else SHALL behave as without `--json`: the steps, the reconnect rules, the browser unless `--no-open`, and the exit status. Diagnostics of `ssh` itself MAY still appear on standard error.

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

### Requirement: Known Hosts Command
The hidden command `splitty remote-hosts --json` SHALL print the known hosts (see `remote-hosts`, Known Hosts) as one JSON array of objects, in the order of that list, each with `target`, `source` (`recent` or `ssh-config`) and, for recent hosts, `lastConnected` in seconds since the Unix epoch. Missing or unreadable files SHALL contribute nothing, and the command SHALL exit with status 0.

#### Scenario: Recent and configured hosts
- **WHEN** `vm1` is a recent host and `~/.ssh/config` names `Host api.devpod`
- **THEN** `splitty remote-hosts --json` prints an array with `vm1` (`recent`) first and `api.devpod` (`ssh-config`) after it

### Requirement: Remote Connect Port Forwards
`splitty connect` SHALL accept `--forward <list>`, repeatable, where the list is comma-separated and each entry is `PORT` or `LOCAL:REMOTE`, ports from 1 to 65535. An entry that does not parse, a remote port named twice, or a local port named twice SHALL be rejected with exit status 2 before `ssh` runs.

For each entry, the CLI SHALL forward local `127.0.0.1:<local>` to `localhost:<remote>` on the host, in the same `ssh` process as the daemon's port:
- the local port is `LOCAL` when given, and the remote port otherwise;
- when that local port is not free, it is any free loopback port.

The CLI SHALL hold every local port reserved until `ssh` takes it over, and SHALL rebuild the forwards on the same local ports whenever it rebuilds the tunnel (Remote Connect Reconnect). Once the tunnel is verified, it SHALL print one line per forward, `Forwarding http://localhost:<local> -> <target>:<remote>`. When the local port differs from the one asked for, the line SHALL say so.

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

### Requirement: Run A Managed Job
`splitty run [--tab | --split <left|right|above|below> | --new-screen] [--background] [--name <name>] [--cwd <dir>] [--screen <id>] [--pane <id>] [--request-id <id>] [--json] -- <program> [args...]` SHALL start a managed job. The default placement SHALL be a new screen and all placements SHALL preserve existing focus. `--background` SHALL explicitly select that default focus behavior, not shell backgrounding.

#### Scenario: Start backend as an inactive tab
- **WHEN** `splitty run --tab --background --name backend --cwd ./backend --json -- npm run dev` runs with a valid anchor
- **THEN** it sends literal argv `npm`, `run`, `dev`, the absolute caller-resolved directory and a request ID, and prints the accepted job's identifiers and outcome as JSON

#### Scenario: Explicit shell syntax
- **WHEN** the program arguments contain spaces, semicolons, dollar signs or quotes
- **THEN** Splitty passes them literally; shell interpretation happens only when the caller explicitly launches a shell such as `sh -lc`

### Requirement: Job Targeting And Retry Identification
For tab/split launches, the CLI SHALL use explicit pane/screen flags, otherwise the grant's configured anchor on a scoped socket, otherwise the pane environment. Missing targets SHALL fail without a global-active-screen fallback. The CLI SHALL resolve omitted cwd to its current directory. It SHALL generate a request ID when omitted and expose it on success and uncertain failures so the caller can retry that attempt.

#### Scenario: Lost launch response
- **WHEN** a launch transport fails after sending the request
- **THEN** the error reports its request ID and an unknown outcome, rather than claiming nothing started or retrying with a new ID

#### Scenario: No anchor outside a pane
- **WHEN** a user requests `run --tab` without explicit, grant or environment targets
- **THEN** the CLI reports a target error and sends no launch

### Requirement: Job Inspection And Logs Commands
`jobs` SHALL list visible jobs and `inspect <job-id>` SHALL return one job's status and launch metadata; both SHALL support `--json`. `logs <job-id>` SHALL support `--tail <lines>`, `--since <cursor>`, `--limit <bytes>`, `--wait <duration>`, `--format text|raw` and `--json`, using the managed-jobs read contract. Defaults SHALL be text, the last 100 lines, 64 KiB and no wait; raw format SHALL require `--json` and return base64.

#### Scenario: Read incremental output
- **WHEN** a client calls `logs <id> --since <cursor> --wait 5s --json`
- **THEN** one bounded JSON result reports the data, next cursor, loss, more-data flag, job state and output completion, without human progress text on stdout

#### Scenario: Mutually exclusive selections
- **WHEN** both `--tail` and `--since` are given, or raw output is requested without JSON
- **THEN** the CLI exits with usage status 2 without reading output

### Requirement: Job Input Stop And Forget Commands
`input <job-id>` SHALL read at most 16 KiB of UTF-8 from stdin and send it unchanged only to that job; it SHALL not translate backslash escapes or append Enter. `stop <job-id>` and `forget <job-id>` SHALL implement the corresponding managed-jobs operations. All three SHALL support `--json` and never infer a target from UI focus.

#### Scenario: Exact input
- **WHEN** stdin contains a literal backslash followed by `n`
- **THEN** `input` sends those two bytes, not a newline, and reports oversized or invalid UTF-8 input before sending anything

### Requirement: Agent Grant Administration
`agent create --name <label> [--screen <id> --pane <id>]` SHALL create a grant, using pane environment targets when no anchor flags are supplied; `agent list` SHALL list grants. `agent revoke <id>`, `agent assign <id> --job <job-id>` and `agent reclaim --job <job-id>` SHALL administer access, with JSON output available. These commands SHALL require owner-socket discovery and SHALL be refused under scoped discovery or TCP targeting.

#### Scenario: Reclaim before privileged manual work
- **WHEN** the user runs `agent reclaim --job <id>` through the owner socket
- **THEN** the job keeps running under user ownership and the previous agent's access is withdrawn

### Requirement: Agent Exec And Resume
`agent exec <grant-id> -- <program> [args...]` SHALL resolve a live grant through the owner socket and launch a local client with `SPLITTY_AGENT_SOCK` set to that grant's socket and `SPLITTY_SOCK` removed. It SHALL make its own CLI available in the child's PATH while preserving the remaining search entries, through the stable CLI directory while its link names that same CLI (see `terminal-sessions`, Host CLI Follows The Daemon). It SHALL not create a new grant, stop jobs on client exit, transfer job ownership, or change the client's permission/sandbox settings. Its exit status SHALL reflect the launched client's result.

#### Scenario: Relaunch Claude
- **WHEN** the user exits `agent exec <id> -- claude` and later runs it again against the same server
- **THEN** Claude uses the same grant and its previous jobs remain accessible without rerunning them

#### Scenario: Client can find the CLI
- **WHEN** `agent exec` is invoked by absolute path and the client's inherited PATH does not contain that CLI's directory
- **THEN** the launched client can invoke `splitty agent guide` by name while retaining the selected grant's scope

#### Scenario: Client keeps finding the daemon's CLI
- **WHEN** a client launched with `agent exec` keeps running while the daemon is updated to another installation
- **THEN** `splitty` in the client runs the other installation's CLI

### Requirement: Structured Job Command Failures
With `--json`, job commands SHALL emit exactly one JSON result on stdout, including errors with a stable code and, when allocated, request/job identifiers. Diagnostics SHALL go to stderr. Usage errors SHALL exit 2, failed control operations 1, and successful operations 0; inspecting an exited job SHALL not copy its process exit code into the CLI's exit status.

#### Scenario: Inspect an unsuccessful job
- **WHEN** `inspect <id> --json` successfully fetches a job that exited 7
- **THEN** the JSON reports `exitCode` 7 and the CLI exits 0

### Requirement: Scoped Administrative Commands Are Refused
With `SPLITTY_AGENT_SOCK` set, the CLI SHALL reject owner administration, credential minting, terminal attachment and server lifecycle/remote-launch entry points, including `issue-bootstrap`, `attach`, `serve`, `daemon`, `connect`, `remote-handshake` and `remote-bridge`, before file access or process launch. This CLI guard SHALL not be described as an OS sandbox.

#### Scenario: Scoped caller tries credential or daemon commands
- **WHEN** a scoped invocation runs `issue-bootstrap` or `daemon stop`
- **THEN** the CLI fails without reading the token file, contacting the owner socket or signalling the daemon

#### Scenario: Scoped caller tries to attach to a terminal
- **WHEN** a scoped invocation runs `splitty attach <pane-id>` while a daemon is running
- **THEN** the CLI fails without reading the token file or contacting any server

### Requirement: Local Agent Workflow Guide
`splitty agent guide` SHALL print a built-in Markdown guide identifying the installed CLI version and exit 0. `--json` SHALL print one object with `version` and `guide` string fields instead. Neither form SHALL discover a server, connect to a socket, read configuration or credentials, launch a process, or change state. The command SHALL work outside Splitty and under a missing, revoked or otherwise unusable scoped socket.

#### Scenario: Read the guide offline
- **WHEN** a caller runs `splitty agent guide` without a running server or accessible configuration directory
- **THEN** the versioned Markdown is printed successfully and no configuration or server state is created

#### Scenario: Scoped caller reads structured guidance
- **WHEN** `SPLITTY_AGENT_SOCK` names an unavailable grant and the caller runs `splitty agent guide --json`
- **THEN** it receives one JSON object containing the guide and CLI version, without owner discovery or network access

### Requirement: Agent Workflow Guidance Content
The guide SHALL explain listing existing jobs before launch, background placement, literal argv and cwd, stable job/request IDs, inspection, bounded cursor logs, exact input and targeted stop/forget. It SHALL distinguish ordinary terminals from managed programs and owner operations from scoped job access. It SHALL state client/server lifetime limits, sandbox limits and that terminal output is untrusted data.

#### Scenario: Agent follows a simple server example
- **WHEN** an agent reads the guide to start an HTTP demo
- **THEN** it finds a direct `run` example using `python3 -m http.server` and a loopback bind, plus commands for reading and stopping the returned job
- **AND** it is not instructed to create a custom application, type the command into an existing shell, or disable its sandbox

#### Scenario: Agent needs an interactive shell
- **WHEN** an owner-authorized agent consults the guide for a normal terminal
- **THEN** it finds the ordinary `split` command, while a scoped reader is told to use only allowed job operations and not bypass its grant

### Requirement: Open In Client Editor
`splitty open [path] [--with vscode|zed]` SHALL request opening an existing file or folder in the attached client editor. Omitted path means `.`; relative paths SHALL resolve against the caller's current directory on the originating machine. The command SHALL use `SPLITTY_SESSION_ID` or explicit `--session` and existing control discovery. Shell integration SHALL NOT define a `splitty-open` function or any other alias for it.

#### Scenario: Relative file and default folder
- **WHEN** a pane invokes `splitty open src/main.rs`, or invokes `splitty open` without a path
- **THEN** it requests respectively the resolved file or current directory of the caller, never a path on the client PC for a remote session

#### Scenario: Invalid input or missing context
- **WHEN** the file is missing, special, unsupported, or the session is absent, or an unknown editor is selected
- **THEN** the command exits unsuccessfully with an explanation without requesting a launch

#### Scenario: Outcome is a handoff
- **WHEN** the UI accepts, rejects, disconnects, expires, or cannot dispatch the request
- **THEN** the CLI reports the corresponding outcome and exits zero only for dispatched handoff, without claiming the editor connected

#### Scenario: No shell wrapper
- **WHEN** an integrated Bash or Zsh started by Splitty runs `splitty-open`
- **THEN** the shell reports that the command is not found, as for any unknown command

### Requirement: Attach Command
`splitty attach [<target>]` SHALL show one terminal session of a server in the invoking terminal: what the user types there SHALL go to the session, the session's output SHALL appear there, and every other client showing the session SHALL keep showing it. The command SHALL run until the user detaches, the session ends or the connection is lost. It SHALL require a terminal on standard input and output, and SHALL be available on Linux and macOS.

#### Scenario: Attach over an SSH login
- **WHEN** a user logged in over SSH to a machine whose daemon runs a pane with `npm run dev` runs `splitty attach <pane-id>`
- **THEN** the terminal shows that session's output, and a line typed there reaches the process

#### Scenario: Shown in a window at the same time
- **WHEN** a session is shown in a Splitty window while a terminal is attached to it and the process prints a line
- **THEN** both the window and the terminal show the line, and the window keeps the session after the terminal detaches

#### Scenario: No terminal
- **WHEN** `splitty attach <pane-id>` runs with standard input or output not a terminal, for example `ssh host splitty attach` without `-t`
- **THEN** the CLI prints `Error: splitty attach needs a terminal` and exits with status 1 without contacting a server

#### Scenario: Native Windows
- **WHEN** `splitty attach` runs on Windows outside WSL
- **THEN** the CLI prints an error that the command is not available on this platform and exits with status 1

### Requirement: Attach Target Selection
`<target>` SHALL name a pane or a session by its id, or by a prefix of at least 4 characters that matches exactly one pane or session id of the server; a target that matches none or several SHALL fail with a command error naming the candidates. Without a target, on a terminal, the command SHALL list the server's panes bound to a session, numbered, with screen name, pane title and working directory, and attach to the one the user picks. It SHALL NOT default to `SPLITTY_PANE_ID`.

#### Scenario: Pane by prefix
- **WHEN** a pane's id starts with `3f9a` and no other pane or session id does, and the user runs `splitty attach 3f9a`
- **THEN** the CLI attaches to that pane's session

#### Scenario: Ambiguous prefix
- **WHEN** two pane ids start with `3f9a` and the user runs `splitty attach 3f9a`
- **THEN** the CLI prints an error listing both panes and exits with status 1 without attaching

#### Scenario: Pick from the list
- **WHEN** the user runs `splitty attach` without a target against a daemon with three terminal panes and answers `2`
- **THEN** the CLI lists the three panes numbered 1 to 3 with screen name, title and working directory, and attaches to the second

#### Scenario: Not the current pane by default
- **WHEN** the user runs `splitty attach` without a target inside a Splitty pane
- **THEN** the CLI shows the list instead of attaching to the pane it runs in

#### Scenario: Own session refused
- **WHEN** the target resolves to the session named by the command's own `SPLITTY_SESSION_ID`
- **THEN** the CLI prints `Error: cannot attach to the session this command runs in` and exits with status 1

### Requirement: Attach Server And Credential
`splitty attach` SHALL use the server given by both `--addr` and `--token`, otherwise the daemon recorded in the daemon info file, never `SPLITTY_SOCK`. For the daemon it SHALL read the owner-only token file of this machine, obtain a single-use WebSocket ticket over loopback and open the WebSocket with the server's own origin, as `remote-bridge` does. It SHALL print no credential. Without either server it SHALL use the keepers of this machine (Attach Without A Daemon), and without those fail with a command error naming both ways.

#### Scenario: Daemon on this machine
- **WHEN** a daemon runs and the user runs `splitty attach <pane-id>` outside any pane
- **THEN** the CLI obtains a ticket with the token file and streams the session over the daemon's loopback WebSocket
- **AND** no token or ticket appears in its output

#### Scenario: Inside a pane of an embedded server
- **WHEN** `splitty attach` runs in a pane whose `SPLITTY_SOCK` names an embedded server while no daemon and no keeper runs
- **THEN** the CLI prints `Error: no running Splitty daemon found (splitty attach uses daemon.json, or --addr with --token)` and exits with status 1

#### Scenario: Explicit server
- **WHEN** the user runs `splitty attach --addr 127.0.0.1:9900 --token <token> <pane-id>`
- **THEN** the CLI resolves the target and streams the session from the server on `127.0.0.1:9900`, regardless of `daemon.json`

### Requirement: Attached Terminal
While attached, the command SHALL keep the terminal in raw mode and show the session's buffered output, then its live output. Buffered output SHALL NOT perform clipboard operations or make the terminal answer report requests (see `terminal-sessions`, Client Handling of Session Streams). On ending, it SHALL restore the terminal's settings and turn off the alternate screen, a hidden cursor, mouse reporting, bracketed paste, focus reporting, application cursor and keypad modes and text attributes.

#### Scenario: Replay without replies
- **WHEN** a session's buffered output contains an OSC 11 color query and an OSC 52 clipboard write, and a terminal attaches
- **THEN** the terminal's clipboard is unchanged, and no reply to the query reaches the session
- **AND** an OSC 11 query the program sends after the attach is answered as usual

#### Scenario: Detach out of a full-screen program
- **WHEN** a terminal is attached to a session running `vim` with mouse reporting on, and the user detaches
- **THEN** the terminal is back on its normal screen with a visible cursor, mouse reporting off and its previous line settings, and the shell it was started from works as before

### Requirement: Attach Detach Key
The detach key SHALL be Ctrl+\ unless `--detach-key ctrl-<key>` names another control character, where `<key>` is one of `a` to `z`, `\`, `]`, `^` or `_`; any other value SHALL be a usage error. The detach key SHALL NOT reach the session. On it, the command SHALL send `detach`, restore the terminal, print `[detached]` and exit with status 0; the session SHALL keep running.

#### Scenario: Detach keeps the process running
- **WHEN** a terminal is attached to a session running a build and the user presses Ctrl+\
- **THEN** the CLI prints `[detached]` and exits with status 0
- **AND** the build keeps running and the session can be attached again

#### Scenario: Another detach key
- **WHEN** the user attaches with `--detach-key ctrl-]` and presses Ctrl+\ and then Ctrl+]
- **THEN** the Ctrl+\ reaches the session and the Ctrl+] detaches

#### Scenario: Invalid detach key
- **WHEN** the user runs `splitty attach --detach-key alt-x <pane-id>`
- **THEN** the CLI exits with status 2 before contacting a server

### Requirement: Attach Size Reports
The command SHALL report its terminal like a shown pane (see `terminal-sessions`, PTY Size Policy): before `attach` it SHALL send `activity` with `takeover: true`, once attached a `resize` with its terminal's size and `redraw: true`, then a `resize` whenever the terminal's size changes, and `activity` while the user types, at most once every 5 seconds. It SHALL send no `hide` and no `idle`.

#### Scenario: The terminal takes the size
- **WHEN** a window shows a session at 120×40 under the `active` policy and a terminal of 80×24 attaches to it
- **THEN** the session's PTY size becomes 80 columns and 24 rows and the window marks the size as set by another client

#### Scenario: Terminal window resized
- **WHEN** the user resizes the attached terminal's window from 80×24 to 100×30 while it is the only active client
- **THEN** the session's PTY size becomes 100 columns and 30 rows

#### Scenario: Detach gives the size back
- **WHEN** the terminal of the previous scenario detaches and the window's user types
- **THEN** the session's PTY size becomes 120 columns and 40 rows again

### Requirement: Attach End
When the session's process exits, the command SHALL show the remaining output, restore the terminal, print `[exited with code <n>]`, `[ended by signal <name>]`, or `[session closed]` for a session closed without an exit code, and exit with status 0. When the connection to the server is lost, it SHALL restore the terminal, print `Error: lost the connection to the server` and exit with status 1. When its own terminal hangs up, it SHALL end without sending input to the session or closing it.

#### Scenario: Process exits while attached
- **WHEN** the program of an attached session exits with code 3
- **THEN** the CLI shows the program's last output, prints `[exited with code 3]` and exits with status 0

#### Scenario: Daemon stops while attached
- **WHEN** the daemon is stopped while a terminal is attached to one of its sessions
- **THEN** the terminal's settings are restored, the CLI prints `Error: lost the connection to the server` and exits with status 1

#### Scenario: SSH connection drops
- **WHEN** the SSH connection of a terminal attached to a session drops
- **THEN** the session keeps running with no input added and can be attached again

### Requirement: Daemon Restart Command
`splitty daemon restart` SHALL stop the running daemon as `daemon stop` does, wait until it has exited, and start a daemon of the invoking binary on the stopped daemon's port, as `daemon start` does; without a running daemon it SHALL start one as `daemon start` would. With `--end-sessions` it SHALL end the kept sessions before the start. A failed start SHALL be reported as `daemon start` reports it.

#### Scenario: Update without ending sessions
- **WHEN** a daemon runs a pane with `sleep 1000` and the user runs `splitty daemon restart`
- **THEN** a new daemon process serves the same port, and the pane's session still runs the same `sleep` process

#### Scenario: Restart without a daemon
- **WHEN** no daemon runs and the user runs `splitty daemon restart`
- **THEN** a daemon starts as with `splitty daemon start`

### Requirement: Attach Without A Daemon
Without `--addr` and `--token` and without a running daemon, `splitty attach` SHALL list and resolve its targets among the live keepers of this machine (see `session-keepers`) and control the chosen keeper directly, without taking it over; a keeper another client controls SHALL fail with a command error. When a daemon takes the keeper over, the command SHALL continue through the daemon's session stream without showing the buffered output again.

#### Scenario: Attach while the daemon is stopped
- **WHEN** the daemon was stopped with `splitty daemon stop` and the user runs `splitty attach <pane-id>`
- **THEN** the terminal shows the session's output, and a line typed there reaches the process

#### Scenario: The daemon starts while attached
- **WHEN** a terminal is attached directly to a keeper and `splitty daemon start` runs
- **THEN** the terminal keeps showing the session without repeating its output, and typing keeps reaching the process

#### Scenario: Controlled by another terminal
- **WHEN** no daemon runs and another terminal already controls the target's keeper
- **THEN** the CLI prints an error that another client controls the session and exits with status 1

### Requirement: Download To The Window
`splitty download <path>` SHALL resolve the path relative to the working directory, following symbolic links, take a regular file as it is and a folder as the archive of Download Folders As Zip, refuse anything else, and post the file with its `SPLITTY_SESSION_ID` (or `--session`) to `POST /api/file/offer` of the server it discovers (see `control-api`). While it waits it SHALL say on stderr that the file waits for Save in Splitty; it SHALL print the outcome and exit with status 0 only when the file was saved. Outside a Splitty terminal without `--session` it SHALL fail with a usage error, and a server without the endpoint SHALL be reported as needing an update.

#### Scenario: Saved
- **WHEN** a user runs `splitty download build/report.pdf` in a pane and chooses Save in the window
- **THEN** the command prints that the file was saved and exits with status 0

#### Scenario: Declined
- **WHEN** the user dismisses the offer in every window that showed it
- **THEN** the command prints that it was declined and exits with a non-zero status

#### Scenario: Neither a file nor a folder
- **WHEN** a user runs `splitty download` on a FIFO, a device or a missing path
- **THEN** the command fails with a usage error and no window is asked

### Requirement: Download Folders As Zip
For a folder, `splitty download` SHALL first pack it into a zip archive named `<folder name>.zip` in a new directory below `splitty/download/` of the user's cache directory that only the owner can read, report on stderr what it packed, and then offer the archive. The command SHALL remove the archive when it ends, whatever the outcome, also when it is interrupted or its terminal closes; the next folder download SHALL remove archives older than 24 hours that a killed command left behind.

#### Scenario: A folder saved as a zip
- **WHEN** a user runs `splitty download build` in a pane, `build` being a folder, and chooses Save in the window
- **THEN** the window saves `build.zip`, which unpacks to a folder `build` with the folder's files, the command exits with status 0, and no archive remains in the cache

#### Scenario: Interrupted while waiting
- **WHEN** the user presses Ctrl+C while the command waits for Save of a folder's archive
- **THEN** the command removes the archive and exits with a non-zero status

### Requirement: What A Folder Archive Holds
A folder's archive SHALL hold one top-level folder of the folder's name with every file and folder below it, hidden ones included, with their Unix permissions and modification times. Symbolic links and special files below the folder SHALL be left out and their number reported on stderr. An entry that cannot be read or whose name is not valid UTF-8 SHALL fail the command, and packing SHALL stop with a usage error once the archive exceeds 512 MiB; neither offers anything.

#### Scenario: Links left out
- **WHEN** a folder holds a file, an empty folder, an executable script and a symbolic link, and the user downloads it
- **THEN** the archive holds the file, the empty folder and the script with its executable bit, not the link, and stderr says one entry was skipped

#### Scenario: Too large
- **WHEN** a folder packs to more than 512 MiB
- **THEN** the command fails with a usage error naming the limit, no window is asked, and no archive remains in the cache
