# Design

## Context

See [proposal.md](proposal.md) for motivation and scope. The existing local hub already owns directory merging, metadata persistence and invalidation. `splitty-server/src/hub/directory.rs` returns version, host order, targets, aliases, optional source/timestamps and cached Screens; `service/dispatch.rs` exposes it as `Operation::HostDirectory`. `client_session/handler.rs` sends `hub.changed` on the ordered local session connection. `docs/protocol.ts` defines the unchanged `HubDirectory` shape.

The native client currently owns one session stream and a separate `LocalServiceClient` for settings/profiles/templates. `ApiStream` supports both embedded Unix clients and owned helper streams. Its request receiver deliberately consumes non-reply frames, so it must never receive from the terminal session stream. `AppearanceSnapshot` provides an existing pattern for immutable, owner-bound, charged presentation.

`Directory` and `DirectoryHost` currently live in the server and use `splitty_remote::hosts::Source` and the persistent catalog's `CatalogScreen`. Client-core's server dependency is absent on Windows; importing those records would break its platform boundary. `splitty-remote` has no dependency on `splitty-core`, so moving its host storage types wholesale into core would introduce unnecessary layering changes.

## Goals / Non-Goals

**Goals:** establish one UI-free, independently owned passive hub reader, make data freshness and ownership explicit, and expose the result through the native Hosts destination without any connection authority in that view.

**Non-Goals:** generalize the local workspace into a remote workspace, implement host mutation/connection commands, change the service's directory or storage semantics, replace the web controller, or add a new generic scheduling framework. API authority and operation names stay unchanged.

## Decisions

### HD1: Share the response contract at the existing core API seam

Add `splitty_core::native_api::directory` with serializable/deserializable response records for the existing directory, host, cached Screen and source enum. Keep exact field names, optional-field omission, enum spellings and integer types. Use constructors/readers or `#[non_exhaustive]` record types according to the coding guide. Unknown additive response fields may be ignored; invalid required fields and unsupported versions fail client admission.

Have `hub/directory.rs` produce these shared records. Keep its merge, ordering and hidden-host policy unchanged. Map the remote host source enum and persisted `CatalogScreen` explicitly at this boundary; do not move either persistence type, alter strict persistence decoding, or add a remote-to-core dependency. The small conversion expresses storage-to-response ownership; it is not a second wire schema. Preserve existing server type paths with re-exports if callers need them.

Client-core decodes the shared response into a charged snapshot and validates presentation admission. There is no client-side remerge, hostname canonicalization, or second wire DTO. Existing real-service fixtures plus exact JSON assertions prove that the extracted producer contract matches both the native reader and browser response.

Alternatives: a private client response DTO avoids server edits but repeats the wire schema; direct server imports violate the Windows boundary; moving all host storage types enlarges the scope and dependency graph. A small API response extraction is the chosen compromise (provisional, tbd).

### HD2: Dedicated local hub stream and a narrow client seam

Add a `hub` module in client-core, publicly exposing `HubClient`, immutable `HostDirectorySnapshot`, and typed presentation/update state with private fields and readers. `HubClient` accepts only a locally composed `ApiStream` and the owning session connection UUID; its public operation is passive directory refresh. It does not expose arbitrary API requests, host selection, SSH targets for execution, or any mutation.

Add an internal API-stream opening seam to `EmbeddedService` and `OwnedService`, reused where practical without changing settings semantics. Embedded composition uses `Connector::embedded(..., Role::Main)`; owned composition uses `Owner::open_context(None, false, ...)`. Never use `Connector::host`, `Some(target)`, HTTP, or filesystem host discovery from the client. The extra stream is admitted through existing quotas and owns its cancellation scope.

Keep settings and hub schedulers independent. Extending `LocalServiceClient` with all future hub operations would mix unrelated state and could eventually block settings behind 180-second forwarding calls. One dedicated hub read stream with one retained pending request is sufficient; no unbounded channels or per-host tasks.

Typed decode may use `api_stream` helpers or a small neutral decoder extracted from `local_service::decode_with_budget`. Avoid making hub depend on the settings feature only for JSON decoding. Preserve existing callers and resource accounting if that helper moves.

