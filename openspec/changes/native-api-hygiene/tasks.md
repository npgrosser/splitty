# Tasks

## 1. Catalog descriptions

- [ ] 1.1 `native_api::Description` carries an error style per operation (`plain`, `job`, `viewer`) and typed arguments (text, count, body); request validation keeps requiring strings for every non-body argument
- [ ] 1.2 The dispatcher decodes bodies and count arguments by the operation's description (no path prefix, no list of names); the viewer operations take typed bodies
- [ ] 1.3 `docs/native-operations.json` lists the error style and count arguments; `docs/protocol.ts` says what count arguments are
- [ ] 1.4 Tests, unit and in-process: the published catalog matches the descriptions (`published_catalog_matches_the_checked_policy`); invalid `tail`/`limit`/`waitMs` and undecodable job, viewer and plain bodies keep their status and error shape over native requests (extend the dispatch/conformance tests)

## 2. Control socket from the catalog

- [ ] 2.1 A `Control` scope marks the twelve operations of the control socket's route set; validation treats it as a backend operation
- [ ] 2.2 One request translation (query and path arguments, JSON content type, body, validation) shared by the browser gateway and the control socket; the control socket routes its catalog operations through it into the service's native entry point; `POST /api/daemon/stop`, the job router and the grant administration stay as they are
- [ ] 2.3 Both transports answer 415 for a body operation whose `Content-Type` is not JSON, before reading the body
- [ ] 2.4 Test, in-process (control router): every catalog operation is routed on the control socket exactly when its scope is `Control` (Control Socket Route Set)
- [ ] 2.5 Test, in-process (control router and gateway): a malformed `/api/mux/op` body and an unknown `/api/mux/neighbor` query argument get the same status and JSON error on both transports (Malformed request on both transports)
- [ ] 2.6 Test, in-process (gateway and control router): `text/plain` and a missing content type answer 415 without effect; `application/json; charset=utf-8` is processed (JSON Request Bodies)
- [ ] 2.7 Check that the web client and the CLI send `Content-Type: application/json` with every body

## 3. Attach and leftovers

- [ ] 3.1 `cli::attach` writes text session messages through a sink of strings; the WebSocket path and `native::client::SessionSink` frame them
- [ ] 3.2 Remove `Hub::open_forward_with`, `SessionStore::new` and the default cookie name; tests use isolated stores
- [ ] 3.3 Remove `control::spawn_service`; the remote-window helper, the embedded service of client-core and the Tauri shell call `control::spawn_at`
- [ ] 3.4 `splitty_remote::tunnel::Step::name` is the one spelling of the connection steps (serialization, remote-window helper, Tauri provider); unit test `connect_events_encode_as_one_object_with_an_event_field` keeps covering it
- [ ] 3.5 `docs/architecture.md`: the control socket's routes come from the catalog

## 4. Checks

- [ ] 4.1 Targeted: `cargo test -p splitty-core native_api`, `cargo test -p splitty-server http:: service:: cli::attach`, `cargo test -p splitty-remote tunnel`
- [ ] 4.2 Full: `cargo fmt --all --check`; `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`; `cargo test --workspace --exclude splitty-gui`; `cargo clippy -p splitty-gui --all-targets -- -D warnings` and `cargo test -p splitty-gui --lib`; `openspec validate --specs`
