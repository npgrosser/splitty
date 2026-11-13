# Tasks

## 1. Client-core

- [x] 1.1 Add `native_workspace/presence.rs`: window presence, the reports of one connection (welcome greeting, interaction at most every 5 s, idle, takeover), `size` event validation and the cell the window shares with the worker; unit tests for the throttle, idle, visibility, the greeting before attaches and the validation
- [x] 1.2 Drive it from `SourceRuntime`: read `pty-size-v1` at the welcome and greet before any attach, drop the `activity` on `attached`, send `hide` and `activity`/`idle` only with the capability, nudge the PTY by one row after an attach without it, validate `size` events of attached sessions and publish `Update::PtySize`; the window's interaction and hiding from the cell, and the `TakeOverSize` command, routed to the local source and the host sources
- [x] 1.3 In-process tests (two connections on a temp-dir embedded service): an untouched window that re-attaches does not take the size; a window that becomes hidden sends `idle` and the other client's size applies; takeover takes the size; a `size` event reaches the window as `Update::PtySize`

## 2. Native window

- [x] 2.1 Keep the terminal emulator at the PTY size of the latest `size` (the pane's until one arrives), report the pane's size, paint and hit-test the grid with its offset, mark the area outside a smaller grid, move a taller one so its cursor row stays visible, and show the size badge with its takeover
- [x] 2.2 Report interaction, visibility and focus from the workspace to the worker, pass size updates to the views (also to views created later)
- [x] 2.3 View tests (`--features ui-test-support`) for the letterbox and the takeover state, and unit tests of the marker layout

## 3. Records

- [x] 3.1 `docs/architecture.md` (native size display), `docs/native-ui-handoff.md`, relationship descriptions in `architecture/`, NPS-1 to NPS-3 in `openspec/open-decisions.md`

## 4. Checks

- [x] 4.1 `cargo test -p splitty-client-core`, `cargo test -p splitty-gpui --features ui-test-support`, `cargo fmt --all --check`, Clippy with `-D warnings` for the touched crates (gpui also with `ui-test-support`), the architecture check, `openspec validate --specs` and `openspec validate native-pty-size-presence`