### HD3: Coalesced reads, two identities and explicit availability

The runtime starts directory setup/read only after the local `mux.welcome` advertises `hub-host-decisions-v1`. Initial readiness, accepted `hub.changed`, and an explicit directory Refresh/Retry mark the reader dirty. The experimental client reports unavailable for an older service without this capability; it does not reconstruct the directory from legacy endpoints or poll a supporting hub (provisional, tbd).

Keep a monotonic refresh generation separate from the owning session UUID and from the API stream's own transport/request IDs. At most one setup/read future is retained across cancelled `select!` branches. Each invalidation advances the generation and sets one dirty flag. A completed obsolete generation is discarded; the next read observes the newest generation. Failed setup/read does not busy-loop: it settles into an error and waits for a new invalidation or explicit retry. A broken API stream is finished before a replacement is opened. A healthy stream may be reused after a rejected response.

The presentation state distinguishes unavailable, first load, first failure, valid empty/nonempty snapshot, refreshing with a last-good snapshot, and failed refresh with a last-good snapshot. A read generation is not a server revision: it identifies accepted client refresh work only. Errors use bounded, locally classified text and status; raw reply bodies are not retained as diagnostics.

The native worker receives `hub.changed` from its sole ordered session reader, independently of whatever welcome/events the auxiliary API receiver consumes. `Update` carries the owner UUID and generation with directory state. The GPUI owner rejects an update from a different current session or older accepted generation. Replacing the session owner clears the old directory and cancels its reader; old data must not masquerade as the new service's directory. This change adds these checks and test seams without implementing general service reconnect.

### HD4: Charge data through presentation and join cleanup

Bound a directory body by the existing 1 MiB native API response limit before typed decoding. Retain the wire reply while reserving typed decode storage with `codec::reserve_json_typed`; retain `Charged<Directory>` inside an immutable snapshot. Share snapshots with `Arc`, not copied vectors. Validation requires version 1, exact valid server target identities, unique host targets, valid source values, unique per-host Screen IDs, and existing host-order/catalog field bounds. Enforce the catalog's 200 Screens per host and 256-character ID/200-character name limits; validate unsigned timestamps/counts without lossy casts. The byte ceiling, existing catalog bounds and lease budget limit the aggregate; do not mistakenly cap the merged directory at the catalog's 20 hosts, since SSH configuration and forwarding-only hosts also contribute.

Reserve scratch for any retained derived data and validation collections before allocation, or use bounded borrowed validation where practical. Render strings as plain text, with control/bidi handling at the presentation boundary; retain raw target identity separately from any display normalization. Do not silently truncate invalid responses into apparently complete success.

One last-good snapshot and one incoming replacement may coexist; both retain charges until their final consumers drop them. Saturation reports an admission/read error and preserves the previous snapshot. Updating a view releases its old `Arc`; hiding Hosts does not accumulate history. A latest-state/coalescing delivery slot is preferred for hub presentation so a stalled UI cannot queue repeated directory copies; it remains separate from ordered terminal output delivery.

`finish` cancels retained setup/read work, awaits its completion and finishes the owned API stream. Native worker shutdown includes this result in cleanup completion alongside existing service/settings cleanup. Closing the Hosts view alone releases view state, not the local service; closing the workspace prevents late result publication. Cancellation must not abandon setup futures or clear cleanup evidence merely because a receive branch was cancelled.

### HD5: Passive Hosts presentation integrated after local chrome

The user task is inspecting known hosts and their last observed Screens. The `native-workspace-chrome` change owns the surrounding navigation and drawer geometry. This change adds a real Hosts navigation entry together with its content through a focused `workspace/hosts.rs` view seam; the UI owns focus/scroll state only and consumes client-core presentation. Core implementation and tests can proceed before chrome integration. No placeholder Hosts action is added to the chrome change.

Show alias with the exact SSH target still discoverable, cached Screen names and pane counts, and last-seen information when supplied. A host with no observation is distinct from an observed empty Screen list. Neither a cached directory nor a successful local API request means a host is online/offline or has an active connection. Do not manufacture that state or show disabled remote action placeholders. Cached rows are informational; there is no host expansion gesture that pretends to implement the existing live-browse contract.

