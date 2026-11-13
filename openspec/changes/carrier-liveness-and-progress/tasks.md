# Tasks

## 1. Liveness per carrier

- [ ] 1.1 Remove the session loop's heartbeat (ping tick, idle timeout, ping send) and the `Ping`/`Pong` session messages with the sender's ping; the native carrier loop no longer answers session pings
- [ ] 1.2 Drop the session loop's sweep of editor requests and file offers; each waiting request keeps its own deadline
- [ ] 1.3 Session-loop comments and log lines describe the session, not a WebSocket connection
- [ ] 1.4 Test, in-process (native carrier, `native` module tests): a native client that stops answering transport pings is ended within 45 seconds and its client registration released; a client whose peer answers pings and sends nothing else is still registered after several minutes (Responsive client, Silent client, Peer of the previous release)
- [ ] 1.5 Tests kept for the browser carrier, in-process: `a_silent_browser_expires_even_while_the_native_hop_is_healthy` (Silent browser) and `preparation_handoff_preserves_the_browser_ping_phase_and_pong_window` (a healthy delayed pong keeps the browser); the expiry unit tests `an_offer_nobody_saves_expires` and `ambiguity_rate_pending_and_expiry_are_bounded` show the requests' own deadlines without a session

## 2. Host progress over one channel

- [ ] 2.1 The hub no longer repeats preparation progress as text after accepting a native host stream; `remote.ready` is the first application message
- [ ] 2.2 Step names defined once in the hub (one name per step for the transport report); the gateway orders transport reports by that definition and drops every `remote.progress` text before `remote.ready`
- [ ] 2.3 The desktop client keeps ignoring `remote.progress` text before `remote.ready` (older hubs), with a comment saying so
- [ ] 2.4 The Tauri web view adapter (`native_hub.ts`) shows live transport progress and drops every `remote.progress` text before `remote.ready`
- [ ] 2.5 Test, in-process (hub native stream tests): progress arrives as transport reports during preparation, and after acceptance the first text is `remote.ready` (Progress on a native host stream)
- [ ] 2.6 Test, in-process (gateway): a backend that repeats progress as text before `remote.ready` (older hub) reaches the browser with each step once; existing gateway progress tests keep passing (Connect a host without Splitty, Hub of the previous release)
- [ ] 2.7 Test, unit (vitest, `native_hub.test.ts`): an older hub's text progress before `remote.ready` is dropped, live progress is shown (Hub of the previous release)

## 3. Docs

- [ ] 3.1 `docs/protocol.ts`: host streams carry progress as transport reports, `remote.progress` only on browser sockets
- [ ] 3.2 `docs/architecture.md`: liveness belongs to the carrier; the session loop has no heartbeat; one progress channel

## 4. Checks

- [ ] 4.1 Targeted: `cargo test -p splitty-server client_session:: native:: hub:: gateway`, `cargo test -p splitty-client-core native_workspace::remote`, `pnpm test src/lib/remote/native_hub.test.ts` in `frontend/`
- [ ] 4.2 Full: `cargo fmt --all --check`; `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`; `cargo test --workspace --exclude splitty-gui`; `cargo clippy -p splitty-gui --all-targets -- -D warnings`; in `frontend/`: `pnpm check`, `pnpm test`, `pnpm build`; `openspec validate --specs`
