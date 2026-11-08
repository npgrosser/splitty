# Tasks

## 1. Bounded client notification policy

- [ ] 1.1 Add a UI-free notification ledger with bounded owned event fields, captured session identity, source-scoped ID deduplication, severity classification and at most 100 records. Verify unit tests cover enabled process/idle/generic events, zero/nonzero and user-initiated exits, malformed/oversized fields, duplicate IDs and FIFO eviction.
- [ ] 1.2 Admit JSON facts only for the current source token and event connection ID, independently of `execution_token` send readiness; resolve sessions through that source's exact pane/session relationship. Verify unit tests cover two sources with colliding IDs, a queued current-token fact after readiness revocation, a stale-token fact after replacement, and unknown/unscoped session ordering.
- [ ] 1.3 Keep bounded dedup/read history separate from visible badge bindings. Hide bindings on token replacement; preserve at most 100 records through empty/incomplete reset snapshots, then restore originally bound unread badges from fresh canonical current-token mapping only when `(SourceId, PaneId, SessionId)` matches, even when Screen changed. Never later bind an unknown-at-arrival event. Prune records on conclusive current-model pane removal/session replacement or source disposal. Verify unit tests cover the reset interval without false pruning, restoration without event replay, retained read flags, duplicate ID delivery, a moved pane, same PaneKey with a new session, conclusive removal, source removal and local-owner replacement.
- [ ] 1.4 Implement watched-pane admission, pane-read operations and count/title/severity queries without server effects. Verify unit tests cover focused watching, activation, return from hidden/unfocused state, severity priority and one-pane versus whole-Screen counts.
- [ ] 1.5 Add the new client-core module and relationships to `architecture/client-core.c4`, and describe ledger ownership, exact-token fact admission and charged-event release in the native section of `docs/architecture.md`. Verify the architecture model check recognizes the module and its declared dependencies.

## 2. Authoritative notification settings

- [ ] 2.1 Add an independent typed `home.get` notification-settings job to the existing local auxiliary scheduler, stream and budget, with same-service `settings.changed` refresh, latest-generation acceptance and last-good failure handling. Verify in-process tests extending the existing local settings fixtures cover first-read admission, failure isolation from appearance, superseded/foreign reads, no second stream and no settings write.
- [ ] 2.2 Feed accepted switches into the ledger and purge entries for disabled categories; re-enabling must not restore them. Verify unit tests cover master/type gates, `process.idle` and generic admission under master enablement, refresh reset, and read-failure retention.
- [ ] 2.3 Update `architecture/client-core.c4` and the native settings-read account in `docs/architecture.md` for the independent scheduler path. Verify architecture checks identify the intended relationships and no server or protocol relationship was added.

## 3. GPUI observation and Drawer projection

- [ ] 3.1 Consume existing source-scoped JSON notification events in the GPUI workspace and give the core ledger window focus, visibility and active-pane observation. Mark the active pane read on focus/visibility return and deliberate pane activation. Verify unit-level production-view tests cover watched arrival, retained offline delivery, stale-generation rejection, return and activation without workspace commands, settings writes or persistence.
- [ ] 3.2 Project per-Screen unread count/newest-title summary and per-pane highest severity through GPUI Kit components and semantic theme tokens. Verify unit-level production-view tests cover two panes on one Screen, severity priority, source collisions, read updates and accessible summary content.
- [ ] 3.3 Add `gpui_workspace -> splitty.client_core.client_notifications` to `architecture/gpui.c4` and document the window observation boundary in `docs/architecture.md`. Verify the architecture model and production-view tests reflect the same ownership.

## 4. Integration checks

- [ ] 4.1 Run targeted client-core notification unit/in-process tests and GPUI Drawer production-view tests, then targeted strict Clippy for the touched crates; verify the focused results pass before broader checks.
- [ ] 4.2 Before merge, run once: `cargo fmt --all --check`, `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`, `cargo test --workspace --exclude splitty-gui` (after `pnpm install --dir architecture` where needed), and `openspec validate --specs`; verify all pass. This full check is deferred until integration review.
