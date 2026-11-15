## Context

See proposal.md, Why. The state this design starts from:

- `remove-tauri-shell` has landed: `crates/splitty-tauri`, its CI, the remote windows of `desktop-remote` (the editor handoffs stay), the remote-window, shell-command and packaged-content rules of `access-control`, `desktop-backend --backend ssh` with its SSH helper, and Desktop Stdio Lifetime Binding (replaced by Desktop Backend Child Lifetime in `server-modes`) are gone. The deltas here for `access-control`, `client-connections`, `desktop-remote`, `remote-hosts` and `server-modes` start from that change's text where both touch one requirement (Resource Admission, Payload Sources, Desktop App Hosting On macOS And Linux, Remote and Local Folder Editor Handoff), and leave the requirements only it removes to it.
- The preparation on the branch `task/server-tests-off-gateway` has landed: every in-process test of service behavior uses the control router or the native dispatch, and the tests about the gateway itself sit in `crates/splitty-server/src/http/tests/gateway.rs`.
- The native app reaches its local service in-process (Linux, macOS) or through `desktop-backend --transport native-v1 --backend embedded --owner` inside WSL (Windows), and hosts through the hub's native host streams. It never opens TCP to the service, never reads `auth-token` and never uses a browser credential.
- The CLI reaches a server through `SPLITTY_AGENT_SOCK`, `SPLITTY_SOCK` or the daemon's control socket; `--addr`/`--token` is the only TCP path. `splitty attach` uses the daemon's private native endpoint or a keeper; with `--addr`/`--token` it goes through the gateway's `/api/ws-ticket` and `/ws` (`crates/splitty-server/src/cli/socket.rs`).

### Code inventory

Deleted whole (lines on `main` at the time of writing):

| Path | Lines | What it is |
|---|---|---|
| `crates/splitty-server/src/http/gateway.rs` | 1269 | Gateway router, backends (embedded, daemon, remote), `/ws`, `/ws/remote`, catalog-derived `/api` |
| `crates/splitty-server/src/http/gateway/api_client.rs` | 307 | Gateway-owned native API carrier per backend revision |
| `crates/splitty-server/src/http/session.rs` | 420 | Session store, cookies, bootstrap codes, tickets, `/api/session`, `/api/bootstrap`, `/api/ws-ticket` |
| `crates/splitty-server/src/http/middleware.rs` | 207 | Host, Origin and Fetch Metadata guards, bearer/cookie authentication |
| `crates/splitty-server/src/http/browser_wire.rs` | 691 | WebSocket frame translation to native frames |
| `crates/splitty-server/src/http/server.rs` | 607 | Admitted TCP accept loop, HTTP/1 serving with deadlines |
| `crates/splitty-server/src/http/tests/gateway.rs` | about 2400 after the preparation | Gateway tests |
| `crates/splitty-server/src/embed.rs` | 62 | Embedded `frontend/dist`, themed `index.html` |
| `crates/splitty-server/src/cli/socket.rs` | 41 | Ticket and WebSocket for `attach --addr` |
| `crates/splitty-core/src/auth/` | 267 | Process token, one-shot codes, constant-time compare |
| `crates/splitty-server/tests/http_surface.rs` | 1915 | Smoke tests of `splitty serve` (the control-socket tests move, see Test plan) |
| `crates/splitty-server/tests/remote_connect/exposure_crash.rs` | 251 | Gateway shutdown probe and gateway reuse after a killed coordinator |
| `frontend/` | about 57 000 in `src` (298 files, 98 test files) | The Svelte client, its tests and tooling |
| `scripts/install.sh`, `packaging/linux/splitty-serve.service` | | systemd user service running `splitty serve` |
| `scripts/testing/managed-jobs-browser.mjs`, `remote-forwards-browser.mjs`, `remote-forwards-browser.test.mjs`, `scripts/testing/README.md` | | Playwright browser checks |

Cut:

