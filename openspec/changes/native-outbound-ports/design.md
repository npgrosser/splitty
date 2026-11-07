# Design

## Context

See [proposal.md](proposal.md). This follows native host management and consumes its fixed-local ownership conventions without putting forwarding into its metadata scheduler.

- `server/src/hub/forwards.rs` and `preferences.rs` already own active/saved mappings, per-key ordering, restoration, loopback reservations and supervisor cleanup. `service/dispatch.rs` exposes the existing `forward.*` and `forwardPreferences.*` operations with `LocalHub` authority.
- `core/src/native_api.rs` gives opening/stopping a forward a 180-second deadline. `client-core/src/api_stream.rs` and `transport/src/api_request.rs` currently execute one request at a time per mutable stream. Sharing the host metadata FIFO would make a slow SSH attempt block unrelated work.
- `server/src/client_session/ports.rs` emits `ports.changed` initially and on changed Linux scans. Other platforms emit nothing. `native_workspace/source.rs` has source-generation admission but no retained port observation.
- The web reference is `frontend/src/lib/remote/port_forwarding.ts`, `ports.ts`, `manual_port.ts` and the Hosts port components. Domain behavior is useful reference; popup reservations, DOM focus helpers and browser polling compatibility are not native infrastructure.
- `gpui/src/terminal/links.rs` refuses remote loopback URLs today. `client-core/src/external_links.rs` already normalizes bounded HTTP(S) URLs and recognizes loopback/wildcard addresses more broadly than the web helper.
- The Windows native workspace runs its local service inside the explicitly selected WSL distribution. Current tunnel verification runs there, whereas `cx.open_url` invokes the Windows browser. No Windows-side endpoint authority verifier is present.

## Goals / Non-Goals

**Goals:** Finish explicit outbound mapping use without coupling it to remote Screen lifetime; make source observations, service snapshots, mutation acknowledgements and browser consent separate owned states; retain responsive terminal and metadata work during slow forwarding.

**Non-Goals:** See the proposal. In particular, this change does not make the client a tunnel supervisor, interpret remote output as consent, port browser popup workarounds, or resolve cross-runtime loopback authority by inventing a proxy protocol.

## Decisions

### 1. One Ports feature with separate observation and service state

Add `splitty-client-core::ports` as a cohesive UI-free feature module. Keep validated mapping keys, request/result types, bounded response decoding and row projection there. `SourceWorkspace` owns its observation type under `workspace` and retains an optional `Arc<Charged<PortObservation>>`; `SourceRuntime::event` accepts a list only for its current source/connection, reserves retained allocation before copying, and clears it on disconnect or replacement. Retain at most 256 unique positive ports per source and bounded session/process text. A malformed or over-limit replacement is rejected atomically; it does not fabricate a successful empty list. No observation survives as live data for a replacement source.

The local-owner Ports state retains active forwards and saved preferences independently, including last-good values and read failures. Projection joins by `(canonical target, remote port)`, without requiring directory membership or an open Screen. A forgotten host with an existing mapping remains manageable as forwarding work; it is not silently re-added to the saved host directory. Observe remote sources for forwarding suggestions; local observations do not create forwards.

This keeps observation state with the connection that establishes its meaning, rather than in a widget or a second registry. Portable client reply types must not import server persistence structs: active server structs are serialization-oriented, and persisted preference structs may intentionally skip deserializing runtime errors. Introduce shared response contracts under `core::native_api` only where both adapters genuinely consume them, preserving existing JSON exactly.

### 2. Bounded owned API jobs, independent of reads and host metadata

The native worker owns one Ports controller. Each admitted mapping operation owns its API setup/request/cleanup future through `OwnedService`; a dedicated passive stream reads active and saved state. Reuse `ApiStream` and transport transactions. Do not introduce a generic job framework, another SSH connector, or a multiplexing rewrite.

Initially admit at most four simultaneously running mapping jobs and sixteen pending intents across the feature (NOP-1). One mapping has one active mutation lane; its follow-up Stop is ordered after already submitted work while immediately revoking older browser-open consent. Different mappings use independent jobs, so one slow target cannot monopolize all progress. Admission and retained response bytes remain bounded by existing process/connection budgets; keep slots until cleanup is observed. Duplicate retained request identities cannot resubmit work. Saturation is an identified refusal, never an unbounded backlog or a claimed Stop.

Reads refresh on first use, explicit refresh, owner replacement and `hub.changed`; coalesce invalidations and schedule a fresh read after mutation settlement. A supporting service needs no timer. An unsupported capability is visible; do not add native polling merely to imitate the web fallback. A passive read and mutation acknowledgement have separate generations: invalidation can supersede a snapshot but cannot erase a valid acknowledgement. A read begun before a mutation cannot overwrite the newer mapping state. Failed active and saved reads preserve each resource separately.