Provide one explicit passive Refresh/Retry path with keyboard access and visible pending/failure feedback. Preserve local terminal and Screen state when navigating to/from Hosts. Use existing native theme/zoom tokens, stable target and `(target, screen ID)` identities, a single list scroll owner and bounded/virtualized row rendering for large directories. No view-side reads, parsing or mutations in `render`; status selection comes from the client-core state. The integration follows the GPUI Kit design and coding guides, with actual-window inspection after the chrome seam settles.

### HD6: Preserve the next-stage boundary

Future native remote Screens require a source-keyed connection registry, explicit opened-screen references, generation-bound commands and source-qualified pane/view IDs. `LocalWorkspace::apply_catalog_inner` currently opens/resyncs every catalog Screen and local startup creates a default Screen; neither policy may be applied to a remote directory. Reuse canonical `ScreenModel`/mux projection later while separating available catalog from opened references. A directory row in this increment never becomes a workspace model or an attach route.

Future forwarding remains hub-owned; client-core will own current-intent reconciliation and browser-opening continuation. Browser popup workarounds and dockview synchronization have no place in this increment. The web client continues using the same server API; it does not execute the new Rust client orchestration.

## Architecture impact

- `architecture/core.c4`: extend the existing `splitty.core.native_api` component description with the shared directory response contract; its nested module does not create a separate top-level component.
- `architecture/server.c4`: add `splitty.server.hub -> splitty.core.native_api` for response projection; keep existing hub-to-remote metadata and service relationships.
- `architecture/client-core.c4`: add `splitty.client_core.hub` for passive refresh, admission and presentation. Add edges from `client_root`, `embedded`, `owned_service` and `native_workspace` to it as used. Hub depends on `api_stream`, core `native_api`, remote `ssh` for validation, and transport budget/codec/wire/queue facilities actually referenced. If bounded decode moves, record the changed edges for settings/appearance/profiles/templates in the same implementation change. Tests' direct dependencies remain modeled.
- `architecture/gpui.c4`: add `gpui_workspace -> splitty.client_core.hub`; nested `workspace/hosts.rs` remains within the existing workspace component. Preserve the canonical workspace layout dependency and add no DockArea/layout authority.
- `docs/architecture.md`: update Client connections/native workspace composition and Hub-owned host metadata and passive updates to describe the third, API-only stream, owner/generation checks, shared response type, passive view, bounds and cleanup. Verify `docs/protocol.ts` stays identical to the extracted response shape; no wire-format extension is intended.

## Risks / Trade-offs

- Continuous invalidations can supersede reads → coalesce to one pending refresh, retain last-good data, and prove the final quiescent state is delivered.
- A large but valid directory can exceed native admission → explicit recoverable failure, no truncated success; preserve the last-good snapshot and test budget saturation.
- Schema extraction can accidentally change optional fields or strict storage decoding → exact response JSON fixtures and unchanged persistence types/readers.
- Shared auxiliary receiver can consume session traffic → physically separate streams and an in-process interleaving test, not an assumption based on operation names.
- Capability-gated availability omits old-hub support → explicit unavailable state, leaving legacy web fallback intact; any future native fallback is a separate scoped decision.
- Presentation and core may land at different times → complete core tasks independently, integrate only after the chrome Hosts destination exists, and do not mark the change complete before that integration.

## Migration Plan

No data migration, listener change, protocol bump or installation is needed. Land the shared response extraction with serialization conformance, then the dedicated client reader/runtime integration, then the Hosts content. The feature may be rolled back by removing the native reader/view; existing host metadata and browser behavior remain valid. Use the host platform for checks; no Windows build or release work is part of this change.

## Provisional decisions

- **HD1 — 2026-11-07 (provisional, tbd):** share the directory response under core `native_api::directory`, converting server storage/source records at the boundary without moving persistence types.
- **HD3 — 2026-11-07 (provisional, tbd):** the experimental native passive directory requires `hub-host-decisions-v1`; older local services report unavailable rather than gaining a second legacy merge/polling implementation.

Record these decisions in `openspec/open-decisions.md` alongside this change's implementation record.
