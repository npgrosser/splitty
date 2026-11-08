# Design

## Context

See `proposal.md` for motivation, dependency and scope. `native-source-routing` supplies immutable `SourceToken`/`TerminalToken` authority and stable `ScreenKey`/`PaneKey`/`LayoutKey` namespaces. The experimental executable remains a documented subset of released clients; the existing creation, remembered-reference, forwarding and remote-only-window contracts are not removed or claimed complete by this increment.

Concrete reuse and gaps:

| Existing owner | Reusable behavior / required change |
| --- | --- |
| `workspace.rs::ScreenModel` | Canonical `ScreenState`, selected tabs/focus, projection, session bindings, pending pane admission, split/tab operations and close impact inputs. Basic split/tab already omit explicit CWD/profile and inherit on the service. |
| `workspace/catalog.rs::LocalWorkspace` | Owns all local Screen models and collection-wide validation. Today `apply_catalog_inner` creates a model for **every** catalog entry and initializes selection from `activeScreenId`; `native_workspace` additionally creates initial Screen 1. Those are local policies, not general remote behavior. |
| `workspace/catalog/refresh.rs::WorkspaceRefresh` | Coherent opening catalog → snapshots → trailing catalog collection, bounded staged bytes and route replacement. Today it resyncs **all** catalog entries. Change its requested set to the source's admitted opened references while preserving local behavior. |
| `connection.rs::Client` | Ordered charged events, exact session attachment/input/visibility, independent connection UUID, capability-gated byte input and joined backend cleanup. Reuse across sources. |
| `embedded.rs`, `owned_service.rs` | Add a narrow host-stream adapter. Unix uses its existing embedded state and `Connector::host_admitted`; owned/Windows uses the existing `Owner::open_context(Some(target), false, context, progress)`. No helper/SSH implementation in client-core. |
| `hub.rs`, `native_workspace::RuntimeHub` | Keep passive directory ownership and physically separate local API stream. Reuse lifecycle lessons: retain setup/closing futures, revoke their contexts, latch cleanup failure. Do not make this reader the remote connection owner. |
| `splitty-server/src/hub/native.rs` | Already validates/prepares the assigned target, sends native readiness followed by application `remote.ready` and `mux.welcome`, records handshake catalog and restores enabled saved forwards. Leave these effects there. |
| `splitty-transport/src/negotiation.rs`, `peer.rs` | Preparation errors retain charged `Closed` privately; `Peer` currently discards received terminal `Closed` metadata. Add narrowly scoped typed read access to preserve actual retryability/diagnostics. |
| `splitty-server/src/client_session/mux_handler.rs::handle_resync` | Missing Screen resync logs and sends no snapshot. A client must use the subsequent ordered catalog fence/deadline, not await a nonexistent missing-Screen reply forever. |
| `frontend/src/lib/connection/{registry,screen_sync}.ts`, `lib/mux/remote_screen_sync.ts`, `lib/remote/remote_socket.ts` | Reference demand ownership, local/remote policy separation and reconnect semantics. Do not port Dockview reconciliation, browser tickets, WebSocket queues or xterm reset workarounds. |

## Goals / Non-Goals

**Goals:** One tested native flow from known host to a live existing Screen; shared canonical collection and interaction code; source-isolated commands/events; bounded demand, preparation, recovery and shutdown; honest cached/live/failure states.

**Non-Goals:** No generic connection framework, abstract emulator, second layout tree or duplicated `RemoteWorkspace` algorithm. No new persistence/protocol version, local-service reconnect feature, remote-only Windows startup or frontend migration. Existing local advanced actions remain supported; remote advanced actions outside the explicit allowlist remain unavailable at both UI and worker boundaries.

## Decisions

### NR1: Share one source collection and add one window selection owner