### 3. Explicit requests and authoritative outcomes

Freeze a narrow seam before GPUI changes: validated `ForwardRequest` constructors for Start, Stop, Save, Forget and Open; `MappingKey` readers; `ForwardSnapshot`; and `ForwardResult` carrying full local `SourceToken`, request ID, exact key, action and bounded diagnostic/outcome. A `ManageForward` worker command always captures the assigned local owner, even when a remote Screen is selected. Source events never gain local administration authority.

Start includes optional exact local port and optional Remember intent. A successful Start followed by failed Save is a partial outcome: the verified live mapping remains visible and the error identifies only the unsaved preference. Save without starting and label edits are metadata operations; omitted labels preserve and explicit empty labels clear. Stop and Forget retain the server's distinct semantics. An alternate local port changes a saved rule only after the server confirms success. Validate target, ports and label before effects and validate response identity before publication.

Lost, mismatched or uncertain replies never restart the mutation, including after stream reopening. Reconcile through passive reads while keeping the original request uncertain; do not turn a matching read into its acknowledgement. An admitted server operation may finish after its caller closes. Shutdown discards unsent intents, revokes browser consent and joins retained carriers; no new Start, Save or browser continuation is submitted during cleanup. Already accepted forwards remain service-owned until Stop or service exit.

### 4. A revocable browser-open continuation

Treat Open as a deliberate Start/verify action followed by a passive active-list confirmation for the exact target/remote/local mapping and `up` state (NOP-2). Construct the permitted URL locally; never dispatch an arbitrary URL returned in a mapping response. Plain Ports opening uses `http://localhost:<local>`; remote terminal links preserve their admitted HTTP(S) scheme, path, query and fragment while replacing host and port. Missing explicit ports resolve to 80/443. Reuse `WebUrl` normalization and loopback classification, including its IPv4, IPv6 and localhost variants.

A terminal gesture captures its immutable terminal/source token and normalized link when activated; it does not re-read the currently selected Screen after an asynchronous wait. A panel action captures the local owner, mapping key and request generation. Both worker and final UI dispatch revalidate the still-active open consent. Stop, a superseding mapping action, explicit cancellation, owner replacement or window close revokes it. Terminal detachment/replacement/reconnect revokes a terminal-origin continuation; changing unrelated selection does not retarget it. Closing the initiating form/Ports surface revokes its pending open continuation but does not implicitly stop an admitted forward (NOP-3). A late reply cannot dispatch after its consent was revoked or its verification observed the mapping stopped or changed. Mapping request generations are client intent generations, not server-issued reservations.

Passive confirmation and final consent checks do not reserve a listener through OS browser connection: another client can Stop or another process can rebind after the last observation. The existing API cannot make browser dispatch atomic with mapping lifetime; this change adds no lease or protocol to claim otherwise.

GPUI receives a one-shot, identified eligible-open result; the platform opener remains a small final effect. No background detection, read, restored rule or remote event can produce browser consent. Local terminal URLs keep their existing direct behavior.

### 5. Reuse the desktop contract; verify Windows/WSL behavior explicitly

The existing server verifies OpenSSH forwarding setup, responsiveness and absence of an IPv6 rival in its own network environment (`remote/src/tunnel/forward.rs`). The web forwarding controller rewrites to `localhost:<confirmed local port>` and verifies the active mapping; Electron's `frontend-electron/src/main.ts::openExternally` validates HTTP(S) and hands that URL to the Windows system browser. There is no separate cross-runtime endpoint-binding protocol to port. Reuse that contract and the native validated opener rather than introducing a new permanent Windows policy.

A WSL-side check alone does not establish how a particular Windows networking configuration resolves localhost. Add a bounded owned Windows/WSL probe (NOP-4): native Windows app, its selected WSL helper, an owned remote HTTP fixture with a distinguishable response, and competing Windows/WSL IPv4/IPv6 listeners. Check the actual URL/response and preserve the competing service; retain `local_port_conflict` and explicit alternate-port behavior. Distinguish observed success in that environment from general platform coverage. If the probe reproduces a namespace mismatch or wrong endpoint, fix the concrete adaptation or explicitly withhold that affected opening path with an explanation; never fall back to the original remote loopback URL. Missing evidence alone does not introduce a permanent disable or approval gate.

