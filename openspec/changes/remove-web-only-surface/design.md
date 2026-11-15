## Context

`remove-browser-access` (2026-11-15) removed the web client, the browser gateway, the process token, `serve` and `connect`; `remove-tauri-shell` removed the shell around the web client. Its design kept the wire unchanged for released native clients (RBA-7) and listed what was left over. This change removes the part of that surface that no remaining client uses.

Remaining clients and peers, against which every item was checked:

1. The released native client at every tag from v0.3.0 on (v0.3.0-rc.1, v0.3.0, v0.3.1 to v0.3.8, v0.4.0 to v0.4.4, v0.5.0) and the current tree: `crates/splitty-client-core` and `crates/splitty-gpui`.
2. The CLI (`crates/splitty-server/src/cli`, `src/main.rs`), agent grant clients, `splitty attach`.
3. Hubs and hosts of other versions, both directions (`crates/splitty-server/tests/version_skew.rs`, `src/hub`, `src/host`, `crates/splitty-remote`).
4. Persisted data: `config.toml`, theme files, workspace and template files, hub files (`remote-catalog.json`, `remote-host-preferences.json`, `remote-host-order.json`, `recent-hosts.json`, `remote-screens.json`), the exposure registry. (`settings.json` is no Splitty file; Splitty never reads or writes one.)

Evidence command, written `T(<re>)` below: for every tag above and HEAD, `git grep -n -E '<re>' <tag> -- crates/splitty-client-core crates/splitty-gpui` (and, where named, the CLI, hub and host paths). "0 everywhere" means no match at any tag and none at HEAD. Web use was read from `frontend/` at the same tags.

How a removed item reaches an old sender:

- An unknown native operation is refused by request validation: the native connection answers 400 `unsupported or malformed native request` and keeps the connection. The owner control socket never routed these operations (it routes only the control route set) and answers 404 as before.
- A session message of an unknown type is refused by the native session validation, which ends that connection (relayed host streams validate the same way). This conflicts with `mux-protocol`, Malformed and Unknown Messages, which promises an `error` event and an open connection; the conflict exists today for every unknown type and is recorded as a finding (RWS-6), not changed here.
- An unknown workspace operation over the session protocol is ignored without a response (Malformed and Unknown Messages); over `POST /api/mux/op` it is a 400.
- An event type a client does not know is ignored by every native client (they dispatch on the message type and treat any other type as unknown), and absent optional fields are not read.

### Inventory

