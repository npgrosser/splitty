# Tasks

## 1. Update and retry decisions

- [x] 1.1 Share SemVer precedence and return update/retry metadata from browser/native hub setup, retaining existing replacement checks; extend unit and in-process tests for older/newer/equal/invalid versions, legacy migration and permanent errors, and update docs/protocol.ts.
- [x] 1.2 Make the window consume positive hub update metadata and retry classification, with correct keeper wording; extend frontend unit tests for ready/error metadata, newer versions, permanent retry overrides and migration offers.

## 2. Directory ownership

- [x] 2.1 Add the merged passive directory read to HTTP and the native operation catalog, honoring aliases, hidden hosts, saved order and active/saved forward hosts; extend unit/in-process tests for merging, ordering, no SSH effects and restricted callers; update protocol and architecture docs/model.
- [x] 2.2 Persist validated handshake catalogs at successful readiness and ordered later local observations; extend in-process/unit tests for empty catalogs, renames/removals, hidden hosts, invalid data and storage errors without failing host relay.
- [x] 2.3 Switch the supporting-hub window to merged directory reads and ordered observation messages, removing its persistence queue from that path; retain older-hub compatibility and test live overlays, stale reads, hidden entries and observation order at frontend unit level.

## 3. Passive change feed

- [x] 3.1 Emit coalesced local invalidations for host metadata and forward/reverse-forward mutations, restore errors and supervisor changes, subscribing before welcome; extend in-process/unit tests for mutation and up/down notifications and no injection into relayed host streams.
- [x] 3.2 Refresh directory and forward state on local invalidation/reconnect, queue refresh during mutations and disable polling for capable hubs; extend frontend unit tests for cross-window invalidation, mutation races, reconnect and older hubs; document capability/messages.

## 4. Integration

- [x] 4.1 Run targeted Rust hub/remote/service tests and frontend remote controller tests; validate this change and reconcile its artifacts and fixed baseline findings.
- [ ] 4.2 After integrating the four server changes, run cargo fmt --all --check, cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings, cargo test --workspace --exclude splitty-gui; frontend pnpm check, pnpm test, pnpm build; openspec validate --specs. Run GUI checks as in CLAUDE.md if desktop Rust code changes. Sync and archive only after all checks pass.
