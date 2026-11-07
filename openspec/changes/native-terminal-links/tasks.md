# Tasks

## 1. Target resolution and policy

- [ ] 1.1 Add bounded owned OSC 8 and plain-text link hits to the terminal adapter; extend adapter unit tests for explicit-target precedence, wraps/history/alternate screen/reflow, Unicode and wide cells, punctuation, malformed targets and extraction limits, with compile-time ABI checks for new native calls.
- [ ] 1.2 Add a pure parsed HTTP(S) target type in client-core; unit-test normalization, forbidden schemes, relative/missing-host/control-character inputs and raw/normalized length bounds. Update Cargo dependencies without unrelated lockfile upgrades.
- [ ] 1.3 Update the architecture model and native terminal concept documentation for extraction, policy and platform effects; verify the architecture test passes.

## 2. Gesture and presentation

- [ ] 2.1 Implement target preview and captured Ctrl/Cmd-click with fresh release validation, lifecycle cancellation and bounded hover reuse; extend pointer unit tests for changed target, dragging/outside release, geometry/focus cancellation and preserved selection/application routes.
- [ ] 2.2 Add a private real-window smoke with an isolated OS-opener spy; verify OSC 8 and plain/soft-wrapped links, rejected schemes, no automatic/ordinary-click activation, changed-target cancellation, exact application input and clean shutdown. A real window is required to exercise pointer dispatch, focus, geometry and the platform handoff that in-process tests cannot establish; inspect the feedback screenshot.

## 3. Integration

- [ ] 3.1 Run affected client-core/native unit tests, native all-target Clippy, formatting, diff checks, OpenSpec validation and exact-frozen-binary Wayland/X11 GPU workspace smoke. Record precise evidence and limitations, then refresh the preview. Full-workspace merge CI is deferred outside this evaluation increment.
