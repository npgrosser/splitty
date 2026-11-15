## ADDED Requirements

### Requirement: Native Transport Version Negotiation
A native connection SHALL negotiate transport version 1 independently of mux and verify the service instance/limits before application requests. Each framing/service handshake SHALL fail within 5 seconds if incomplete. A launcher MAY report preparation progress under its existing startup deadlines, but SHALL admit no application input until service negotiation completes. Unsupported versions, invalid framing and mismatched discovery SHALL fail before dispatch without a fallback listener. The transport hello SHALL carry no host-open intent: a hello with `openHost` SHALL be refused before transport acceptance on every carrier, and host streams SHALL be opened only through the owner helper's open command (see Remote Screen Hub Access in `access-control`).

#### Scenario: Unsupported endpoint
- **WHEN** a client reaches an endpoint with an unsupported native transport version
- **THEN** it reports incompatibility and sends no application operation or TCP fallback request

#### Scenario: Stale discovery
- **WHEN** the endpoint's service instance differs from the instance in the discovery record
- **THEN** the client rejects it and refreshes discovery without sending queued input

#### Scenario: Helper still preparing its backend
- **WHEN** a native helper has negotiated framing but is still starting WSL or preparing SSH
- **THEN** it can report bounded progress, but no application request is dispatched before the verified service generation is ready

#### Scenario: Host-open intent in the hello
- **WHEN** a peer sends a hello with `openHost` to the daemon's native endpoint or on an assigned direct or SSH remote carrier
- **THEN** the hello is refused before transport acceptance and no SSH preparation or host stream starts

## REMOVED Requirements

### Requirement: Native Transport Negotiation
**Reason**: Its host-open intent in the transport hello served only the browser gateway's host streams. The rest of the requirement moves to Native Transport Version Negotiation.
**Migration**: Open host streams through the owner helper's open command.
