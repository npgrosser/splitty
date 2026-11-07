# Tasks

## 1. Shared byte input

- [ ] 1.1 Add capability, allowlist and bounded charged base64 input dispatch reusing existing PTY writes; extend unit/in-process dispatcher tests for exact ordered bytes without attach, invalid/oversized rejection, unknown sessions and write failure, and synchronize docs/protocol.ts and frontend protocol types.
- [ ] 1.2 Carry bytes through native callback/worker/client with capability fallback and existing route/generation guards; extend unit/in-process client/runtime tests for old-server UTF-8 fallback, rejected binary input, attach/replay/stale routes, exact raw bytes and atomic bracketed paste.

## 2. Native pointer input

- [ ] 2.1 Extend pinned Ghostty FFI/ABI and owning engine with mode-aware pointer encoding and checked geometry; unit tests cover tracking modes, five formats, high-byte legacy coordinates, motion deduplication, mode transitions and geometry changes.
- [ ] 2.2 Translate GPUI button/motion/wheel events with Shift gesture latch, admitted-press tracking and lifecycle settlement; unit tests cover selection override, modifier changes, wheel routing, failed admission and cancelled/replaced gestures.
- [ ] 2.3 Add isolated real-window smoke fixture for application press/release/motion/wheel, legacy high bytes, Shift selection/copy/paste and tracking off; a real window is required to verify event capture, hit testing and private clipboard interaction. Document commands, evidence and platform limits in docs/gpui-spike.md.

## 3. Integration and architecture

- [ ] 3.1 Update architecture component descriptions and protocol dependency plus docs/architecture.md; verify architecture integration tests and protocol type checking.
- [ ] 3.2 Run targeted core/server/client/GPUI tests and Clippy, cargo fmt --all --check, pointer smoke and hardware Wayland/X11 workspace smoke; validate the change and all main specs. Full workspace CI, packaging and other-platform builds remain deferred for this feature branch experiment.
