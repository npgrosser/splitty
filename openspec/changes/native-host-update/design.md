# Design

## Context

See [proposal.md](proposal.md) for motivation and scope. The existing `Hub::reach` installs and discovers through `splitty-remote`, evaluates `update_plan` or `migration_offer`, conditionally stops/migrates, and verifies the replacement before bridging. Ordinary updates require an older parseable version, session keepers, verified instance identity and the matching prepared replacement. Legacy migration additionally requires host-owned proof. The native owner and embedded connector already carry `update: bool`.

The missing path is in the native client: `OwnedService::host_stream` and `EmbeddedService::host_stream` pass false; `remote.rs::observe_prelude` discards ready metadata; `workspace/window/diagnostic.rs` discards structured update/migration fields. `HostActionTarget` already captures the fixed local owner, target, source and attempt. The remote worker already retains setup and close futures, rejects stale actions and preserves explicit remote Screen references across recovery.

The existing web state machine is evidence for interaction scope, not a new dependency. Its destructive-update warning branches do not reflect the current keeper-only update policy and must not be copied.

## Goals / Non-Goals

**Goals:** one UI-free native update workflow over existing concrete adapters; bounded semantic presentation; explicit confirmation and at-most-once attempt admission; unchanged backend authority and cleanup ownership on Unix and Windows/WSL.

**Non-Goals:** a generic remote service facade, duplicated installer/version policy, exact-instance consent receipts, web/WASM migration, or completion of `native-remote-screens` acceptance. No new wire fields or maintenance operation are expected.

## Decisions

### NHU-1: Preserve host-scoped update intent

Confirmation means update the identified host to the assigned backend's version while preserving keeper sessions. It does not promise to mutate exactly the initially observed daemon instance. A fresh backend handshake and the existing conditional command remain authoritative. Already-current daemons connect without a stop; newly ineligible daemons are refused. Equal-version builds with different workspace capabilities are not eligible under the existing strict version-precedence policy; retain the incompatibility explanation and manual backend remedy instead of manufacturing native eligibility.

A receipt binding confirmation to one remote observation would enforce a stronger contract than the existing Host Update requirement. It is not necessary for the accepted host-scoped effect and is excluded. The UI must not claim a remote process is reserved while its dialog is open.

### NHU-2: Retain semantic offers in the existing client model

Extend `workspace/window/host.rs` and the bounded diagnostic decoder. Expose a private-field immutable offer with readers for observed version, replacement version, ordinary versus legacy kind, keeper preservation and optional legacy observed session count. Ready offers come only from the matching target's validated prelude; failed-connection offers come only from typed preparation `Closed` metadata. Preserve an accepted `remote.ready` offer when the following `mux.welcome` lacks `workspace-catalog-v1`: the attempt must fail safely but retain its update remedy after cleanup. Keep the outer envelope's retryability authoritative.

Use the existing charged JSON decoding/retention pattern. Version fields are nonempty bounded strings (64 bytes); legacy session counts must fit the backend's unsigned count; contradictory, absent or malformed eligibility never becomes an actionable offer. Unknown peer fields remain forward-compatible. No process proof, PID, executable selector or remote command reaches this presentation.

An ordinary offer needs the local backend's version, absent from `remote.ready.update`. Add one coalesced owner-bound `build.get` read to the existing local auxiliary API owner in `local_service.rs`, exposing only bounded build-version presentation to the native worker. It neither contacts hosts nor consumes terminal events. Missing/failed build information withholds ordinary confirmation with a retryable read action; it does not guess the GPUI executable's version. Owner replacement clears the value; late replies cannot populate another owner. Legacy offers already carry both versions and do not require inventing a second version comparison.

This small offer/plan model is the portable sharing seam: semantic facts and validation, without GPUI types, UI sentences, SSH orchestration or SemVer policy. Keep it within the existing workspace capability rather than create a crate or broad facade.

### NHU-3: Capture and revalidate confirmation in the worker

Add explicit request, confirm and dismiss commands beside existing host commands in `native_workspace.rs`. Request carries the captured `HostActionTarget` and a request identity; the worker derives a plan from current accepted facts. The plan captures local `SourceToken`, exact host/source/attempt, offer kind and versions, keeper behavior and the legacy observed count. Retain at most one pending confirmation per window; replacement or dismissal revokes the previous request. Freeze the readers and command/update signatures before GPUI integration.

Confirm checks both the retained request and current accepted facts before consuming it. Duplicate, fabricated, stale or already-consumed plans cannot start an update. Invalidations include local owner replacement/shutdown, source disposal or attempt replacement, loss of a previously usable connection, offer replacement/ineligibility, changed confirmed facts and explicit dismissal. A structured failed-attempt migration/update offer, including an accepted ready offer followed by workspace capability rejection, remains valid after that attempt's cleanup; the expected failure that delivered it is not a second invalidation. Rename, pin/order and unrelated catalog changes do not revoke an otherwise unchanged plan. Metadata removal alone preserves a still-owned source, so it does not invent a new source lifecycle rule.

Leaving the initiating confirmation surface revokes unconfirmed intent; a delayed plan delivery must not reopen it. Moving selection to another Screen does not retarget the captured action. Revalidate at worker admission even if GPUI has already hidden a stale dialog.

