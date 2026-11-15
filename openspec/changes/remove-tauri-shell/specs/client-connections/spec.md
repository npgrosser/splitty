## MODIFIED Requirements

### Requirement: Bounded Delivery
Native frames SHALL be at most 16 MiB. API bodies SHALL be at most 1 MiB or their existing smaller endpoint limit. Each connection SHALL admit at most 64 pending API requests and 32 MiB of application-owned queued/in-flight transport data; all connections together SHALL reserve at most 128 MiB of such transport data per process. Application-owned reassembly and queues SHALL count against these limits. Incomplete negotiation SHALL hold no application state.

These numeric allowances do not bound memory a framework allocates before the application handler validates a request, storage outside the accounted transport buffers, or total process memory. These exclusions SHALL NOT relax application frame/body limits, transport-buffer accounting, request limits, connection/worker admission, or role checks.

#### Scenario: Oversized frame
- **WHEN** a native transport peer announces a frame longer than the limit or exceeds its allowed reassembly budget
- **THEN** the connection ends before the application allocates the declared transport payload or partially applies it
- **AND** this preallocation guarantee does not cover framework ingress allocation that precedes the application handler

#### Scenario: Frozen client
- **WHEN** a client stops consuming output and reaches the delivery limit
- **THEN** it is disconnected without silently dropping part of a terminal message, other clients remain usable and the session keeps capturing bounded output

#### Scenario: Request flood
- **WHEN** a client already has 64 pending API requests
- **THEN** additional requests are refused with a busy result without starting their operations

### Requirement: Resource Admission
Admission SHALL precede application-owned per-client state allocation, backend dialing and worker spawn, with no unbounded wait queue. Framework ingress allocation before the application handler is outside this admission guarantee. Initial ceilings SHALL be:

- 256 accepted transport connections per process, including unauthenticated gateway HTTP and handshakes;
- 128 live application streams/native handles and 16 pending connection establishments per process;
- 64 live streams per assigned backend;
- 32 live streams and 4 pending opens per native window/shared trust domain or authenticated browser session;
- 32 owned SSH/transport-helper workers per process, including setup and deliberate forwarding workers.

Anonymous work SHALL consume process quotas. Excess authenticated work SHALL receive a busy/503 result; anonymous excess SHALL close before upstream work. Slots SHALL release exactly once after cleanup; worker slots only after observed exit. Cancellation and liveness of admitted clients SHALL remain usable. Handshakes SHALL time out in 5 seconds, incomplete native frames in 60 seconds, and unauthenticated HTTP header/idle reads in 15 seconds.

#### Scenario: Idle stream flood
- **WHEN** a client opens many idle streams that remain below the queued-byte budget
- **THEN** stream/handle/worker counts stay within all applicable quotas, excess work causes no extra dial/spawn, and existing clients retain cancellation and liveness

#### Scenario: Incomplete handshake and frame flood
- **WHEN** callers hold connections with incomplete handshakes, headers or frames
- **THEN** process admission remains bounded before authentication and the specified deadlines release those connections without upstream work

#### Scenario: Quota recovery
- **WHEN** an admitted open is cancelled, times out, fails, or its owned worker exits
- **THEN** its capacity returns exactly once after cleanup and can serve another caller without leaking or prematurely reusing a worker slot

### Requirement: Feature Parity Without Hidden Listeners
Released native clients SHALL support the existing settings, themes, shell profiles, templates, notifications, files, editor confirmations, zoom and host-management features allowed by their role. Using those features SHALL NOT start a fallback HTTP listener or grant arbitrary native privileges. Explicitly experimental development executables MAY implement a documented subset; they SHALL retain the same transport admission, backend identity and authority boundaries and SHALL NOT replace released clients.

#### Scenario: Native desktop workflow
- **WHEN** a desktop user saves settings, changes a theme, transfers a file, confirms an editor request and changes the zoom
- **THEN** the operations retain their existing validation and lifetime behavior without starting a terminal-server TCP listener

#### Scenario: Experimental executable
- **WHEN** a development-only native executable provides a terminal and document probe
- **THEN** it identifies its experimental scope, keeps released clients available and introduces no fallback HTTP listener

## REMOVED Requirements

### Requirement: Native Document Lifetime
**Reason**: It bound native connection handles to a WebView window, its top-level document generation and role, and described same-origin popouts and child frames: properties of the Tauri shell's packaged documents. The native app has no web documents, reloads, child frames or popouts. Its connections stay bound to one backend and one generation by Fixed Backend And Connection Identity, and the Windows native app chooses its distribution at start, so no live connection can be readdressed to another distribution.
**Migration**: None.
