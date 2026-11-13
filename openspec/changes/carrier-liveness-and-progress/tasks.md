# Tasks

## 1. Liveness per carrier

- [x] 1.1 Remove the session loop's heartbeat (ping tick, idle timeout, ping send) and the `Ping`/`Pong` session messages with the sender's ping; the native carrier loop no longer answers session pings
- [x] 1.2 Drop the session loop's sweep of editor requests and file offers; each waiting request keeps its own deadline
- [x] 1.3 Session-loop comments and log lines describe the session, not a WebSocket connection
- [x] 1.4 Test, in-process (native carrier, `native` module tests, `the_native_carrier_alone_ends_a_silent_peer_and_keeps_a_responsive_one`): a native client that stops answering transport pings is ended within 45 seconds and its client registration released; a client whose peer answers pings and sends nothing else is still registered after several minutes (Responsive client, Silent client, Peer of the previous release)
- [x] 1.5 Tests kept for the browser carrier, in-process: `a_silent_browser_expires_even_while_the_native_hop_is_healthy` (Silent browser) and `preparation_handoff_preserves_the_browser_ping_phase_and_pong_window` (a healthy delayed pong keeps the browser); the expiry unit tests `an_offer_nobody_saves_expires` and `ambiguity_rate_pending_and_expiry_are_bounded` show the requests' own deadlines without a session

## 2. Host progress over one channel

- [x] 2.1 The hub no longer repeats preparation progress as text after accepting a native host stream; `remote.ready` is the first application message
- [x] 2.2 Step names defined once in the hub (one name per step for the transport report); the gateway orders transport reports by that definition and drops every `remote.progress` text before `remote.ready`
- [x] 2.3 The desktop client keeps ignoring `remote.progress` text before `remote.ready` (older hubs), with a comment saying so
- [x] 2.4 The Tauri web view adapter (`native_hub.ts`) shows live transport progress and drops every `remote.progress` text before `remote.ready`
- [x] 2.5 Test, in-process (hub native stream tests, the shared `prelude` check): progress arrives as transport reports during preparation, and after acceptance the first text is `remote.ready` (Progress on a native host stream)
- [x] 2.6 Test, in-process (gateway): a backend that repeats progress as text before `remote.ready` (older hub) reaches the browser with each step once (`an_older_hubs_repeated_progress_text_is_not_forwarded_to_the_browser`); `native_host_open_pins_the_local_hub_before_any_ssh_and_keeps_remote_authority` now requires `remote.ready` first on native host streams; existing gateway progress tests keep passing (Connect a host without Splitty, Hub of the previous release)
- [x] 2.7 Test, unit (vitest, `native_hub.test.ts`): an older hub's text progress before `remote.ready` is dropped, live progress is shown (Hub of the previous release)

## 3. Docs

- [x] 3.1 `docs/protocol.ts`: host streams carry progress as transport reports, `remote.progress` only on browser sockets
- [x] 3.2 `docs/architecture.md`: liveness belongs to the carrier; the session loop has no heartbeat; one progress channel
- [x] 3.3 `architecture/server.c4`: the `http -> hub` relationship names the host steps the gateway translates

## 4. Checks

- [x] 4.1 Targeted: `cargo test -p splitty-server client_session:: native:: hub:: gateway`, `cargo test -p splitty-client-core native_workspace::remote`, `pnpm test src/lib/remote/native_hub.test.ts` in `frontend/`
- [x] 4.2 Full: `cargo fmt --all --check`; `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`; `cargo test --workspace --exclude splitty-gui`; `cargo clippy -p splitty-gui --all-targets -- -D warnings`; in `frontend/`: `pnpm check`, `pnpm test`, `pnpm build`; `openspec validate --specs`
