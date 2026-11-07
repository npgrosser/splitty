# Tasks

## 1. Terminal adapter

- [ ] 1.1 Implement owned search lifecycle, guarded allocation, query/match limits and incremental progress; unit-test normal full-history matching, resource failure/recovery, reset and drop, and verify capped dense cases with a temporary native benchmark including process memory.
- [ ] 1.2 Convert tracked native results to owned clipped cell spans and navigation; unit-test Unicode/wide/combining cells, soft wraps, viewport-crossing highlights, navigation wrap and retained anchor, selection independence, output, resize and screen-buffer changes.

## 2. Native presentation

- [ ] 2.1 Add per-terminal search editing, result state and yielding scheduling in a nested UI module; unit-test pure result/input policy where useful and smoke-test Ctrl/Cmd+F, Enter/Shift+Enter, Escape, query refinement and pointer/keyboard isolation from a real raw PTY. A window smoke is necessary because in-process engine tests cannot establish platform event propagation or focus.
- [ ] 2.2 Integrate focused-pane index ownership and cached-row highlight composition; smoke-test tab/Screen switches, retained query, background snapshots, selection copy and terminal reset, and visually inspect the actual search overlay.
- [ ] 2.3 Update existing architecture component descriptions, native client documentation and preview instructions with search scope/limits and measured evidence; verify architecture tests and documented invocation.

## 3. Integration

- [ ] 3.1 Run targeted native tests, all-target native Clippy, formatting and architecture validation; run actual-window search acceptance and both supported local GPU startup paths against the exact frozen candidate, and validate changed specifications. Full-workspace CI remains deferred for the native MVP; do not claim a full-workspace pass.
- [ ] 3.2 Integrate the verified increment on the native feature branch, sync/archive its specifications, commit and push, and refresh the preview metadata with the exact tested binary hash; verify main is unchanged and the tracked worktree is clean.
