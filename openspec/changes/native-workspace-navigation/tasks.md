# Tasks

## 1. Local viewport state

- [x] 1.1 Add the per-Screen maximize reference and canonical presentation projection to the existing client workspace model. Extend unit tests for unchanged authoritative layout, selected-tab retention, independent Screen choices, invalidated group identity, focus fallback, and restoring after admitted split/move or explicit focus outside; failed admission must preserve the previous view choice.
- [x] 1.2 Route maximize/restore through the native worker and use the shared projection for visible pane claims. Extend existing real-service in-process tests to verify that hiding/restoring groups preserves session identity and output, withdraws hidden size claims, republishes restored sizes, and leaves full-scope close assessment intact.

## 2. Native navigation

- [x] 2.1 Add maximize/restore controls, Mod+M, Ctrl+Shift+Space and Mod+Shift+1–9 using the existing workspace command path. Extend focused unit tests for platform modifiers, rejected extra modifiers and normal terminal input; verify raw Shift and layout-aware US/German shifted digits, including ambiguous symbols and unknown-layout pass-through. Verify unavailable indexes, empty workspace and editor/modal focus priority through the actual-window driver, where focus and native dialog ownership can be observed.
- [x] 2.2 Extend the existing real-window driver with maximize/restore, tab selection/insertion, hidden output, split/move restoration, Screen switching and actual new/direct-Screen key gestures. Verify stable PTY identities, close cancellation including a busy hidden group, and normal owned cleanup. A native-window check is required for geometry, focus and platform key delivery, which model tests cannot establish.
- [x] 2.3 Update existing architecture component descriptions and local viewport ownership in `docs/architecture.md`, plus native usage/evidence in `docs/gpui-spike.md`; verify the focused architecture check and document exercised platforms and remaining release-parity limits.

## 3. Focused integration

- [x] 3.1 Run formatting, targeted client-core/GPUI unit and in-process tests, targeted Clippy, the native navigation interaction check, hardware workspace smokes and strict OpenSpec validation. Record results and limits. Broad workspace/CI and platform release gates remain deferred beyond this native increment.
