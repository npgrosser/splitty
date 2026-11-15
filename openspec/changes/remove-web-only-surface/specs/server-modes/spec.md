## ADDED Requirements

### Requirement: Server Mode In The Status
The server SHALL report its mode to clients as `serverMode` with the value `"daemon"` or `"embedded"` in the response of `GET /api/mux/status` (see `control-api`). `mux.welcome` SHALL NOT carry it (see `mux-protocol`, Welcome Announces Capabilities).

#### Scenario: Status from a daemon
- **WHEN** a client sends `GET /api/mux/status` to a server started with `splitty daemon start`
- **THEN** the response contains `"serverMode": "daemon"`

#### Scenario: Status from an embedded server
- **WHEN** a pane process of the desktop app's embedded service requests `GET /api/mux/status` through `SPLITTY_SOCK`
- **THEN** the response contains `"serverMode": "embedded"`

## REMOVED Requirements

### Requirement: Server Mode Reporting
**Reason**: `mux.welcome` no longer carries `serverMode`; no remaining client read it there. The status report moves to Server Mode In The Status.
**Migration**: Read `serverMode` from `GET /api/mux/status`.
