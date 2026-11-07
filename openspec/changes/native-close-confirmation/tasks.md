## 1. Close assessment and worker execution

- [x] 1.1 Add bounded close-plan assessment and validation under the existing workspace component. Extend unit tests for idle/busy/unresolved targets, hidden Screens, pending creations, replaced sessions, changed membership/generation, newly busy impact, harmless title changes and duplicate request identity; verify no stale plan produces a destructive intent.
- [x] 1.2 Route pane, Screen and owned-window close through the native worker's checked execution path, including explicit matching window approval and closing-state command rejection. Extend unit and real-service in-process tests for failed admission, stale approval/rejection, cancellation preserving PTYs, one accepted close, later queued mutations not executing, old snapshots followed by late unscoped close failures, ordered settlement before the next request, exactly-once owned cleanup, early startup close and verified cleanup completion after failure; existing screen/session tests remain passing.

## 2. Native confirmation and lifetime

- [x] 2.1 Connect pane buttons/shortcut, Screen close and cancellable window-close interception to the shared assessment. Keep the UI/update receiver alive until matching worker approval, with one prompt and restored focus on cancellation. Extend focused UI tests for repeated gestures, cancellation, removed prompt targets and mismatched request/generation approval; keep the legacy probe's separate behavior unchanged.
- [x] 2.2 Extend the native interaction smoke with an actual busy terminal: cancel and confirm pane/Screen closes, cancel window close with a busy hidden Screen, then confirm window close and verify owned sessions/processes terminate normally. A real-window smoke is necessary to validate native prompt routing, titlebar close veto and visible focus; model/in-process tests alone cannot establish those behaviors. Record the exercised platform and excluded forced/non-cancellable quit paths.
- [x] 2.3 Update `docs/architecture.md` and descriptions/actual relationships in `architecture/client-core.c4` and `architecture/gpui.c4` for close assessment, prompts and approved shutdown, retaining existing components. Verify the focused architecture test and document that no wire/backend policy changed.

## 3. Focused verification

- [x] 3.1 Run formatting, targeted model/runtime/UI unit and in-process tests, deterministic transport shutdown-race and foreign-budget regression tests, targeted Clippy, the native confirmation smoke and OpenSpec validation. Record results and limits; broad workspace/CI and release-parity gates are deferred beyond this bounded native increment.
