# remote-hosts Specification

## Purpose
Defines what a remote host needs for Splitty and how a client installs its own server there over SSH: probing the host, the payload sources, the per-version install directories and their cleanup. How the client then reaches the host's daemon is covered by `cli`; the integrity rules of the install by `access-control`.

## Requirements

### Requirement: Supported Hosts
A remote host SHALL be reachable with the system `ssh` client and SHALL run Linux or macOS (`uname -s` `Linux` or `Darwin`) with a POSIX `sh`, `uname`, `mkdir`, `mktemp`, `cat`, `tar`, `gzip`, `ls`, `rm`, and `sha256sum` or `shasum`; a macOS host, which has no `/proc`, also `ps`. Any user SHALL be allowed, including root. A host with another kernel MUST be reported with a message naming the kernel it runs, before anything is uploaded. The supported architectures are `x86_64` (`uname -m` also `amd64`) and `aarch64` (also `arm64`). The client SHALL install only a server it holds for the host's kernel and architecture (see Payload Sources), and the check that the unpacked server runs guards against a mismatch.

#### Scenario: Unsupported kernel
- **WHEN** `splitty connect` probes a host whose `uname -s` is neither `Linux` nor `Darwin`
- **THEN** it reports that only Linux and macOS hosts are supported, names the kernel it found, and exits with status 1 without uploading anything

#### Scenario: Root on the host
- **WHEN** the SSH user on the host is root
- **THEN** the install and the connect proceed as for any other user

#### Scenario: aarch64 host
- **WHEN** a released static `splitty` for x86_64 connects to a Linux host whose `uname -m` is `aarch64` or `arm64` and that has no Splitty
- **THEN** it installs the aarch64 server it carries

#### Scenario: macOS host
- **WHEN** a released client connects to a host whose `uname -s` is `Darwin` and `uname -m` is `arm64`, and that has no Splitty
- **THEN** it installs the macOS arm64 server it carries and connects as to a Linux host

### Requirement: Server Installation
Before it asks the host for its daemon, `splitty connect` SHALL probe the host in one SSH call for its kernel, architecture, home directory, and whether the client's own version is installed in `~/.local/share/splitty/remote/<version>/` with a checksum marker that matches the client's payload.
- When the version is installed and matches, the client SHALL use it without uploading anything.
- Otherwise it SHALL upload the payload over the SSH connection's standard input into a new directory of mode 0700 below `~/.local/share/splitty/remote/`, verify the payload's SHA-256 checksum on the host, unpack it, check that the unpacked server runs on the host (`splitty --version`), record the checksum as the marker, and only then move it to `~/.local/share/splitty/remote/<version>/`, replacing a broken install of the same version.
- A host with neither `sha256sum` nor `shasum` SHALL get nothing installed, and the client SHALL say that the host lacks a SHA-256 tool, not that the upload was corrupted.
- It SHALL print that it installs, naming the version, the host and the architecture.
- After an install, and whenever the probe found the version installed, the client SHALL run the installed server by its absolute path for every later command on the host.

A `splitty` that the user installed on the host elsewhere SHALL NOT be changed or removed.

#### Scenario: Host without Splitty
- **WHEN** a client with a payload connects to a Linux x86_64 host that has no Splitty
- **THEN** it installs the server to `~/.local/share/splitty/remote/<version>/`, verifying its checksum on the host, and runs the handshake there by that path

#### Scenario: Already installed
- **WHEN** the client's version is installed on the host with a matching marker
- **THEN** the client uploads nothing and uses the installed server

#### Scenario: Corrupted upload
- **WHEN** the bytes that arrive on the host do not match the payload's checksum
- **THEN** nothing is moved to `~/.local/share/splitty/remote/<version>/`, the temporary directory is removed, and the client reports the checksum failure and exits with status 1

#### Scenario: Host without sha256sum
- **WHEN** the host has `shasum` but no `sha256sum`
- **THEN** the checksum is verified with `shasum -a 256` and the server is installed

