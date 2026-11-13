# Tasks

## 1. Service

- [ ] 1.1 Question broker: a change feed that moves when a question is published or settled (answered, cancelled, timed out, helper gone)
- [ ] 1.2 Session connections with local hub authority: `ssh-prompts-v1` in `mux.welcome`, `ssh.prompts.changed` on every change and once after the greeting while questions are pending; none on remote-window connections or relays
- [ ] 1.3 SSH command: drop the direct askpass program route (`SPLITTY_ASKPASS_PROGRAM`, the program a desktop process registers, the WSL environment forwarding); the broker stays the only route
- [ ] 1.4 Test (in-process, extends the hub notification conformance test's setup): a question published through the broker's helper socket notifies a main connection, `ssh.prompts.get` lists it, `ssh.prompts.answer` reaches the waiting helper and a further notification follows; a connection opened while a question is pending is notified after its greeting; a remote-window connection gets no capability and no notification and is refused both operations

## 2. Native client

- [ ] 2.1 Client core: a question reader on its own local API stream in the native workspace, opened on the first notification (or at once for polling without the capability), coalesced reads, answers in order, bounded validated list published to the UI, a command to answer
- [ ] 2.2 Client core: remove the prompt re-exports from the crate root; the embedded service offers the askpass helper entry for the app executable; the WSL launch no longer carries an askpass path
- [ ] 2.3 Native app: question dialogs fed by the workspace's updates, answered through its command; the standalone askpass mode, the Windows askpass registration and the WSL inspection of the askpass path go
- [ ] 2.4 Test (in-process, client core, next to the shares/hub worker tests): an isolated embedded service, a question published through the broker reaches the worker's update stream, the answer command reaches the waiting helper and the next update lists no question; polling fallback against a service without the capability (unit, over a scripted API stream)
- [ ] 2.5 Test (unit, native app view test): the dialog's answer and dismissal go to the answer sink once

## 3. Model and docs

- [ ] 3.1 `architecture/`: relationships of `client_session`, `client_root`, `embedded`, `native_workspace`, `gpui_askpass`, `gpui_main`; new boundary rule in `check/tests/model.rs` (production code of `splitty_client_core` names `splitty_remote::askpass` only from `embedded`) with a unit test of the path-prefix form; README rule list
- [ ] 3.2 `docs/protocol.ts`: `ssh-prompts-v1`, `ssh.prompts.changed`, the question list and answer shapes
- [ ] 3.3 `docs/native-ui-handoff.md`: SSH questions of the native app; `openspec/open-decisions.md`: SPP-1 to SPP-4

## 4. Checks

- [ ] 4.1 Targeted tests: `cargo test -p splitty-remote askpass`, `cargo test -p splitty-server ssh_prompts`, `cargo test -p splitty-client-core ssh_prompts`, `cargo test -p splitty-gpui --features ui-test-support askpass`, `cargo test -p splitty-architecture`
- [ ] 4.2 Full check: `cargo fmt --all --check`, `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`, `cargo clippy -p splitty-gpui --all-targets --features ui-test-support -- -D warnings`, `cargo test --workspace --exclude splitty-gui`, `cargo test -p splitty-gpui --features ui-test-support`, `openspec validate --specs`