Evolve the existing `LocalWorkspace` implementation into a common **`SourceWorkspace`** rather than building a parallel model. Its `models` map is the source's opened-Screen collection, containing the existing `ScreenModel`s. Keep canonical projection, route lookup, candidate validation, pane reservations, same-source observation and basic operations here once. Move the local catalog/bootstrap decisions into a concrete local policy; add a concrete remote policy for catalog metadata and explicitly opened references. These are two small policy variants in this model, not trait/provider implementations forwarding every method. Use nested modules under `workspace`; no new crate.

The source policy separates:

- **Local:** authoritative local catalog membership opens all local Screens, preserves initial local active hint and current Screen 1 bootstrap, and retains current local creation/template/transfer behavior.
- **Remote:** catalog entries are metadata only. `opened` contains explicit references in first-open order, with loading/ready/missing state. An open admits a reference and a model with no fabricated layout or pending creation; only its confirmed snapshot can establish routes. An empty catalog never invokes creation. A missing reference remains removable and is never recreated. Repeated Open for the same reference selects it without another model/attachment.

Remote policy-specific catalog/attempt fields do not belong in `ScreenModel`. Keep `ScreenModel`'s pure projection and operations portable; retain its existing native `Client` adapter rather than adding a speculative transport trait. Extract/move common methods as needed, not a second implementation of them. In particular, close impact, route admission and candidate validation must not be copied into remote-only helpers.

Add **`WindowWorkspace`** as the UI-facing bounded collection of source presentations and the sole globally shown `Option<ScreenKey>`. It owns local-source identity and the flattened opened-Screen navigation order. It exposes a selected `&ScreenModel`, read access to a source by `SourceId`, the fixed local source, ordered opened references, and exact source/terminal admission. The existing renderer consumes that same selected model; it does not switch between separate local and remote view implementations. Source models may retain their last selected Screen/focus for return, but only the window selection authorizes shown-Screen geometry and user navigation.

Do not build a second facade forwarding every `LocalWorkspace` method. Core commands resolve their source and invoke its existing common model operations; GPUI reads the chosen canonical model and shared presentation helpers. Migrate existing local tests while preserving behavior before adding remote policy tests. The new names are proposed seams, not a request for public constructors usable only in tests; assignment remains private to actual owners.

**Alternatives rejected:** cloning LocalWorkspace opens/attaches every remote Screen; one model containing all hosts with raw IDs collides; parallel local/remote model facades duplicate admission and rendering; a generic catalog-policy framework is unnecessary for the two real policies.

The initial UI handoff freezes these concrete read responsibilities (names can be adjusted once, before dependent edits): `WindowWorkspace::local_source() -> &SourceWorkspace`, `source(SourceId) -> Option<&SourceWorkspace>`, `shown_key() -> Option<&ScreenKey>`, `shown() -> Option<&ScreenModel>`, ordered opened-reference iteration, and a ready-only `execution_token(SourceId) -> Option<SourceToken>`. Source state exposes its stable identity, source kind/assigned target and bounded catalog/host-state readers; existing Screen projection readers remain on `ScreenModel`. A retained offline snapshot is presentation only: it does not expose a ready terminal route. Runtime admission independently checks readiness even if a caller retained an older token.

At that same handoff, freeze `Update::Workspace` carrying the window presentation, the source/attempt-qualified host status update, unchanged charged `Event`/`Attached` envelopes, concrete host action target readers, and Connect/Retry/Cancel/Open/Detach/Terminate plus global navigation command variants. Fixed-local API commands retain their current local owner fields. Freeze before Sol begins dependent rendering edits; no UI/backend policy is implemented in GPUI to bridge a missing core seam.

### NR2: Keep exact target, stable source and attempt authority distinct

One remote source exists per exact validated target per native worker while browse/open demand or its cleanup exists. Target bytes come from the accepted local merged-directory snapshot; accept valid server identities exactly, never trim, resolve aliases, canonicalize or rewrite them. Alias is display metadata. Recheck the fixed local owner and current known-host membership before first Connect. A stale directory row cannot contact a newly selected target. Directory refresh failure can retain a same-owner last-good known entry; no fabricated host entry is accepted.