#### Scenario: Payload does not run on the host
- **WHEN** the unpacked server cannot run on the host, for example because its architecture does not match
- **THEN** nothing is moved into place and the client reports that the server does not run on the host

### Requirement: Payload Sources
The client SHALL take the server it installs from the first available of these sources:
1. the archive named by the environment variable `SPLITTY_REMOTE_PAYLOAD`, a gzip-compressed tar archive with the server binary `splitty` and its `shell-integration` directory at the top level;
2. an archive of that layout built into the client for the host's kernel and architecture, with its checksum computed when the client was built. The Linux x86_64 and aarch64 archives and the macOS x86_64 and aarch64 archives are named at build time by `SPLITTY_REMOTE_PAYLOAD_X86_64`, `SPLITTY_REMOTE_PAYLOAD_AARCH64`, `SPLITTY_REMOTE_PAYLOAD_DARWIN_X86_64` and `SPLITTY_REMOTE_PAYLOAD_DARWIN_AARCH64`. The released static server for x86_64 carries the other three, and the released Tauri apps and Linux packages carry all four;
3. the client itself, when it is a static Linux build for the host's architecture and the host runs Linux, together with the `shell-integration` directory next to it if there is one.

The released Electron app SHALL run the static x86_64 server, so that it installs on Linux and macOS hosts of both architectures. The checksum the host verifies SHALL be computed by the client over the exact bytes it sends. When no source fits the host, the client SHALL install nothing and run `splitty` from the host's `PATH` instead. When that is missing too, its error SHALL name both ways out: installing `splitty` on the host, or a client that carries a server for the host's architecture.

#### Scenario: Development payload
- **WHEN** `SPLITTY_REMOTE_PAYLOAD` names an archive built by `packaging/windows/build-wsl-payload.sh` and the host has no Splitty
- **THEN** `splitty connect` installs the server from that archive

#### Scenario: No payload and no splitty on the host
- **WHEN** the client has no payload for the host and the host has no `splitty` in its `PATH`
- **THEN** the client reports that `splitty` is not installed on the host and that a client with a server for the host's architecture would install it, and exits with status 1

#### Scenario: Desktop app on an aarch64 host
- **WHEN** a released Tauri app opens a remote window for an aarch64 host without Splitty
- **THEN** it installs the aarch64 server it carries

#### Scenario: A Linux server is not offered to a Mac
- **WHEN** a client carries only the Linux payloads and connects to a macOS arm64 host without Splitty
- **THEN** it uploads nothing and looks for `splitty` in the host's `PATH`

### Requirement: Old Server Versions Removed
After installing a version, the client SHALL remove every other version directory below `~/.local/share/splitty/remote/` whose server binary no process on the host is running. A version that a process runs, such as the daemon, SHALL be kept. On a host without `/proc`, the running executables SHALL be taken from `ps`.

#### Scenario: Daemon of an older version keeps running
- **WHEN** the host's daemon runs from `~/.local/share/splitty/remote/0.1.0/` and a client of version 0.2.0 installs its server
- **THEN** both version directories remain, and the daemon keeps running

### Requirement: Remote Handshake
`splitty remote-handshake --json` SHALL print exactly one JSON object and exit with status 0, describing the host to a client in one call:
- `handshake`: the format version, 1;
- `server`: the `version`, the `protocol` version and the absolute `path` of the server that answers;
- `daemon`: `running`, and for a running daemon its `pid`, `port`, `version`, `protocol` version, whether it keeps its sessions across a restart (`keepsSessions`, from `daemon.json`; false when the file does not say) and whether this call `started` it;
- `screens`: the running daemon's screens, each with its `id`, `name` and number of `panes`, in the daemon's order; empty without a daemon.

With `--ensure-daemon --port <port>`, it SHALL first start the daemon on `127.0.0.1:<port>` when none is running, as `splitty daemon start` would, and report it with `started` true. When the daemon cannot be started, it SHALL print the reason to standard error and exit with status 1. A running daemon SHALL never be stopped or replaced, whatever its version. The command prints no credential.

