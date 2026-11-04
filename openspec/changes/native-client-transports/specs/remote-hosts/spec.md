# Spec Delta

## MODIFIED Requirements

### Requirement: Remote Handshake
`splitty remote-handshake --json` SHALL print one format-2 JSON object and no credential: the answering server's version, mux protocol, absolute path and transport versions; daemon running state, PID, version, protocol, instance, transportVersion, startedAt, keepsSessions and started; and its ordered screen summaries. When no daemon is running, the screen list SHALL be empty. `--ensure-daemon` SHALL start a missing private-endpoint daemon without a port hint, never stop/replace a running one. Failure SHALL go to stderr with status 1.

#### Scenario: Handshake without a daemon
- **WHEN** no daemon runs and no ensure option is given
- **THEN** output reports running false and no screens with status 0

#### Scenario: Daemon started by the handshake
- **WHEN** a missing daemon is requested with `--ensure-daemon`
- **THEN** the new daemon has no TCP listener and output reports started true and its native transport/instance

#### Scenario: Daemon of another version
- **WHEN** a different daemon version already runs
- **THEN** it is left running and its actual capabilities are reported separately from the answering binary

#### Scenario: Daemon that keeps its sessions
- **WHEN** discovery says keepsSessions true
- **THEN** the handshake preserves that value, treating a missing legacy field as false

#### Scenario: Legacy daemon
- **WHEN** a format-2 answering binary discovers a live legacy daemon
- **THEN** it reports native transport as unsupported without connecting to or replacing the legacy daemon

### Requirement: Protocol Version
The server SHALL have a protocol version, an integer that changes when the mux/session application protocol changes incompatibly; it is 1. A daemon SHALL record its protocol version in `daemon.json` (see `server-modes`). A `daemon.json` without it SHALL count as protocol 1. A client that relays the protocol to a host's daemon SHALL use the daemon's protocol version from the handshake to decide whether it can talk to that daemon; it supports its own protocol version and the one before. Native transport support SHALL be negotiated independently; mux compatibility alone SHALL not authorize a legacy TCP fallback.

#### Scenario: Older info file
- **WHEN** `daemon.json` has no `protocol` field
- **THEN** the handshake reports the daemon's protocol version as 1

### Requirement: Remote Bridge
`splitty remote-bridge --transport native-v1` SHALL relay the negotiated native client connection between its inherited stdin/stdout and the verified daemon client endpoint on the host. Frames SHALL retain order and text/binary payloads, with a maximum of 16 MiB and rejection of invalid framing before application dispatch. Stdout SHALL contain only protocol bytes. EOF or a clean daemon close SHALL end with status 0; a missing daemon, failed ownership check or incompatible transport SHALL produce bounded stderr diagnostics and status 1.

#### Scenario: Welcome through the bridge
- **WHEN** a compatible daemon runs and native bridge negotiation completes
- **THEN** the first application event is that daemon's mux.welcome, after transport negotiation

#### Scenario: No daemon
- **WHEN** no daemon runs
- **THEN** the bridge reports it on stderr and exits with status 1 without opening TCP

#### Scenario: Remote path is not a local destination
- **WHEN** the host reports a path in discovery
- **THEN** only the host-side bridge resolves its configured endpoint and the client opens no local socket at the supplied path

## ADDED Requirements

### Requirement: Confirmed Legacy Daemon Migration
Handshake discovery of a legacy daemon SHALL report either a bounded migration offer or a blocked reason, without changing that daemon. An offer SHALL require an older keeper-backed daemon, the matching replacement binary, verified owner/process identity and compatible live keepers. A client SHALL expose migration only as an explicitly confirmed action; ordinary connect, Retry and reconnect SHALL NOT migrate a legacy daemon. The confirmation SHALL identify the versions and explain that keeper-managed sessions continue while clients reconnect.

The host SHALL revalidate the conditional proof immediately before signaling only the verified process generation, then start the replacement with keeper adoption and require a fresh native handshake. Changed discovery, unsupported process identity guarantees, incompatible or missing keepers, a newer daemon or a mismatched replacement SHALL refuse before signaling. No legacy TCP connection, session-ending option or forced termination SHALL be introduced. An ambiguous result, cancellation or reconnect SHALL NOT replay a migration request; another migration needs another explicit confirmation.

#### Scenario: Confirm migration of an eligible legacy host
- **WHEN** an eligible legacy host reports a migration offer and the user confirms it
- **THEN** one conditional migration runs, the new native daemon adopts the compatible keeper sessions and the app connects using fresh service identity

#### Scenario: Retry without confirmation
- **WHEN** an ordinary connection or Retry encounters a legacy daemon
- **THEN** it reports the offer or blocked reason and leaves the daemon and its sessions unchanged

#### Scenario: Conditional proof is no longer valid
- **WHEN** the process generation, owner, daemon discovery or keeper eligibility differs at migration time
- **THEN** migration refuses without signaling any process

#### Scenario: Unsupported migration platform
- **WHEN** the host cannot provide the process-identity guarantees required for legacy migration
- **THEN** it reports a blocked reason and manual migration guidance, without a weaker signaling or TCP fallback

#### Scenario: Migration result is lost
- **WHEN** SSH ends after a confirmed migration might have begun
- **THEN** migration is not replayed automatically; an ordinary later connection may discover the result without another migration request
