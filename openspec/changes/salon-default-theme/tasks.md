# Tasks

## 1. Themes

- [x] 1.1 Add Splitty Salon and Splitty Salon Light to the UI theme set, as terminal palettes and as core theme files; unit: the bundled catalog test loads both identifiers as a UI theme and a palette, and the core's bundled test parses both with their accent and their ANSI blue
- [x] 1.2 List them in `docs/themes.md` under "What ships"

## 2. Default

- [x] 2.1 Make `splitty-salon` and `splitty-salon-light` the default identifiers of the settings, of the app's catalog and of the core's fallback theme; unit: the settings tests for a first start and a stored earlier default, the core's fallback test, the catalog's default pair test; in-process: the control conformance test for an unknown theme name
- [x] 2.2 Describe the default in `docs/themes.md` ("What ships", "Choosing") and add SDT-1 and SDT-2 to `openspec/open-decisions.md`

## 3. Checks

- [x] 3.1 `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo test -p splitty-gpui --features ui-test-support`, `openspec validate --specs` and `openspec validate salon-default-theme` pass
