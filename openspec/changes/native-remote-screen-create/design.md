# Design

## Context

See [proposal.md](proposal.md) for motivation and scope. The native workspace already owns one `WindowWorkspace`, one `SourceWorkspace` per source, a common `SourceRuntime`, and the same native `Client` over embedded and owned service adapters. Host groups are explicit opened references; the remote live catalog is metadata, not an instruction to open all Screens.

Relevant current contracts:

| Existing seam | Reuse and required extension |
| --- | --- |
| `connection.rs::Client::create_screen_with_ids` | Already sends one `mux.op/create_screen` with caller-reserved IDs and no profile. Permit omitted default name through the same serializer; preserve existing named callers. |
| `workspace/catalog.rs::create_screen` | Already reserves model/pane capacity and tracks mutation generation before publishing a successful enqueue. Its `require_local` and local catalog policy cannot simply be removed. |
| `WindowWorkspace::source_mut` / `SourceEdit` | Supplies the remaining aggregate Screen/pane allowance and reconciles global selection. Creation must go through this owner. |
| `WorkspaceRefresh` | Collects opening catalog, explicitly opened Screen snapshots and a trailing catalog fence, with one bounded recollection and the existing observation deadline. Reuse its current-connection generation and atomic candidate commit. |
| `apply_remote_catalog` | Updates only opened references, has no pending-creation settlement and currently ignores the local `pending_screens` watermark logic. Add explicit remote creation handling without importing unrelated live entries. |
| `native_workspace::local_command` / `Remotes::command` | Existing `CreateScreen(String)` is local-only. Add an explicit remote command, rather than changing the meaning of the local shortcut. |
| `SourceRuntime::select_created` / `remote::merge_scoped` | Existing local creation can follow canonical membership; remote merging presently drops `select_created`. Carry a valid follow through the common effects path, with one owner for its revocation. |
| `HostGroupTarget` / `HostActionTarget` | GPUI already captures logical group identity; the Core's host action captures local owner, exact target, source and attempt. Creation additionally requires the current execution token, unlike port opening. |

The server reducer already assigns the first unused `Screen N` when `name` is absent and uses its own default profile when `shellProfileId` is absent. It generates Session/layout IDs and rejects a nonempty existing Screen ID. No new mux operation, acknowledgement protocol or server version inference is needed. `docs/protocol.ts::CreateScreenOp` still incorrectly spells `newPanelId` and lists obsolete `ownership`; correct that touched declaration against `mux/ops.rs` while documenting existing optional-name semantics.

## Goals / Non-Goals

**Goals:** extend the existing UI-free collection with bounded creation intent and observation; preserve canonical authority and aggregate capacity before effects; expose a useful host-group action with honest progress and recovery.

**Non-Goals:** see the proposal. In particular, no speculative local terminal layout, queued offline creation, generic operation framework, new stream, per-host API scheduler, or compensating remote close. Existing local creation and initial local Screen bootstrap keep their behavior.

## Decisions

### 1. Admission is source-exact and UI-free

Use a separate `CommandKind::CreateRemoteScreen { request: RemoteScreenCreateRequest }` in the fixed local-owner orchestration envelope. The immutable request privately captures the existing `HostActionTarget`, an explicitly supplied remote execution `SourceToken`, a request ID and shared revocable follow permission. Preparing a request validates the captured execution token rather than acquiring a newer one for an old callback. The request ID correlates admission failure/pending presentation and suppresses repeated delivery only while that request is the retained pending or latest outcome record; it is not a new wire receipt or permanent deduplication history. Once a later distinct request replaces that bounded record, no eternal request-ID guarantee is claimed. The current UI/worker/transport path must not replay old commands. Reuse the host action's owner/target/source/attempt rather than copying those fields into a second host identity type.

A narrow `WindowWorkspace` validation/capture reader supplies availability to the UI and is reused at worker admission. It verifies:

- the fixed local lifecycle owner and current host action identity;
- the exact current remote execution token, existing opened group and non-closing source;
- a supported, initialized, successfully settled live catalog and no active ownership-reconciliation/close operation;
- no unresolved creation for this host, remaining aggregate Screen/pane capacity and room for one more entry within the accepted live-catalog bound.

A ready source can be targeted while any other source is selected. Do not apply the shown-pane action gate. The outer envelope admits local navigation orchestration; the worker routes the mutation through the exact captured remote execution admission in the existing SourceRuntime. After accepting the current local-owner Screens navigation it withdraws pending browsing/connect continuations, even if subsequent remote mutation admission fails. GPUI enters Screens with a pure presentation helper only after successful enqueue; it must not enqueue a separate ReleaseHostBrowsing operation whose failure could split creation from navigation. No Connect, Retry or capability fallback is issued. Creation uses the existing required `workspace-catalog-v1`; unrelated capabilities such as Screen placement are not prerequisites.

