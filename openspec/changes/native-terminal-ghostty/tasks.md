## 1. Native dependency and owned engine

- [x] 1.1 Add version/hash-pinned explicit native dependency setup, static Cargo linking, matching private C declarations and CI/development setup.
- [x] 1.2 Implement the thread-confined owned engine, bounded per-operation effects, replay suppression/reset, resize/scrollback, input/paste, selection/copy and scoped render snapshots.
- [x] 1.3 Unit tests: exercise actual engine split UTF-8, live/replay replies and reset, resize, keyboard modes, paste framing/limits, selection under mutation and wide/combining render data.

## 2. Native presentation and scheduling

- [x] 2.1 Adapt the terminal view to Ghostty while preserving generation-scoped input, manual clipboard/IME integration, selection, scrolling and resize.
- [x] 2.2 Cache visible terminal shaping with explicit invalidation; draw complete requested frames without reshaping unchanged rows. Add focused unit coverage for cache reuse/invalidation.
- [x] 2.3 Remove unconditional window invalidation, poll CEF independently from painting, and retain document/render acknowledgement and shutdown behavior.
- [x] 2.4 Update the LikeC4 module relationships and native-probe/dependency documentation in the same change.

## 3. Verification and completion

- [x] 3.1 Run targeted engine/presentation unit tests and existing client-core in-process tests; run the existing native real-window smoke on Wayland and X11 with CEF disabled and enabled. Real-window smoke is necessary to exercise the graphics backend and optional engine composition.
- [x] 3.2 Measure optimized idle, input-to-paint, sustained output and scrolling with graphics device/profile recorded and CEF disabled/enabled; document observed improvements and remaining limits without claiming unverified platforms.
- [ ] 3.3 Run the full check once: cargo fmt --all --check; cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings; cargo test --workspace --exclude splitty-gui (including installed architecture dependencies); openspec validate --specs. Validate optional CEF feature compilation where touched.
- [ ] 3.4 Commit implementation, archive the internal change, retain main specifications unchanged, and carry results into the parent migration alongside the separately verified local-screen increment.
