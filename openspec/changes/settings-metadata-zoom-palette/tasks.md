# Tasks

## 1. Server

- [x] 1.1 Zoom levels in the settings core: one list, published as the `values` of `appearance.zoom` with its bounds and without `step`; stored levels outside the bounds fall back to 1.0; a settings update whose numeric value is not one of a field's listed `values` is refused, comparing numbers by value
- [x] 1.2 Palette rule in the theme core (`terminal.theme`, then the palette for the mode, else the theme's own); `theme.get` / `GET /api/home/theme` answer with the chosen palette's terminal colors and `terminalPalette`
- [x] 1.3 Tests (unit): metadata lists the levels; an update to 1.3 is refused and to 1 (an integer) and 0.67 accepted; the palette rule for every order and empty names; theme resolution with a named, a by-mode, a missing and the theme's own palette
- [x] 1.4 Tests (in-process): `theme.get` and `GET /api/home/theme` give equal answers with a stored `terminal.theme` and `terminalPalette` (extends the existing conformance test)

## 2. Web client

- [x] 2.1 `zoom.ts` steps through the levels of the settings metadata (only a reset without them); the Appearance zoom choices from the same list; the zoomed terminal font stays rounded
- [x] 2.2 Settings changes of `terminal.theme`, `terminal.themeDark` or `terminal.themeLight` reload the theme
- [x] 2.3 Tests (vitest): steps from a metadata list (Levels from the server, Level between the listed ones, Bounded steps), no steps without metadata, Rounded terminal font size; a palette settings change reloads the theme

## 3. Native client

- [x] 3.1 Client core: zoom levels from the settings limits (the server's `values`, else the core's levels within the service's bounds); `Zoom` validates against them, steps through them and rounds the terminal font scale with the `terminals` scope; the zoom executor reads the metadata with the settings; the fallback limits list the levels
- [x] 3.2 Client core: the palette rule of the server's core for the native window
- [x] 3.3 GPUI: zoom levels from the appearance snapshot for steps and display; the terminal metrics take any finite positive scale; the terminal palette choice uses the shared rule
- [x] 3.4 Tests (unit, in-process): zoom steps through server levels and older services' levels, display of a level beyond 3.0, rounding (client core unit); a zoom batch against a service whose metadata widens the levels (client core in-process, extends `local_service/zoom_tests.rs`); settings editor offers the listed levels (GPUI unit); palette choice (existing GPUI appearance tests)

## 4. Docs

- [x] 4.1 `docs/protocol.ts`: settings metadata zoom levels and their validation, `terminalPalette` and the `terminal` colors of `theme.get`
- [x] 4.2 `docs/architecture.md` (zoom levels, rounding, palette rule); `openspec/open-decisions.md` bullet for SMZ-1 to SMZ-5
- [x] 4.3 Architecture model: `client_zoom -> client_settings`, `client_zoom -> config`, `client_settings -> config`, `appearance -> theme` (client core), `gpui_appearance -> client_settings` (GPUI); `docs/themes.md` notes that the browser UI follows the palette settings

## 5. Checks

- [x] 5.1 Targeted tests of what the change touched
- [x] 5.2 Full check: `cargo fmt --all --check`, `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`, `cargo test --workspace --exclude splitty-gui`, `cargo test -p splitty-gpui --features ui-test-support`, in `frontend/` `pnpm check`, `pnpm test` and `pnpm build`, `openspec validate --specs`
