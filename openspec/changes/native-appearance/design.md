# Native appearance design

## Context

The local service already implements `home.get`, `theme.get`, validation, persistence and theme resolution. The native session client accepts Text/Binary events; the low-level API request helper discards unrelated frames. Native terminals currently use 15 px text, 21 px rows and fixed colors. Theme tokens are strings, including CSS-oriented values. See proposal.md for scope.

Existing settings semantics distinguish UI and terminal fonts: `frontend/src/App.svelte` maps `appearance.fontSize` and `appearance.fontFamily` to chrome CSS variables, and the Appearance settings section offers System Sans. Both browser emulator adapters instead take terminal font size/family from `TerminalConfig`. `terminal_mount.ts` supplies terminal colors from the resolved appearance theme through `getXtermTheme`; the separate `terminal.theme` control currently has no runtime consumer. The native adapter preserves these meanings rather than assigning the chrome settings a new terminal meaning.

## Goals / Non-Goals

Keep settings authority in the service, preserve byte budgets through UI consumption, and make refresh independent of terminal delivery. No generic API framework, settings mutation, CSS interpreter, new wire format, new dependency or cross-process settings synchronization is introduced.

## Decisions

1. Add a UI-free appearance module using the embedded adapter's existing Connector to open one API-only stream. A session's source connection identity binds refreshes and returned snapshots. Never consume API replies on the live session stream or expose ServiceState to the UI.
2. The reader owns one client, at most one pollable in-flight future and a coalesced dirty generation. A refresh request during a read supersedes that result and schedules one latest read. The worker selects the reader future alongside session events and commands; it never awaits an appearance deadline before continuing terminal work. Explicit finish cancels the pending request, recovers the client and awaits its cleanup. No spawned request tasks or forwarding queues are needed.
3. Request HomeGet, project its appearance fields, then request ThemeGet with the explicit fixed or OS-selected name. These existing reads are not an atomic server transaction; same-service change events mark an in-flight result stale. No global revision or backend barrier is invented. Return a typed snapshot retaining independent decoded-allocation leases; retain each reply charge until its replacement projection is charged and decoded. Match the existing 1 MiB API-body ceiling, limit the selected theme identifier to the resolver's 64 bytes and font-family input to 4 KiB before forming presentation/request data.
4. GPUI supplies light/dark preference and maps a bounded supported palette to native widgets. Apply checked hex RGB/RGBA, `rgb(...)`, `rgba(...)` and transparent colors where supported; reject malformed or unsupported values to native fallbacks. The terminal engine receives concrete RGB defaults and ANSI colors. Alpha colors require deliberate composition against the selected background.
5. Appearance font overrides apply only to workspace chrome. The zero font-size sentinel and empty family use resolved UI theme font defaults; invalid or unavailable theme values fall back to the working native UI font. A CSS family list is not a literal GPUI font name: choose an available family from a bounded comma-separated, optionally quoted list and map supported generic UI families deliberately. Terminal typography remains the existing native monospace font at 15 px text / 21 px rows. Do not derive terminal size from appearance fonts, introduce a terminal-font setting, activate dormant `terminal.theme`, or reinterpret xterm-specific metrics as native guarantees. Terminal palette changes invalidate cached rows without restarting terminals; any changed available bounds from chrome layout produce size reports only for visible panes. Reset/new terminals inherit the current colors.
6. Initial read failure retains working defaults and publishes status. Later failures retain the last valid snapshot. Repeated changes coalesce; no retry of a settings mutation exists because this increment never writes. Same-service events and OS scheme changes trigger refresh; separate embedded services have independent cached configuration, so cross-service live synchronization is explicitly absent.

## Architecture impact

Add `splitty.client_core.appearance` with edges to core config/theme types, native API transport, service operation catalog and transport accounting. Embedded owns construction; the native workspace runtime later owns polling and shutdown. GPUI workspace and terminal presentation consume the snapshot through a small native adapter; Ghostty stays settings- and transport-independent. Update architecture/client-core.c4, architecture/gpui.c4 and the native-workspace section of docs/architecture.md as integration lands.

## Risks / Trade-offs

- Separate API stream consumes bounded admission capacity; one stream per owned workspace, explicit close and existing Connector quota sharing limit it.
- Read responses may complete after newer settings; local generation checks discard them and coalesce the next read.
- Color/font strings are more expressive than native presentation; support a documented subset with visible fallback rather than silently persisting substitutions.
- Service construction currently loads and may migrate home settings. New reader tests use an explicit in-memory service fixture, avoiding the production constructor and any user configuration writes.

## Provisional decisions

- **NA-1** (2026-11-07, provisional, tbd): apply appearance font overrides to chrome, falling back to resolved UI theme defaults and then working native UI defaults. Terminal typography stays native monospace at 15 px text / 21 px rows; terminal-font configuration and display zoom remain separate.
- **NA-2** (2026-11-07, provisional, tbd): support chrome typography, terminal colors and semantic chrome colors; CSS layout, effects, dormant `terminal.theme` behavior and cross-service live synchronization are excluded.

## Migration Plan

Land the API reader and its tests first. Integrate runtime polling and GUI presentation after concurrent native navigation work. Keep runtime/GUI tasks incomplete until their behavior and actual-window evidence pass. Rollback is confined to the experimental native client; settings files and existing browser behavior remain compatible. Use focused tests and architecture validation for this native MVP increment; broader cutover/platform gates remain separate.

### Shared-reader verification

The first implementation stage passes six targeted appearance tests, client-core all-target Clippy with warnings denied, touched-file formatting checks, all architecture tests and strict change validation. The tests cover separate session delivery, retained decoded leases, stale generation suppression, cancellation, malformed/oversized or unaccountable responses, and explicit recovery after a failed read. Runtime polling, visual conversion and actual-window evidence remain open tasks.
