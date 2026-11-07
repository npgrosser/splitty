# Design

## Context

See proposal.md for motivation and the parent migration's terminal engine decision for the source/API review. The isolated native dependency probe already built and linked Ghostty 1.0.0 with Zig 0.17.0 on Linux. The current GPUI view owns Alacritty directly, shapes each visible character on every frame, and is invalidated by a 16 ms whole-window poll. The optional CEF message loop currently runs during rendering, so removing idle invalidation requires moving that polling to its explicit scheduling path.

Existing `client-connections` guarantees retain service ownership, generation-scoped traffic, replay suppression and cleanup. This change alters no wire or release behavior.

## Goals / Non-Goals

Own Ghostty resources safely, preserve the current probe's terminal functionality, avoid repeated shaping of unchanged content and stop needless idle painting. Use concrete terminal-local types with no generic provider trait or duplicated scrollback/grid. Service-driven workspace layout follows in a separate increment after this foundation.

## Decisions

1. Use `libghostty-vt` 1.0.0 at commit `0fa3d577a5614150ff8e2c152de5fb9d5c6d2dec`, matching headers and Zig 0.17.0. An explicit setup script verifies source, dependency and toolchain archive hashes and installs to a user-selected/cache prefix. Cargo checks the prefix/version and statically links the library; it never downloads or executes setup implicitly. Curated private declarations cover only the used C API and avoid a libclang runtime build dependency. Validate ABI-sensitive types and declared functions against the pinned headers.
2. A UI-free terminal engine module owns the terminal, callback allocation, input encoders and render snapshot. It remains thread-confined. Safe frame/row/cell borrows prevent snapshot mutation while borrowed. Native selection references never cross mutation; key UTF-8 storage remains valid for the event's lifetime. All unsafe operations stay in this module with their safety invariants.
3. Collect protocol replies in a bounded per-operation buffer, suppressed throughout replay and discarded before live data resumes. Paste output is submitted as one logical bounded message to preserve framing under channel pressure. Retain current paste sanitization, manual copy/paste policy and the 64 KiB input ceiling. Do not install clipboard read/write, file or shared-memory image callbacks. Session generation stays in the view/connection wiring. Reset reconstructs native state and clears pending effects.
4. The view handles focus, geometry, GPUI text input and drawing. Cache shaped runs for visible rows, keyed by engine row identity/change information and presentation metrics; invalidate on reset, resize, font/style or selection changes. Draw the complete visible viewport on every requested paint, reusing shaped content instead of assuming immediate painting retains clean rows.
5. Window polling can remain bounded for this increment, but not notify unconditionally. Poll the optional offscreen engine independently from render and notify only when its image or visible window/document state changes. Preserve smoke frame acknowledgements and service cleanup. Measure the actual optimized executable; no inference from upstream benchmarks.

## Architecture impact

In `architecture/gpui.c4`, retain `terminal` as native presentation and add `terminal_engine` for UI-independent Ghostty ownership. `terminal -> terminal_engine` is the only application dependency on that engine; app/session wiring continues to use `terminal` and client-core. Register the new module from `gpui_main`. No Ghostty types enter client-core, mux or workspace APIs. Update the native-probe concept in `docs/architecture.md`; document native dependency setup and retain the original spike evidence as historical.

## Risks / Trade-offs

- C ownership/lifetime mistakes: private unsafe boundary, scoped borrowing, callback copies, explicit destructor order and focused lifecycle tests.
- Static libraries/toolchain differ across targets: pin setup inputs, preserve Linux/macOS/native Windows CI checks and state which targets were actually executed. Windows client libraries are separate from the WSL server.
- Emulator behavior differs: exercise split UTF-8, replies/replay, control/application-cursor keys, paste framing, selection, scrollback, Unicode and resize. Extended input, images and synchronized-output rendering remain outside the current probe's documented scope.
- Software graphics and debug builds distort performance: record profile/backend/device and measure idle, input, output and scroll separately; retain raw results and avoid universal performance claims.

## Provisional decisions

- NG-1 (2026-11-07, provisional, tbd): keep terminal parsing and rendering state on the UI thread with bounded delivery for this increment; improve scheduling/caching first and move parsing only if measurements justify the additional ownership handoff.
- NG-2 (2026-11-07, provisional, tbd): use static Ghostty linking from an explicit setup prefix and curated private C declarations; no implicit Cargo downloads or bindgen/libclang dependency.

## Migration Plan

Build the engine boundary and native dependency setup, adapt the existing terminal, then validate both native display backends and optional CEF. Run targeted tests and one full repository check. The released Electron/Tauri applications remain available throughout. Reverting the development-only change restores the Alacritty baseline without migrating data.