#### Scenario: Handshake without a daemon
- **WHEN** no daemon runs on the host and a client runs `splitty remote-handshake --json`
- **THEN** it prints `daemon` with `running` false and an empty `screens` list, and exits with status 0

#### Scenario: Daemon started by the handshake
- **WHEN** no daemon runs and a client runs `splitty remote-handshake --json --ensure-daemon --port 9811`
- **THEN** a daemon is started on `127.0.0.1:9811` and the output reports it running on port 9811 with `started` true

#### Scenario: Daemon of another version
- **WHEN** a daemon of another version runs and a client runs the handshake with `--ensure-daemon`
- **THEN** the daemon keeps running, and the output reports its version and protocol version next to those of the answering server

#### Scenario: Daemon that keeps its sessions
- **WHEN** the host's daemon wrote `keepsSessions: true` into `daemon.json` and a client runs the handshake
- **THEN** the output reports the running daemon with `keepsSessions` true
- **AND** for a daemon whose `daemon.json` has no such field it reports `keepsSessions` false

### Requirement: Protocol Version
The server SHALL have a protocol version, an integer that changes when the WebSocket protocols change incompatibly; it is 1. A daemon SHALL record its protocol version in `daemon.json` (see `server-modes`). A `daemon.json` without it SHALL count as protocol 1. A client that relays the protocol to a host's daemon SHALL use the daemon's protocol version from the handshake to decide whether it can talk to that daemon; it supports its own protocol version and the one before.

#### Scenario: Older info file
- **WHEN** `daemon.json` has no `protocol` field
- **THEN** the handshake reports the daemon's protocol version as 1

### Requirement: Remote Bridge
`splitty remote-bridge` SHALL connect its standard input and output to the WebSocket of the daemon running on the same host, for a client that reaches the host over SSH:
- Each WebSocket message SHALL travel as one frame: one byte for its kind (1 text, 2 binary), its length as a 4-byte big-endian integer, then that many bytes. A frame longer than 16 MiB or of an unknown kind SHALL end the bridge.
- Frames read from standard input SHALL be sent to the daemon as messages; messages from the daemon SHALL be written to standard output as frames, in order.
- The bridge SHALL end with status 0 when its standard input ends or the daemon closes the WebSocket, and with status 1, and a message on standard error, when no daemon runs or the WebSocket cannot be opened.

#### Scenario: Welcome through the bridge
- **WHEN** a daemon runs and a client starts `splitty remote-bridge` on the host
- **THEN** the first frame on the bridge's standard output is a text frame with the daemon's `mux.welcome`

#### Scenario: No daemon
- **WHEN** no daemon runs on the host and a client starts `splitty remote-bridge`
- **THEN** the bridge prints that no daemon is running and exits with status 1

### Requirement: SSH Client
The client SHALL reach hosts with the system's OpenSSH client: on Windows with `%SystemRoot%\System32\OpenSSH\ssh.exe` when that file exists and with `ssh` from `PATH` otherwise, on other systems with `ssh` from `PATH`. On Windows it SHALL start `ssh` without a console window. The user's SSH configuration, keys and agent of that client apply.

#### Scenario: Windows without WSL
- **WHEN** a client on Windows connects to a host and `C:\Windows\System32\OpenSSH\ssh.exe` exists
- **THEN** it runs that `ssh.exe`, and no console window appears

### Requirement: Known Hosts
A client SHALL know two kinds of hosts:
- **Recent hosts**, recorded in `recent-hosts.json` in the client's configuration directory as `{"version": 1, "hosts": [{"target": "<target>", "lastConnected": <seconds since the Unix epoch>}]}`, most recent first, at most 20 entries. Recording a target moves it to the front, or adds it there. The file SHALL be replaced atomically, with mode 0600 in an owner-only directory. A file that cannot be read or parsed SHALL count as empty and be replaced on the next record.
- **Configured hosts**, the names on the `Host` lines of the user's SSH configuration (`~/.ssh/config`, on Windows `%USERPROFILE%\.ssh\config`) and of the files it includes with `Include`, where `~` is expanded, relative paths are resolved against the `.ssh` directory, and `*` and `?` match file names. Names with `*` or `?`, and names starting with `!`, SHALL be skipped. The client SHALL only read these files.

