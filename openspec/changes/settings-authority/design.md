# Design

## Context

See proposal.md. Home settings are saved in `config.toml`, but control metadata and the browser controls currently disagree. Existing GET and PATCH response bodies are used by older clients.

## Goals / Non-Goals

**Goals:** One server-owned set of defaults and constraints, atomic validation, all-window synchronization, and explainable shell fallback.

**Non-Goals:** Settings on a different host, new settings UI layout, or window-only preferences.

## Decisions

1. Add `GET /api/home/settings/metadata` and native `home.metadata`, returning `{defaults,limits}` while keeping existing GET/PATCH bodies. Numeric constraints are `{min,max,step}`; discrete constraints are `{values}`; keys are dotted wire field paths. This avoids a breaking settings response shape. The alternative was changing GET, which would break old clients.
2. Validate the submitted patch before merge and persistence. Reject an invalid field with `{error,field}` and keep the previous values. On load, continue forgiving old invalid stored values so the server can start. The alternative was silently coercing updates, which conceals errors and leaves optimistic clients wrong.
3. Store both scroll sensitivities as floating point, with 0.1 and 0.5 steps respectively. This matches current controls and xterm's numeric options. The alternative was integer-only controls, which would remove current precision.
4. Emit the complete settings object through `settings.changed` after persistence; native and browser clients share the typed event. The alternative was patches, which need ordering and reconnect recovery logic.
5. The server resolves absent, unknown, disabled and unavailable shell profile IDs. The created event carries a small stable fallback reason. Legacy clients that explicitly send a default still work. A metadata-less older server keeps its returned settings as provisional reset values; controls keep legacy safe ranges until metadata is available.

## Provisional decisions

- **D1 (2026-11-06, provisional, tbd):** A separate metadata endpoint preserves the old settings body. Alternative: version the existing response.
- **D2 (2026-11-06, provisional, tbd):** Fractional scroll sensitivity is persisted as floating point. Alternative: integer-only controls.
- **D3 (2026-11-06, provisional, tbd):** Failed optimistic updates restore the last confirmed settings, and display the server error at the settings section level. Alternative: field-level inline markers.

## Architecture impact

Existing `splitty.core.config` owns defaults, limits and validation; `splitty.core.shell_profiles` owns fallback. Existing `splitty.server.service` and `splitty.server.http` expose metadata and broadcast changes through `splitty.server.client_session` and `splitty.core.protocol`. These relationships already appear in `architecture/*.c4`; no new component or relationship is needed. `docs/architecture.md` gains the settings authority and event flow concept.

## Risks / Trade-offs

- A settings event can race a local pending update → a successful response replaces the optimistic value with the authoritative whole object; failures reload/restore the last confirmed object.
- Older servers lack metadata and events → their settings remain editable with compatibility bounds and explicit reload behavior.
- A known shell profile can be unavailable only on the server's platform → fallback is server-only and reports a stable reason.

## Migration Plan

New clients probe metadata and retain compatibility with old GET/PATCH; old clients ignore the new event and endpoint. Existing integer TOML scroll values deserialize into floating point settings, and removed terminal keys are ignored on load. Rollback uses the prior binary and existing config file; fractional values may be reset by that prior binary.
