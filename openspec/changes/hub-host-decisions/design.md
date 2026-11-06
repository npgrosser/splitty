# Design

## Context

See proposal.md. The current native transport already refuses downgrades and unsafe daemon replacement through the remote supervisor's shared update plan. The remaining defect is the window's version inequality and repeated retry classification. Host streams must stay opaque after readiness. Forward managers own asynchronous restoration and supervision, so changes must originate there rather than only in HTTP handlers.

## Goals / Non-Goals

**Goals:** one hub-owned host directory, accurate update/retry metadata, ordered catalog observations, passive event-driven refresh for local clients over both carriers.

**Non-Goals:** parsing relayed host frames, moving automatic forwarding into the hub, changing saved-forward limits, changing reconnect consent, or rebuilding the frontend client core.

## Decisions

- H1: Reuse the existing SemVer precedence comparison and replacement plan from splitty-remote. Add `update: {available, keepsSessions}` to ready and incompatible messages, and `retryable` to errors. An ordinary update is offered only for a provably older daemon with session preservation, a verified instance and the matching prepared replacement; the existing replacement checks remain authoritative. Malformed versions never authorize an update. Legacy hubs without this metadata do not get speculative ordinary update offers; positive migration offers remain supported.
- H2: `GET /api/remote/directory` and native `remote.directory.get` return known/configured/alias hosts, cached screens, and active/saved forward and reverse-forward hosts, deduplicated, hidden entries filtered, with saved group order. Reads never connect. The frontend adds only live connections/open screens and applies user ordering to those live-only additions.
- H3: On successful readiness, the hub validates and persists the handshake's screen list. Later `hub.catalog.observe` messages on the local main connection carry `{target,screens}` through the existing catalog validator. This preserves rename/count freshness without inspecting host frames. Old PUT requests remain compatible. Hidden hosts remain hidden, including late observations. Window observations are ordered by the admitted connection; persistence and failure handling belong to the hub.
- H4: A coalescing watch signal covers metadata, active/saved forwards, restoration failures and supervisor up/down changes. The local connection sends `hub.changed` after welcome, and advertises `hub-host-decisions-v1`. Clients refresh passive reads on this signal and reconnect, retaining mutation-generation guards and reconciling again after the last in-flight mutation. No event arrives on a host's relay from its intermediary. Polling remains only when an older local hub lacks the capability.
- H5: Native and browser transports share the directory operation and observation handler. Remote-window service requests and owner/grant sockets gain no hub-management operations. Observations confer only the existing catalog-metadata permission, never connection or forwarding authority.

## Risks / Trade-offs

- Coalesced events can combine several transitions: refresh reads current state and subscriptions precede initial welcome/read, so the final state is retained.
- A slow window can report stale host data: observations remain advisory cached data, ordered on each local connection; live host state always takes precedence in the view.
- Handshake catalogs may exceed cache limits: apply the existing validation, report persistence failure, and preserve the previous cache without failing an otherwise usable SSH connection.
- Legacy clients may continue PUT catalog writes and polling: retained endpoints permit incremental upgrades.

## Architecture impact

Existing components `hub`, `client_session`, `service`, `http`, `remote.tunnel` and the web frontend change; no new top-level component. The client-session component gains a relationship to hub for passive changes and validated observations. `docs/architecture.md` documents directory ownership and event-driven forwarding refresh. `docs/protocol.ts` and the native operation catalog describe additions.

## Migration Plan

Additive capability and message fields preserve old clients and hosts. New clients retain polling and legacy catalog persistence only for a local hub without the capability. New update actions require explicit positive hub metadata. Rollback keeps all on-disk formats unchanged.

## Provisional decisions

H1-H5 above are provisional, tbd (2026-11-06).