The follow-up `native-host-management` adds a separate explicit target-entry command with shared SSH validation and confirmed local metadata preparation. It does not relax this directory-row command's admission. Its worker-owned browse withdrawal also covers preparation that has not yet published a source; display-name edits and forgetting remain local metadata operations.

Allocate a private `SourceId` when the real host demand is admitted, before an asynchronous open. Keep it through retries while that demand exists. A failed initial attempt needs no fake connection UUID or synthetic ready model. Each successful `Client` supplies its actual new connection UUID for `SourceToken`; each attempt also has a monotonic source-local sequence because cancellation/progress/Retry must be fenced even before a connection exists. This is the only new lifecycle sequence. Do not add a window UUID: the owned command channel supplies window authority.

The worker owns target-to-source membership. Remote payloads cannot register sources, change targets or impersonate local API events. `remote.ready.target` and transport `Purpose::Host` must equal the assigned target. Source IDs never travel on the wire and are not persistence keys. On final demand removal and completed cleanup, discard the runtime entry; a later Connect gets a new source. The passive Hosts row remains owned by the local directory.

Command routing remains explicit:

| Intent | Capture and owner |
| --- | --- |
| Connect known host | Existing `Command` envelope captures fixed local `SourceToken`; exact known target is validated against that owner's accepted directory. Deduplicate an already demanded source. |
| Cancel browse, Retry or reconnect cancellation | A core-created host action target captures local owner, exact target, assigned source and attempt sequence. It is valid without a remote ready token but cannot cancel/retry a newer lifetime. |
| Open existing live Screen | Remote `SourceToken` plus `ScreenKey`; require ready live catalog membership and available aggregate capacity. Cached rows offer Connect first; they do not enqueue an open against an unknown future connection. |
| Switch or relative Screen navigation | Window-owned operation over grouped opened references. Capture the current window/local lifecycle owner; resolve destination keys against that same window. A destination that is offline may be selected for viewing its status but grants no execution token. Absolute selection never relies on mutable active-source lookup. |
| Remote tab/split/pane close/terminal/geometry | Existing source envelope and exact target/route; require currently ready source and explicit opened membership. Input requires exact attached route, not catalog generation. Geometry additionally requires globally shown Screen and existing route/revision checks. |
| Detach | Window/local lifecycle owner plus exact admitted remote `ScreenKey` and source lifetime; permitted while offline or setup is pending. It revokes the reference immediately, without a stale connected token being needed. |
| Terminate | Ready source token, explicit Screen key, request ID and assessed affected identities. No reconnect replay or retargeting. |
| Appearance, zoom, Drawer preferences, profiles/library and passive directory | Always fixed local source/API owner, even while a remote Screen is shown. Remote settings/hub notifications never drive local reads or writes. |

Keep the current command envelope; add only the concrete host/window intents required above. Establish exact signatures before parallel UI changes. An optional owner remains reserved for the existing startup window-close race; it never authorizes a host open. Source events/attachment deliveries retain their existing charged envelopes. Publish admitted source identity/state before its dependent events. Remote status deliveries carry source/attempt identity; superseded progress, EOF, results and close outcomes cannot alter a newer source or settle a same-numbered request elsewhere.

### NR3: Distinguish passive observations, live catalogs and opened references

Hosts shows the existing local directory merged for presentation with the window's own source states. Merely visiting Hosts, filtering, reading aliases or expanding already cached details does not connect. An explicit Connect starts browse demand; its connected detail can list the live catalog. Ending that browse (collapse/leave connected details) releases browse demand. Open references keep the connection alive independently, including hidden Screens. Navigating away from Hosts therefore cannot disconnect an opened terminal.

One host connection serves all its opened Screens and its explicit browse demand. A pending open is also demand until resolved or cancelled. Zero demand cancels timers/setup/reads, closes that stream and awaits cleanup; forward supervisors are not owned by this demand. Initial failure remains visible with Retry while browse demand remains. Repeated clicks do not create a second pending attempt or a retry queue.

