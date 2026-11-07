# Design

## Context

See proposal.md for motivation. The web Ghostty adapter exposes OSC 8 and detected URL providers through a Ctrl/Cmd activation gesture; desktop generic openers allow HTTP(S). The native adapter already owns terminal cells, retained history and pointer encoding, while the view owns pointer capture and selection. It has no hyperlink path. The existing Tauri navigation helper is tied to that shell and cannot serve as a native client dependency.

## Goals / Non-Goals

Keep emulator interpretation, client policy and platform effects separate. Return owned data, never retained Ghostty references. Preserve existing input routing and bound pointer-time work. The web remains a working reference; neither WASM sharing nor a web refactor is needed for this increment.

## Decisions

1. `Engine::link_at(col, row)` returns an owned `LinkHit { uri, kind }` (`Osc8` or `Detected`). It rejects out-of-bounds cells, copies explicit targets within one exclusive borrow and never retains native grid references. OSC 8 takes precedence even when malformed, oversized or disallowed; its visible label cannot become a fallback target.
2. Plain detection extracts the complete clicked logical line using row wrap metadata, including retained rows beyond the viewport. Blank cells separate text; wide tails map to their owning grapheme and wrap spacers add no bytes. Bound URI and line bytes to 16 KiB, the logical line to 128 rows and inspection to 80,000 cells. If completeness cannot be established within those limits, return no detected target. Use [linkify 0.11](https://docs.rs/linkify/0.11.0/linkify/) for URL boundaries and byte offsets with schemes required; this avoids owning a growing punctuation parser. Detection is not validation.
3. `splitty-client-core::external_links::WebUrl::parse` returns a bounded normalized absolute HTTP(S) URL using the existing locked `url` parser, rejects control characters and missing hosts, and performs no I/O. Native terminal interpretation does not depend on this policy module. GPUI alone calls the platform opener with the validated URL; no server command or output event can invoke it.
4. Ctrl or Cmd plus left press without Shift captures an allowed link locally, ahead of application mouse reporting. Ordinary clicks and Shift selection retain their existing routes. The captured route consumes its entire press/move/release sequence, even after cancellation. Release must be inside the same cell, without drag, with the same view/session and geometry, and freshly resolve to the captured URI and origin. Releasing the modifier before mouse-up does not redirect the admitted gesture. Output alone does not cancel a still-identical target; a fresh hit protects against changed output. Scroll, resize, hide, reset and focus/window loss invalidate captured activation.
5. Hover displays the allowed actual target and a modifier hint without changing selection or sending input. Hover reuse must be invalidated by terminal/viewport mutations independently of search state. Release always performs a fresh lookup; a cached hover cannot authorize opening. Link feedback must not intercept terminal gesture completion. No promise that the operating system successfully starts a browser is inferred from the asynchronous platform request.

## Architecture impact

Add `client_core.external_links` to `architecture/client-core.c4` and an edge from `gpui.terminal`. Extend descriptions of `gpui.terminal_engine` and `gpui.terminal` for owned link hits and deliberate activation. Document this separation in the native terminal section of `docs/architecture.md`. Third-party matching stays inside the engine adapter, URL policy stays UI-free, and there is no engine-to-client-core edge.

## Provisional decisions

- **NL-1** (2026-11-07, provisional, tbd): 16 KiB URI/logical-line bytes, 128 rows and 80,000 visited cells; oversized or incomplete detection yields no target rather than a prefix.
- **NL-2** (2026-11-07, provisional, tbd): Ctrl/Cmd-left-click without Shift admits local link activation even when an application requests mouse events; the admitted gesture remains local until completion or cancellation.

## Risks / Trade-offs

- Untracked native references can become invalid after mutation → consume them only within a synchronous engine borrow and copy all returned text.
- OSC 8 labels can differ from their targets → preview the parsed actual target and require explicit activation; generic opener remains HTTP(S)-only.
- Plain detection boundaries can differ from the web provider → exercise punctuation, Unicode, wraps and hard boundaries; do not claim exact matcher parity.
- Pointer-time scanning could compete with rendering → enforce limits before growth, reuse only current hover results and avoid scanning during painting.
- Browser startup is platform-dependent → real-window smoke substitutes only the OS opener using a private process environment; GPU smoke remains separate from input evidence.

## Migration Plan

No data migration. Build and evaluate on the native feature branch, retain the previous preview until exact-binary checks pass, and roll back by reverting this increment. Full merge CI remains outside the native evaluation scope.