Linux acceptance can finish while this scoped Windows follow-up remains unrun, provided documentation does not claim Windows acceptance. Record the actual backend/opener environments and limitations. Do not invent a proxy, forwarding wire operation or speculative runtime identity protocol. Existing Windows/WSL reverse-sharing restrictions remain unchanged. Exact-source consent and passive mapping verification are shared native correctness requirements; cross-environment collision coverage is additional evidence, not an existing stronger protocol guarantee.

### 6. Native interaction composition

The task is to find a remote service, identify its mapping/state and either use or manage it. Extend the existing Hosts destination with a consistently aligned Ports region rather than a separate navigation system (NOP-5). Give detected, saved/paused, rebuilding and failed mappings distinct text states. Keep the primary Forward/Open/Resume action visible; a Kit menu holds Stop, Remember/label and Forget as appropriate. Explain that Forget leaves a live forward running.

A retained Kit form accepts a known or entered target, remote port, optional exact local port, opt-in Remember and optional label. Save without starting is a separate explicit action. Use shared-core validation, preserve submitted fields/errors, and identify partial outcomes. Keyboard submission, field traversal, menu access, Escape and focus return follow existing Host form conventions; leaving the surface cannot redirect or silently restart work. Keep terminal entities and their focus/history stable. Reuse theme/rem geometry and one scroll owner, with stable target/port row identity and bounded rendering. Do not recreate the web markup or popup reservation flow.

## Architecture impact

Add a `client_ports` component for `splitty_client_core::ports` in `architecture/client-core.c4`; relationships include `native_workspace -> client_ports`, `client_ports -> client_workspace` for source identity and observation readers, `client_ports -> api_stream/owned_service` for owned API jobs, `client_ports -> external_links` for URL policy, and existing core native API/remote SSH validation/transport budget dependencies actually used. Keep this direction acyclic: observation types stay under `workspace`, and that module never imports the Ports controller. The controller consumes existing workspace source identities/readers; worker commands compose the two owners.

In `architecture/gpui.c4`, extend `gpui_workspace -> client_ports` for presentation/commands and route terminal activation through workspace ownership while retaining `terminal -> external_links` for hit admission. Preserve the service/remote forwarding components and their ownership. If portable response types move under `core::native_api`, update existing server-native API relationships, not storage ownership. Update the Client connections, terminal links and Remote Screens/forwarding sections of `docs/architecture.md`, and the supported/unsupported matrix in `docs/gpui-spike.md`. No new external dependency or protocol operation is planned.

## Risks / Trade-offs

- Slow API jobs can consume finite stream capacity → four active jobs, bounded pending work, independent passive reads and joined cleanup; verify pressure with existing process budgets.
- Multi-step Remember can partially succeed → publish live and saved outcomes separately; never roll back a verified live tunnel merely because saving failed.
- Cross-window Stop can race delayed opening → reject observed stopped/changed mappings and revoked consent without reposting an uncertain request; a Stop/rebind after final verification remains an existing non-atomic browser-dispatch limitation.
- Unknown discovery can resemble an empty host → retain an explicit not-reported state and allow manual mappings without assuming availability.
- Different desktop/backend loopback namespaces → run scoped endpoint/collision probes, fix or withhold reproducibly affected paths, and report untested coverage without blocking Linux completion.

## Migration Plan

No persistent data or wire migration. Existing server/web behavior remains unchanged. Introduce the core seam and tests first, then native controls and source-qualified link routing, then owned Linux evidence. Rollback removes the new UI and client feature without deleting saved rules or changing tunnel semantics. Platform acceptance beyond the tested endpoint scope remains unclaimed; a demonstrated endpoint mismatch must be fixed or its affected path withheld.

## Provisional decisions

- **NOP-1 (2026-11-08, provisional, tbd):** independent owned API jobs, initially four active and sixteen pending, with per-mapping order and an independent passive reader.
- **NOP-2 (2026-11-08, provisional, tbd):** browser Open requires a validated opening acknowledgement plus passive confirmation of the exact up mapping; no mutation replay is used to reconcile a lost reply.
- **NOP-3 (2026-11-08, provisional, tbd):** browser consent is separately revocable; closing its initiating surface does not implicitly stop an admitted service-owned forward.
- **NOP-4 (2026-11-08, provisional, tbd):** reuse the existing desktop localhost opening contract; run bounded Windows/WSL endpoint/collision probes and fix or withhold demonstrably mismatched paths, without blanket disabling for missing evidence or adding a proxy protocol.
- **NOP-5 (2026-11-08, provisional, tbd):** Ports extends the existing Hosts destination and completes explicit outbound workflows; reverse sharing and detection-driven automation remain separate parity work.
