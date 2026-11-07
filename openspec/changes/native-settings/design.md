# Design

## Context

See [proposal.md](proposal.md). `HomeConfig` already persists the candidate fields and the native catalog exposes `home.get`, `home.metadata`, `home.update`, `theme.get`, and `theme.list`; server validation, atomic persistence, and `settings.changed` are shared with the web client. `LocalServiceClient` currently owns a separate API stream for appearance/profile reads and bounded Drawer/zoom writes. Its appearance snapshot contains only Appearance and Drawer settings, and the GPUI workspace has no Settings editor. Native colors currently come from the resolved UI theme; `terminal.theme` is saved but is not applied by GPUI. GPUI Kit 0.9 registers two built-in defaults (`Default Light` and `Default Dark`), can load additional `ThemeSet` JSON, and exposes semantic token updates. The product's theme resolver owns the existing persisted theme names and colors.

## Goals / Non-Goals

**Goals:** Give the first editor a real, small set of effective controls; preserve one settings authority and one local API owner; separate terminal colors from UI colors; make native controls readable in each service theme; preserve source and request identity across asynchronous work.

**Non-Goals:** Create another local settings file or UI-only persisted theme setting; offer fields the native workspace ignores; change server storage, operation shapes, or remote settings ownership; make every web settings section available at once.

## Decisions

1. **Local API owner and typed snapshot.** Extend `crates/splitty-client-core/src/local_service.rs` and `appearance/local.rs` with bounded `home.metadata`, `theme.list`, and a fuller home presentation snapshot. Keep the actual `HomeConfig` and metadata values service-owned; expose a typed, bounded native editor model with effective values, defaults, limits, and catalog names. The local connection ID remains fixed when the selected Screen changes. A separate remote API owner is not used for these controls. Alternative: direct HTTP calls from GPUI would create a second authority and bypass the owned cancellation/admission path.

2. **Small field-specific writes.** Use `home.update` with one validated field path per action: `appearance.themeMode`, `appearance.theme`, `appearance.themeDark`, `appearance.themeLight`, `appearance.fontSize`, `appearance.fontFamily`, `appearance.zoom`, `appearance.zoomScope`, `terminal.theme`, `terminal.closeOnExit`, `drawer.pinned`, and `drawer.sidebarWidth`. Theme names come from `theme.list`; enum/numeric values and reset defaults come from `home.metadata`. `appearance.fontSize` follows published values (0 or 8–24), while `appearance.fontFamily` offers theme default (`""`) and bounded installed family names from GPUI's font inventory; the server still validates each patch. Extend `LocalServiceClient`'s single owned API stream with a FIFO of at most 16 accepted settings intents, one active request total and at most one accepted or active intent per field. Each carries the local connection ID and unique request ID. Reject overflow and duplicate-field admission before any request. On normal completion, release the field and refresh the latest generation; on explicit rejection, keep the confirmed value and show the field error; on uncertain completion, block that field until a successful `home.get` refresh reconciles it. Do not replay a write, debounce it, or show an optimistic saved state. On close, discard unsent intents, cancel/join the active owned stream and report an in-flight unknown outcome as uncertain. Alternative: sending whole settings objects risks replacing concurrent changes and is unnecessary for this subset.

3. **Palette selection.** Interpret `terminal.theme == "default"` as the active resolved UI theme's terminal tokens. Otherwise request the named theme with `theme.get` and use only its terminal tokens; keep chrome on the appearance-selected theme. Verify the returned resolved name matches the requested name before presenting it as saved, because the resolver can fall back. Maintain the last valid palette during read failure, then use UI-theme terminal tokens if there is no prior valid palette. A separate terminal palette generation prevents stale results from repainting terminals. Alternative: a new palette format or hardcoded GPUI palette duplicates the canonical theme resolver.

4. **Theme projection.** For this first editor, keep the service-resolved UI theme as the product palette. After `Theme::change` selects the correct Kit mode, project the validated UI colors into Kit 0.9 semantic roles via its public theme update seam, including background/foreground, sidebar, popover, border, muted, primary, focus and status roles. Existing `NativeAppearance` remains the source for terminal colors and direct chrome drawing until those call sites can use semantic Kit roles. Preserve readable Kit defaults for an invalid or unsupported token; log/report one bounded fallback notice. The bundled Kit names are not added to the persisted product selector in this increment; native Kit preset selection, including Kit's loadable `ThemeSet` themes, remains open for a later independent design. Alternative: using only Kit's light/dark defaults in this increment leaves controls visually disconnected from the selected product theme.

5. **Settings surface and lifecycle.** Add a Settings destination beside the existing Drawer/Hosts/Templates destinations, with Appearance, Terminal and Workspace groups only. Use Kit controlled Switch/Select/Slider/Button/Field components where current 0.9 signatures support them; hold retained input state and focus in the Settings view, not in render. Keep navigation, value, disabled/pending, error and reset states visible. Escape dismisses the top overlay and returns focus; opening Settings preserves the selected Screen and terminal sessions. Long theme lists own their scrolling region. A testable keyboard path must reach every control. This follows the Kit design and coding guides rather than copying web DOM/CSS controls.

## Architecture impact

`architecture/client-core.c4`: extend `local_service` and `appearance` descriptions/relationships for metadata, catalog and bounded settings updates using the existing `splitty.core.native_api`, `splitty.core.config`, and `splitty.core.theme` edges. `architecture/gpui.c4`: add a Settings presentation component under `gpui_workspace`; extend the `gpui_workspace` to `gpui_appearance` relationship for independent terminal tokens and semantic Kit projection. No crate or service dependency direction changes. In `docs/architecture.md`, update **Settings authority** and the native workspace appearance description to document the editor's local owner and palette separation. No reducer, PTY or wire concept changes.

## Risks / Trade-offs

- [A theme name was removed or resolver fell back] → compare the returned name, keep a valid displayed palette, show an error, and never rewrite saved config to hide the mismatch.
- [Mutation reply is lost after persistence] → mark outcome uncertain, refresh the local source, and never replay automatically.
- [Rapid edits race with `settings.changed`] → use the single 16-intent FIFO with one active write, reject duplicate-field admission, and apply only current connection/generation snapshots; render confirmed values until reconciliation.
- [Kit colors and direct chrome colors diverge] → project one resolved snapshot into both paths; test representative light/dark/custom token sets and audit contrast/semantic states in the native window.
- [Metadata from an older server is missing] → keep current values visible; disable controls whose valid choices/defaults cannot be established, while terminal use continues.

## Migration Plan

No file migration is needed. The native editor writes existing `config.toml` fields through the existing service API; older clients continue to receive complete `settings.changed` values. Rollback to the prior native executable leaves saved settings interpretable by the web and server clients. Do not change existing values simply by opening the editor.

## Provisional decisions

- **NS-1 (2026-11-07, provisional, tbd):** First native editor exposes the twelve listed effective fields across three groups; other web settings remain absent until native behavior exists.
- **NS-2 (2026-11-07, provisional, tbd):** `terminal.theme = "default"` inherits the active UI theme; a named theme supplies only terminal colors through the existing theme resolver.
- **NS-3 (2026-11-07, provisional, tbd):** This first editor uses the service-resolved appearance as its theme source and Kit defaults as semantic fallbacks; native Kit preset selection is a separate open increment, including the Kit's loadable `ThemeSet` themes.