| Path | What goes | What stays |
|---|---|---|
| `crates/splitty-server/src/main.rs` (3040) | `Serve`, `Connect`, `IssueBootstrap`, `CtlOpts.addr`/`token`, `DaemonAction::Start` hidden `addr`/`token`/`remote`, `Restart.addr`, `resolve_token`, `resolve_loopback_bind`, `serve_embedded_gateway`, `serve_daemon_gateway`, `monitor_daemon_gateway`, `handle_connect`, `report_connect`, `connect_rejected`, `connect_failed`, `open_in_browser`, `describe_transport`'s TCP arm, their tests; roughly 700 lines | Daemon, control commands, `attach`, `desktop-backend`, host commands, `remote-hosts`, `connect_shutdown` (used by `desktop-backend`), new hidden stubs (RBA-9) |
| `crates/splitty-server/src/http/mod.rs` (108) | `BrowserState`, `canonical_origins`, module declarations of the deleted files | `ControlState`, `IntoResponse for Reply`, `build_control_router` (moved, RBA-10) |
| `crates/splitty-server/src/http/routes.rs` (308) | `refuse_upgrade`, `content_security_policy`, `security_headers`, `serve_frontend`, `apply_headers` (about 170 lines) | `control_routes`, `control_operation`, `stop_daemon_instance` |
| `crates/splitty-server/src/http/operation.rs` (265) | nothing but gateway-only helpers, if any remain unused | request translation for the control socket |
| `crates/splitty-server/src/http/jobs.rs` (185) | nothing | job and grant routes |
| `crates/splitty-server/src/cli/discovery.rs` (190), `cli/mod.rs` (277) | `Transport::Tcp`, flag arguments of `discover`, bearer auth in `request_within`, TCP test fixtures | Unix-socket transport, discovery order of agent, `SPLITTY_SOCK`, daemon |
| `crates/splitty-server/src/cli/attach/mod.rs` (436), `attach/session.rs` (536) | `explicit_server`, the TCP branch, the `SessionFrame` impl for tungstenite messages | daemon endpoint and keeper paths |
| `crates/splitty-server/src/cli/jobs.rs` (828) | `Transport::Tcp` checks; tests that serve a router over TCP move to a Unix socket | everything else |
| `crates/splitty-server/src/native/client.rs` (1197) | `Connector::local_host`, `local_host_admitted` (only the gateway opens a host stream through a local daemon's endpoint) | everything else |
| `crates/splitty-server/src/hub/reverse.rs`, `hub/mod.rs` | `reserve_local_port` and the `reserved` port set (only the gateway filled it) | reverse sharing; the exposure registry keeps refusing internal listeners |
| `crates/splitty-remote/src/tunnel/mod.rs` (1415) | `session_url`, `ConnectEvent`, `SshRemote`/`open`/`open_with_update` and `supervisor::supervise`/`connect`/`GIVE_UP_AFTER` where `remove-tauri-shell` left no other user (the SSH `desktop-backend` helper uses `open_with_update`) | `RemoteError`, `Failure`, `PortReservation`, update plan and migration offer, everything the hub uses |
| `crates/splitty-remote/src/tunnel/forward.rs` (826) | `parse_forwards`, `PortForward` list parsing and its errors if nothing else uses them | `reserve`, `reserve_exact`, `check_forward_address`, `PortTunnel` |
| `crates/splitty-remote/src/tunnel/exposure/registry.rs` (888), `exposure/unsupported.rs`, `exposure.rs` | `bind_gateway`, `GatewayListener`, `GatewayGuard`, the `bind` path | `Share`, `check_sharing_supported`, decoding of `Gateway` records (RBA-12) |
| `crates/splitty-server/Cargo.toml` | `web-assets`, `rust-embed`, `mime_guess`, `open`, axum's `ws` feature, hyper's `server` feature, `tokio-tungstenite` if attach was its last user, dev `reqwest` `cookies`; `tower` becomes a dev-dependency | everything else |
| `crates/splitty-client-core/Cargo.toml`, `src/appearance.rs` | `default-features = false` on `splitty-server` (redundant); the font path into `frontend/public/` | the font, moved to a crate-owned path |

What the `http` module is afterwards: `ControlState`, the `Reply` response conversion, `build_control_router`, `control_routes`, `stop_daemon_instance`, the job and grant routes and the request translation, all served only over Unix sockets. The name no longer fits: it suggests a network listener. RBA-10 folds it into `control`.

### Dependencies inside the parts to remove

- `crates/splitty-client-core/src/appearance.rs:26-28` compiles in `frontend/public/JetBrainsMonoNerdFontMono-Regular.ttf`. The font moves before `frontend/` is deleted.
- `splitty attach --addr --token` is the only user of `cli/socket.rs` and of tungstenite in the CLI.
- The hub's real-SSH smoke tests (`remote_connect.rs`, `HubServer`) reach the hub only through `splitty serve`, `/api/ws-ticket` and `/ws/remote`. They are the only end-to-end tests of hub install, update, forwards, saved forwards, reverse sharing and the bind gate through real SSH; they move to the owner backend (Test plan).
- `remote-handshake`, `remote-bridge`, `remote-migrate`, `remote-relay` and `desktop-backend` are untouched; a released native client depends on all of them on hosts.
- The askpass broker (`splitty_remote::askpass`) is described in `architecture/remote.c4` as being "for browser clients", but the embedded service and the owner backend use it for the native app's SSH questions (`ssh.prompts.*`). It stays; only its description changes. `GET /api/remote/ssh-prompts` is the catalog operation `ssh.prompts.get` that the native client reads after each `ssh.prompts.changed`; it stays.
- The hub's preparation step names (`install`, `daemon`, `update`, `started`, `bridge`) are also the native transport progress phases; only the gateway's translation into `remote.progress` text goes.

## Goals / Non-Goals

**Goals:**

- No TCP listener of the terminal service, no browser credential and no process token remain in the product; every remaining surface is a Unix socket or a pipe with its owner checks, plus the deliberate forwarded ports and reverse-share relays.
- The tree builds, lints and tests after each task group (A, then B, then C).
- Released native clients keep working against this version's hosts, and this version's native client against older hosts.

**Non-Goals:**

- Removing catalog operations, messages, capabilities or state that only the web client used (Follow-up inventory).
- Changing the control socket's framing or route set, the grant sockets, the keepers, the native endpoint, the native transport or the hub's host protocol.
- A replacement for browser access, including a development feature flag.

## Decisions

The decisions taken without the maintainer's confirmation are listed under Provisional decisions. Further technical choices:

- **Removed commands as stubs.** `serve`, `connect` and `issue-bootstrap` stay as hidden subcommands that accept any arguments, print one line and exit 2 (RBA-9). Removed flags (`--addr`, `--token`, `--oneshot-token`, `--open`, `daemon start --addr/--token/--remote`, `daemon restart --addr`) are not kept: the argument parser rejects them with its usage error and status 2, the status they already had.
- **The control router moves under `control`.** `crates/splitty-server/src/control.rs` becomes `control/mod.rs` with `control/router.rs` (from `http/mod.rs` and the control part of `http/routes.rs`), `control/jobs.rs` and `control/operation.rs`; the in-process tests under `src/http/tests/` move to `src/control/tests/` unchanged (RBA-10). `control` stays `#[cfg(unix)]`; nothing outside Unix used the router.
- **CLI unit tests over a Unix socket.** The tests in `cli/mod.rs` and `cli/jobs.rs` that served a router on `127.0.0.1:0` serve it with `axum::serve` on a `UnixListener` in a temporary directory instead and keep their assertions.
- **The exposure registry keeps older gateways' claims.** A live `Gateway` entry written by an older Splitty process of the same user stays a reserved port for sharing; this version writes none (RBA-12). Its tests create such an entry through a test-only helper behind the crate's `testing` feature.
- **Recent hosts.** `connect` was one of three writers of `recent-hosts.json`; the hub's ready host streams keep recording (the SSH `desktop-backend` helper goes with `remove-tauri-shell`), so `remote-hosts --json` and `/api/remote/hosts` keep their meaning (RBA-11).
- **Reverse-forward reservations.** The hub's own `reserved` port set only ever held gateway ports and goes; `local_port_reserved` stays for ports the exposure registry reserves.
- **Specs.** Requirements whose subject was only the gateway, the browser client or `serve`/`connect`/`issue-bootstrap` are REMOVED with a reason naming why the rule has no subject left; mixed requirements are MODIFIED to what holds for the native client, the CLI and hosts. Every removal in `access-control` was checked against the surfaces that remain (Risks); the part of Loopback-Only Listener that keeps a subject (no service listens on TCP) is restated as No Terminal-Service TCP Listener. A MODIFIED block cannot drop a scenario, so a scenario whose name stays true keeps its name with new content (for example Explicit target in `cli`: `--addr`/`--token` are now a usage error), and a requirement whose scenarios named only the web window or the browser is REMOVED and ADDED under a new name with its remaining text unchanged: Independent Client Lifetimes (Independent Connection Lifetimes), Heartbeat and Liveness (Connection Liveness), Status Notifications in the Window (Program Status Notices In The Window), Host Screen Directory Cache (Host Screen Directories), Terminal Palette Choice (Terminal Palette Rule), Confirmed Screen Replacement (Confirmed Template Replacement), Two Server Modes (Daemon And Embedded Modes), Embedded Mode Lifetime (Embedded Service Lifetime), and Ephemeral Port And Bind Before Detach (Daemon Binds Before Detach, without its port half). The hub's preparation, formerly specified by reference to `connect` (`cli`, Remote Connect Tunnel), is written out in `remote-screens`, Remote Screen Hub Endpoint, and its SemVer cases move to Host Update. Requirements that name operations by their HTTP path keep the names; `control-api`, HTTP Form Of Native Operations, says that no listener serves that form.

## Provisional decisions

All dated 2026-11-15, open to the maintainer's feedback; each has a bullet in `openspec/open-decisions.md`.

- **RBA-1** (provisional, tbd): browser access is removed without replacement and without a build feature that keeps it for development; terminal access without the app stays `splitty attach`, also over SSH. Alternative: keep the gateway behind a non-default cargo feature for development.
- **RBA-2** (provisional, tbd): `splitty connect` is removed, not turned into "open this host in the desktop app". Alternative: make it hand the target to a running desktop app.
- **RBA-3** (provisional, tbd): the CLI's explicit TCP target (`--addr` with `--token`) goes with the gateway; control commands and `attach` reach a server only through the control, agent and native sockets or a keeper. Alternative: keep a TCP control target for SSH tunnels.
- **RBA-4** (provisional, tbd): the process token is gone and nothing reads or writes `auth-token`; an existing file is left untouched, neither read nor deleted, and the release notes say it can be deleted. Alternative: delete it at daemon start.
- **RBA-5** (provisional, tbd): the owner control socket keeps its HTTP framing. Alternative: move it onto the native framing.
- **RBA-6** (provisional, tbd): `docs/protocol.ts` stays the wire reference, without the browser-only messages; a reference derived from the Rust types is out of scope. Alternative: generate it now.
- **RBA-7** (provisional, tbd): wire compatibility — nothing a released native client (0.3.0 and later) sends or relies on changes; a host daemon of this version keeps serving such a client, and this client keeps reaching older host daemons. Alternative: drop the transitional compatibility paths together with the gateway.
- **RBA-8** (provisional, tbd): the change is breaking for the CLI (`serve`, `connect`, `issue-bootstrap`, `--addr`, `--token`); the next release notes it, and releasing is not part of the change. Alternative: deprecate first and remove a release later.
- **RBA-9** (provisional, tbd): `serve`, `connect` and `issue-bootstrap` remain hidden stubs that print one line naming `splitty daemon start`, the desktop app and `splitty attach` (also `ssh -t <host> splitty attach`) and exit 2, until the next minor release after the one that ships this change. Alternative: delete them and let the parser report an unknown subcommand.
- **RBA-10** (provisional, tbd): what remains of `splitty_server::http` moves into `splitty_server::control` (`router`, `jobs`, `operation`); the architecture component `http` goes and `control` takes its remaining relationships. Alternative: keep the name `http` for the HTTP framing of the control socket.
- **RBA-11** (provisional, tbd): the hidden `splitty remote-hosts --json` stays (RES-2 keeps it for scripts); `connect --json` goes with `connect`, so RES-2 holds only for `remote-hosts`. Alternative: remove `remote-hosts` with the shells that consumed it.
- **RBA-12** (provisional, tbd): the exposure registry no longer creates gateway claims but keeps decoding them and keeps a live one, written by an older Splitty process of the same user, reserved against reverse sharing. Alternative: drop the gateway kind, which would make the registry refuse a record an older process still writes.
- **RBA-13** (provisional, tbd): `GET /health` goes with the TCP listener; the control socket gets no replacement route, since `splitty daemon status` and `GET /api/mux/status` on the control socket answer liveness. Alternative: serve `/health` on the control socket.

## Risks / Trade-offs

- [The hub's real-SSH coverage depends on the gateway today] → The `HubServer` fixture of `remote_connect.rs` is rebuilt on `desktop-backend --transport native-v1 --backend embedded --owner` with the owner client in `splitty_remote::owner`, as `an_owned_backend_asks_its_ssh_questions_over_the_owner_connection` already does, before the gateway code is deleted (task group A).
- [Supervisor reconnect, reinstall after a rebuild and daemon restart on a rebooted host were tested through `connect`] → The hub runs install and `remote-handshake --ensure-daemon` on every host stream; the moved tests open a host stream, break it (kill the bridge, remove the install, stop the host daemon) and open the next one, asserting the same host-side outcomes. The ten-minute `connect` retry loop has no successor to test: reconnecting is the native client's (`remote-screens`, Remote Screen Reconnect).
- [Daemon replacement under a browser gateway] → `an_explicit_daemon_browser_gateway_survives_daemon_replacement_and_detaches_cleanly` goes; native discovery of a replaced daemon stays covered by `native_discovery_loss_corruption_and_concurrent_start_cannot_replace_a_live_daemon` and `a_shell_outlives_stop_restart_update_and_crash_of_its_daemon`.
- [Security invariants that still have subjects] → Checked in code for what stays: the control socket (`control.rs`: 0700 directory, 0600 socket, peer UID check, no credential, narrow routes), grant sockets (`control::spawn_grant`, same checks, job routes only), keeper sockets (`splitty-core` keeper, same rules), the native endpoint (`native::endpoint`, peer checks both ways, instance verification), forwarded ports (`tunnel::PortTunnel`, `-L 127.0.0.1:…`, announced as `localhost`) and reverse-share relays (`host::relay`, owner check of connecting sockets). None of them reads the token, a cookie or a ticket, so removing those rules weakens none of them. The rules that keep a subject are rewritten without their browser clauses rather than removed: Hub Port Forwards, Hub Reverse Forwards, Internal Endpoints Cannot Be Shared, Forwarding Reservation Lifetime, JSON Request Bodies, Credential Files Are Owner-Only.
- [Forward URLs and cookies] → With no Splitty origin on `127.0.0.1`, the reason for announcing forwards as `localhost` (no Splitty cookie) is gone, but announcing `localhost` stays: a browser may still hold a cookie of an older Splitty gateway, and the rule costs nothing.
- [Users of `serve`/`connect`/`issue-bootstrap`/`--addr`] → Breaking; the stubs name the ways that remain (RBA-9), and the release notes say it (RBA-8).
- [Concurrent open changes rewrite the same requirements] → The overlaps are listed below and each has a task.
- [Archive order with `remove-tauri-shell`] → The deltas here are based on the main specs with that change's deltas applied; if it is reworked before landing, the overlapping requirements here are rebased in the same task that rebases the others.

### Overlaps with open changes

Open changes with MODIFIED deltas of requirements this change modifies or removes:

- `remove-tauri-shell` (lands first): `access-control` (no common requirement), `client-connections` Resource Admission, `desktop-remote` Remote and Local Folder Editor Handoff, `remote-hosts` Payload Sources, `server-modes` Desktop App Hosting On macOS And Linux. The text here is the main spec with that change's delta applied plus this change's edit; Desktop Backend Child Lifetime, which that change adds, keeps its `serve --desktop-stdio` scenario, and Removed Browser Commands keeps that pointer (task 0.1 rebases if that change is reworked).
- `native-host-update` (`remote-screens`, Host Update): this change modifies Host Update (no `/ws/remote` `update=1`, SemVer cases moved in from `cli`). The delta here is based on the current main text, which is newer than `native-host-update`'s delta; task 4.2 rebases whichever of the two is archived second.
- Drafts that name requirements renamed here: `terminal-bell` (Status Notifications in the Window), `workspace-restore` (Two Server Modes, Embedded Mode Lifetime); task 4.3.
- No other open change modifies a requirement touched here; `native-drawer-host-ports` (Native Forwarded Browser Activation), `native-remote-ux` (Saved Forward Preferences API, Stop And Forget Saved Forwards, Remembered Remote Screens) and `native-screen-reorder` (Remote Screens In The Window, Layout Operations, Screen Rename, Activation and Order) touch requirements this change leaves alone.

## Test plan

The levels are those of `CLAUDE.md`.

- In-process: the gateway tests in `src/http/tests/gateway.rs` are deleted. The remaining in-process tests move with the router to `src/control/tests/` unchanged. `conformance.rs` loses its gateway comparisons and keeps native-versus-control comparisons.
- Smoke, `http_surface.rs`: the gateway tests are deleted. The tests that only used `serve` to get a server with a control socket — `control_socket_serves_mux_operations`, `control_socket_cannot_reach_privileged_endpoints`, `control_socket_is_private_to_the_owner`, `agent_control_socket_is_private_and_only_routes_owned_jobs`, `agent_socket_rejects_another_uid_even_if_file_modes_are_loosened`, `create_screen_over_a_screen_with_panes_is_rejected`, `an_agent_reads_and_answers_a_review_through_the_cli`, `inherited_diagnostics_do_not_block_failed_command_inventory` — move to `tests/daemon_lifecycle.rs` against `splitty daemon start`; they stay smoke tests because they exercise the binary's own socket, peer checks and CLI. `control_socket_cannot_reach_privileged_endpoints` asserts that `/api/session`, `/api/bootstrap` and `/api/ws-ticket` answer 404, as before. `removed_desktop_stdio_and_busy_browser_binds_fail_before_creating_a_workspace` becomes a check that the `serve` stub exits 2 without a workspace or listener.
- Smoke, `daemon_lifecycle.rs`: `an_explicit_daemon_browser_gateway_survives_daemon_replacement_and_detaches_cleanly` and `connect_keeps_its_local_gateway_and_cookie_when_the_native_host_is_replaced` are deleted (Risks). `agent_cli_is_discoverable_in_packaged_sessions_and_exec_clients` and `managed_jobs_survive_cli_clients_and_resumed_grants` drop `issue-bootstrap`, `serve` and `--addr`/`--token` from their refused-command lists and assert that the flags are usage errors. `a_busy_tcp_port_is_irrelevant_and_legacy_listener_flags_are_refused` keeps the busy-port half and asserts the parser's usage error for the flags. The `auth-token` absence checks stay as regression checks (RBA-4).
- Smoke, `remote_connect.rs`: `HubServer` moves to the owner backend; `the_hub_relays_a_host_through_real_ssh`, `the_hub_updates_a_host_daemon_of_another_version`, `the_hub_waits_for_real_ssh_bind_after_the_pre_auth_gate`, `saved_forwards_restore_lazily_after_real_hub_restart_and_forget_stays_live`, `the_hub_forwards_a_port_a_host_pane_listens_on` and `a_shared_port_reaches_a_local_service_from_the_host_through_real_ssh` keep their assertions on it. The `connect` tests whose subject is the host — `a_host_without_splitty_is_reported`, `connect_starts_a_daemon_on_a_host_without_one`, `connect_installs_its_server_and_survives_a_reboot_and_a_rebuild`, `connect_reconnects_and_hands_over_sessions_through_real_ssh` — become hub tests on the same fixture (the host-side outcome is the same; only the local side changes). `the_bridge_carries_native_frames_through_real_ssh` and `fixture_cleanup_refuses_processes_outside_its_network_namespace` stay. The gateway-only tests — `connect_json_reports_progress_the_session_and_its_end`, `connect_opens_only_its_locally_issued_launch_url_by_default`, `connect_json_reports_why_it_failed_and_records_nothing`, `permanent_reconnect_failure_releases_gateway_and_all_local_descendants`, `connect_forwards_ports_of_the_host_through_a_reconnect`, `connect_rejects_an_invalid_forward_before_ssh_runs`, `the_browser_verifies_open_and_cross_window_stop_through_real_ssh` — are deleted; their host-side parts (forwards through a reconnect, recording a recent host only on readiness) are asserted in the moved hub tests.
- Smoke, `remote_connect/exposure_crash.rs`: deleted; the share-reservation lifetime after a killed coordinator stays covered by `splitty-remote`'s `tunnel/exposure/registry/crash.rs` and `tests/native_exposure.rs`, which probes with the test-only gateway-record helper instead of `bind_gateway` (two shares of one port do not conflict, so a second `Share::reserve` cannot show the block).
- `version_skew.rs` and `native_endpoint.rs` need no change; a released native client's paths are unchanged (RBA-7).
- Frontend vitest goes with `frontend/`.

## Migration Plan

- Order: task group A (server, core, remote, CLI and their tests) with the moved smoke fixtures first, then the deletions; group B (`frontend/`, builds, CI, release, scripts, packaging); group C (specs, model, docs). The tree builds after each group; the architecture check needs the model update of group C in the same commit as group A's module changes, so the model edits that track code (removing `http`, `embed`, `auth`, the relationships) are part of group A's last task.
- Users: `auth-token` stays on disk, unused (RBA-4). A running older `splitty serve` or `connect` of the same user keeps working until it ends; this version neither stops nor adopts it, and its port stays reserved against sharing (RBA-12).
- Rollback: revert the change; no data format changes.

## Architecture impact

- `architecture/server.c4`: component `http` removed, its remaining relationships (`jobs`, `service`, `control` grant sockets, `splitty.core.*` in tests, `splitty.remote.ssh_askpass` in tests, `splitty.transport.*` in tests, `cli` in tests, `hub` and `client_session` in tests) move to `control`; component `embed` removed with `embed -> splitty.core.config` and `embed -> splitty.core.theme`; `main -> http`, `main -> splitty.core.auth`, `main -> splitty.remote.tunnel 'open, supervise, GIVE_UP_AFTER, ConnectEvent'`, `main -> splitty.transport.admission` and `main -> splitty.transport.negotiation` (gateway reasons) removed or narrowed; `control -> http` removed; descriptions of `main`, `cli`, `hub`, `control` updated.
- `architecture/core.c4`: component `auth` and `auth -> config` removed.
- `architecture/remote.c4`: `ssh_askpass` described as the local service's prompt broker; `tunnel` without the browser gateway.
- `architecture/landscape.c4`: container `web` and its relationships (`user -> splitty.web`, `splitty.web -> splitty.server.http` twice, `splitty.mcp -> splitty.server.http` retargeted to `control`, `splitty.core.auth -> splitty.daemon_files`) removed; system and store descriptions updated (no `auth-token`).
- `architecture/views.c4`: `splitty.web` and `splitty.server.http` out of the connection view.
- `docs/architecture.md`: "Client Autonomy (Shadow Mode)" with "Start-up and reload" and "Attaching to sessions", the browser parts of "PTY Size", "Two Protocol Paths", "Connection Lifecycle", "Server Modes" with "Listener and authority inventory", "Remote Hosts", "Remote Screens", "Module Map" with "Browser preparation and delivery", and the web-client halves of the port and editor sections are removed or rewritten; `docs/networking.md` loses the browser gateway path.

## Follow-up inventory

Candidates for the follow-up change, found on the way (not hunted systematically). "Unused" means no `Operation::<name>` and no path match in `crates/splitty-client-core`, `crates/splitty-gpui`, `crates/splitty-server/src/cli` or `src/main.rs`:

- Catalog operations unused by the native client and the CLI (`crates/splitty-core/src/native_api.rs`): `theme.get` (`GET /api/home/theme`), `theme.list` (`GET /api/home/themes`), `shellProfiles.update`, `template.extract`, `template.save`, `command.send` (`POST /api/command`; the native client receives `command` events but sends none), `host.list` (`GET /api/remote/hosts`), `hostPreferences.get`, `hostOrder.get`, `remoteScreens.update` (`PUT /api/remote/screens`), and the legacy `catalog.get`/`catalog.update` (`/api/remote/catalog`, kept "for older clients" in `hub/catalog.rs`).
- Host streams through a local daemon's private endpoint: the daemon endpoint accepts a host-open intent (`wire::OpenHost`), whose only client was the gateway's `Connector::local_host` (`src/native/client.rs`); the server side stays here because a native peer could send it.
- `listenAddr` in `GET /api/mux/status` (`control-api`, Server Status) is now specified as always `null`; dropping the field is the follow-up's (a released native client may read it, RBA-7).
- `remote.progress` text before `remote.ready`: native readers ignore it from hubs of the previous release (`remote-screens`, Hub of the previous release scenario); the tolerance can go once those hubs are out of support.
- The web-client polling path of `ssh.prompts.get` and the hub's change feed for `hub-host-decisions-v1` web windows (`docs/architecture.md`, Hub-owned host metadata).
- Layout shadowing: `arrange_panes` for drags and `select_tab` reporting were the browser workspace window's; the native client sends `arrange_panes` only for a whole tab group (`mux-protocol`, Window Layout Changes Reach the Server).
- `mux.welcome` fields `clientId` and `serverMode` and the `desktop=1` launch parameter's consumers in the frontend (`docs/protocol.ts`).
- `service::State` and `client_session` comments and fields that name "browser adapters" (`src/service/mod.rs`, `src/hub/reverse.rs:103`, `src/native/client.rs:151`) once their code is gone.
