# Design

## Context

See proposal.md for motivation. Template CRUD and extraction already use authenticated service API operations; application is the capability-gated session `mux.op/apply_template`, not an API mutation. The pure reducer replaces a Screen's complete pane/session set, emits kills then spawns, preserves existing Screen name/order and names a new Screen from the template. The web client has a separate optimistic projection and command guard. The native client already owns coherent workspace collection, close impact assessment, bounded auxiliary settings jobs and canonical tiled/floating presentation.

## Goals / Non-Goals

Keep server validation, extraction and effects authoritative; keep reusable preparation, operation admission and outcomes outside GPUI. Preserve immediate pending layout feedback without speculative session IDs or attachment routes. Cover the complete local saved-template workflow. No general framework, new service, file format, host-management UI or shared WASM commitment is introduced.

## Decisions

### Share pure projection and command filtering

Extract `mux::project_template(&ApplyTemplate) -> TemplateProjection` from authoritative application. It validates and returns canonical root/floating trees, sorted retained terminal names and skipped-web count; a pane-ID iterator traverses those roots. It requires complete supplied IDs and generates no IDs, sessions or effects. The reducer fills omitted IDs first, uses this same projection and retains workspace uniqueness checks and effects. This avoids copying `template_intent.ts` or running a whole optimistic reducer that would invent SessionIds.

A pure template command-retention helper removes both command and restore mode for every deselected pane. The native library uses it for save and layout-only apply. The existing web guard keeps its small adapter but snapshots the exact template before approval, refuses command execution without a registered handler and consumes every approval/cancel once. Normal no-command application remains direct.

### Separate library data from workspace lifecycle

`splitty-client-core::templates` owns bounded immutable, charged template/catalog drafts and explicit API requests/results. Apply plans, target/choice/result types and pending presentation live in `workspace/catalog/templates.rs`, because they depend on Screen state, capacity and ClosePlan. This gives workspace a one-way dependency on template data. GPUI owns only inputs, lists, confirmation rendering and focus.

Rename the concrete owned auxiliary client to `LocalServiceClient` and extend its existing fair settings/zoom scheduler with one active/queued template job slot. List/load/extract/save/delete remain explicit jobs with connection/request IDs, retained cancellation-safe futures and joined shutdown. Reads can be refreshed deliberately; admitted mutations are never replayed. Replies and derived drafts remain charged and bounded (existing 1 MiB API limit, bounded catalog/string counts, no silent truncation). A slow library request must not consume PTY output capacity or block whole-window shutdown.

Save starts from one server extraction after selecting its optional relative root. The immutable result is reviewed for name and included commands, then saved without another extraction that could change displayed launch data. Overwrite requires a separate explicit choice for the captured slug/draft; delete likewise binds its exact slug. The existing API offers no versioned compare-and-swap, so a fresh existence check is not presented as protection against concurrent file replacement.

### Capture preparation and consent

Preparation allocates new pane IDs and, for a new Screen, its Screen ID once. It captures the template, owner, root, target and shared projection without changing workspace state. Replacement additionally captures `assess_close(Screen)` over hidden/floating/managed sessions. Submission rechecks capability, owner, target, close impact, current admission capacity and conflicting work. Changed identities or increased busy/unresolved impact require a fresh review. Replacement confirms that existing terminals end even for layout-only; it submits only `apply_template`, never a separate close operation. Commands are listed from the immutable prepared content, with explicit Run/Layout-only/Cancel and no Enter-to-run default. Root resolution remains on the target server; native path widgets do not reinterpret host paths using the UI host OS.

Command selection/name/root edits invalidate the old review. Library replies from a dismissed or superseded dialog cannot overwrite newer input, reopen a modal or authorize another mutation. Missing capabilities and non-applicable extracted layouts (for example nested floating splits) produce a clear error before application or saving an unusable native draft.

### Keep pending presentation distinct from authoritative state

