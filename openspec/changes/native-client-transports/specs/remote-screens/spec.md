# Spec Delta

## MODIFIED Requirements

### Requirement: Remote Screen Hub Endpoint
An explicit browser gateway SHALL accept `/ws/remote` upgrades with `ticket` and `target`, authenticated like `/ws`. An authorized native main client SHALL be able to open the equivalent host stream through its assigned service. For either client stream the hub SHALL:
1. validate the target as `splitty connect` does;
2. make sure the host has a server and a running daemon, as `splitty connect` does (see `cli`, Remote Connect Tunnel, and `remote-hosts`), without a TCP port hint and requiring native transport compatibility;
3. start `splitty remote-bridge --transport native-v1` on the host over SSH, using the installed server's absolute path when it installed one or found one installed;
4. send `remote.ready`;
5. relay every application message between the client stream and the host daemon unchanged, text as text and binary as binary, in order.

Before the relay the hub SHALL send only these application text messages:
- `{"type": "remote.progress", "step", "message"}`, with `step` one of `install`, `daemon`, `started` and `bridge`;
- exactly one `{"type": "remote.ready", "target", "daemon": {"version", "protocol", "keepsSessions", "started"}, "screens": [{"id", "name", "panes"}]}`, where `keepsSessions` is the handshake's (see `remote-hosts`, Remote Handshake);
- or `{"type": "remote.error", "code", "message", "hint"?}`, after which it SHALL close the socket. `code` is one of `invalid_target`, `host_key`, `auth`, `unreachable`, `unsupported_host`, `install`, `daemon`, `incompatible`, `bridge` and `internal`.

After `remote.ready`, the hub SHALL send no application message of its own; transport negotiation/liveness is outside this application stream. When the connection to the host ends, it SHALL close the WebSocket with code 4001 or the native stream with the equivalent host-lost reason. When the socket closes, it SHALL end its `ssh` process. Each socket SHALL use an SSH connection of its own. Once `remote.ready` is sent, the server SHALL record the target as a recent host (see `remote-hosts`, Known Hosts).

#### Scenario: Connect a host without Splitty
- **WHEN** a client opens `/ws/remote?target=vm1` with a valid ticket and `vm1` has no Splitty
- **THEN** it receives `remote.progress` messages for the install and the daemon start, then `remote.ready` with `started` true, then the host daemon's `mux.welcome`

#### Scenario: Unknown host key
- **WHEN** `ssh` to the target fails host key verification
- **THEN** the client receives `remote.error` with code `host_key` and a hint to run `ssh <target>` once in a terminal, and the socket closes

#### Scenario: Host connection lost
- **WHEN** the `ssh` process of a relaying socket ends
- **THEN** the server closes the socket with code 4001

#### Scenario: Client closes the socket
- **WHEN** the client closes a relaying socket
- **THEN** the server ends that socket's `ssh` process, and the host daemon and its sessions keep running

#### Scenario: Ready from a daemon that keeps its sessions
- **WHEN** a client opens `/ws/remote` for a host whose daemon keeps its sessions
- **THEN** its `remote.ready` carries `keepsSessions` true in `daemon`

### Requirement: Remote Screen Protocol Check
Before it starts the bridge, the server SHALL require a supported native transport and compare the host daemon's protocol version from the handshake with its own. It SHALL accept the same version and the one before (see `remote-hosts`, Protocol Version). A missing or unsupported native transport SHALL also be incompatible. Otherwise it SHALL send `remote.error` with code `incompatible`, the daemon's `version`, `protocol` and `keepsSessions`, and its `screens` as in `remote.ready`, and close the socket. It SHALL NOT stop or replace the host's daemon.

#### Scenario: Newer host
- **WHEN** the host daemon's protocol version is higher than the server's
- **THEN** the client receives `remote.error` with code `incompatible` naming the daemon's version, and the daemon keeps running

#### Scenario: What an update would end
- **WHEN** the host daemon is incompatible and runs two screens
- **THEN** the `remote.error` lists both screens with their numbers of panes

### Requirement: Remote Screen Reconnect
When a host's connection drops after `remote.ready`, the client SHALL mark that host's screens as reconnecting, keep them from taking input, and reconnect with a delay that grows to 10 seconds, with a fresh ticket for every browser attempt or a freshly authorized native stream for a native client. A failed ticket request for a host SHALL NOT be reported as the local session's authentication error. No queued terminal input or mutation from the lost connection SHALL be replayed. After a successful reconnect, the client SHALL resync the host's screens and re-attach their sessions. When the connection fails with an error code that retrying cannot fix, the client SHALL stop retrying and show the error with its hint and a retry. When a host cannot be reached again, its screens SHALL be shown as offline with a retry.

#### Scenario: Network drop
- **WHEN** the connection to `vm1` drops and the host is reachable again 20 seconds later
- **THEN** `vm1`'s screens show "reconnecting…" and then their content again, with the output written in the meantime

## ADDED Requirements

### Requirement: Native Host Management Surface
An authorized native main client SHALL have equivalents of the existing host, preference, order, catalog, remembered-screen and forwarding API operations through its assigned service. Reads and metadata edits SHALL retain their existing absence of connection side effects. Remote-window and restricted-agent connections SHALL not gain local hub or forwarding authority.

#### Scenario: Native host metadata
- **WHEN** a native main window reads or edits its saved host metadata without requesting a connection
- **THEN** the same validated metadata is returned or saved and no SSH connection or listener is started

#### Scenario: Native saved forwarding
- **WHEN** a native main window deliberately connects a host
- **THEN** only that host's enabled saved forwards/reverse forwards may restore, with the existing Stop, Forget, ordering and endpoint-exclusion rules
