## MODIFIED Requirements

### Requirement: Supported Hosts
A remote host SHALL be reachable with the system `ssh` client and SHALL run Linux or macOS (`uname -s` `Linux` or `Darwin`) with a POSIX `sh`, `uname`, `mkdir`, `mktemp`, `cat`, `tar`, `gzip`, `ls`, `rm`, and `sha256sum` or `shasum`; a macOS host, which has no `/proc`, also `ps`. Any user SHALL be allowed, including root. A host with another kernel MUST be reported with a message naming the kernel it runs, before anything is uploaded. The supported architectures are `x86_64` (`uname -m` also `amd64`) and `aarch64` (also `arm64`). The client SHALL install only a server it holds for the host's kernel and architecture (see Payload Sources), and the check that the unpacked server runs guards against a mismatch.

#### Scenario: Unsupported kernel
- **WHEN** the hub probes a host whose `uname -s` is neither `Linux` nor `Darwin`
- **THEN** it reports `remote.error` with code `unsupported_host`, saying that only Linux and macOS hosts are supported and naming the kernel it found, without uploading anything

#### Scenario: Root on the host
- **WHEN** the SSH user on the host is root
- **THEN** the install and the connect proceed as for any other user

#### Scenario: aarch64 host
- **WHEN** the hub of a released static `splitty` for x86_64 opens a Linux host whose `uname -m` is `aarch64` or `arm64` and that has no Splitty
- **THEN** it installs the aarch64 server it carries

#### Scenario: macOS host
- **WHEN** a released client connects to a host whose `uname -s` is `Darwin` and `uname -m` is `arm64`, and that has no Splitty
- **THEN** it installs the macOS arm64 server it carries and connects as to a Linux host

### Requirement: Server Installation
Before it asks the host for its daemon, the hub SHALL probe the host in one SSH call for its kernel, architecture, home directory, and whether the client's own version is installed in `~/.local/share/splitty/remote/<version>/` with a checksum marker that matches the client's payload.
- When the version is installed and matches, the client SHALL use it without uploading anything.
- Otherwise it SHALL upload the payload over the SSH connection's standard input into a new directory of mode 0700 below `~/.local/share/splitty/remote/`, verify the payload's SHA-256 checksum on the host, unpack it, check that the unpacked server runs on the host (`splitty --version`), record the checksum as the marker, and only then move it to `~/.local/share/splitty/remote/<version>/`, replacing a broken install of the same version.
- A host with neither `sha256sum` nor `shasum` SHALL get nothing installed, and the client SHALL say that the host lacks a SHA-256 tool, not that the upload was corrupted.
- It SHALL report the `install` step, naming the version, the host and the architecture.
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
- **THEN** nothing is moved to `~/.local/share/splitty/remote/<version>/`, the temporary directory is removed, and the hub reports the checksum failure as `remote.error` with code `install`

#### Scenario: Host without sha256sum
- **WHEN** the host has `shasum` but no `sha256sum`
- **THEN** the checksum is verified with `shasum -a 256` and the server is installed

#### Scenario: Payload does not run on the host
- **WHEN** the unpacked server cannot run on the host, for example because its architecture does not match
- **THEN** nothing is moved into place and the client reports that the server does not run on the host

### Requirement: Payload Sources
The client SHALL take the server it installs from the first available of these sources:
1. the archive named by the environment variable `SPLITTY_REMOTE_PAYLOAD`, a gzip-compressed tar archive with the server binary `splitty` and its `shell-integration` directory at the top level;
2. an archive of that layout built into the client for the host's kernel and architecture, with its checksum computed when the client was built. The Linux x86_64 and aarch64 archives and the macOS x86_64 and aarch64 archives are named at build time by `SPLITTY_REMOTE_PAYLOAD_X86_64`, `SPLITTY_REMOTE_PAYLOAD_AARCH64`, `SPLITTY_REMOTE_PAYLOAD_DARWIN_X86_64` and `SPLITTY_REMOTE_PAYLOAD_DARWIN_AARCH64`. The released static server for x86_64 carries the other three, and the released Linux and macOS desktop apps carry all four;
3. the client itself, when it is a static Linux build for the host's architecture and the host runs Linux, together with the `shell-integration` directory next to it if there is one.

The checksum the host verifies SHALL be computed by the client over the exact bytes it sends. When no source fits the host, the client SHALL install nothing and run `splitty` from the host's `PATH` instead. When that is missing too, its error SHALL name both ways out: installing `splitty` on the host, or a client that carries a server for the host's architecture.

#### Scenario: Development payload
- **WHEN** `SPLITTY_REMOTE_PAYLOAD` names an archive built by `packaging/windows/build-wsl-payload.sh` and the host has no Splitty
- **THEN** the hub of a client started with that variable installs the server from that archive

#### Scenario: No payload and no splitty on the host
- **WHEN** the client has no payload for the host and the host has no `splitty` in its `PATH`
- **THEN** the client reports that `splitty` is not installed on the host and that a client with a server for the host's architecture would install it, and installs nothing

#### Scenario: Desktop app on an aarch64 host
- **WHEN** a released Linux or macOS desktop app opens a host on aarch64 without Splitty
- **THEN** its local service installs the aarch64 server the app carries

#### Scenario: A Linux server is not offered to a Mac
- **WHEN** a client carries only the Linux payloads and connects to a macOS arm64 host without Splitty
- **THEN** it uploads nothing and looks for `splitty` in the host's `PATH`

