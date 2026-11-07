# Design

## Context

See proposal.md for motivation. The existing web client defines thirteen zoom steps in `frontend/src/lib/zoom.ts`, persists an absolute appearance patch and distinguishes window and terminal scopes. Native appearance already reads the same settings but ignores zoom. Terminal geometry is currently spread across fixed 15 px font and 21 px row constants. Chrome mixes rem spacing with explicit pixel dimensions.

`appearance.font_size` and `font_family` are chrome settings, as the settings spec explicitly states. The web terminal's separate `/api/config` read has no current backend route (a recorded baseline finding); it is not evidence of service-owned terminal font fields. This change therefore uses existing zoom settings and does not invent new typography configuration.

## Goals / Non-Goals

Support both existing scopes completely in the native local workspace. Preserve terminal entities, history, authoritative layout and PTY ownership while scaling rendered content and matching input geometry. Keep pure client intent policy, settings I/O and presentation distinct. No display DPI override, speculative second settings store, web refactor or global serialization guarantee across independent clients.

## Decisions

1. Add a small UI-free `zoom` module with the existing thirteen steps, `ZoomAction` and validated level/scope projection. The web function and display-zoom spec are the behavior reference; this is native client policy, not a new backend authority or a claim of shared WASM execution. Unsupported/non-finite values use the established defaults.
2. Extend the existing one-owner auxiliary settings connection with bounded ordered zoom jobs, renaming its read-only-facing type to `LocalSettingsClient`. Read jobs retain their independent appearance/profile generations; mutation acknowledgements have separate request identities and are never discarded by those read generations. Alternate ready read and write work so neither repeated keys nor refreshes starve the other. Each zoom job reads current settings, computes one next step and submits only `{"appearance":{"zoom":...}}` once when needed. It then requests a fresh appearance read. No-op saturation still returns a confirmed level without writing.
3. A zoom job explicitly handles the native reply's `uncertain` flag and failures after mutation admission. Report an unknown outcome without retrying that action; reconcile through a fresh read before later queued intents. Later intents are separate accepted user actions, not retries. Reject foreign connection identities and capacity overflow visibly; retain API request ownership through polling cancellation and join it at shutdown. HomeGet followed by HomeUpdate is not atomic across independent clients: existing last-writer-wins behavior remains and full settings are never echoed back as a patch.
4. Handle Ctrl on non-macOS or Cmd on macOS plus `+`, `=`, `-`, `0` before terminal/editor routing, rejecting Alt or conflicting primary modifiers. Use the framework's logical keys without guessing unavailable physical key codes. The UI submits an identified intent and briefly reports a confirmed level or failure; it does not calculate or install a competing speculative persisted level. Shortcut keys never reach the terminal.
5. Project window scope through `window.set_rem_size(16 * level)` and convert static native chrome font sizes and dimensions to rems (`base_px / 16`). Terminal scope retains chrome at its base rem size. Measured bounds, drag ratios and absolute positions remain actual pixels. Scale tab-header hit testing, marker/divider widths, screen controls, rename/close/profile/search overlays and footer together; keep oversized toolbar/popup content reachable at 300%. Native OS-owned prompts follow platform scaling.
6. Centralize terminal font and row metrics at native defaults 15/21 multiplied by level in both scopes. Use one metrics value for shaping, glyph-cache validity, rows/columns, painting, selection/search, cursor/composition, pointer/link mapping, pixel-wheel conversion, mouse physical geometry and IME bounds. Zoom invalidates cached shapes and active gestures without resetting the engine or session. Rendering must not use new metrics with old measured geometry. Compare VT pixel cell metrics as well as grid size before resize: zoom or DPI can change pixel geometry while rounded columns/rows stay equal. Publish PTY dimensions only for currently shown panes after measurement; hidden entities retain desired metrics until shown.
7. Cancel active pane placement/divider gestures before applying new zoom, because their captured pixel bounds are obsolete. Preserve canonical workspace ratios, selected tabs, close/profile request identities and per-terminal search queries. Newly created and reactivated panes receive the current confirmed zoom.

## Architecture impact

Add `client_core.zoom` to `architecture/client-core.c4`, used by local settings jobs, native workspace commands and GPUI presentation. Update `local_settings` and `native_workspace` descriptions for owned mutation outcomes and fairness. Update `gpui.terminal` for coherent zoom metrics and `gpui_workspace` for confirmed native display scale. Describe settings I/O and geometry boundaries in the native sections of `docs/architecture.md`. The engine continues to accept grid/pixel geometry only and does not learn application zoom policy.

## Provisional decisions

- **NZ-1** (2026-11-07, provisional, tbd): admit at most 32 queued zoom intents beside one running auxiliary job, alternating ready read and mutation work. Overflow is visible and never silently drops an admitted action.
- **NZ-2** (2026-11-07, provisional, tbd): preserve native baseline typography at 15/21 px and scale it uniformly; window scope additionally scales chrome from a 16 px rem baseline. This introduces no general terminal-font fields and does not alter OS DPI.

## Risks / Trade-offs

- Queued actions and settings refreshes can race → compute each action from a fresh service read, retain mutation identity independently and refresh after each outcome; do not claim cross-client atomic increments.
- A lost reply can hide an applied save → expose uncertainty and reread, without automatic mutation retry.
- Mixed px/rem geometry can produce wrong hit targets → convert presentation constants and corresponding hit tests together; exercise both scopes and extreme levels in real windows.
- Hidden panes or unchanged row counts can leave stale VT pixel geometry → retain desired metrics and compare both size signatures at visible layout.
- Backend-specific keyboard/IME behavior → verify available Linux layouts, pointer and IME geometry; do not claim macOS/Windows or manual IME composition acceptance from Linux tests.

## Migration Plan

No settings-format migration. Existing saved zoom is honored on startup; default 1.0 retains current geometry. Keep the previous preview until a frozen candidate passes both scope checks, persisted restart and the hardware workspace smoke. Full merge CI remains outside this feature-branch evaluation.