The worker remains the owner of pending/outcome data. Expose immutable readers and a small source/request-qualified admission/outcome update if required to release the UI's enqueue latch; use the existing publication channel. Keep one bounded pending-or-latest outcome record per host, with no unbounded request history. The request ID is generated for deliberate activation, not during render. Ordinary distinct later activations are new requests, not retries.

Alternative rejected: teach the existing local `CreateScreen` to infer its target from selection. That would change the local shortcut and make stale UI captures ambiguous.

### 2. Reserve once before sending and retain an explicit reference

Extend the existing creation path narrowly with an explicit remote mode. Allocate fresh Screen/Pane IDs once, validate the client identity, check the aggregate allowance via `SourceEdit`, and reserve one Screen plus the initial pending pane before enqueueing `create_screen`. A failed enqueue rolls back the record, model reservation and reference atomically.

After successful enqueue, add exactly that requested Screen as an opened reference at the end of its host group. It is pending observation, with no fabricated name, layout, session or executable route. Keep a bounded record containing request ID, stable source, submitted connection, Screen/Pane IDs, mutation watermark, observation status and selection-follow intent. A small extension of the existing pending-screen ownership is preferred to a second creation collection. These are client intent fields, not a replacement canonical Screen model.

The reference itself supplies connection demand. Detaching the group's old Screen while the new reference is pending does not silently discard the new request. The pending pane is counted exactly once: use its existing model reservation when present, otherwise account for the retained unresolved record. Once a complete canonical candidate proves the requested Screen's current presence or absence, convert or release the reservation inside that candidate before checking aggregate capacity. Its initial pane may have been replaced or removed. Candidate failure must leave the current model's reservation and shared follow permission intact. All candidate adoption still checks the window-wide allowance before replacing routes.

Alternative rejected: wait for a catalog to discover a new ID without reserving it. The remote policy intentionally ignores unopened entries, and this would permit a mutation before the window has capacity to represent it.

### 3. Reconcile with the existing ordered collector

Creation marks the source dirty and requests the existing coherent refresh; it does not implement a UI resync loop. Pass the consumed catalog watermark to remote catalog settlement, which currently discards it at the remote branch. A collection started before the creation mutation cannot settle that record or delete its pending model/reference. Keep the newer dirty intent so a post-submission collection follows after the old pass drains.

The collector requests a snapshot only when the live opening catalog contains an explicitly opened reference, including the reserved new ID. Commit a complete candidate containing the ending catalog and required snapshots atomically. A standalone snapshot/spawn/nudge is not enough to settle creation; it can trigger or contribute to the bounded normal observation. A requested ID not present in a conclusive post-submission catalog becomes a missing reference without recreation. A present ID with a missing or invalid snapshot remains unresolved and reports the existing bounded observation failure.

Settle creation after the coherent candidate, not prematurely inside catalog parsing before snapshots have passed validation. Preserve ordinary remote semantics: unrelated catalog insertion, active hints and external moves never add references or select a Screen. Do not assume the first pane is unchanged by confirmation time; canonical state may already include subsequent host-side edits. Display that state without asserting a fabricated initial session.

The existing parsed-op failure carries `op` and the reserved `newPaneId` as `paneId`; the outer `screenId` on this workspace-level request can be empty. Correlate a creation failure only on the current source connection, `op == create_screen` and its exact reserved initial Pane ID; do not depend on the empty outer Screen ID or parse error text. An uncorrelated `mux.op_failed` must not be assigned to this request by guessing; refresh and report uncertainty instead. An effect/spawn failure may leave a canonical Screen, so a failure notice does not itself delete its reference or bypass observation/capacity settlement. No success acknowledgement is invented: the useful confirmed result is the canonical Screen currently observed.

### 4. Uncertainty survives reset as observation intent, not execution authority

`reset_connection` currently discards local pending operations. For a submitted remote creation it must retain the explicit reference and minimal unresolved record while clearing old routes, attachment readiness, mutation-send authority, old observation watermarks and automatic selection follow. Keep no `Client`, `Event`, transport delivery lease or pending send across reset. Rebind observation only after the same stable source/target has a fresh authorized connection; a replacement local owner or disposed source cannot inherit it.

