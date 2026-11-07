# Tasks

## 1. Native adapter

- [x] 1.1 Add the platform-aware UI-free client crate and native owned-service composition; preserve frame/decode/document budgets, connection identity, replay phase, mux session ownership and awaited shutdown. Add unit/in-process tests for budget/identity failures, create/attach/input/resize, reconnect without duplicate processes, viewer reads/annotations and independent service cleanup.

## 2. Native rendering

- [x] 2.1 Add the separately invoked GPUI executable with exact dependency pins, bounded UI/worker channels and explicit experimental/isolated scope. Implement service-backed terminal cells, colors, cursor, scrolling, selection, explicit clipboard input and native IME composition; test parser/input/replay suppression and size routing at unit/in-process level.
- [x] 2.2 Render service-read Markdown with inert resources and links, Unicode-safe selection/source mapping and server-stored annotation marks. Add unit/in-process tests for multibyte targets, annotation persistence through resync and safe rendering policies. Preserve the existing embedded-annotation close guard for interactive use.
- [x] 2.3 Implement the optional fixed-fixture offscreen engine probe with bounded frames, native clipping/overlay and no service authority. Test frame validation/isolation at unit level, attempt a real engine launch and record actual success or precise build/runtime failure; never substitute a fabricated frame as engine evidence.

## 3. Evidence and integration

- [x] 3.1 Launch the built native window through Linux Wayland and X11 where available; collect actual rendering evidence and a terminal/document/composition matrix, including sustained output, selection, resize, reattach, cleanup, clipboard, IME/dead keys and fonts. Mark unperformed macOS/Windows/hardware/manual checks explicitly unverified. A native-window smoke is justified because parser/in-process tests cannot establish platform rendering, overlays or input-method behavior.
- [x] 3.2 Update the LikeC4 containers, module relationships and views, architecture/development documentation, reproducible build/launch instructions, dependency choices, parent migration draft and roadmap with spike evidence and remaining cutover gates. Keep existing release clients and browser security in place; update CI prerequisites if workspace builds need them.

## 4. Validation

- [x] 4.1 Run targeted native adapter/rendering tests and architecture checks, then the full check once: cargo fmt --all --check; cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings; cargo test --workspace --exclude splitty-gui; openspec validate --specs and strict change validation. Run frontend check/test/build only if frontend files change. Archive only after the bounded spike tasks are complete; retain the full migration draft and its unverified platform gates.
