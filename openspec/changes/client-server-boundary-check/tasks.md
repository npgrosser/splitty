# Tasks

## 1. Font without the web build

- [ ] 1.1 `terminal_font_data` compiles the font from `frontend/public` on every platform; drop the relationship `appearance -> embed` from `architecture/client-core.c4`; verify with `cargo test -p splitty-client-core appearance` (unit) and the architecture check

## 2. Embedded shutdown through the runtime

- [ ] 2.1 `Runtime::close` closes every registered client's streams before shutting down jobs and terminals; `embedded::shutdown` calls only `close`; in-process test in `splitty-server` (`service::runtime`) that a connected embedded session client's sender is closed after `close`, failing without the change

## 3. Boundary rule in the architecture check

- [ ] 3.1 The scanner marks references in test code: items under a `cfg` that requires `test`, `#[test]` functions, and module files declared under such a `cfg`; unit tests in `architecture/check/src/code.rs` cover inline modules, test files, `cfg(all(test, …))` and `cfg(any(test, feature))`
- [ ] 3.2 A check of production references from one crate into another, allowed only from named components, and one of the workspace crates a crate may name; unit tests in `architecture/check/src/check.rs` with a fixture where a forbidden reference fails and a test reference passes
- [ ] 3.3 `tests/model.rs` applies the rules: client-core production code names `splitty_server` only from `embedded`, `connection`, `api_stream`; `splitty_gpui` names only `splitty_client_core`; document the rule in `architecture/README.md` ("What the test checks")
- [ ] 3.4 Relationship titles from client-core into the server that only tests back say so; verify with `cargo test -p splitty-architecture`

## 4. Model descriptions

- [ ] 4.1 `landscape.c4`: client-core and gpui descriptions; container relationships `gpui -> wsl`, `gpui -> config_files`, `client_core -> server`, `user -> gpui`; README file table; `docs/architecture.md` names the boundary; verify with `pnpm --dir architecture validate` and the architecture check

## 5. Native app without frontend/dist

- [ ] 5.1 Default feature `web-assets` of `splitty-server` gates `rust-embed`, `mime_guess` and the embedded assets; client-core depends on the server without default features; verify `cargo build -p splitty-gpui` with `frontend/dist` moved away, and `cargo test -p splitty-server --test http_surface` (smoke, existing) with it
- [ ] 5.2 `docs/development.md`, CI and release workflow notes state that only the `splitty` command needs `frontend/dist`

## 6. Checks

- [ ] 6.1 Targeted tests of the touched crates, then `cargo fmt --all --check`, `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`, `cargo test --workspace --exclude splitty-gui`, `cargo test -p splitty-gpui --features ui-test-support` and `openspec validate --specs`
