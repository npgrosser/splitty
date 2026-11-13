# Tasks

## 1. Tests on the production path (in-process)

- [x] 1.1 `router(state, hub)` and a new `browser_router(state)` in `http/tests.rs` build
      `gateway::router` over `gateway::Backend::embedded`; `conformance.rs` and `managed_jobs.rs`
      use `browser_router` instead of `build_router`
- [x] 1.2 Account for every test of `http::tests` (163 before and after) — see the table below
- [x] 1.3 Adjust the assertions that described the old router's extractors

| Category | Tests | Notes |
| --- | --- | --- |
| Ported to `gateway::router` (in-process, `oneshot`) | 49 | Bodies unchanged except the rows below: `tests.rs` 31, `conformance.rs` 9 (HTTP side of each native/HTTP parity check), `annotations.rs` 2, `move_pane.rs` 2, `viewer.rs` 2, `managed_jobs.rs` 1, `pane_state.rs` 1, `scratchpad.rs` 1 |
| Already on the production path | 32 | Serve `gateway::router` on a loopback port (`Served`): the `/ws` and `/ws/remote` relay tests, ports, forwards restored on ready, editor, files, program status, review waits over HTTP, mux tabs, PTY size |
| Not touching a router | 82 | Native framing, control socket, `gateway.rs` adapter tests, service-level tests; unchanged |
| Obsolete | 0 tests, 1 assertion | The 415 for a non-JSON `Content-Type` came from axum's `Json` extractor of the removed handlers; the gateway decodes bodies by the catalog |

Adjusted assertions (in-process): `remembered_screens_are_kept_in_the_configuration_directory`,
`remote_catalog_rejects_invalid_updates_without_writing` and
`forward_preferences_return_json_for_malformed_json` (renamed from `…_and_preserve_415`) expect
the gateway's 422 for a body that is not JSON; `conformance::a_native_reverse_request_cannot_share_a_fresh_browser_gateway`
addresses the served gateway's own host; `conformance::framed_native_welcomes_before_replies_and_enforces_the_fixed_role_catalog`
waits for the gateway's API carrier to leave after the gateway is dropped;
`move_pane::a_cli_move_nudges_both_screens_and_a_moved_shell_still_splits` sends its CLI requests
to a gateway of its own over the same service.

No test exercised the removed WebSocket code (`ws::handle_socket`, `Hub::serve`); their
production counterparts are covered by the `Served` relay tests and `http::tests::gateway`.

## 2. Remove the old router

- [ ] 2.1 Delete `build_router`, `build_router_with_frontend`, `apply_middleware`,
      `routes::api_routes_with` and the handlers only it registered, the old `/ws` and
      `/ws/remote` upgrades, and `ws.rs`
- [ ] 2.2 Delete `Hub::serve`, `setup`, `relay`, `say`, `report_progress`, `message_of`,
      `forward_message`, `describe`, `Sink`, `Stream`, `End`, `Setup` and `MAX_EARLY`
- [ ] 2.3 Replace `AppState` with `ControlState` (the service only) for the control-socket
      router; `control::spawn`/`spawn_at` take the service; the tests keep their own fixture
      with the browser credentials

## 3. One post-ready step in the hub

- [ ] 3.1 If the bookkeeping after a host is reached is still written more than once, make it one
      `Hub` function

## 4. Docs and model

- [ ] 4.1 docs/architecture.md and docs/networking.md describe only the gateway path
- [ ] 4.2 architecture/server.c4: remove `ws`, update `http`, `control`, `main` relations; `cargo test`
      architecture check passes