| Event | Reference / reservation / outcome |
| --- | --- |
| Enqueue refused | Remove the unsent record and tentative reference; restore capacity; report not sent. |
| Current complete observation contains requested ID | Adopt its actual canonical state, convert pending capacity, release per-host creation admission, and follow only if still authorized. |
| Current post-submission observation confirms absence | Keep a removable missing reference; release its unused pane reservation and unresolved creation slot. Same-connection evidence can report no current created Screen; do not invent an acknowledgement. |
| Disconnect, failed/invalid observation or deadline | Retain the explicit reference, unresolved reservation and per-host exclusion; mark uncertain. Normal bounded recovery/passive refresh may continue, but never send Create again. |
| Fresh connection confirms presence | Adopt that exact ID from its current coherent observations, without claiming a recovered acknowledgement or restoring old follow. |
| Fresh connection confirms absence | Mark currently missing and release unused local pane capacity/per-host exclusion; retain the fact that the old request's historical completion was unknown. Absence does not prove nonexecution. |
| Explicit detach | Remove the reference and unresolved record, revoke follow, withdraw its client claims and let existing demand/cleanup rules release local resources. Never send close_screen as rollback. |
| Source disposal / accepted window close | Revoke all creation UI authority and follow; join existing stream cleanup. Host sessions remain owned by the host. |

The opened missing reference continues to consume its normal Screen-reference slot until removal; releasing its unused pane reservation is not permission to attach late data without admission. If a later coherent observation finds the ID, the normal candidate checks must reacquire capacity before establishing routes. Under pressure, keep an honest unresolved/unavailable reference and preserve other routes. No eviction or partial attachment is allowed.

A failed observation cannot free reservations merely to enable another create. Explicit removal provides an escape from a permanently unreachable host; it must explain that remote creation may already have occurred. A subsequent deliberate creation uses new IDs and current admission, never resends the old operation. No persistence across application restart is introduced.

### 5. Follow is revocable navigation intent

Default behavior selects the newly confirmed Screen through the existing global selection/visibility/attachment path. The intent is captured at creation and canceled by any later explicit Screen selection, another creation's newer navigation intent, leaving the Screens destination, disconnect, detach or accepted close. Preserve one authoritative follow decision in the Core; the source runtime may carry delivery bookkeeping but cannot recreate a canceled model intent.

Reuse existing cancellation on `select_window_screen` / `cancel_created_follow`; extend it to the new record. A cloneable request shares only a revocable atomic navigation permission: GPUI can cancel that exact request synchronously on destination departure even when the worker queue is full. No Drop implementation cancels another clone, and the handle performs no domain mutation, connection or remote cancellation. `remote::merge_scoped` carries the captured request through effects; final publication revalidates owner, connection and current request, then consumes permission immediately before global selection with no intervening await. Preparing an effect does not consume permission. Newer local or remote creation supersedes older follows globally.

If navigation cancels follow, creation still completes and the reference remains available. GPUI must not focus a newly rendered terminal independently of the worker's selected identity.

### 6. Default names and launch settings stay server-owned

Add an optional-name route through the existing `create_screen_with_ids` serializer, keeping the named public call compatible. For the new default remote action omit `name` entirely rather than send an empty string. Also omit profile/CWD/command/ownership fields. The server atomically chooses the smallest unused Screen N over its full current workspace, including unopened Screens and concurrent creations, and resolves its own launch defaults.

This is a narrowly native clarification of the existing Screen Default Names Per Host contract. Preserve its full existing scenarios and all Web rules in the delta. There is no client-side name reservation or naming-policy copy.

### 7. One host-group command surface

Expose a quiet, keyboard-reachable Kit Button/Action for New Screen on the existing Drawer host header, with an accessible host-qualified name. Keep the host caption's navigation and the Ports dropdown's meaning unchanged; do not hide Screen creation inside a Ports-only menu. The command projects shared availability and pending state, captures exact authority on deliberate activation, and uses one submission method for pointer/keyboard paths.

On successful UI enqueue, entering the Screens destination is part of this deliberate creation action; a failed enqueue preserves the previous destination. Later destination changes cancel follow. Do not navigate again when an asynchronous admission/outcome publication arrives. Use the existing pending Screen/reference presentation, host feedback and focus path. Unknown name is a temporary state label, not a guessed Screen N. Explain offline/reconciling/capacity/uncertain states with actionable bounded feedback; no extra confirmation is required for a normal additive Screen creation. Preserve Kit focus, disabled behavior, Drawer autohide and compact rem-based geometry. No new painter, modal flow or permanent status bar is needed.

### 8. Finite Linux platform event dispatch

Actual-window confirmation exposed sustained native-event starvation in the pinned Linux platform: the GUI kept accepting keyboard callbacks while its foreground workspace poll did not run for 1.995 seconds. The worker then waited 1.073 seconds on its full bounded update channel and the command channel rejected a suffix of the input. Once polling resumed, 64 updates took 755 microseconds. The trace contained neither a slow collector/reconciliation nor a poll-task exit. Creation/transport logic is not an appropriate owner for this scheduler problem.