The list of known hosts SHALL be the recent hosts first, then the configured hosts not already listed, in the order of the files, then hosts with a saved display alias that are not already listed, each target once. Hidden targets SHALL be excluded. Alias-only entries SHALL have source `recent` without a connection timestamp. A name that is not a valid target (see `access-control`, Remote Access Through SSH Tunnel) SHALL be left out of both.

#### Scenario: DevPod workspaces
- **WHEN** DevPod wrote `Host api.devpod` into `~/.ssh/config`
- **THEN** `api.devpod` is a configured host

#### Scenario: Patterns skipped
- **WHEN** `~/.ssh/config` has `Host *.internal !bastion jump`
- **THEN** only `jump` is a configured host

#### Scenario: Included files
- **WHEN** `~/.ssh/config` has `Include config.d/*` and `~/.ssh/config.d/work` names `Host vm2`
- **THEN** `vm2` is a configured host

#### Scenario: Reconnect to a recent host
- **WHEN** `vm1` is the third recent host and the user connects to it again
- **THEN** `vm1` is the first recent host and appears once

#### Scenario: Corrupted file
- **WHEN** `recent-hosts.json` holds something that is not the format above
- **THEN** there are no recent hosts, and the next record replaces the file with one entry

### Requirement: Host Display Preferences And Removal
The client SHALL keep optional display aliases and hidden markers keyed by canonical SSH target in `remote-host-preferences.json` in its private configuration directory, atomically written with mode 0600. An alias SHALL NOT change an SSH target or connection identity. Aliases SHALL be trimmed, contain at most 200 Unicode characters and no controls; null or blank input SHALL clear an alias. Missing or corrupt preference files SHALL read as empty, and invalid saved rows SHALL be discarded individually.

Forgetting a host SHALL remove its recent entry, alias and cached screen directory and persist a hidden marker, including for a configured SSH alias. It SHALL NOT edit SSH configuration, end sessions or port forwards, or remove open-screen references. Automatic hub reconnection and directory observations SHALL NOT reveal a hidden host. Explicit re-addition or a successful explicit CLI/desktop connection SHALL reveal it. Metadata changes SHALL preserve unrelated hosts under concurrent clients.

#### Scenario: Alias is display metadata
- **WHEN** the user assigns Home Lab to `me@192.0.2.44`
- **THEN** that alias survives restart, the real target remains available, and connecting still uses `me@192.0.2.44`

#### Scenario: Forget a configured host
- **WHEN** the user removes a host also present in SSH configuration
- **THEN** it remains absent from saved suggestions after restart and automatic reconnect, and its active sessions and forwards continue

#### Scenario: Re-add a host
- **WHEN** the user explicitly enters a forgotten target again
- **THEN** its hidden marker is cleared and it can appear in the host list again

#### Scenario: Alias survives recent-history eviction
- **WHEN** a host with a saved display alias leaves the 20-entry recent history
- **THEN** it remains discoverable by its target and display alias until forgotten or its alias is cleared

### Requirement: Host Server Apart From The Desktop App
On a macOS host, the client SHALL run every command of the server it installed (the handshake, `daemon stop`, `remote-bridge` and `issue-bootstrap`) with the environment variable `SPLITTY_CONFIG_DIR` set to `<home>/.config/splitty`, `<home>` being the home directory the probe reported. That server's daemon, configuration, token, saved state, templates, known hosts and control sockets thereby live apart from those of a Splitty desktop app or CLI of the same user, which keep `~/Library/Application Support/splitty`. The variable SHALL reach the daemon and its panes, so that the `splitty` installed next to the server reaches the same daemon from a pane. On a Linux host the client SHALL NOT set it.

