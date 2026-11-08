# Design

## Context

See [proposal.md](proposal.md). `SourceWorkspace::rename_screen` and `Client::rename_screen` already send the existing bounded operation and request an ordered catalog refresh. The remote runtime allowlist currently excludes `RenameScreen`. GPUI's editor, captured entrypoint, commit validation and Drawer input rendering are explicitly local-only; editor reconciliation must use the captured source rather than the selected source.

## Goals / Non-Goals

**Goals:** reuse source-owned mutation/observation and one inline editor for local and remote opened Screens, preserving exact identity and local behavior.

**Non-Goals:** see the proposal. No mutation scheduler, new request protocol, optimistic name store, remote creation/recovery change or general dialog abstraction is needed.

## Decisions

### NRR-1: Reuse existing source admission (provisional, tbd)

Expose a narrow side-effect-free rename availability check in the existing source collection and reuse it before mutation. Preserve the current local checks; remote targets additionally require canonical current presence, not a missing or unconfirmed reference. The serializer retains its existing name bound. Permit `RenameScreen` in the existing remote command path with its captured source envelope and connection field. Reuse current close/readiness gates and mutation watermark/collector rather than deriving authority from selection or adding a local-owner hub operation.

The worker channel already belongs to the window; the remote execution token routes only its assigned source. GPUI additionally captures the window lifecycle owner so an editor cannot survive replacement of its owning view context. The rename sends no local profile or launch information and does not change selection.

### NRR-2: One exact editor, independent of selection (provisional, tbd)

Generalize the existing rename editor to retain window owner, source token and `ScreenKey`. Drawer callbacks capture these identities from the rendered row; activation and commit both revalidate them without acquiring a fresh replacement connection. Keep local smoke/helper entrypoints as wrappers over the same editor path.

Reconcile editor validity through `window_model.source(editor.screen.source_id())`, not the shown source. Unrelated source updates or selection do not reset the input or commit to a new target. Removed/replaced/unavailable targets close the editor without sending; deliberate Enter/Blur commit and Escape cancellation preserve existing interaction behavior. The remote Drawer renders that same input only for its exact captured row, with double-click/F2/context entrypoints following existing Kit behavior. No separate edit button is added.

Alternative rejected: change only the remote allowlist and use the selected source at commit. That would leave the editor local-only and allow cross-source ambiguity.

### NRR-3: Canonical naming remains authoritative (provisional, tbd)

Reuse the existing `rename_screen` operation and ordered catalog refresh. Keep the old displayed name until a canonical observation supplies its effective replacement; do not add an optimistic name registry or automatic retry. Existing failure presentation reports submission failure. If an admitted operation loses its connection, ordinary reconnect observations may reveal its effective name but never resend it or fabricate an acknowledgement. Renaming is not a navigation command and must not steal terminal/editor focus or alter creation-follow authority by inference.

## Architecture impact

Update the existing `client_workspace` and `native_workspace` descriptions/relationship in `architecture/client-core.c4` for reusable rename admission and exact remote dispatch. Update the existing GPUI workspace description/relationship in `architecture/gpui.c4` and the native editor/source-identity account in `docs/architecture.md`. No new crate, top-level component, dependency, protocol shape or server capability is needed; the connection serializer remains unchanged.

## Risks / Trade-offs

- A selected source changes while another row is edited → validate and reconcile the editor's captured source and Screen, with colliding-ID tests.
- A reference becomes missing or unconfirmed before commit → apply shared availability at both UI commit and Core send; never recreate it.
- A connection or window owner is replaced during editing → discard the old capture rather than obtaining replacement authority.
- Ordinary canonical refresh resets input/focus → retain editor entity/value while its exact target remains valid.
- Rename overlaps creation or other remote work → preserve existing collector ordering and close gates; no new scheduler or follow policy.

## Migration Plan

No stored format or rollout is required. Add shared admission/dispatch tests, then the existing editor integration and input tests. Use the existing owned remote-screen window fixture for bounded actual-input evidence. Rollback removes the native entrypoint without undoing names already chosen on the host. Full workspace CI remains the deferred pre-merge checkpoint.

## Provisional decisions

- **NRR-1 (2026-11-08, provisional, tbd):** rename only opened canonically observed remote Screens through existing exact-source dispatch and admission.
- **NRR-2 (2026-11-08, provisional, tbd):** share the existing inline editor with captured window/source/Screen identity and selection-independent reconciliation.
- **NRR-3 (2026-11-08, provisional, tbd):** adopt canonical name observations without optimistic duplication, offline queuing or automatic replay.
