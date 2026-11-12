# Tasks

## 1. The released build

- [x] 1.1 Add `scripts/testing/fetch-last-release.sh`: it resolves the newest release that is not a prerelease (or `--tag`), downloads `splitty-wsl-x86_64.tar.gz` and `SHA256SUMS` with `gh`, checks the archive against the one line naming it, unpacks the static `splitty`, compares its `--version` with the tag and prints `SPLITTY_TEST_RELEASED_BINARY` and `SPLITTY_TEST_RELEASED_VERSION` (VS-1). Verify: a run into an empty directory prints the verified path; a checksum line that does not match fails; a non-empty output directory is refused.

## 2. The smoke test

- [x] 2.1 Add `crates/splitty-server/tests/version_skew.rs` (smoke: a real `splitty` of each build; an in-process test links one version of every type, as its module doc says). Every process runs with a cleared environment, an allowlist (`PATH`, `HOME`, `SHELL=/bin/sh`, `TMPDIR`, `XDG_*`) and a throwaway home below `/tmp`; leftovers are stopped by PID only after their environment shows that home. Without `SPLITTY_TEST_RELEASED_BINARY` the tests print `SKIPPED` and pass; with `SPLITTY_TEST_REQUIRE_RELEASED_BINARY=1` they fail (VS-4). Verify: `cargo test -p splitty-server --test version_skew` without the variable reports the skip.
- [x] 2.2 Smoke, client and daemon, both directions: the other build's `daemon status`, `status`, `new-screen`, `list-screens`, `list-panes`, `split`, `close-pane`, `rename-screen`, `send-keys` and `daemon stop --end-sessions`; its `attach` at 100x30, resized to 120x40 and re-attached at 90x28, with the shell's `stty size` checked each time; the current native client through each build's `remote-bridge` (welcome, `mux.status`, the same attach, resize and re-attach) (VS-3). Verify: `the_clients_of_one_build_use_the_daemon_of_the_other` passes against the last release.
- [x] 2.3 Smoke, daemon and keepers, both directions: one build's daemon stops and keeps its shell (the keeper's executable checked); the other build's `attach` controls the keeper without a daemon; the other build's daemon adopts it with the same pane, session and shell PID, then attach, resize and re-attach. Verify: `a_daemon_adopts_the_keepers_the_other_build_started` passes against the last release.
- [x] 2.4 Smoke, the command in a pane, both directions: the other build's CLI in a pane reaches the daemon over `SPLITTY_SOCK` and targets its screen and pane implicitly (`list-screens`, `rename-screen`, `split`), the other build attaches the split; after the other build's daemon took over, the shell's `$SPLITTY_CLI_DIR/splitty` is the new daemon's and both builds' CLIs reach it. Verify: `the_command_in_a_pane_reaches_the_daemon_of_the_other_build` passes against the last release.
- [x] 2.5 Smoke, files, both directions: one build writes `state.json` (a screen with two panes, sessions ended) and `config.toml` (size settings through the current native client, `daemon.json` read by it); the other build restores the screen with the same pane IDs, serves the settings beside a key neither knows, and attaches, resizes and re-attaches a respawned shell (VS-5). Verify: `the_files_one_build_writes_are_read_by_the_other` passes against the last release.

## 3. CI, release and docs

- [x] 3.1 CI: in the "Rust tests" job of `.github/workflows/ci.yml`, after the workspace tests, fetch the last release into `$GITHUB_ENV` and run the test with `SPLITTY_TEST_REQUIRE_RELEASED_BINARY=1`. Verify: the workflow parses and the step runs the command that passes locally.
- [x] 3.2 Release: a `version-skew` job in `.github/workflows/release.yml` (after `version`) fetches the last release and runs the test; `publish` needs it (VS-2). Verify: the workflow parses and `publish.needs` lists `version-skew`.
- [x] 3.3 Document running the test locally in `docs/development.md` ("Version-skew smoke"). Verify: the documented commands run as written.

## 4. Findings

- [x] 4.1 Narrow the "version skew" entry of `docs/issues/openspec-baseline-findings.md` to the boundaries the test leaves out. Verify: the entry names what is covered and what is not.
- [x] 4.2 Record what the first run found: a daemon drops the settings a newer version added when it starts (server-modes), with file and line. Verify: the entry names `service/runtime.rs` and `config/settings.rs` with lines.

## 5. Checks

- [x] 5.1 Targeted: `cargo fmt --all --check`; `cargo clippy -p splitty-server --all-targets -- -D warnings`; `cargo test -p splitty-server --test version_skew` with `SPLITTY_TEST_RELEASED_BINARY` from the fetch script and once without it; `cargo test -p splitty-server --test daemon_lifecycle`.
- [ ] 5.2 The full check before the merge: `cargo fmt --all --check`, `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`, `cargo test --workspace --exclude splitty-gui` (the architecture check needs `pnpm install --dir architecture` once) and `openspec validate --specs`.
