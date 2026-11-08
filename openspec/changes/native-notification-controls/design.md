# Design

## Context

See [proposal.md](proposal.md). The native editor currently retains `AppearanceSnapshot`, and common editor types live under `appearance/settings.rs`. `LocalServiceClient` already owns one API stream, a 16-intent settings FIFO, same-field exclusion, cross-path exclusion for zoom/Drawer commands, three fair read features, and identified mutation outcomes. Notification reads are independent and copy six required booleans before releasing charged replies; the ledger uses them without retaining transport events.

Today admission, write generations, reply classification, deferred update delivery, uncertain-field release and UI completion all assume appearance. Adding notification fields to those assumptions would couple switches to successful theme resolution and let the wrong read complete a write. No additional stream, mutation queue or transport operation is needed.

## Goals / Non-Goals

**Goals:** Preserve a single bounded mutation path while making its observation domain explicit. Let typed notification reads support boolean edits and service-default resets independently from themes. Keep acknowledged values and badge policy authoritative.

**Non-Goals:** A universal settings store or plugin framework, moving theme ownership, wrapping the server's full settings object, new notification delivery mechanisms, or a web-client rewrite.

## Decisions

### NC-1: Common contracts have a settings boundary (provisional, tbd)

Move `SettingsField`, `SettingsEdit`, `SettingsValue`, `SettingsMetadata` and `SettingsLimits` to `splitty_client_core::settings`; leave `TerminalPreferences` and theme validation in appearance. Update real callers rather than introducing permanent forwarding modules. Add exactly four boolean fields and `SettingsDomain::{Appearance, Notifications}` with `SettingsField::domain()`. Appearance includes its existing terminal/Drawer presentation fields; these domains identify independent authoritative read lifecycles, not UI sections.

Keep one bounded field intent and existing command envelope. Appearance value lookup must reject fields outside its domain instead of manufacturing a notification value. Notification lookup uses its own typed snapshot. Alternative separate notification edits/queues would duplicate identity, exclusion and outcome handling; a single aggregate snapshot would recreate the unwanted theme dependency.

### NC-2: Notification observation owns values and bounded reset evidence (provisional, tbd)

Extend `NotificationSettingsRead` with an owned notification snapshot: six effective booleans, four optional exact reset defaults and bounded metadata error. Keep its owner/generation identity and existing effective-result reader usable by the unread ledger. Read `home.get` and `home.metadata` on the existing notification job; decode metadata through the existing charged typed decoder and common validation, copy only four boolean defaults, and release all charged metadata before delivery. A metadata failure is auxiliary: valid effective switches still publish and edits remain available, while unavailable defaults cannot drive Reset. Do not use client defaults or `AppearanceSnapshot` metadata. A failed effective read keeps last-good same-owner values; replacing the owner clears authority.

A successful typed read proves that these existing boolean fields are supported. No theme/catalog evidence or numeric limit is needed to submit their two possible values. Master enablement changes only its own field, preserving category, toast and desktop preferences.

### NC-3: One scheduler routes by settings domain (provisional, tbd)

Route admission to last-good appearance or notification evidence. Allocate each write and required readback generation in its field's domain; only appearance writes affect terminal palette generations. Validate successful mutation replies against the edited domain's typed home projection. Notification acknowledgement never requires `HomeAppearance` or theme operations.

Replace the `home_read` boolean with the successfully observed domain and retain the current appearance behavior: a valid appearance home read can reconcile uncertainty even if later theme resolution fails. Accept the observation's generation before releasing only that domain's uncertain requests. `SettingsReconciled` identifies its domain and matching fields; deferred updates carry domain and generation so an OS appearance refresh cannot discard or validate a queued notification update. Retain the existing FIFO bounds, fair-read policy, same-field and legacy zoom/Drawer exclusions, no replay, owner checks, cancellation and joined shutdown.

### GPUI observation and controls

The existing Settings view keeps separate owner-bound appearance and notification projections. Effective value, reset evidence and completion-generation lookup dispatch through the field domain. Appearance refresh cannot clear a notification write, nor a notification read an appearance write. Matching request identity remains necessary for acknowledgement or uncertainty release. Forward notification observation even while Settings is closed, so opening it uses the current accepted data.

Add one Notifications section with controlled Kit switches for enablement and the three process categories, existing per-field pending/failure feedback, and Reset only where its published default exists. No optimistic ledger purge: only an accepted authoritative notification read changes policy and clears disabled entries. Owner replacement clears both projections and pending controls. Do not expose toast/desktop switches before native delivery exists.

## Architecture impact

- `architecture/client-core.c4`: add `splitty.client_core.client_settings` for `splitty_client_core::settings`; actual references from appearance, local_service and native_workspace use that component. Appearance remains responsible for theme/presentation projection, local_service for independent observations and the one writer. Add only actual test/production relationships found by the architecture check; module registration alone is not a dependency edge.
- `architecture/gpui.c4`: add `gpui_workspace -> splitty.client_core.client_settings`; its existing local_service relationship includes notification editor snapshots and identified domain-aware outcomes.
- `docs/architecture.md`: update native local settings ownership, per-domain generations, notification reset evidence and mutation/uncertainty lifecycle. The UI-free ledger remains separate and web settings remain usable.

## Risks / Trade-offs

- A field is matched to the wrong generation → derive domain centrally from its typed field, tag deferred/reconciled updates, and test mixed-domain interleaving and supersession.
- Metadata fails after successful effective settings → publish effective values independently and disable only unavailable reset actions.
- Acknowledgement arrives before a broadcast/readback → show pending confirmation until the matching domain reaches the required generation; never display reply values as saved.
- Moving common contracts changes imports widely → make a behavior-preserving extraction first, run existing focused settings tests, then add notification semantics separately.
- Shutdown interrupts a write → preserve existing uncertain outcome and owned stream cleanup, with no automatic retry.

## Migration Plan

No persisted data, server behavior or wire format changes. The feature extends the experimental native branch. Existing clients can share the same settings; reverting removes only the native controls and shared-contract reorganization.

## Provisional decisions

- **NC-1, 2026-11-08 (provisional, tbd):** Extract common editor contracts from appearance and use two explicit observation domains with one bounded mutation queue.
- **NC-2, 2026-11-08 (provisional, tbd):** Typed effective booleans authorize compatible edits without metadata; only exact service defaults authorize Reset, and metadata failure does not block notification observation.
- **NC-3, 2026-11-08 (provisional, tbd):** Route write acknowledgement, fresh readback, deferred delivery and uncertainty release by field domain; never replay an uncertain mutation.