One identified pending apply exposes owner/request/target Screen ID/name, borrowed root/floating trees, pane names and projected slot selection. GPUI displays placeholders immediately after queue admission, including a pending new-Screen chip, without inserting a speculative ScreenState or session route. Canonical routes and output remain alive until a coherent commit. Preview IDs never get input, attachment, visibility or resize messages. Canonical panes hidden by the active preview withdraw size claims; central admission rejects stale resize claims for them. Later local navigation hides the preview and cancels deferred following, but does not cancel an already submitted operation.

One ordered session submission is followed by a catalog fence and coherent authoritative observation. Compare the supplied IDs and projected placement, not just an op-failed flag. An effect can fail after reducer commitment. Confirm layout separately from terminal readiness, preserve successful siblings, and retain bounded failed-pane diagnostics rather than waiting forever for all Starting records to attach. Capture service-reported profile fallback against the intended pane IDs and show it after confirmation as a notice, separate from failed launches. Results distinguish confirmed application (possibly with launch errors), verified non-application and unverified outcome; mutations are never retried. Invalid/uncertain observations keep the last coherent routes. Unrelated Screens retain their process/entity identities.

Pending template reservations count as unresolved whole-window close impact. Pane/Screen structural actions conflict with application; whole-window close remains available, revalidates its impact and joins owned service shutdown. The modal dismisses before the ordinary close prompt, so input has one owner.

### Native presentation

Add a scroll-reachable Templates toolbar action and Alt+P/Cmd+P. One modal contains library, extraction/save, root and immutable review/overwrite/delete modes. Reuse native text inputs and scrolling. Its full surface occludes underlying pointer/wheel input, cancels active layout/terminal gestures and owns keyboard handling ahead of PTY/workspace shortcuts. Returning restores the current terminal or open find input. The renderer reuses canonical shape traversal for the pending skeleton; it does not add another docking model.

## Architecture impact

Update `splitty.core.mux` and `splitty.core.template` in architecture/core.c4 for shared projection/filtering. Add `client_templates` and rename `local_settings` to `local_service` in architecture/client-core.c4; update direct relationships for charged template API data, core template/mux helpers, workspace plans and native runtime outcomes. Workspace depends on template data, not the reverse. Update GPUI workspace relationships and descriptions in architecture/gpui.c4. Revise Client connections, Authoritative templates and consent wording in docs/architecture.md; qualify client shortcuts and extend docs/gpui-spike.md. Existing wire shapes in docs/protocol.ts remain authoritative and are checked against the touched operations.

## Risks / Trade-offs

- Replacement is destructive even without commands → display and revalidate its full close impact, never imply process-preserving layout application.
- Partial effects can coexist with committed layout → reconcile authoritative IDs and surface individual launch failures; no rollback fiction or replay.
- Pending views could claim nonexistent sessions or stale sizes → separate session-free projection and central presentation admission, tested with real output/size observations.
- Library operations could starve settings or retain unbounded data → one bounded explicit job slot, charged snapshots, fair scheduling and joined cancellation.
- Concurrent template-file writes are not versioned → confirmations identify the file/draft; document the existing last-write behavior without inventing a CAS guarantee.
- Web and native remain separate clients → share actual domain projection/filtering in Rust and align consent behavior, without imposing a speculative transport/WASM rewrite.

## Provisional decisions

- **NLT-1** (2026-11-07, provisional, tbd): expose the complete local library workflow; keep remote host selection and YAML editor/import/export UI separate.
- **NLT-2** (2026-11-07, provisional, tbd): existing-Screen replacement always needs explicit replacement confirmation; new-Screen no-command application needs no extra warning. Command execution has an explicit choice with no Enter default.
- **NLT-3** (2026-11-07, provisional, tbd): use a session-free pending projection with canonical routes retained separately; later navigation hides it without cancelling the mutation.
- **NLT-4** (2026-11-07, provisional, tbd): reuse the existing concrete auxiliary stream and file API semantics; do not add versioned file writes, a generic task framework or another service.

## Migration Plan

Preserve the accepted floating preview until the new candidate passes targeted checks and private real-window acceptance. Template formats and operations remain compatible; only missing-handler web command consent changes from execution to refusal. Rollback can restore the previous binary without migrating stored templates. Linux acceptance does not establish Windows/macOS interaction; full merge CI stays deferred on the evaluation branch.