| Item | Kind | Defined in | Used by | Evidence | Verdict |
|---|---|---|---|---|---|
| `shellProfiles.update`, `PUT /api/shell-profiles` | operation | `native_api.rs` catalog, `service/operations.rs` `put_shell_profiles`, `shell_profiles.rs` `apply_shell_profiles_update` | web settings `shell_profiles.ts` (v0.3.0–v0.5.0) | `T(ShellProfilesUpdate\|shellProfiles\.update\|/api/shell-profiles)`: 0 everywhere in native, CLI, hub, host; native reads `shellProfiles.get` only | Remove (RWS-2) |
| `host.list`, `GET /api/remote/hosts` | operation | catalog, `service/dispatch.rs` `HostList` | web `api.ts` | `T(HostList\|host\.list\|/api/remote/hosts)`: 0 everywhere except `host.forget` (DELETE, same path) | Remove; `host.forget` stays |
| `hostPreferences.get` | operation | catalog, dispatch `HostPreferencesGet` | web | `T(HostPreferencesGet\|hostPreferences\.get)`: 0 everywhere; native reads aliases and hidden state from `remote.directory.get` | Remove; `hostPreferences.update` stays |
| `hostOrder.get` | operation | catalog, dispatch `HostOrderGet` | web | `T(HostOrderGet\|hostOrder\.get)`: 0 everywhere; the directory carries the saved order | Remove; `hostOrder.update` stays |
| `catalog.get`, `catalog.update`, `/api/remote/catalog` | operation | catalog, dispatch `CatalogGet`/`CatalogUpdate`, `hub/catalog.rs` | web `host_directory.ts` (v0.3.x only) | `T(CatalogGet\|CatalogUpdate\|catalog\.(get\|update)\|/api/remote/catalog)`: 0 everywhere; natives use `hub.catalog.observe` and `remote.directory.get`; LocalHub scope, so no hub calls them on a host | Remove the operations; the hub's directory store stays |
| `create` | session message | `protocol/messages.rs` `MSG_CREATE`, `dispatcher.rs` `handle_create`, `native_api.rs` session list | web `session_client.ts` (every tag) | `T("create"\|MSG_CREATE)` over client crates, CLI, hub, host: 0 (natives start sessions with workspace ops and `mux.pane_spawned`; `attach` sends `attach`) | Remove |
| `close` | session message | `MSG_CLOSE`, `handle_close`, session handler job interception | web `terminal_session.ts` | `T("close"\|MSG_CLOSE)` as above: 0; natives close panes with `close_pane` | Remove |
| `reorder_screens` | workspace op | `mux/ops.rs`, `reducer.rs`, decoders in `mux_handler.rs` and `service/operations.rs` | web `screen_sync.ts` | `T(reorder_screens\|ReorderScreens)`: 0 everywhere; natives send `move_screen` | Remove |
| `created` | event | `ServerEvent::SessionCreated`, `MessageType::Created` | web | `T(MessageType::Created\|"created")`: no native handler; CLI and hub do not read it | Remove |
| `title` | event | `ServerEvent::TitleChanged`, `MessageType::Title`, `watch_osc_title`/`watch_title`, `service/pane_state.rs` exit path | web tab titles | `T(MessageType::Title\|TitleChanged)`: 0 handlers; natives read `displayTitle`/`busy` from snapshots | Remove the event; snapshot metadata and the foreground-process feed for notifications stay |
| `attached.homeDir` | field | `SessionAttached` | web | `T(homeDir\|home_dir)` in client crates: 0 reads of the event field | Remove |
| `notice.kind` | field | `protocol/notice.rs` `NoticeKind` | web `notifications.ts` (v0.5.0) | v0.3.x natives classify events themselves; v0.4.0+ read `severity`, `title`, `setting`, `suppressed` (`notifications.rs` `notice()`), not `kind` | Remove |
| `mux.welcome.clientId`, `.screens`, `.serverMode` | fields | `ServerEvent::MuxWelcome`, `mux_handler.rs` `send_welcome` | web | natives read the type and `capabilities` (`native_workspace/source.rs`); `attach` and the hub read the type; status keeps `serverMode` | Remove; internal client identities stay |
| `scratchpad-v1` | capability | `protocol/capability.rs` | web | `T(scratchpad-v1\|SCRATCHPAD)`: 0 native reads; every server from v0.3.0 announces it | Remove the announcement |
| `serverDerivedPaneState` | capability | `capability.rs` | web | `T(serverDerivedPaneState\|SERVER_DERIVED)`: 0 native reads | Remove the announcement |
| `listenAddr` | status field | `service/operations.rs` `get_mux_status` | none (`null` since v0.3.0) | no reader at any tag besides v0.3.0 Electron smoke scripts, which are gone | Remove |
| `openHost` in the transport hello | transport field | `splitty-transport` `wire::Hello`/`OpenHost`, `codec/json.rs`, `negotiation::server_main`/`client_with_host`, daemon endpoint branch in `native.rs`, `native/client.rs` `endpoint_host_fixture` | the browser gateway's local host streams (v0.5.0 `Connector::local_host`) | `T(open_host\|OpenHost\|client_with_host)`: 0 in native, hub, CLI; the desktop app opens host streams with the owner helper's open command | Remove (RWS-5) |
| `terminal.renderer`, `terminal.ligatures`, `terminal.windowPaddingBalance`, `terminal.xterm.*` | settings | `config/settings.rs` | web terminal | `T(renderer\|ligatures\|windowPadding\|xterm\|scrollSensitivity)` in client crates: 0 (one comment); native patch set does not contain them | Remove |
| `tabs.*`, `window.*`, `experimental.*` (`linkedSashes`, `resizeDebounce`), `drawer.resizable` | settings | `config/settings.rs` | web | `T(linkedSashes\|resizeDebounce\|resizable)`: `resizable` decoded with a default by v0.3.x natives, never required; HEAD has no reader | Remove |
| `terminal.linkTarget`, `terminal.webviewMode` patch tolerance | settings | `validate_patch` skip | web clients before their removal | no sender at any tag from v0.3.0 | Remove the skip (RWS-3) |
| theme `[effects]` | theme key | `theme/types.rs`, `resolve.rs`, bundled themes | web effects | natives v0.3.0+ decode the theme answer with `effects` defaulted; HEAD has no reader | Remove; files with it still load |
| `layout.dnd-overlay-scope`, `layout.dnd-overlay-target` | theme keys | `LayoutTokens` | web drag overlay | v0.3.0/v0.3.1 natives require the `layout` object but default every token | Remove the tokens; keep `layout: {}` (RWS-4) |
| CLI `Transport` with one variant | code | `cli/discovery.rs` | CLI internals | single `LocalSocket` variant since the TCP target went | Replace by the socket path |
| `tokio-tungstenite` dev-dependency | code | `crates/splitty-server/Cargo.toml`, `control/tests.rs` harness | the control tests' message type | the harness only uses the crate's message enum; the session crate has its own | Remove |
| `RemoteError`, `Failure` in the SSH tunnel | code | `splitty-remote/src/tunnel/mod.rs` | the removed `connect` supervisor | no use outside the module's own tests | Remove |
| comments, test names | code | see task 2.3 and 4.3 | — | they describe a browser, gateway, WebSocket, token or frontend as present | Reword or remove |

