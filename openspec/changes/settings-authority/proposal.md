# Proposal

Part 3 of the sequence in `frontend-client-core` (design, "Sequence"); independent of parts 1, 2 and 4. Related: `pty-size-policy` D8 (the settings window edits only the local server's settings), `display-zoom` (zoom stored per server).

## Why

Settings live on the server (`config.toml`, `/api/home/settings`), but the window carries its own copy of their defaults, limits and rules, and the copies have drifted:

- **Defaults** are written twice. The TypeScript `defaults` and `mergeFromServer` repeat the Rust defaults, and `drawer.sidebarWidth` is 240 in the window and 200 on the server.
- **Limits** disagree. Slider ranges for font size, title bar height and scrollbar width differ from the server's `validate`.
- **Types** disagree. The xterm section sends fractional scroll sensitivities (steps of 0.1 and 0.5) into integer fields, so the update fails to parse, and the window keeps its optimistic value until a reload.
- **Validation** has a gap. The `editors` section is validated only in the window (`editor_preferences.ts`), not by the server.
- **Profiles** are resolved twice. The default shell profile is resolved in the window as well as on the server. The draft rules of the shells section repeat the server's profile update rules.
- **Changes** are not shared. Settings are loaded once at startup, so another window never sees a change.
- **Stale fields** remain. The server still validates the removed `link_target` and `webview_mode` fields.

## What Changes

- **Defaults and limits from the server.** It returns defaults and limits (ranges, steps, allowed values) with the settings. The settings window builds its controls and its reset from them instead of its own copy.
- **Complete validation.** The server validates every section, including `editors`. Types follow the controls: whether scroll sensitivities become fractional or the controls become integer is decided when this change is next. A rejected update returns which field failed. The window reverts its optimistic value.
- **Change event.** A change is pushed to every window of that server over its client connection, and each window applies it.
- **Profile fallback.** When a known profile is not available, the server falls back to the default profile and reports why. The window stops resolving the default profile itself.
- **Stale fields** leave validation.

## Capabilities

### New Capabilities

- `settings`: where settings live (per server), reading and updating them, defaults and limits, validation and its errors, the change event. No capability covers settings today.

### Modified Capabilities

- `terminal-sessions`: Shell Program Selection, the fallback for an unavailable profile.
- `mux-protocol`: the change event.
- `control-api`: the settings endpoints, if they are specced there.

## Impact

- **Core:** `config` (settings, validation, defaults and limits), `shell_profiles`.
- **Server:** the settings operations in `service`, their routes, and the client connection for the change event.
- **Frontend:** `lib/settings.ts` (defaults and merge removed), settings sections (bounds from the server), `shell_profiles.ts`, `editor_preferences.ts`.
- **Docs:** `docs/protocol.ts`.
- **Both transports:** new requests are added to the native operation catalog (`docs/native-operations.json`) as well as the HTTP routes, and new messages travel over the WebSocket and native connections alike (`native-client-transports`).

Out of scope: window-local preferences that are not server settings (renderer choice, drawer state, scratchpad font size); editing a host's settings from another machine; a settings UI redesign.

## Carried-over design notes

- 2026-11-04, `frontend-client-core` inventory:
  - Window defaults and merge: `settings.ts` (about lines 152–258). Server defaults and validation: `config/settings.rs` (about lines 313–555).
  - Fractional steps: `XtermSection.svelte` (about lines 128 and 139), sending into `i32` fields (`settings.rs` about lines 171–172).
  - Window-only editor sanitizer: `editor_preferences.ts` (about lines 14–22).
  - Default profile resolution: `shell_profiles.ts` (about lines 60–66) and `shell_profile_model.ts` repeat `default_profile` (`shell_profiles.rs` about lines 153–158). `resolve` errors for a known but unavailable ID (about lines 126–130).
