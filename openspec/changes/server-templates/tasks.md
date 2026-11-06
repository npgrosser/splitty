# Tasks

## 1. Core template intent

- [ ] 1.1 Implement pure apply/extract conversions, validation and atomic workspace replacement/creation; unit tests cover splits, tabs, floating bounds, metadata, legacy web pruning, invalid references/IDs, empty extraction and unchanged state on rejection.
- [ ] 1.2 Carry restore modes through session records and spawn effects; extend effect-runner unit tests to verify command, cwd, profile and restore mode delivery and truthful partial failures.
- [ ] 1.3 Update architecture/core.c4 and docs/architecture.md for the mux-template relationship and server ownership; verify model matches dependencies.

## 2. Server and transport

- [ ] 2.1 Admit apply_template on window session protocol, announce capability, report malformed/rejected operations and pane spawn outcomes; extend mux-handler in-process tests for replacement, result ordering, resync and failures.
- [ ] 2.2 Add authenticated extraction HTTP/native operation with server process enrichment; in-process route/native tests cover parity, missing/empty screens, root-relative extraction and session-connection extraction; frontend unit tests cover disconnects.
- [ ] 2.3 Reject template apply on generic mux APIs and keep extraction absent on control/agent sockets; extend in-process security tests.
- [ ] 2.4 Update docs/protocol.ts and docs/native-operations.json for touched payloads/capability and extraction catalog; validate catalog parity.

## 3. Window client

- [ ] 3.1 Replace layout/template_apply.ts and layout/template_extract.ts with pure template intent projection and server extraction, using queued snapshots for new screens and preserving confirmation/layout-only/cancel choices; frontend unit tests cover projection, consent, one op, delayed mount and no direct spawning.
- [ ] 3.2 Gate actions by capability and offer host update; retain early spawn results and resync failed applications, including malformed intent; frontend unit tests cover older hosts, early spawn and rejection.

## 4. Integration checks

- [ ] 4.1 Run targeted core/server template, effect and mux tests plus frontend template/mux/snapshot tests and change validation; record results.
- [ ] 4.2 Run the one full pre-merge check: cargo fmt --all --check; cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings; cargo test --workspace --exclude splitty-gui; frontend pnpm check, pnpm test and pnpm build; openspec validate --specs (install architecture dependencies first).
