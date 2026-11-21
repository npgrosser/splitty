# Tasks

## 1. Defaults

- [x] 1.1 Server: the default `appearance.theme`, `themeDark` and `themeLight` are `splitty-dark`, `splitty-dark` and `splitty-light`, the default `themeMode` stays `fixed`, and the fallback theme is `splitty-dark`; verify with the unit tests of the settings (defaults, a first start without a settings file, a stored `splitty-classic` that stays: scenarios "First start", "Settings from before the default changed") and of the theme resolver (unknown theme), and the server's in-process conformance test (missing theme answers Splitty Modern)
- [x] 1.2 Native catalog: the default UI themes and default palettes are `splitty-dark` and `splitty-light`; verify with the unit tests of the catalog and the appearance (an unavailable theme on a light system shows `splitty-light`; Windows 11 Dark's palette is checked against the `splitty-classic` palette by name, Windows 11 Light takes the default light palette) and the in-process settings view test (the theme field shows Splitty Modern)
- [x] 1.3 Client core: default settings show `splitty-dark` on a dark and on a light system preference, and the pair `splitty-dark` / `splitty-light` once they follow the system (scenarios "First start", "Following the system without a chosen pair"); verify with the unit test of the theme name and the in-process appearance tests of the local service

## 2. Docs

- [x] 2.1 `docs/themes.md` names Splitty Modern as the default in a fixed theme mode and Splitty Modern Light as the palette of Windows 11 Light; `openspec/open-decisions.md` keeps CDT-2 and CDT-3 as they hold now and drops CDT-1 and CDT-4; verify by reading both against the code

## 3. Checks

- [x] 3.1 Targeted: `cargo test -p splitty-core`, `cargo test -p splitty-client-core`, `cargo test -p splitty-server --lib`, `cargo test -p splitty-gpui --features ui-test-support`
- [x] 3.2 Full check before the merge: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (runs the architecture check), `openspec validate --specs`
