# Design

## Context

See [proposal.md](proposal.md). The native Drawer already groups opened remote references by `SourceId`, but renders each caption as inert text. `ForwardSnapshot::rows(WindowWorkspace)` joins current-source discovery with active and saved mappings. The existing Ports view owns feedback, forms and browser-origin tracking; the client Core already captures the local owner, exact mapping/request, validates the acknowledged local port, verifies the same Up mapping passively and supplies a revocable one-shot browser handle.

`OpenOrigin` currently distinguishes Hosts from a captured terminal. Treating a Drawer action as Hosts would fail outside that destination. The Drawer is rendered outside destination content, so its logical host group remains available during Settings/Hosts navigation and autohide. A host group may disappear while its source remains alive for browsing, then reappear under the same source ID; identity checks alone cannot keep an old unsubmitted popup revoked.

## Goals / Non-Goals

**Goals:** Reuse one mapping projection and one submission/result path. Add exact passive navigation and an explicit host-group initiator whose UI lifetime is distinct from transport readiness and popup lifetime.

**Non-Goals:** A new forwarding policy, a second host directory or sorted row owner, attaching browser intent to the currently selected terminal, new host creation flows, or new protocol fields.

## Decisions

### DP-1: Existing rows feed a bounded Kit menu (provisional, tbd)

Capture the local `SourceToken`, remote `SourceId` and canonical target for the rendered group. Aliases remain display-only. The group is valid while the local owner matches, that source still names the same target, and at least one opened reference belongs to it. Do not require the remote execution token: forwarding is owned by the local Hub and can remain useful while the host's Screen connection is offline or reconnecting.

Build at most twelve mapping entries from existing target-filtered `ForwardRow`s, plus management access and truthful status. Report total known rows without claiming discovery completeness. Distinguish initial loading, unavailable/partial reads and no current discovery from a valid empty report. Describe only an observed Up mapping as a current endpoint; a saved local port is a preference and a rebuilding forward remains rebuilding. Longer labels use bounded plain text and the standard Kit popup, placement and keyboard behavior.

An Up row says Open; other supported states offer ensure-forward-and-open. Both use the existing `ForwardRequest::open(key, row.local(), false, None)` contract, preserving explicit saved-port conflicts and opt-in Remember. Never construct a localhost URL from row text. Immediately before submission, recheck group/owner, mapping presence, pending state, captured action class and local-port choice against the current projection. If they changed, request a fresh choice without effects.

### DP-2: Popup capture and admitted browser intent are separate (provisional, tbd)

Add one bounded current Drawer-popup context with a fresh identity, captured group binding and no transport lease. Establish it from the Kit open-change interaction callback; the popup's content builder only reads context and rows. The pinned Kit DropdownMenu content builder runs while rendering, so it must not mutate workspace state. Item callbacks capture that context identity, validate it and consume it when admitting an action. Dismissal, replacement or group loss clears unsubmitted authority permanently; ordinary reappearance of the same source cannot recreate the old context.

Extend the existing `OpenOrigin` with the admitted Drawer group binding, and keep the existing single `submit_forward`/result implementation with an explicit origin. Admitted intent no longer depends on popup state. It survives popup dismissal, autohide/unpin, Screen selection, Settings/Hosts navigation and ordinary reconnect while the logical group remains. On model reconciliation, revoke it if the last group reference disappears, its source is disposed or its canonical target changes; existing owner reset, Stop, explicit cancel and whole-window close also revoke it. Remove UI authority immediately and send the existing cancellation intent best-effort. A full command queue must still prevent a late browser dispatch. Revocation does not claim an admitted service-owned forward stopped.

The final result handler validates the retained group origin in addition to its existing exact owner/request/mapping and handle checks. Drawer handles must have no terminal override. Only consuming the verified one-shot handle dispatches its validated URL. Keep errors and unconfirmed outcomes visible from the Drawer, including after its popup closes, without restoring a permanent prototype status bar.

Alternative: key Drawer consent to the current destination or remote execution token. That would cancel valid work during ordinary UI navigation or reconnect and couple Hub-owned ports to terminal send readiness. Another alternative, tying consent to the popup, would cancel the intended operation when selection normally dismisses the popup.

### DP-3: Navigate by current row identity (provisional, tbd)

Share a small UI-only `HostGroupTarget` (local owner, source ID, canonical target) with capture/current readers; it is not a transport-attempt `HostActionTarget`. Narrowly extract the existing `HostRow` construction from `render_hosts` into one reusable projection. Caption activation locates the exact current Host row; Manage ports locates its PortHeader. Recompute the index at action time from canonical target, and use the existing list scroll handle. Do not capture row indices or resolve aliases. Support this focus navigation even when Hosts is already open, and respect existing form/modal/navigation guards. No BrowseHost, Connect, screen creation or forwarding request is needed to navigate; passive refresh may reuse the current Hosts entry path.

### Bounded admission remains shared

Retain the existing 64-feedback capacity guard before queue admission or open-origin insertion, same-key pending suppression and Stop supersession. Both current and Drawer entrypoints must use that path. Keep a single feedback table and origin registry; do not add per-popup request queues or unbounded context history.

## Architecture impact

- `architecture/gpui.c4`: update `gpui_workspace` and its existing client-ports/client-workspace relationships for Drawer initiators, shared host-row navigation and verified browser dispatch. Any helper remains a workspace child; no new top-level component or duplicate controller is planned.
- `architecture/client-core.c4`: no new component or dependency; forwarding policy and proof ownership stay in the existing client-ports component.
- `docs/architecture.md`: extend native Drawer/Hosts/Ports ownership and browser-origin lifetime with the distinction between ephemeral menu authority and admitted logical-group intent.

## Risks / Trade-offs

- A group is detached and reopened without source disposal → clear the active popup context and pending origins on the intervening model publication; new references cannot restore revoked authority.
- A port changes or is stopped while a menu is open → revalidate action class/local choice before admission, then retain existing acknowledgement and passive-verification checks before browser dispatch.
- Popup focus or autohide fights the Drawer → reuse Kit focus/overlay behavior and verify real pointer/keyboard dismissal, trigger return and unpinned reveal; dismissal does not manufacture an API action.
- Extracting host rows changes ordering → use the same projection for rendering and navigation, preserve existing order, and test colliding aliases and reordered data.
- Queue or feedback capacity is exhausted → refuse before new effects or retained intent; revocation removes UI authority even when best-effort cancellation cannot enter the queue.

## Migration Plan

This adds native UI entrypoints with no persisted data, protocol or service changes. Reverting removes those entrypoints and their local interaction state; independently owned forwards remain governed by the service's existing lifecycle.

## Provisional decisions

- **DP-1, 2026-11-08 (provisional, tbd):** Project a bounded Drawer port menu from existing mappings and exact canonical host-group identity, independently from remote terminal readiness.
- **DP-2, 2026-11-08 (provisional, tbd):** Keep one revocable popup context and separately retain admitted group-scoped browser intent; ordinary popup/UI visibility changes are not logical group closure.
- **DP-3, 2026-11-08 (provisional, tbd):** Reuse the current HostRow projection for exact passive navigation and scroll targeting, without creating connection or forwarding demand.