### Requirement: Host Server Apart From The Desktop App
On a macOS host, the client SHALL run every command of the server it installed (the handshake, `daemon stop` and `remote-bridge`) with the environment variable `SPLITTY_CONFIG_DIR` set to `<home>/.config/splitty`, `<home>` being the home directory the probe reported. That server's daemon, configuration, saved state, templates, known hosts and control sockets thereby live apart from those of a Splitty desktop app or CLI of the same user, which keep `~/Library/Application Support/splitty`. The variable SHALL reach the daemon and its panes, so that the `splitty` installed next to the server reaches the same daemon from a pane. On a Linux host the client SHALL NOT set it.

#### Scenario: Mac with the desktop app
- **WHEN** a hub connects to a Mac whose user runs the Splitty desktop app, and starts a daemon there
- **THEN** the daemon writes `daemon.json` to `~/.config/splitty`, the desktop app keeps its own server and sessions, and neither attaches to the other's daemon

#### Scenario: Update of a macOS host
- **WHEN** the user updates a macOS host from the hub
- **THEN** only the daemon in `~/.config/splitty` is stopped and replaced, and the desktop app on that Mac keeps running with its sessions

### Requirement: Bounded Host Preparation
Preparing a host terminal connection SHALL have a fixed ten-minute ceiling shared by every client path through the hub. An install payload upload MAY exceed the ordinary two-minute command deadline while writes progress; it SHALL stop after at most eight minutes or sixty seconds without a successful stdin write. After stdin completes, command output/process completion SHALL have at most two minutes. No-input setup commands SHALL retain that two-minute bound. Preparation messages and unrelated stdout/stderr activity SHALL NOT extend upload or overall preparation deadlines. Framing and service negotiation SHALL retain their five-second limits.

Cancellation or a deadline SHALL terminate only the attempt's owned workers and retain admission until their exit is observed. It SHALL NOT repeat admitted mutations or weaken checksum verification before installation publication.

#### Scenario: Slow progressing installation
- **WHEN** an initial payload upload takes more than two minutes but progresses within its idle and total limits
- **THEN** the preparation wrappers allow it to finish and proceed to verified daemon discovery

#### Scenario: Stalled upload with output activity
- **WHEN** the stdin upload makes no write progress for sixty seconds while the process still produces output
- **THEN** setup fails and its owned worker is terminated and reaped before releasing worker capacity

#### Scenario: Continuously progressing upload exceeds total limit
- **WHEN** upload writes keep progressing but the upload takes eight minutes
- **THEN** setup fails without renewing its deadline from progress

#### Scenario: Upload finished but command stalls
- **WHEN** stdin has completed and the command does not finish within two minutes
- **THEN** setup fails rather than consuming the remaining host-preparation allowance indefinitely

#### Scenario: Preparation cancelled during upload
- **WHEN** the owning connection is cancelled while stdin is blocked
- **THEN** setup is cancelled immediately and worker capacity remains reserved until exit is observed

### Requirement: SSH Questions Of The Local Service
A local service that runs `ssh` for its hub SHALL hand the questions OpenSSH asks its askpass program (passwords, key passphrases, host key confirmations) to its local clients, never to the terminal it may have been started from. `ssh.prompts.get` (HTTP `GET /api/remote/ssh-prompts`) SHALL list the pending questions with their identifier, host target, question text and whether it is a confirmation; `ssh.prompts.answer` (HTTP `POST /api/remote/ssh-prompts`, body `{id, answer}`, `answer` `null` to cancel) SHALL settle one question once and report `{accepted}`. Both are local hub operations: remote-window connections and host relays SHALL be refused them.

A service that announces `ssh-prompts-v1` in `mux.welcome` SHALL send `{"type":"ssh.prompts.changed"}` on its local main connections when a question appears or goes, and once after the greeting while questions are pending. The notification SHALL carry neither question text nor answer, MAY coalesce, and SHALL not be sent on remote-window connections or into host relays.

The native client SHALL show the questions of its local service, whether that service is embedded or the WSL backend, read with `ssh.prompts.get` after each notification, and answer through the service; it SHALL not poll a service that announces the capability, and it SHALL not hand the service an askpass program of its own (provisional, tbd). Without the capability it SHALL poll `ssh.prompts.get` at most once a second and stop at the first refused read (provisional, tbd). A question the user dismisses SHALL be answered with a cancel.

#### Scenario: A password question reaches the native window
- **WHEN** `ssh` asks for a password while the local service connects a host for a native window
- **THEN** the service sends `ssh.prompts.changed`, the window reads the question with `ssh.prompts.get` and shows it
- **AND** the answer the user gives reaches the waiting `ssh` through `ssh.prompts.answer`, and the next read no longer lists the question

#### Scenario: Questions of the WSL backend
- **WHEN** the native app on Windows runs its local service in WSL and that service's `ssh` asks a question
- **THEN** the question reaches the app's window over the backend's connection like one of an embedded service, and no further app process or window is started for it

#### Scenario: Dismissed question
- **WHEN** the user cancels or closes a question
- **THEN** the client answers it with `null`, `ssh` gets no answer and the authentication is cancelled

#### Scenario: Pending before the client connected
- **WHEN** a local main client connects while a question is pending
- **THEN** it receives `ssh.prompts.changed` after the greeting and can read and answer the question

#### Scenario: Restricted connections
- **WHEN** a remote-window connection or a host relay is connected while questions appear
- **THEN** it receives no `ssh.prompts.changed`, its welcome lacks `ssh-prompts-v1`, and its `ssh.prompts.get` and `ssh.prompts.answer` are refused

#### Scenario: Older local service
- **WHEN** the native client's local service does not announce `ssh-prompts-v1`
- **THEN** the client polls `ssh.prompts.get` at most once a second and answers as above
- **AND** when the service refuses the read, the client stops polling and shows no question