Use `remote.ready` as the handshake observation, then validate `mux.welcome` and request `mux.workspace_resync`. Require exact `workspace-catalog-v1` for live native remote browsing/opening; missing capability produces a stable unsupported state and no mutation/legacy fallback. Do not describe handshake/cached entries as a live catalog. `ScreenInfo` provides live IDs/names but no pane counts: use confirmed opened snapshots for current counts and retain handshake/cache counts only as explicitly last-observed, otherwise unknown. Never resync or attach an unopened Screen merely to populate counts.

Catalog updates can change live membership/names without opening or selecting anything. Ignore the remote `activeScreenId` for window selection. Global navigation is local catalog order first, then remote groups in the accepted saved host order, then unarranged hosts in first-open order; each remote group's references use first-open order. Existing host-order reads are reused, with no reorder mutation/UI in this slice. Relative navigation uses this same flattened order and does not wrap.

For explicit Open and every refresh/reconnect, reuse the coherent collector with an **opened-only requested set**. Send resyncs, then an ordered trailing workspace-catalog request. A missing Screen yields no resync response today: absence from the trailing catalog marks the reference missing; a present entry lacking its required snapshot is an incomplete/invalid observation, not success. Bound a pass to a 15-second observation deadline after welcome, with at most one recollection for concurrent membership change, then show a read failure awaiting a new invalidation or explicit Retry. Do not loop on EOF, failed resync or a still-missing Screen. Catalog presence is not an active lease; mutations always recheck current connection and admitted route.

Spontaneous snapshots/spawns for unopened or detached remote Screens never allocate views or attachments. A pane moved externally between two opened Screens on the same source is coherently reconciled and retains its view. A pane moved into an unopened Screen leaves the opened collection and detaches; that destination is not opened to preserve it. Keep source-wide collision validation within the opened collection. Opened missing references reserve a small reference slot but have no executable route.

No client catalog persistence writer is added. The hub already records handshake observations and invalidates the local directory. Live observations remain live window state; any later `hub.catalog.observe` writer is a separately scoped follow-on.

### NR4: A concrete, retained remote lifecycle over existing adapters

Add `OwnedService`/`EmbeddedService` host-open methods accepting the exact validated `SshTarget`, a caller-owned revocable `queue::Context` and charged progress callback. `update` is always false. Unix uses the service's existing Connector and admission domain; owned uses its existing Owner. Do not make a new Connector per target or invent a separate admission domain that bypasses the window limit.

The native worker retains one small runtime state per real remote source: admitted demand, stable source ID, attempt sequence, preparation context/future, optional ready Client, read state, optional closing future, reconnect deadline/timer, last bounded diagnostic and first cleanup failure. It is not a general retry/task registry. Keep remote runtime fields outside cloned workspace presentations.

Lifecycle states are Connecting, Ready, Reconnecting, Failed/Offline and Closing, with catalog loading/failure represented separately. Readiness for terminal actions additionally requires the matching catalog/snapshot/attachment. Retain at most the latest bounded progress per source; duplicated control/application preparation progress must not create an unbounded log. Setup and cleanup are polled alongside local traffic, other sources and the independent settings/Hub streams, never awaited sequentially in the main command path.

Cancellation closes the attempt context promptly but **retains and polls the setup future to completion**. If cancellation races a successful returned client, close and join that client before releasing its source/open slot. `Client::next_event` can be selected and cancelled because the backend Peer owns the persistent reader; never restart byte framing or abandon an owned future returning a Client. A pending close is taken once into a retained future; repeatedly cancelling a polling wait does not drop the close outcome. Preserve the first cleanup error through later state publications and final finish. New attempts for that source wait for prior setup/read/close ownership to settle.

Use a bounded fair multiplexer over admitted sources. Keep at most one outstanding session read per source and the existing bounded command/update channels; no task per frame and no unaccounted event clone. Saturation must leave cancellation possible. A blocked/failing host cannot block unrelated settings, local terminal input or another host's setup. Preserve the existing transport's fail-on-budget behavior rather than buffering indefinitely.

