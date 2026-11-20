# Tasks

## 1. Tokens and resolution

- [x] 1.1 Parse `bevel`, `control-radius`, `field`, `active-text`, `menu-hover` and `title-bar` in the `splitty` object; resolve them into the appearance with their derivations, each value validated alone
- [x] 1.2 Edge painter: reliefs, sides, flat fallback
- [x] 1.3 Unit tests: token parsing and bounds, partly unsupported `title-bar`, derived highlight text, painter geometry (scenarios "Unsupported chrome token", "Selection color the theme's text does not read on", "Partly unsupported title bar")

## 2. Title bar

- [x] 2.1 Resolve title-bar colors by `titleBarStyle` and shape by `windowDecoration`; frame corner from the theme without a minimum
- [x] 2.2 Draw fill, inactive colors, alignment, weight and the three caption button styles on Linux, Windows and macOS
- [x] 2.3 Unit tests of the resolution rules; in-process test that each button style routes close through the owner (extends the title bar's close test; scenario "Theme title bar under each decoration")

## 3. Workspace chrome

- [x] 3.1 Corners follow the theme in every view; no minimums
- [x] 3.2 Selected entries use `active-text`; menu rows use `menu-hover` and readable text
- [x] 3.3 Borders go through the edge painter by element role; text fields take `field`
- [x] 3.4 In-process view tests: drawer, context menu and tab bar render with a beveled square theme, selected text uses `active-text` (scenario "Theme with 3D edges and square corners")

## 4. Toolkit

- [x] 4.1 The vendored toolkit theme carries bevel and field background; buttons, fields, checkboxes, menus, popovers and dialogs draw them; the local patch is recorded with the other vendored changes
- [x] 4.2 Unit tests of the toolkit painter; in-process test that a beveled theme renders button and field

## 5. Themes and docs

- [x] 5.1 `ui/windows.json` with Windows 11 Dark, Windows 11 Light and Windows Classic and their terminal palettes; unit tests that they resolve with their shapes and palettes (extends the bundled-catalog tests)
- [x] 5.2 `docs/themes.md`: token table, what ships, decoration section; `openspec/open-decisions.md`: ENT-1 to ENT-8

## 6. Checks

- [x] 6.1 Targeted: `cargo test -p splitty-gpui --features ui-test-support`, the toolkit's tests
- [x] 6.2 Full check before the merge: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `openspec validate --specs`
