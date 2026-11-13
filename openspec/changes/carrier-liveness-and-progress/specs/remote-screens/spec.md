## MODIFIED Requirements

### Requirement: Remote Screen Hub Endpoint
An explicit browser gateway SHALL accept `/ws/remote` upgrades with `ticket` and `target`, authenticated like `/ws`. An authorized native main client SHALL be able to open the equivalent host stream through its assigned service. For either client stream the hub SHALL:
1. validate the target as `splitty connect` does;
2. make sure the host has a server and a running daemon, as `splitty connect` does (see `cli`, Remote Connect Tunnel, and `remote-hosts`), without a TCP port hint and requiring native transport compatibility;
3. start `splitty remote-bridge --transport native-v1` on the host over SSH, using the installed server's absolute path when it installed one or found one installed;
4. send `remote.ready`;
5. relay every application message between the client stream and the host daemon unchanged, text as text and binary as binary, in order.

Preparation progress SHALL travel once per stream. The steps are `install`, `daemon`, `update`, `started` and `bridge`, in this order. On a native host stream the hub SHALL report them only as transport progress reports during preparation, before the stream is accepted; such reports MAY also carry a phase that is not a step, such as `connecting`. On a browser `/ws/remote` socket the gateway SHALL translate those reports into `{"type": "remote.progress", "step", "message"}` text messages, at most one per step and only for a step later than the last one sent, and SHALL send no `remote.progress` it did not translate.

Before the relay the hub SHALL send only these application text messages, on a native host stream after its acceptance and on a browser socket after the translated progress:
- exactly one `{"type": "remote.ready", "target", "daemon": {"version", "protocol", "keepsSessions", "started"}, "screens": [{"id", "name", "panes"}]}`, where `keepsSessions` is the handshake's (see `remote-hosts`, Remote Handshake);
- or `{"type": "remote.error", "code", "message", "hint"?}`, after which it SHALL close the socket. `code` is one of `invalid_target`, `host_key`, `auth`, `unreachable`, `unsupported_host`, `install`, `daemon`, `incompatible`, `migration_unavailable`, `bridge` and `internal`.

After `remote.ready`, the hub SHALL send no application message of its own; transport negotiation/liveness is outside this application stream. When the connection to the host ends, it SHALL close the WebSocket with code 4001 or the native stream with the equivalent host-lost reason. When the socket closes, it SHALL end its `ssh` process. Each socket SHALL use an SSH connection of its own. Once `remote.ready` is sent, the server SHALL record the target as a recent host (see `remote-hosts`, Known Hosts).

The ready message SHALL include `update: {available, keepsSessions, toVersion?}`, the hub's update plan for the host (see Host Update); `toVersion` is the version the host's daemon gets and SHALL be present whenever `available` is true. Every hub error SHALL include `retryable`; the client SHALL use this classification when present and retain legacy classification only for older hubs. After readiness the hub SHALL validate and persist the handshake directory without interpreting relayed frames. Invalid or unwritable cache data SHALL not fail an otherwise usable connection.

#### Scenario: Connect a host without Splitty
- **WHEN** a client opens `/ws/remote?target=vm1` with a valid ticket and `vm1` has no Splitty
- **THEN** it receives `remote.progress` messages for the install and the daemon start, then `remote.ready` with `started` true, then the host daemon's `mux.welcome`

#### Scenario: Progress on a native host stream
- **WHEN** a native main client opens a host stream for a host without Splitty
- **THEN** it receives transport progress reports for the install and the daemon start during preparation, and after acceptance `remote.ready` is the first application message, followed by the host daemon's `mux.welcome`, without a `remote.progress` text

#### Scenario: Hub of the previous release
- **WHEN** a native client or the gateway opens a host stream through a hub of the previous release, which repeats its progress as `remote.progress` text after acceptance
- **THEN** the reader ignores every `remote.progress` text before `remote.ready`, so each step is shown once, from the transport progress reports; no capability is involved

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

#### Scenario: Ready from an older host that can be updated
- **WHEN** a client opens `/ws/remote` for a host whose daemon keeps its sessions and runs an older version that the hub can replace
- **THEN** its `remote.ready` carries `update` with `available` and `keepsSessions` true and `toVersion` the hub's own version

#### Scenario: Authoritative retry classification
- **WHEN** a reconnect fails and the hub reports `retryable: false`
- **THEN** the client stops automatic retrying even if an older client classified that error code as transient
