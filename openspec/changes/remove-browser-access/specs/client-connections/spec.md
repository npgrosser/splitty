## ADDED Requirements

### Requirement: Independent Connection Lifetimes
Each application connection, direct or through an app-owned helper, SHALL retain its own service client identity, attachments, resize contributions, subscriptions and file/editor registration. Closing or stalling one client SHALL not close another or transfer its pending responses, registrations or authority. Closing or reloading a document SHALL NOT end the app-owned embedded service. Control-socket requests SHALL retain their existing actor semantics independently of the native connections' attachments.

#### Scenario: Two native clients
- **WHEN** two `splitty attach` terminals use one daemon and one of them closes
- **THEN** only that client's registrations and subscriptions are removed, the other continues and sessions keep running

#### Scenario: Multiple clients of a host
- **WHEN** two desktop apps use one host daemon through their hubs and one stream stalls or fails
- **THEN** the other retains its own ordered stream, identity and pending responses

#### Scenario: Documents share an app-owned embedded service
- **WHEN** multiple document connections use the same app-owned helper and one closes or reloads
- **THEN** only that connection is revoked, the service instance and local sessions remain available to the other and replacement connections
- **AND** an invalid host-open or saturated data stream cannot terminate the other document connections
- **AND** closing the app-owned outer pipe ends the embedded service and its local sessions/jobs

## MODIFIED Requirements

### Requirement: Equivalent Client Operations
Authorized native clients and owner CLI requests over the control socket SHALL observe the same results, validation errors and workspace/session effects for equivalent operations. Transport selection SHALL NOT change job ownership, scratchpad access, file/editor admission or session lifetime.

#### Scenario: Shared state across transports
- **WHEN** a native client changes a screen and a second native client and the owner CLI query that service
- **THEN** all observe the same resulting workspace and session identifiers

#### Scenario: Invalid operation through another transport
- **WHEN** equivalent invalid requests arrive through native IPC and the owner control socket
- **THEN** both reject them with equivalent error/status semantics and no effects

### Requirement: Native Transport Negotiation
A native connection SHALL negotiate transport version 1 independently of mux and verify the service instance/limits before application requests. Each framing/service handshake SHALL fail within 5 seconds if incomplete. A launcher MAY report preparation progress under its existing startup deadlines, but SHALL admit no application input until service negotiation completes. Unsupported versions, invalid framing and mismatched discovery SHALL fail before dispatch without a fallback listener.

#### Scenario: Unsupported endpoint
- **WHEN** a client reaches an endpoint with an unsupported native transport version
- **THEN** it reports incompatibility and sends no application operation or TCP fallback request

#### Scenario: Stale discovery
- **WHEN** the endpoint's service instance differs from the instance in the discovery record
- **THEN** the client rejects it and refreshes discovery without sending queued input

#### Scenario: Helper still preparing its backend
- **WHEN** a native helper has negotiated framing but is still starting WSL or preparing SSH
- **THEN** it can report bounded progress, but no application request is dispatched before the verified service generation is ready

#### Scenario: Owner host-open intent is pinned before preparation
- **WHEN** an authorized main broker opens a separate host stream through a private owner endpoint
- **THEN** the endpoint verifies the expected local service instance before preparing SSH and assigns the validated target for the lifetime of that stream
- **AND** a stale local instance cannot initiate SSH work through a replacement hub

#### Scenario: Remote carriers cannot open a local host stream
- **WHEN** a peer sends host-open intent on an assigned direct or SSH remote carrier
- **THEN** it is rejected before transport acceptance and causes no local host preparation

### Requirement: Resource Admission
Admission SHALL precede application-owned per-client state allocation, backend dialing and worker spawn, with no unbounded wait queue. Framework ingress allocation before the application handler is outside this admission guarantee. Initial ceilings SHALL be:

- 256 accepted transport connections per process, including incomplete handshakes;
- 128 live application streams/native handles and 16 pending connection establishments per process;
- 64 live streams per assigned backend;
- 32 live streams and 4 pending opens per native window/shared trust domain;
- 32 owned SSH/transport-helper workers per process, including setup and deliberate forwarding workers.

Anonymous work SHALL consume process quotas. Excess admitted work SHALL receive a busy result; excess before negotiation SHALL close before upstream work. Slots SHALL release exactly once after cleanup; worker slots only after observed exit. Cancellation and liveness of admitted clients SHALL remain usable. Handshakes SHALL time out in 5 seconds and incomplete native frames in 60 seconds.

#### Scenario: Idle stream flood
- **WHEN** a client opens many idle streams that remain below the queued-byte budget
- **THEN** stream/handle/worker counts stay within all applicable quotas, excess work causes no extra dial/spawn, and existing clients retain cancellation and liveness

#### Scenario: Incomplete handshake and frame flood
- **WHEN** callers hold connections with incomplete handshakes or frames
- **THEN** process admission remains bounded before negotiation and the specified deadlines release those connections without upstream work

#### Scenario: Quota recovery
- **WHEN** an admitted open is cancelled, times out, fails, or its owned worker exits
- **THEN** its capacity returns exactly once after cleanup and can serve another caller without leaking or prematurely reusing a worker slot

## REMOVED Requirements

### Requirement: Independent Client Lifetimes
**Reason**: Restated as Independent Connection Lifetimes for native clients; the browser tabs and gateways its scenarios described are removed.
