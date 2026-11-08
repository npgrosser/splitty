# Tasks

Use targeted checks during implementation. The full gate in 4.2 remains deferred to the later merge checkpoint and stays unchecked until it actually runs.

## 1. Drawer and workspace motion

- [x] 1.1 Replace the Drawer overlay timer with a Kit transition channel and add a separate pinned workspace inset channel. Verify in an existing GPUI production-view in-process test that unpinned reveal leaves the viewport width unchanged, admitted pin moves both surfaces, rejected pin returns from the displayed position, repeated toggles do not add settings commands, and reduced motion settles immediately.
- [x] 1.2 Update the native workspace motion note in `docs/architecture.md` for the two Drawer presentation channels; verify it states that only the existing settings command changes persistence and that the terminal route remains selected by the workspace model.

## 2. Hover and split-drop feedback

- [x] 2.1 Animate Screen-block and terminal-text hover colors over 80 ms using stable Kit transition identities. Extend the existing Drawer production-view in-process test to verify whole-block hover, pane-only text hover, pointer departure and reduced motion without creating an action.
- [x] 2.2 Animate native split-drop highlight bounds using the existing measured target rectangles: 70 ms for a within-group selection and 300 ms for the viewport anchor when those target classes exist. Extend the existing drag production-view in-process test to verify both target classes, rapid target changes and cancellation submit no additional layout command; do not apply Dockview timings to unrelated Screen navigation.

## 3. Vertical Screen motion

- [x] 3.1 Establish a presentation-only mode for the retained terminal view using its existing painter, with fixed recorded geometry and no input handler, pointer or link listener, terminal resize, visible-size report or activity claim. Verify in a focused terminal in-process test that painting an outgoing pane leaves command and size-report counts unchanged. If this cannot be proved with the selected GPUI backend, keep this task and the Screen slide open rather than substituting screenshot capture or a second painter.
- [x] 3.2 Make the viewport render projection explicit about source, Screen and interactive versus presentation-only use; retain at most one outgoing Screen for 200 ms, ordered above or below by the Drawer list. Verify in an existing workspace production-view in-process test that only the current Screen receives input, focus, link dispatch, resize and visible routing; owner replacement, removal and leaving Screens discard the outgoing layer.
- [x] 3.3 Use stable Kit transition identity so rapid Screen A → B → A reversal starts at the displayed position; reduced motion switches immediately. Verify direction, bounded outgoing ownership and reversal in a production-view in-process test, then run one owned actual-window smoke to inspect composited movement because headless geometry alone cannot show visual clipping or overlap.
- [x] 3.4 Update `architecture/gpui.c4` descriptions for the existing workspace and terminal components and document the shared presentation-only painter in `docs/architecture.md`; verify the architecture check passes and the documentation names no second layout or route authority.

## 4. Integration checks

- [x] 4.1 Run targeted GPUI terminal and workspace in-process tests plus the owned motion smoke, and verify Drawer pin, drag and Screen transitions preserve the existing command counts and reduced-motion path.
- [ ] 4.2 Run `cargo fmt --all --check`, `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`, `cargo test --workspace --exclude splitty-gui`, and `openspec validate --specs` once before merge; verify all pass. Install architecture dependencies with `pnpm install --dir architecture` first if absent.

## Drawer and drop validation (2026-11-08)

- Targeted production Drawer UI integration suite: 6 passed, including pin rollback, intermediate geometry, rapid drop reversal, reduced motion and unchanged command admission. Strict GPUI all-target Clippy with `ui-test-support` passed.
- The retained terminal resized ten times over fifteen sampled 16 ms frames during the pinned inset transition; the test observed one pin write and no terminal input. Existing integer-grid checks suffice; no additional PTY debounce was introduced.
- The optimized candidate passed the owned Windows 18-stage workspace and normal-close smoke. A separate owned X11 window run on candidate `a62edcf4868162f15134c860f1d9c0a550e04f1f81f73803e3f3d1a1abc43e4f` captured unpin, overlay close/reveal and pin, then clicked the retained terminal and verified a fresh shell-output marker; application and Xvfb exited normally. Settled captures show no clipping or overlap. Intermediate captures settled too quickly to establish the animation trajectory; production-view geometry tests provide that evidence. Screen-slide tasks remain separate.

## Screen motion integration (2026-11-08)

- `cargo test -p splitty-gpui --no-default-features --features ui-test-support`: all 150 tests passed after integration, including passive painting, outgoing focus isolation, bounded/reversible Screen transitions, reduced motion and existing Drawer/drop command-count checks.
- `cargo clippy -p splitty-gpui --no-default-features --features ui-test-support --all-targets -- -D warnings`: passed. `cargo test -p splitty-architecture`: 9 architecture and 8 edge tests passed with the focused terminal key-context relationship and presentation descriptions.
- Owned Xvfb/software-renderer visual acceptance on debug candidate `903def7fbf90d845660739c64ba9f9ce98d9b576a2cc8d5dfd7f426ed7f97f78` shows full colored terminal content moving as adjacent clipped layers, reversal continuity and settled content. It establishes composition, not hardware performance or Windows timing. Details are recorded in `docs/gpui-spike.md`.
- The full merge gate in 4.2 remains deferred.