Keep the current bounded command/update channels, input admission and exact-source ordering. Patch only the published `gpui-pre-linux` 0.3.11 dependency with finite native event batches and at most one explicitly scheduled continuation for buffered XCB events. Both the native source callback and the platform's after-runnable path must use that same bound. Preserve native event order, XIM handling, release/press key-repeat adjacency across batch boundaries, Expose processing and eventual delivery when XCB has buffered events but its fd no longer signals readiness. Do not replace the issue with larger queues, input replay, slower test typing or an application callback that reenters workspace polling.

Use an explicit Cargo path patch with vendored published source, version/checksum provenance, license and a small documented patch; never modify an external Cargo registry cache as a hidden build prerequisite. Treat this as an external dependency rather than a new Splitty domain crate. Remove the patch only after an equivalent upstream version passes the same regressions. Pure batch/continuation tests establish event preservation and finite scheduling; the existing real-window fixture establishes actual keyboard/worker progress. Temporary payload-free timing instrumentation is private diagnostic evidence, not a new production API.

## Architecture impact

Update descriptions/relationships of `splitty.client_core.connection`, `client_workspace` and `native_workspace` in `architecture/client-core.c4`, and the existing GPUI main/workspace components and relationships in `architecture/gpui.c4`. The connection remains a bounded serializer; client_workspace owns reservation, captured admission and canonical reconciliation; native_workspace owns service polling and effect delivery; GPUI owns only interaction and projection. Existing dependency directions suffice; no new Splitty crate or top-level module is planned. Document the pinned external Linux platform patch and its finite dispatch responsibility in the GPUI composition-root description, `docs/architecture.md` and build documentation; exclude vendored third-party source from workspace membership rather than assigning it application ownership.

Update `docs/architecture.md` in the native client/Remote Screens ownership account to describe pending references, observation across reconnect, capacity release and follow cancellation. Correct only the touched CreateScreenOp declaration/documentation in `docs/protocol.ts`. No transport/server policy changes are planned. Extend existing fixtures, not a public fake-peer API or new process harness.

The active `native-remote-screens` plan still contains explicit exclusions for new remote Screens and an "existing references only" rule. This successor supersedes only those exclusions for the narrow workflow here. During integration, reconcile those prerequisite artifact sentences (and the unsupported-action scenario) to refer to this explicit exception, retaining missing/empty recreation refusal and every unfinished runtime/test obligation. Do not archive or mark that broader change complete, and do not overwrite unrelated newer outbound/link support when coordinating its deltas.

## Risks / Trade-offs

- **No mutation acknowledgement or durable receipt:** current presence/absence cannot reconstruct an unknown historical outcome. Keep request history uncertainty distinct from effective canonical state; do not add a protocol just to claim certainty.
- **Lost reservation during connection reset:** preserve the minimal unresolved record and test admission across reset, rejection and late candidate adoption, not only counters on the original source.
- **Old observation generation consumes a newer create:** explicitly settle only post-submission coherent observations and test collector draining/dirty follow-up.
- **Follow arrives after user navigation:** carry request-qualified cancellation through the Core and test selected identity plus actual terminal focus.
- **Another client edits/removes the Screen immediately:** use its canonical current state; never recreate or infer that the original first pane still exists.
- **Compact host header becomes crowded:** reuse existing Kit control sizing, preserve truncation and focus-ring space, and verify actual bounds/input in the existing production-view fixture and Linux window.
- **Sustained native events starve foreground work:** finite platform batches must reschedule buffered events without losing input or repeat/IME state; prove continuation ownership and actual two-source input progress under the existing queue limits.

## Provisional decisions

- **RSC-1 (2026-11-08, provisional, tbd):** first native creation entry is within an already opened ready host group, with one unresolved creation per host and no connection/retry side effect.
- **RSC-2 (2026-11-08, provisional, tbd):** an unresolved submitted reference and its unused pane reservation survive ordinary connection replacement; current conclusive absence can release local reservation without claiming historical nonexecution. Explicit detach releases only local interest.
- **RSC-3 (2026-11-08, provisional, tbd):** delegate native default remote names to the existing server operation, omitting name and adopting the canonical result; other clients retain their current policy.
- **RSC-4 (2026-11-08, provisional, tbd):** confirmation follows once unless later navigation, destination departure or disconnect revokes it; later recovery never restores old focus intent.
- **RSC-5 (2026-11-08, provisional, tbd):** restore foreground fairness at the pinned external Linux event-loop boundary with explicit vendored source/provenance and bounded continuations; retain application queue limits and ordering rather than adding replay or a second ingress queue.

## Migration Plan

No saved format or server rollout is needed. Implement and test the Core seam first, then wire the GPUI action against it. Existing hosts already accepted for native catalog interaction use the existing create operation. Rollback removes the action/Core extension without deleting host-created Screens; they remain discoverable through existing open/browse behavior. Full workspace checks remain the deferred pre-merge checkpoint, separate from focused implementation checks and actual-window acceptance.
