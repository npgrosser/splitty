# Tasks

## 1. Defaults and names

- [x] 1.1 Server: default `appearance.theme`, `themeDark` and `themeLight`, the fallback theme, and the names of the three renamed themes
- [x] 1.2 Native catalog: default UI themes and default palettes; the theme set shows the new names under the old identifiers
- [x] 1.3 Palette files take a display name from a `# name:` comment; the three renamed palettes carry one
- [x] 1.4 Unit tests: defaults of new settings, the fallback of an unknown theme, the palette comment (scenarios "First start", "Settings from before the default changed", "Palette with a display name"; extends the settings, theme and palette tests)

## 2. Docs

- [x] 2.1 `docs/themes.md` names the default and the renamed themes; `openspec/open-decisions.md`: CDT-1 to CDT-4

## 3. Checks

- [x] 3.1 Targeted: `cargo test -p splitty-core`, `-p splitty-client-core`, `-p splitty-server --lib`, `cargo test -p splitty-gpui --features ui-test-support`
- [x] 3.2 Full check before the merge: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `openspec validate --specs`