#### Scenario: Mac with the desktop app
- **WHEN** a hub connects to a Mac whose user runs the Splitty desktop app, and starts a daemon there
- **THEN** the daemon writes `daemon.json` to `~/.config/splitty`, the desktop app keeps its own server and sessions, and neither attaches to the other's daemon

#### Scenario: Update of a macOS host
- **WHEN** the user updates a macOS host from the hub
- **THEN** only the daemon in `~/.config/splitty` is stopped and replaced, and the desktop app on that Mac keeps running with its sessions

### Requirement: Host Group Order
The client SHALL keep the order in which the user arranged its host groups in `remote-host-order.json` in its private configuration directory, `{"version":1,"order":[target,…]}` with at most 256 canonical SSH targets, each once, atomically written with mode 0600. A missing or corrupt file SHALL read as empty; invalid or repeated saved targets SHALL be discarded individually. Forgetting a host SHALL remove it from the order; reading or changing it SHALL NOT connect to a host.

#### Scenario: Order survives a restart
- **WHEN** the user arranges `vm2` before `vm1` and the client restarts
- **THEN** the order read back lists `vm2` before `vm1`

#### Scenario: Forgotten host leaves the order
- **WHEN** the user forgets `vm1`, which is in the order
- **THEN** the order no longer lists `vm1`, and the other targets keep their order

#### Scenario: Corrupt order file
- **WHEN** `remote-host-order.json` holds something that is not the format above
- **THEN** the order is empty, and the aliases in `remote-host-preferences.json` are unaffected

### Requirement: Remote Relay
`splitty remote-relay` SHALL serve the host side of a reverse forward for a client that reaches the host over SSH, on Linux hosts:
- `splitty remote-relay prepare` SHALL create the directory `relay` in the server's run directory owner-only (or make an existing one owner-only, refusing a symbolic link or a directory of another owner), check that `/proc/net/tcp` is readable, remove socket files there that refuse connections and are older than one minute, print exactly one JSON line `{"version":1,"dir":"<absolute path>"}` and exit with status 0. Without `/proc/net/tcp` it SHALL print the reason to standard error and exit with status 1.
- `splitty remote-relay serve --port <port> --socket <path>` SHALL accept only a socket path directly inside that directory, named by 16 lowercase hex characters and `.sock`. It SHALL wait up to 10 seconds for the socket to exist, refuse it unless it is a socket owned by its own user, make it owner-only, listen on `127.0.0.1:<port>` and, where the host has IPv6 loopback, `[::1]:<port>`, and then print the line `SPLITTY_RELAY_READY_V1` on standard output. When it cannot, it SHALL print one line `SPLITTY_RELAY_FAILED_V1 <code> <message>` with the code `host_port_conflict` (a listener address in use), `listen` (another failure to listen), `socket` or `unsupported`, and exit with status 1.
- For each accepted connection whose client socket belongs to its own user, the relay SHALL connect to the socket and relay bytes both ways until either side closes. Other connections SHALL be closed at once.
- The relay SHALL end when its standard input ends or nothing arrives on it for 45 seconds, and SHALL remove its socket when it ends.

#### Scenario: Prepare
- **WHEN** a client runs `splitty remote-relay prepare` on a Linux host
- **THEN** it prints one JSON line with version 1 and an absolute directory that exists with mode `0700`

#### Scenario: Port in use on the host
- **WHEN** a program on the host listens on `127.0.0.1:11434` and a client starts `remote-relay serve --port 11434` with a valid socket
- **THEN** the relay prints `SPLITTY_RELAY_FAILED_V1 host_port_conflict` and a message, and exits with status 1

#### Scenario: Socket outside the relay directory
- **WHEN** a client starts `remote-relay serve` with a socket path outside the relay directory
- **THEN** the relay refuses it with code `socket` and listens on nothing

#### Scenario: Heartbeat lost
- **WHEN** a relay is serving and nothing arrives on its standard input for 45 seconds
- **THEN** it closes its listeners, removes its socket and exits