Extend `negotiation` with a narrow accessor borrowing the structured charged `Closed` behind a preparation `io::Error`. Preserve an actual peer `Closed` reason through orderly EOF and expose it through native/owned Client adapters as a retained charged value; do not turn a normal receive into an unbounded diagnostics stream. `Closed.message` can contain the hub's serialized `remote.error`; decode that **documented structured payload**, with charged scratch and field/string limits, to obtain code/message/hint. Never parse formatted `error.to_string()` or infer retryability from English text. The outer `Closed.retryable` is authoritative. Preserve an ordinary bounded text message when no structured remote payload exists. This is an accessor/data-retention change, not a new wire schema; transport still knows no host-domain policy.

### NR5: Bounded recovery with revoked execution and fresh emulation

Initial Connect failure always stops for explicit Retry, even if transient. Automatic recovery is eligible only after this demanded source previously reached application `remote.ready`, while demand remains and the failure is retryable. Reject unsupported capability, target mismatch, malformed protocol and explicit nonretryable diagnostics without automatic retries. Treat genuine unclassified transport loss after readiness as transient; do not reinterpret authorization or validation failures as transport loss.

Use growing waits starting at one second, capped at ten seconds, and a fixed **two-minute total recovery interval** measured from loss until readiness returns. This follows the existing in-window host policy (`remote_socket.ts`), not the separate remote-only-window ten-minute policy. A single retry's preparation cannot outlive the remaining recovery interval: revoke its context at the deadline, keep joining cleanup, then show Offline/Retry. Explicit initial attempts retain the established ten-minute host-preparation ceiling. Progress does not reset either deadline. Explicit Retry starts a fresh attempt/interval only after previous ownership has settled; timer wakeups carry the source/attempt and recheck demand.

On loss, revoke terminal input, replies, user mutations and geometry immediately. Keep explicit references and an honest offline presentation, but never upgrade their old tokens to a replacement Client. Discard unsent old input, pending resize and non-idempotent operations; pending destructive outcomes become failed/uncertain, not replayable. A late old EOF/progress must not turn a recovered source offline.

On recovery, retain stable source/Screen/pane namespaces but replace the connection token. Obtain a fresh live catalog and fresh snapshots for **only** the surviving explicit references, accepting the new service's revisions without comparing them to a previous service instance. Reattach only their confirmed session bindings. GPUI must reset or replace the terminal emulator **before** feeding the replacement connection's replay, even when pane/session strings are reused; append-to-old-scrollback is not recovery. Preserve view identity and emulation across ordinary hide/show or same-connection structural updates. Reset pointer/search/geometry state that is invalidated by emulator replacement. Replay suppresses replies/clipboard effects as today; live hidden output/replies remain scoped and continue normally.

### NR6: Detach, terminate and whole-window close have distinct effects

Remote Screen close/default close shortcut removes the explicit reference and revokes its routing without confirmation. Withdraw its visible/size/activity claims and detach only sessions no longer needed by another reference on that source. If it was shown, select a surviving neighbor in grouped order. Release the host stream only when no browse/open demand remains. Never send `close_screen`, `close_pane`, daemon stop or forward-stop as a consequence of detachment.

Remote **Terminate Screen** is separate and always confirms the assessed target/host and affected sessions in this increment. Reuse close-impact collection and execution-time identity/busy/unresolved validation, including hidden/floating panes; include source token and request ID. Pane close retains the existing assessed destructive-pane semantics on the owning host. Remote disconnect or changed target while a prompt waits invalidates it; a prompt cannot silently bind to the reconnected source. A submitted close with lost outcome reports uncertainty and resyncs; it is never sent twice automatically. Completion uses an ordered catalog/snapshot fence, including missing Screen handling. Only a confirmed authoritative absence settles Screen termination.

Whole-window close keeps its existing local assessment, startup race and acceptance-before-quit behavior. Remote sessions are excluded from destructive/busy counts. After acceptance, refuse new mutations, revoke remote demand and timers, join every remote setup/read/close, settings stream and Hub stream, then close the owned local service/Owner. Mark cleanup complete only if all required cleanup outcomes are observed successful; a published earlier close failure cannot be overwritten by final success. An owned carrier failure may invalidate all of its substreams: report that shared failure honestly rather than inventing remote independence from a failed physical owner. Never end a remote daemon, terminate WSL or kill unrelated processes.

