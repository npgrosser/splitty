# Design

## Context

See proposal.md for motivation. The service already exposes deserializable `ShellProfilesState`, `Operation::ShellProfilesGet`, explicit `ScreenOp::NewTab.shell_profile_id`, and fallback details in `mux.pane_spawned`. The web new-tab menu is the behavior reference. The native worker currently reads appearance over an independent API stream with retained cancellation-safe setup/request futures, decoded allocation leases and connection generations.

Ordinary native launch currently copies cached CWD into an explicit override; a prerequisite fix restores the existing workspace-model inheritance contract. The service's metadata observer is sampled, so authority means the latest service-known CWD, not synchronous OS discovery.

## Goals / Non-Goals

**Goals:** reuse profile resolution and typed data, keep all service access outside GPUI, preserve one auxiliary stream and bounded ownership, add a practical keyboard/mouse profile picker.

**Non-goals:** a generic request framework, profile persistence/editor, transport or daemon lifetime changes, a cross-platform/WASM client architecture, or removal of existing web code. The owned embedded service remains the evaluation's local source, as in the released desktop lifetime model.

## Decisions

1. Replace the appearance-only stream owner with concrete `LocalSettingsReader`, containing appearance/profile job variants and independent result variants. Retain one pending request future and one API client, shared cancellation and explicit finish. Feature modules own typed reads; share only bounded JSON admission/decoding. Two duplicated lifecycle implementations or profiles inside `AppearanceSnapshot` would either diverge or couple unrelated failures.
2. Track dirty flags and generations separately. Initial load requests both. Same-service settings changes refresh both; OS preference changes refresh appearance only. Alternate dirty features after each completed or superseded read. Errors are emitted once per attempted generation. No automatic read retry or mutation replay.
3. Keep snapshots charged through UI use, reuse `ShellProfilesState`, and bound presentation independently: at most 128 profiles, profile IDs at most 256 bytes, names/reasons at most 512 bytes. Reject oversized profile presentation atomically and keep the last valid state. The shared response ceiling stays 1 MiB. The UI retains an Arc snapshot and derives bounded visible labels, rather than copying commands/arguments into a second domain model.
4. A captured new-tab choice carries connection, Screen, source pane and explicit profile ID. Revalidate source ownership/current route before submission; stale picker targets do not fall through to a different focused pane. Keep plain tab/split behavior unchanged. Service fallback remains authoritative if availability changes after the read.
5. Show enabled profiles, availability/reason, configured default and experimental status; unavailable entries are disabled. Opening, dismissal, Escape and keyboard activation must not type into a terminal. Fallback status is admitted against connection and pane/session identity and copies only bounded profile ID/reason. It is a nonfatal notice, not a launch retry.

## Architecture impact

In `architecture/client-core.c4`, add `local_settings` (API stream lifecycle/bounded decoding) and `shell_profiles` (charged typed snapshots/read). Narrow `appearance` to appearance snapshots/read. `embedded` creates `local_settings`; `native_workspace` polls its independent results and admits launch/fallback intents; `client_workspace` continues serializing canonical operations. Add only actual dependencies on existing `core.shell_profiles`, native service API and transport allocation modules. Keep in-memory service fixtures with their existing test ownership or move explicitly and update the map accordingly.

In `architecture/gpui.c4`, `gpui_workspace` consumes shell-profile snapshots and submits explicit launch intent. Extract picker presentation into a child module where useful; no terminal-engine dependency. Update the native runtime/local appearance concepts in `docs/architecture.md` to describe the shared auxiliary stream and independent feature ownership. No wire shape changes; verify existing documented names rather than adding a protocol.

## Risks / Trade-offs

- Shared stream head-of-line delay → existing bounded request deadlines, fair scheduling, separate terminal stream, independent errors.
- Availability race → disable known-unavailable entries and show the service's eventual fallback, with no client resolution or replay.
- Replaced source while picker is open → captured identity and worker revalidation; reject stale choice visibly.
- Large configuration or strings → bounded response and typed decode admission, charged retained data and atomic presentation limits.
- Focus leakage → real-window checks of open/dismiss/select and continued PTY input, including switching tabs with the picker open.

## Provisional decisions

- **NSP-1** (2026-11-07, provisional, tbd): profile choice is initially available only for new tabs; normal tabs/splits inherit server defaults. Extend other launch controls after evaluation.
- **NSP-2** (2026-11-07, provisional, tbd): 128 profiles, 256-byte IDs and 512-byte display strings bound native presentation; oversized reads fail visibly and atomically.

## Migration Plan

Implement and verify on the native feature branch. Existing configuration and wire shapes need no migration. Rollback removes the picker and restores the appearance-only reader; it does not modify settings, web clients or persisted workspaces.