### Items checked and kept

| Item | Reason |
|---|---|
| `theme.get`, `theme.list` | read by v0.3.0 and v0.3.1 natives |
| `template.extract`, `template.save`, `remoteScreens.update` | sent by natives up to v0.3.8 |
| `command.send`, `mux.neighbor` | never web-only: an authorized native operation with a native consumer, and a documented control route |
| `create_job`, `stop_job`, `forget_job` session forms, `attached` semantic fields, `managed-jobs-v1`, `hub-auto-forward-v1` | used by native, CLI or hub, or never read by the web client |
| `bell` | the draft `terminal-bell` builds on it |
| `remote.progress` tolerance | hosts and hubs of 0.3.x send it to new natives |
| `ssh.prompts.get`, `hub.changed`, `arrange_panes`, `select_tab` | native uses them |
| `appearance.borderRadius`, `drawer.showTips`, theme `ui` and `terminal` tokens, the `layout` object | required by released natives' decoding |
| `nativeTabs.*`, the legacy `webview` decoding | native settings; restoring old workspace files |
| `shells` in `home.update` | no client sends it, but it was not a web-only path either; it bypasses the shell profile validation, recorded as a finding |
| `docs/protocol.ts` | specs, `openspec/config.yaml` and crate comments refer to it (RBA-6); only entries of removed items change |
| the exposure registry's older gateway claims, `SPLITTY_BROWSER_ASKPASS`, system-browser URL opening | RBA-12, a rename is out of scope, desktop use |

### Corrections to the earlier inventory

- `listenAddr` has been `null` since v0.3.0; no released native reads it.
- `command.send`, `mux.neighbor`, `hub-auto-forward-v1` and `managed-jobs-v1` were never web-only.
- The catalog routes were used by the web client only up to v0.3.x.
- The `RemoteError`/`Failure` leftovers of the `connect` supervisor were not in the list.

## Goals / Non-Goals

**Goals:**

- Remove every item of the inventory marked "Remove", with the old sender's outcome stated above.
- Keep every remaining client working in both skew directions; keep `config.toml` and theme files loading.
- Reword spec requirements that describe the system through the web client where the statement holds for the desktop app; remove the ones whose subject is gone.

**Non-Goals:**

- Removing anything a remaining client uses; renames; refactors beyond the removal; new features.
- Changing the handling of unknown session messages (finding only, RWS-6).
- The hidden `serve`/`connect`/`issue-bootstrap` stubs.

## Decisions