### NR7: Bound window growth, not just each host

Keep transport/process quotas unchanged. Add explicit window admission before creating a source or reference, dialing or reserving a new pane. Initial proposed limits are **8 remote source entries**, **4 pending remote opens**, and the existing **128 Screen / 128 pane-session-or-pending** ceilings shared across local and remote opened collections. Source entries include connecting, failed-with-demand and closing entries; cleanup does not release a slot early. A live catalog may describe up to **200 remote Screens per source**, matching cached-directory scale, without consuming 200 open-model slots; refuse oversize catalogs as unavailable rather than silently truncating and calling them complete.

Apply the existing 16 MiB staged Screen observation ceiling across the window's simultaneous reconciliation batches rather than independently multiplying it per source. Account retained typed remote catalogs, progress, diagnostic strings and decode scratch against existing charged budgets; use shared immutable snapshots so queued presentations do not clone payload allocations. Retain the existing per-connection and process transport charges on every Event until consumption/rejection. The bounded Screen/pane model and terminal count is an additional limit, not a claim to bound all emulator/framework memory.

A central window reservation covers local and remote pending creations before mutation send. Snapshot adoption checks the aggregate candidate before replacing any source state. Capacity refusal preserves surviving routes and leaves the rejected open/refill visible as an error; do not evict another source or partially attach the candidate. A failed submission, confirmed missing/closed reference or final cleanup releases its reservation exactly once. Metadata catalogs cannot reserve sessions or trigger attachment. Busy admission has no unbounded queue; explicit Retry after capacity is freed is sufficient.

### NR8: Reuse the native surfaces without widening authority

The primary task is Connect a known host → inspect live Screens → Open one. Extend existing `workspace/hosts.rs` and Drawer grouping; use existing toolkit buttons/menus, keyboard Actions, theme/spacing tokens and focus restoration. Preserve target visibility alongside aliases. Connect/progress/Retry/error stay with the affected host, cached observations stay marked last-seen, and disconnected opened Screens remain selectable to understand/retry/detach them. No guessed active status, placeholder remote controls or cloned local settings surface.

The common Screen renderer supplies terminal focus, input, selection, copy/paste, mouse behavior, find, resize, tab navigation and maximize using source-qualified keys. Both GUI and worker enforce the remote allowlist: default Tab/Split, selection/focus, terminal I/O, size/visibility/activity, supported within-Screen layout interactions and assessed pane close. Existing floating layouts render canonically; unsupported operations stay unavailable with a reason. Remote profile-menu launches, templates, recreated Screen commands and transfers across remote Screens/sources are rejected before effects. New Screen creation remains outside this increment; successor `native-remote-screen-create` admits only its explicit already-open ready-group action. In particular the current empty-Screen `new_terminal` calls creation and is not reused remotely.

Local theme/font/zoom continue to apply. Local profiles/library remain available as local features, but cannot be submitted to the active remote Screen. Do not infer local API owner from whichever source is shown.

Remote non-loopback HTTP(S) links may use the existing deliberate safe opener. Until forwarding is implemented, remote localhost/loopback/wildcard links are not dispatched as local browser destinations; show a useful unavailable explanation on an explicit click. No automatic forward or unrelated local URL is opened. No viewer/editor/file feature is added by receiving remote messages.

## Architecture impact

Update `architecture/client-core.c4` descriptions/relationships for `client_workspace` (common source collection and window selection/admission), `native_workspace` (bounded real source lifecycles), `owned_service`, `embedded`, `connection` and `hub` (fixed local API owner remains passive). The host adapter adds real `embedded -> splitty.remote.ssh` / `embedded -> splitty.transport.budget` use if its signature requires them, and reuses its `splitty.server.native`/service relationships. `owned_service` uses the existing `splitty.remote.owner` host-open path. Nested workspace/runtime modules do not become new top-level components.

