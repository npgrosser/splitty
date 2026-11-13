# Tasks

## 1. Events without a reader or a sender

- [ ] 1.1 Stop sending `osc133`: drop `ServerEvent::SemanticBoundary`, keep the notification engine's semantic hook in the dispatcher; update `docs/protocol.ts`, `docs/osc-133-semantic-prompts.md` and the web types. Verify with `cargo test -p splitty-core protocol::` (unit, the event's serialization test goes).
- [ ] 1.2 Remove the `notification` event: `ServerEvent::Notification`, the generic case in client-core `notifications.rs` and the web `notifications.ts`, their tests and `docs/protocol.ts`. Verify with a client-core unit test and a vitest case that a `notification` event adds nothing (unit).

## 2. Operations without a caller

- [ ] 2.1 Remove `stats.get`, `serverSettings.get` and `serverSettings.update` from the catalog, dispatch, `docs/native-operations.json` and `docs/protocol.ts`; move the smoke tests' authenticated probe from `/api/stats` to `/api/build`; extend the existing catalog test that unknown operations are refused with the removed names (in-process). Verify with `cargo test -p splitty-server --test http_surface` and the catalog tests.

## 3. Legacy web panes

- [ ] 3.1 Templates: drop `webview` panes when a template is deserialized (pane, slot entry, emptied slots and splits, emptied floating entries), remove `Pane::url`, the web-pane checks in validation and `skipped_web_count` with its notices and filters in core, client-core, gpui and the web client. Unit tests: a YAML file and a JSON request holding a web pane load as the terminal-only template; an all-web template fails to apply.
- [ ] 3.2 Persisted state: read the role `webview` as `terminal`, and on daemon restore close the panes `state.json` recorded as `webview`; drop `PaneRole::Webview`. Unit test: a `state.json` with a terminal and a web pane side by side restores the terminal alone with its identifiers; a Screen of only a web pane goes; a snapshot from an older host with a `webview` pane decodes with that pane as terminal.

## 4. One extraction path for windows

- [ ] 4.1 The native window extracts local Screens with `mux.extract_template` on the local connection, like host Screens; remove the `template.extract` path from client-core. Server: both entry points share one function; spec marks `template.extract` deprecated. Verify with the client-core and gpui template tests (in-process / view tests) and `cargo test -p splitty-server template`.

## 5. Wording and comments

- [ ] 5.1 Control API spec wording for command forwarding and op notifications (in the delta); comments on the client replay filters naming the server release that filters replays itself. Docs: `docs/architecture.md` template paragraphs.
- [ ] 5.2 Full checks from CLAUDE.md, `openspec validate --specs`.
