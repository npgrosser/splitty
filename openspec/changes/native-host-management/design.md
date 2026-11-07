# Design

## Context

See [proposal.md](proposal.md). The shared native runtime already owns exact-target remote source admission, browse/open demand, recovery and shutdown. `HubClient` owns a separate local API stream for passive directory reads. The server already validates `hostPreferences.update` and `host.forget`, persists aliases/tombstones, publishes `hub.changed`, and keeps sessions and forwards independent of metadata removal. The native Hosts view currently renders only accepted directory rows; a failed unnamed new target would otherwise have no visible row.

## Goals / Non-Goals

**Goals:** Complete explicit target connection and ordinary host metadata management without changing SSH authority, adding a second persistence model or moving client policy into GPUI. Keep failed/uncertain work visible, preserve open work, and cancel delayed connection intent when its UI demand ends.

**Non-Goals:** Offline unnamed bookmarks, SSH configuration editing, port controls, host upgrades, remote Screen creation or remembered references. Existing remote acceptance gaps remain tracked in `native-remote-screens`.

## Decisions

1. **One owned hub API stream.** Extend `HubClient` and the existing `RuntimeHub` carrier lifecycle with typed metadata jobs. Keep directory updates separate from identified mutation outcomes. Use a FIFO of at most 16 accepted actions, one active API job, one pending action per exact target and bounded diagnostics. Yield between reads and writes so neither can starve; repeated invalidations coalesce. A separate management stream would duplicate cancellation and reopening ownership for short operations of the same feature.

2. **Validated commands and outcomes.** `ManageHost` carries a request ID and a `HostManagementRequest` constructed through validated Connect, SetAlias and Forget constructors; the command wrapper captures the full local `SourceToken`. Targets use the existing SSH parser and aliases use the server's 200-character/no-controls policy. Connect's omitted alias preserves an existing name; an explicit empty alias in SetAlias clears it. Results identify the original owner, request, target and action, with distinct admitted/pending, applied, rejected and uncertain states. Exact public signatures are frozen before the dependent UI implementation. Only the fixed local owner can submit these commands, including while a remote Screen is selected.

3. **Connect is preparation followed by cancellable browse demand.** Explicit target entry first submits `{target, hidden:false, alias?}` through `hostPreferences.update`. Only a validated successful reply permits a private explicit-target path into the existing remote admission/setup helper. The existing known-row Connect remains directory-bound. Revalidate owner and demand before starting SSH; neither lost replies nor passive refreshes continue the connection automatically. Successful metadata preparation is not host readiness. The existing remote presentation supplies actual progress, Retry/Cancel and failures. There is no fabricated alias or claim that an unnamed offline target was saved.

4. **Withdraw browse intent in the worker.** Add an owner-bound `ReleaseHostBrowsing` command to withdraw every pending typed-connect continuation and current browse interest atomically. Use it when leaving Hosts, rather than enumerating only the UI's possibly stale source snapshot. `WithdrawHostConnect` captures the request identity when cancelling one Connect dialog, preserving unrelated browse demand even when cancellation races source admission. Individual Cancel/Retry actions retain exact attempt protection. Already submitted metadata may finish and refresh after cancellation, but may not dial. Open references continue owning their sources. Normal shutdown discards unsent writes, joins retained work and never replays a submitted mutation on stream reopening.

5. **Authoritative reconciliation and current work.** Invalidate read generations on admitted mutation; preserve valid mutation acknowledgement independently of `hub.changed` invalidation. Refresh after success or uncertainty, retaining last-good directory on failure. A matching later read is not a replacement acknowledgement. Render accepted directory entries plus explicitly demanded runtime presentations, deduplicated by target. Targets absent from the directory are labelled as current connection work, not saved metadata. Forget removes metadata and its display alias, without releasing browse, detaching Screens, stopping forwards or recreating the saved entry. Once temporary demand ends, an unnamed failed attempt can disappear normally.

6. **Retained native forms.** GPUI owns input entities, focus, dialog/menu state and rendering only. Use a Kit Connect dialog with target input, known-target suggestions and optional display name; use a short display-name editor and row menu for metadata actions. Provide Enter submission, keyboard suggestion selection, Escape cancellation and focus return; show validation/pending/error beside the relevant action and suppress duplicate submission. Leave the existing Screen renderer and canonical layout untouched. Current-work rows explain why an active forgotten target remains available. Menus may improve on the web presentation while preserving its operation semantics.

## Architecture impact

Extend `architecture/client-core.c4`'s `hub` and `native_workspace` descriptions and actual relationships for typed host mutations, shared target validation and browse-continuation ownership. `architecture/gpui.c4` keeps the host form under `gpui_workspace` and its existing `splitty.client_core.hub` relationship. Update the Client connections/Remote Screens ownership sections of `docs/architecture.md` and the native subset in `docs/gpui-spike.md`. No new top-level crate, module, backend operation or persistence format is planned. Check the existing host preference shapes in `docs/protocol.ts` and correct discrepancies only if found.

## Risks / Trade-offs

- A metadata write may persist before its reply is lost; removal may also fail after writing its tombstone. Report uncertainty for ambiguous or server-error outcomes and refresh without replay or starting SSH.
- Leaving Hosts can precede source publication: worker-owned withdrawal must cover queued and active preparation, not only visible rows.
- A forgotten connected target can still own work: distinguish current connections from the saved directory instead of recreating an alias or hiding live session ownership.
- Older peers may reject metadata operations: preserve the last confirmed directory and show the failure; ordinary already-supported connections remain usable.
- The remote increment is not fully accepted yet: preserve its open matrix and use the existing owned two-host fixture for this feature's actual-window proof.

## Provisional decisions

- **NHM-1 (2026-11-08, provisional, tbd):** Connect is the entry action; there is no new offline unnamed bookmark format.
- **NHM-2 (2026-11-08, provisional, tbd):** Host mutations share the existing hub API stream with separate outcome state and a bounded 16-action queue.
- **NHM-3 (2026-11-08, provisional, tbd):** Worker-owned browse withdrawal covers pending preparation; current unsaved/forgotten connection work remains distinguishable from directory entries.