Update `architecture/gpui.c4` for shared source presentations, Hosts intents, global Screen selection and terminal replay reset. GPUI depends on client-core only for backend/model policy. Update `architecture/transport.c4` for retained `negotiation`/`peer` diagnostics and any real adapter relationships in `architecture/server.c4` and `architecture/remote.c4`; no new transport-to-server/domain dependency is allowed. Extend architecture tests for actual changed imports, including test-only fixtures, rather than suppressing missing edges.

Update `docs/architecture.md` sections **Client connections**, **Remote Screens** and **Hub-owned host metadata and passive updates** with these ownership and recovery boundaries. Wire shapes remain unchanged; verify `docs/protocol.ts` against the consumed `remote.ready`, `remote.error`, `Closed`, workspace-resync and capability contracts and correct touched documentation discrepancies if found. Do not change storage formats or Hub ownership.

## Risks / Trade-offs

- **Local model extraction could regress mature local features** → first move the common collection under unchanged local tests; keep local policies and advanced operations intact, then add remote policy. Avoid forwarding facades and broad unrelated cleanup.
- **Catalogs omit pane counts and missing resync has no reply** → explicitly separate count freshness and use ordered trailing catalog/deadline tests; never synthesize zero or recreate a missing Screen.
- **SSH preparation and cleanup can be slow** → existing service deadlines and owned cancellation plus retained futures; UI state can say closing while ownership is still held. Two-minute automatic recovery can end a slow retry even though a deliberate first connect allows longer preparation.
- **Remote services can reuse IDs/revisions after restart** → stable source keys plus fresh execution token, fresh authoritative models and emulator reset before replay; old operations and prompts never transfer authority.
- **Shared transport owner can fail all substreams** → isolate ordinary per-source failure and report physical-owner failure separately; do not promise impossible independent survival.
- **Experimental feature subset differs from released clients** → explicit native requirements/exclusions; no weakening of global released-client persistence/forwarding/creation guarantees. Unavailable remote actions must be blocked at admission, not merely hidden.

## Migration Plan

1. Complete `native-source-routing` and preserve its integrated evidence.
2. Refactor the actual local model into the shared collection/window seam with identical local behavior.
3. Add typed diagnostics and narrow host adapters, then tested bounded remote lifecycle/catalog/open/close policy before UI effects.
4. Integrate Hosts/Drawer and common terminal rendering, exercising the full known-host flow and local regression paths.
5. Keep the web client available. No persistent migration is required; reverting the feature leaves host daemons/Screens and existing settings files untouched. Run targeted checks for this branch; full merge checks remain a recorded obligation for main integration, not an implementation-time Windows/full-CI expansion.

## Provisional decisions

These are proposed defaults for review, dated 2026-11-07. Central decision-log integration belongs to the integrating owner; this planning task changes only this change directory.

- **NR1 (provisional, tbd):** evolve the existing implementation into `SourceWorkspace` with concrete local/remote catalog policies plus one `WindowWorkspace`; no parallel remote model/facade.
- **NR3 (provisional, tbd):** explicit Connect holds browse demand until connected details are left/collapsed; Open is offered against a ready live catalog, not queued from a cached row; remote references order by first-open within accepted saved host group order.
- **NR5 (provisional, tbd):** in-window automatic recovery has a two-minute total interval, one-second initial delay and ten-second cap; initial failures require explicit Retry.
- **NR6 (provisional, tbd):** each explicit remote Screen termination confirms its assessed impact; default Screen close and window close only detach remote work.
- **NR7 (provisional, tbd):** 8 remote sources, 4 pending opens, 200 metadata rows per host, aggregate 128 opened Screens / 128 pane-session-or-pending admissions, 16 MiB simultaneous staged Screen observations and a 15-second catalog/snapshot observation deadline.
- **NR8 (provisional, tbd):** remote loopback link opening remains unavailable with an explanation until its deliberate forwarding flow exists; empty remote Screens are never refilled through local creation policy.
