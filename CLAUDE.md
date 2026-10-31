# Working in this repository

The rules for building live in a few places and hold for work without an OpenSpec change too:

- `openspec/config.yaml`: its `context` (platforms and WSL on Windows, security invariants, where the truth lives) and its `rules` for proposals, specs, designs and tasks.
- `openspec/specs/`: what the system guarantees today. `docs/architecture.md`: how it is built (deliberately not specced).
- `architecture/`: the module-level map (LikeC4 model, conventions in its README). `cargo test` checks it against the code; a new crate, top-level module or dependency between components updates it in the same change.
- `docs/issues/openspec-baseline-findings.md`: known bugs, stale docs and test gaps.
- `openspec/open-decisions.md`: provisional decisions waiting for the maintainer's review.
- `roadmap/README.md` (German): order and milestones; the next milestone is listed under "Als Nächstes".

## Tests

Three levels:

- **unit**: logic, pure or over a temporary directory, in the module's `#[cfg(test)]` block; most tests.
- **in-process**: behavior through the server's own entry points inside the test process: the router with `tower::ServiceExt::oneshot`, the session dispatcher, the mux reducer with its effects, and real PTYs through `pty::testing` (skipped at runtime without a PTY, never `#[ignore]`d).
- **smoke**: a real `splitty` process (`crates/splitty-server/tests/http_surface.rs`, `tests/daemon_lifecycle.rs`). They are the slowest and the only tests that exercise the binary, its startup output and the transport.

A test task names its level and extends an existing test where one fits. A new smoke test states why an in-process test cannot show it. Frontend tests are vitest (`frontend/`, `pnpm test`).

## Checks

- During the work: targeted tests of what you touch, for example `cargo test -p splitty-core pty::` or `cargo test -p splitty-server --test daemon_lifecycle`, and `pnpm test <file>` in `frontend/`.
- Before a merge, once (the same as CI; `cargo test` runs the architecture check, which needs `pnpm install --dir architecture` once):
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`
  - `cargo test --workspace --exclude splitty-gui`
  - where `crates/splitty-tauri` is touched: `cargo clippy -p splitty-gui --all-targets -- -D warnings` and `cargo test -p splitty-gui --lib`
  - where `frontend/` is touched: `pnpm check`, `pnpm test` and `pnpm build` there
  - `openspec validate --specs`

## Changes and merges

- OpenSpec changes use `spec-driven` (proposal, specs, design, tasks) when they are large, span several components or the protocol, or change data formats or security invariants; otherwise `light` (`openspec/schemas/light`: proposal with its "Architecture impact" line and a short "Decisions" list, specs, tasks, no design; `openspec new change <name> --schema light`, and `skip_specs: true` in `.openspec.yaml` when no behavior changes). Tiny fixes need no change.
- Drafts (proposal, maybe design, a "Draft" note in their first lines) are completed with specs and tasks when they are next, then implemented with `/opsx:apply` and closed with `/opsx:archive`.
- Do not stop for decisions the maintainer has not made: decide, mark it "(provisional, tbd)" in the change, and add a bullet to `openspec/open-decisions.md`.
- Specs of UI parts (the web frontend, the desktop shells' windows) state only what crosses the protocol or an API and the UX concepts and decisions a reader cannot see in the built app (what closing a remote screen does, what happens on reconnect); no scenario per screen, layout, label or style.
- The repository is public. Docs, specs, comments, tests and commit messages describe the system, not people: no quotes from conversations, no notes on who asked or decided, no personal machines, accounts, host names or paths.
- Archived OpenSpec changes stay local (`openspec/changes/archive/` is gitignored); the main specs are the record.
- Commits follow the conventional style of the log (`fix(pty): …`, `docs(openspec): …`). After an archive, commit right away; never push.
- On Windows, local work runs in WSL by design; remote-only use works without WSL (see `roadmap/README.md`, Milestone 7).
