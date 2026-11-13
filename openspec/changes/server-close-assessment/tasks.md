# Tasks

## 1. Server

- [ ] 1.1 Close assessment over the workspace state in the mux (scope: panes of a screen, screens, workspace; entries with running, starting, scratchpad, annotations; totals and `confirm`); job panes not counted as running
- [ ] 1.2 `mux.assess_close` session message answered by `mux.close_assessed` on the same connection; `close-assessment-v1` in `mux.welcome`; the message admitted as a session operation
- [ ] 1.3 `apply_template` onto a non-empty existing screen requires `replaceConfirmed: true`
- [ ] 1.4 Tests (unit): assessment of busy, starting, scratchpad, annotations, idle, job pane, workspace and unknown targets; template replacement refused without the flag and accepted with it, creation unaffected
- [ ] 1.5 Tests (in-process): the mux handler answers `mux.assess_close` with the request ID, a malformed scope with `error`; the welcome lists the capability; a replacement without the flag yields `mux.op_failed`

## 2. Web client

- [ ] 2.1 Mux client: `close-assessment-v1` flag, `assessClose` request with timeout and rejection on disconnect; the answer belongs to its connection
- [ ] 2.2 Close guard: pane, tabs and screen close ask from the assessment of the screen's own connection, generic confirmation without the capability or on failure; quit and the distribution switch assess the local workspace; the own busy, scratchpad and annotation rules removed; `beforeunload` no longer asks
- [ ] 2.3 Template application onto the current screen assesses it and asks before replacing, then sends `replaceConfirmed: true`
- [ ] 2.4 Tests (vitest): dialog decision from an assessment (ask, close at once, generic without capability and on failure), quit impact, template replacement asking and the flag; mux client request/answer and disconnect

## 3. Native client

- [ ] 3.1 Client core: connection request `mux.assess_close`, source runtime answers it as an update (bounded, failed on disconnect, unsupported without the capability); close plans carry the server impact; the client's own busy and starting rules removed, its pending-state rules kept
- [ ] 3.2 Native window: pane, tabs, Screen and window close wait for the assessment and word the prompt from it, including scratchpads and annotations; template review shows the assessed impact; apply sends `replaceConfirmed: true`
- [ ] 3.3 Tests (unit, client core): mapping of an assessment to the plan's confirmation and counts, per-pane split of a tabs assessment, unsupported and failed assessment ask; template replacement request carries the flag
- [ ] 3.4 Tests (view, `ui-test-support`): existing close and template tests answer the assessment; an idle pane with scratchpad text asks

## 4. Docs

- [ ] 4.1 `docs/protocol.ts`: `mux.assess_close`, `mux.close_assessed`, `replaceConfirmed`, the capability
- [ ] 4.2 `docs/architecture.md`: where the close assessment lives; `docs/native-ui-handoff.md` where the native close flow is described
- [ ] 4.3 `openspec/open-decisions.md`: SCA-2 to SCA-6

## 5. Checks

- [ ] 5.1 Targeted tests of the touched modules
- [ ] 5.2 Full check: `cargo fmt --all --check`, `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`, `cargo test --workspace --exclude splitty-gui`, `cargo test -p splitty-gpui --features ui-test-support`, `pnpm check`, `pnpm test`, `pnpm build` in `frontend/`, `openspec validate --specs`