1. **Sessions start only for panes.** With `create` gone, the session dispatcher keeps `attach`, `detach`, input, resize and the job session forms. The rules for shell selection, startup command, working directory and environment stay and are stated for the pane spawn path (`Effect::SpawnSession` and the job launcher), which already applies them. Session tests that used `create` as setup create a pane through a workspace operation and attach to `mux.pane_spawned`'s session, or use the PTY manager directly in unit tests.
2. **Ending a session is ending its pane.** `close` goes; `close_pane`, closing a screen, process exit and server shutdown end sessions. The session handler's interception of `close` for jobs goes with it.
3. **The welcome keeps `capabilities` and its envelope.** Windows learn the workspace from `mux.workspace_resync`/`mux.workspace_snapshot` (`workspace-catalog-v1`), which every server since v0.3.0 announces. Internal per-connection identities stay for registrations and size reports but are no longer sent.
4. **Remove the hello field, not refuse it.** `openHost` leaves the transport hello type. A hello still carrying it is a hello with an unknown key, which the strict hello decoding already refuses before acceptance, on every carrier (RWS-5).
5. **Settings files are rewritten without the removed keys.** `config.toml` is merged into the defaults and then deserialized; unknown keys are ignored. A load writes the file back, so removed keys disappear at the first load. They have no effect on any client of this version; an older server reading the file afterwards takes its defaults for them. Theme files are the user's and never written; their removed sections are ignored.
6. **Capabilities whose behavior stays are not announced.** `scratchpad-v1` and `serverDerivedPaneState` were read only by the web client; the scratchpad and the server-derived pane state remain and every supported server provides them.
7. **Spec deltas follow the open change `native-screen-reorder`.** Its workspace-model deltas modify Layout Operations and Screen Rename, Activation and Order; this change's deltas are written against its text and require it to be archived first (RWS-7).

## Provisional decisions

All dated 2026-11-15, provisional, tbd, recorded as the block `remove-web-only-surface` RWS-1 to RWS-7 in `openspec/open-decisions.md`.

- **RWS-1:** browser windows of older releases, served by the gateway of an older hub (0.5.0 and earlier `splitty serve`), are not supported clients of hosts of this version: their `create`, `close` and `reorder_screens` are refused, and a refused `create` ends that relayed stream. (provisional, tbd) Alternative: keep the host-side handling of these messages until the releases with the gateway are out of support.
- **RWS-2:** `shellProfiles.update` is removed although a future desktop shell profile editor might use it; shell profiles are edited in `config.toml`. (provisional, tbd) Alternative: keep the operation for that editor.
- **RWS-3:** the update tolerance for `terminal.linkTarget` and `terminal.webviewMode` goes; an update naming them is refused as an unknown setting. Stored values keep being ignored. (provisional, tbd) Alternative: keep skipping them in updates.
- **RWS-4:** the theme answer keeps an empty `layout` object, since natives 0.3.0 and 0.3.1 require the key. (provisional, tbd) Alternative: drop `layout` and accept that those releases no longer decode themes of this server, or keep the two drag-overlay tokens.
- **RWS-5:** `openHost` is removed from the transport hello type, so a hello with it fails as malformed on every carrier, instead of being kept and refused with its own error. (provisional, tbd) Alternative: keep the field and refuse it with a specific error.
- **RWS-6:** removed session message types get today's handling of unknown session types (the native connection ends); the conflict with `mux-protocol`, Malformed and Unknown Messages, is recorded in `docs/issues/openspec-baseline-findings.md`. (provisional, tbd) Alternative: answer `error` and keep the connection, as the spec says, in this change.
- **RWS-7:** `native-screen-reorder` is archived before this change is applied; this change's workspace-model deltas are based on its text. (provisional, tbd) Alternative: fold its remaining task into this change.

## Risks / Trade-offs

- **Version skew.** A window of a released version (0.3.0 on) talking to a host of this version: none of the removed items is sent or required by it (inventory); `layout: {}` stays for 0.3.0/0.3.1 theme reads. A window of this version talking to an older host: older hosts still send `created`, `title`, the old welcome fields, `notice.kind` and the two capabilities; the window ignores unknown types and fields, as today. An older hub relaying for a window of its own version to a host of this version: the hub's window sends none of the removed items. A browser window behind an older hub's gateway loses `create`/`close`/`reorder_screens` on hosts of this version (RWS-1). An older `splitty attach` or CLI uses `attach`, the control routes and the job routes, none removed.
- **Downgrade.** After a downgrade, an older server reads a `config.toml` without the removed keys and takes its defaults; nothing breaks.
- **Test churn.** Many session tests use `create` as setup; their migration is the largest part of the work and must keep each test's assertion.
- **Spec drift.** The spec text that described the system through the web client is reworded in the same change; anything the rewording finds untrue for the desktop app is recorded as a finding instead of being specified.

