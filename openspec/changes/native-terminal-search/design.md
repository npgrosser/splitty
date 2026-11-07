# Design

## Context

See proposal.md. The pinned terminal library already exposes literal ASCII-insensitive incremental search, tracked selected matches and viewport matches. It is thread-confined with the terminal engine. Measurements found roughly 395,000 matches at 80 columns and 2 million at 400 columns for a one-character dense query; uncapped storage approached 833 MiB. Catch-up after 5,000 unobserved lines reached 821 ms. Incremental calls alone are not a latency bound.

## Goals / Non-Goals

**Goals:** full retained-history scope, bounded search storage, transparent incomplete/failure states, input isolation and correct cell-based highlights.

**Non-Goals:** a second terminal text model, moving the engine to another thread, protocol search or a new shared-client abstraction for a view-local operation.

## Decisions

- Keep search ownership in a nested terminal adapter module. Every native result is converted under one engine borrow into owned viewport row spans; native references never escape. Search is freed before terminal reset/destruction.
- Use a pointer-stable custom allocator with a 16 MiB budget, checked alignment and a sticky allocation-failure flag. Check it after every native search call, including result getters. Native code can swallow allocation failures, so a successful return alone is insufficient. Registry/pin bookkeeping remains terminal-owned and small. Pair this with a 20,000-match guard before costly navigation/getters. Either failure drops the index and presents an explicit refine-query state, without treating partial results as complete.
- Limit the query to 1,024 UTF-8 bytes; reject excessive queries without truncation. Literal matching folds ASCII case only; non-ASCII bytes remain exact. Do not expose unsupported regex/case/word options.
- Perform at most one feed and one history tick per scheduled UI turn, yielding between turns. Coalesce mutations; stop scheduling once complete until data, viewport or size changes. Never call the synchronous full-history search routine. This is a scheduling policy, not a guaranteed maximum duration for each native call.
- Maintain at most one active search index per window, for the focused visible pane. Retain per-terminal query/open state when inactive, but release the index and rebuild on activation. This bounds aggregate storage and prevents hidden catch-up. A limit failure stays stopped until the query changes or a deliberate reopen; background output must not cause repeated allocation failures.
- Use the existing text-input widget in a nested terminal UI module. Ctrl/Cmd+F opens, Enter/Shift+Enter navigate older/newer matches with wrap, Escape closes and restores terminal input. Workspace shortcuts defer to the search editor. Editor pointer events cannot reach terminal mouse reporting.
- Paint owned search spans between cached backgrounds and glyphs, excluding locally selected cells. Use screen coordinates then clip to the viewport, preserving matches crossing its boundaries and wide/combining characters. Search navigation does not replace clipboard selection. Refresh retains the tracked navigation anchor; query, resize, reset and index disposal may reset it.

Alternatives: uncapped native search fails measured memory/latency expectations; silently searching a suffix hides missing results; maintaining hidden indexes adds ongoing work; a Rust text snapshot duplicates terminal layout semantics; sharing live native pointers with a worker violates the present ownership boundary.

## Architecture impact

Extend existing `splitty_gpui::terminal_engine`, `splitty_gpui::terminal` and workspace focus integration. Update their descriptions in `architecture/gpui.c4` and the native client section of `docs/architecture.md`; add no top-level component or relationship. The service, mux reducer, `splitty-client`, transport and web frontend remain unchanged.

## Provisional decisions

- **NF1 (2026-11-07, provisional, tbd):** literal ASCII-insensitive full-history search and 1,024-byte query admission, without advanced matching modes.
- **NF2 (2026-11-07, provisional, tbd):** 16 MiB per index and 20,000 matches, with explicit stopped failure rather than partial results; thresholds are measurable policy, not terminal-library guarantees.
- **NF3 (2026-11-07, provisional, tbd):** one active index per window; remember query/open state per terminal and rebuild on reactivation, resetting navigation anchor.

## Risks / Trade-offs

- A native call can still exceed a frame budget → measure capped dense queries, output, navigation, resize and free; do not claim a hard real-time bound.
- Query/result changes race with new terminal output → all native access stays on the engine's owning thread, mutations invalidate owned highlight snapshots, and tests cover output, resize and alternate-screen switches.
- A broad query can fail even with useful partial matches → report the limit explicitly and allow query refinement; preserve the terminal and clipboard selection.

## Migration Plan

No persistent data or wire migration. Build and test on the native feature branch, retain the prior frozen preview until actual-window checks pass. Reverting this increment removes only local search.