### NHU-4: Retain connection replacement and consume intent once

Extend the existing `remote.rs` host lifecycle rather than replace its ownership model. On accepted confirmation, retain one consumed update continuation for that exact source, revoke input/attachment authority for the old connection, cancel normal recovery, and await its retained close/setup cleanup. Keep explicit Screen references and browse demand. Only after observed cleanup may the worker advance the attempt and create the update-enabled setup through `OwnedService`/`EmbeddedService`.

Recheck local owner, source lifetime, demand and cancellation before dispatching that continuation. Normal Connect, Retry and every recovery path construct connect-only setups. The update flag is consumed when the single update setup is dispatched, never stored as a persistent host preference. A second confirmation cannot queue another mutation. If cleanup fails or no current demand remains, abandon the continuation without opening a replacement.

For an offline host whose structured failure offers migration, prior cleanup is already settled and the same path can advance directly. Fresh readiness uses the existing target/welcome checks, then reobserves the retained explicit references through `SourceRuntime`; it neither creates missing Screens nor replays input or layout mutations. Local and unrelated host traffic continue while cleanup or setup waits.

### NHU-5: Cancellation and failure describe what is known

Before update setup dispatch, cancellation withdraws the continuation and no update-enabled connection starts. After dispatch, cancellation closes the existing context but retains setup, a late returned Client and cleanup until joined. Cancellation cannot undo remote maintenance already started.

The current wire does not identify an exact remote mutation commit point. Conservatively describe unsuccessful or cancelled dispatched updates as unconfirmed, alongside the typed failure; never infer that a cancelled connection rolled the daemon back. Disable automatic recovery for that update attempt. Explicit ordinary Retry may observe the effective version and reconnect, but must not send update intent or claim a previously lost update succeeded. A later fresh eligible offer requires a new confirmation. Successful completion requires the fresh ready prelude and accepted workspace readiness, not merely progress or adapter assignment.

### NHU-6: GPUI owns wording and transient controls

Extend the existing Hosts presentation with an explicit Update entry for an actionable offer and a focused confirmation. Render the captured target even when an alias exists, both versions, keeper-session preservation and temporary interruption. For legacy migration, identify the count as observed and explain that the backend verifies again; do not promise an immutable session count. Blocked offers retain the existing actionable diagnostic.

Use the established GPUI Kit confirmation components, semantic theme/rem tokens and current workspace overlay ownership. Pointer and keyboard reach the same command; Escape/Cancel dismiss the pending plan and restore focus. Loading prevents duplicate submission. Update is not styled or described as session termination. Keep all backend decisions and mutation admission out of view callbacks and render code.

## Architecture impact

- `architecture/client-core.c4`: extend `client_workspace` for bounded host offers/plans, `native_workspace` for confirmation and retained update replacement, `local_service` for owner-bound backend-version reading, and `owned_service`/`embedded` for the existing explicit host-update parameter. Preserve their existing relationships through `api_stream`, `splitty.server.native` and `splitty.remote.owner`; update descriptions and any actual test/import edge together.
- `architecture/gpui.c4`: extend `gpui_workspace`'s host confirmation presentation, retaining its dependency on the client workspace/worker rather than the hub or installer.
- Existing `architecture/server.c4` hub/native and `architecture/remote.c4` installer/owner components remain the authority; no production dependency or new component is planned there. Update test relationships only if actual test imports require them.
- Update `docs/architecture.md` sections **Client connections** and **Remote Screens** with offer ownership, captured confirmation, cleanup continuation and connect-only reconciliation. Audit `docs/protocol.ts` for the existing fields this path consumes; correct touched documentation discrepancies without expanding the wire.

## Risks / Trade-offs

- The host can change while confirmation is open → revalidate observed client facts and rely on fresh backend guards; state host-scoped intent honestly.
- Replacement version can differ from the UI executable, especially on WSL → read the assigned backend, never substitute compile-time UI version.
- Stop and handshake errors do not prove whether maintenance happened → preserve an unconfirmed outcome and reconcile with connect-only intent.
- Cancelling a pending future can abandon an owned process or Client → extend retained setup/close state and existing cancellation-gate tests.
- A parallel active change owns broad remote acceptance → keep this slice's evidence and remaining `native-remote-screens` obligations distinct.

## Migration Plan

No persisted state or wire migration. Implement model/read tests, freeze the command seam, integrate the retained worker path, then integrate GPUI against it. Targeted tests and isolated runtime acceptance precede completion; full CI remains the deferred merge checkpoint. Reverting the client addition restores connect-only native behavior without changing remote data or shared backend policy.

## Provisional decisions

- **NHU-2** (2026-11-08, provisional, tbd): reuse the local auxiliary API owner for one coalesced backend-version read, with ordinary confirmation unavailable until that owner supplies a valid version.
- **NHU-3** (2026-11-08, provisional, tbd): at most one pending update confirmation per window; unrelated metadata changes preserve it, source/attempt or offer changes revoke it.
- **NHU-5** (2026-11-08, provisional, tbd): after an unsuccessful dispatched update, require explicit connect-only Retry rather than automatically recovering that update attempt, because the wire lacks a mutation commit acknowledgement.