## Test plan

- **Unit:**
  - `crates/splitty-core/src/protocol/dispatcher.rs` tests: `create` and `close` are refused as unknown session types; tests that used `create` as setup create the session through the PTY manager and attach.
  - `crates/splitty-core/src/protocol/message_type.rs` tests: `created` and `title` are unknown types.
  - `crates/splitty-core/src/protocol/capability.rs`/`mux_handler.rs` welcome test (`the_welcome_lists_its_capabilities_in_a_fixed_order`): the list without `scratchpad-v1` and `serverDerivedPaneState`; the welcome has no `clientId`, `screens`, `serverMode`.
  - `crates/splitty-core/src/protocol/notice.rs`/`messages.rs` tests: notices without `kind`, same severity, title, setting, suppression.
  - `crates/splitty-core/src/mux/store.rs` tests: `reorder_screens` no longer parses; the `move_screen` tests set up order without it.
  - `crates/splitty-core/src/config/settings.rs` tests: a `config.toml` with every removed key loads, keeps the other values and is written back without them; an update naming a removed key or `linkTarget` is a 400 with that field; the fractional zoom level is accepted.
  - `crates/splitty-core/src/theme/*` tests: a theme file with `[effects]` and the drag-overlay tokens loads; the resolved theme has no `effects` and an empty `layout`.
  - `crates/splitty-transport/src/negotiation.rs` tests: a hello with `openHost` is refused on the main and the remote carrier; the owner-preparation test goes.
  - `crates/splitty-core/src/native_api.rs` tests: the removed operation names are refused by request validation.
- **In-process:**
  - `crates/splitty-server/src/control/tests/conformance.rs`: removed operations over a native connection answer 400 and keep the connection; `create` on a session stream is refused; host-open in the hello is refused on the daemon endpoint without SSH preparation; `remote.directory.get` replaces `host.list` as the HTTP-form example.
  - `crates/splitty-server/src/control/tests.rs` and `control/tests/{editor,files,pty_size,pane_state,program_status}.rs`: setup through a pane spawn; status without `listenAddr`; no `title` event on output or exit while snapshots keep `displayTitle` and `busy`.
  - `crates/splitty-server/src/client_session/mux_handler.rs` tests: welcome shape; scratchpad test without the capability check; reorder test removed.
  - `crates/splitty-server/src/hub/native.rs` tests: the host reads go; host-preference and order updates, forget and directory stay.
- **Smoke:**
  - `crates/splitty-server/tests/daemon_lifecycle.rs`: the existing tests that create sessions over a native stream switch to a pane spawn; the host-list calls switch to `remote.directory.get`. No new smoke test.
  - `crates/splitty-server/tests/version_skew.rs`: unchanged and must pass.

## Migration plan

1. Archive `native-screen-reorder` (RWS-7).
2. Apply the task groups; groups 1, 2, 3 and 5 can run in parallel worktrees; group 4 follows group 3; group 6 is last.
3. Release notes of the next release: removed settings and theme sections are ignored and dropped from `config.toml`; browser windows of older releases behind an older hub no longer work against hosts of this version.
4. Rollback is a revert; data written in the meantime (a `config.toml` without the removed keys) is valid for older versions.

## Architecture impact

- `architecture/server.c4`: descriptions change, no relationship is added or removed. `hub` no longer lists `/api/remote/hosts` and `/api/remote/catalog` reads; `native -> splitty.remote.ssh` loses "explicit host-open intent" (the owner helper still validates targets through it); `control -> splitty.remote.ssh` loses "native host-open conformance targets".
- `architecture/transport.c4`, `landscape.c4`, `core.c4`, `client-core.c4`, `remote.c4`: checked, no description names a removed item; the host opening in `client-core.c4` is the window's own and stays.
- `docs/architecture.md`: the connection sequence (`mux.welcome` contents), "Hub-owned host metadata" (no legacy catalog routes), the `serverDerivedPaneState` paragraph, the shell profile fallback in `created`, and the native endpoint's host-open paragraph.
