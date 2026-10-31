# Security findings

The index behind the `security review SR-xx` references in the code. SR-01 to
SR-25 come from an external review on 2026-08-02 and its remediation;
SR-26 onwards from a re-review on 2026-10-03. For the resulting security model
and how to report a vulnerability, see [`SECURITY.md`](../../SECURITY.md).

Severity is as assessed at the time of the finding. "Fixed" means the issue
is closed in the current code; where a test pins the behavior, it is named.

## Summary

| ID | Severity | Finding | Status |
|---|---|---|---|
| SR-01 | Critical | Same-origin proxy rendered foreign web content inside the app | Fixed: removed |
| SR-02 | High | DOM XSS sink; no central URL/scheme check | Fixed |
| SR-03 | High | Native Tauri commands reachable from untrusted contexts | Fixed |
| SR-04 | High | One long-lived token exposed in URL, browser storage, requests and logs | Fixed |
| SR-05 | High | Every shell inherited the master token | Fixed (see note) |
| SR-06 | High | Unsigned release artifacts, no verifiable release path | Mitigated, signing accepted as open |
| SR-07 | High | Direct remote mode without TLS, origin or bind invariants | Fixed: removed |
| SR-08 | Medium | Authenticated SSRF through URL fetchers | Fixed: removed |
| SR-09 | Medium | Unbounded buffering and missing load limits | Mostly fixed |
| SR-10 | Medium | Known dependency advisories, no audit gate | Fixed |
| SR-11 | Medium | Missing web hardening; DevTools in release builds | Fixed, one accepted deviation |
| SR-12 | Medium | Templates ran embedded commands without confirmation | Fixed |
| SR-13 | Medium | SSH target injection; unchecked tunnel start | Fixed |
| SR-14 | Medium | Security documentation contradicted the code | Fixed (again in SR-33) |
| SR-15 | Low | Credential files not hardened on every path | Fixed |
| SR-16 | Low | Path traversal through theme names | Fixed |
| SR-17 | Low | Empty expected token compared as valid | Fixed |
| SR-18 | Info | Unauthenticated `/health` allows fingerprinting | Accepted |
| SR-19 | Medium | WSL backend could run as root | Fixed |
| SR-20 | Medium | Terminal output could read and write the clipboard (OSC 52) | Fixed |
| SR-21 | Medium | `daemon.json` held the master token with default permissions | Fixed |
| SR-22 | Low | Second fail-open comparison with an empty token | Fixed |
| SR-23 | Low | SPA fallback answered removed API paths with 200 | Fixed |
| SR-24 | Design | Bootstrap minting on the control socket would undo SR-05 | Fixed before release |
| SR-25 | Info | Windows build could not be verified locally | Fixed: built in CI |
| SR-26 | Medium | Tauri opener had no URL scope; fallback opened pages in a WebView2 popup | Fixed |
| SR-27 | Medium | No navigation/new-window guard in Tauri; OSC 8 links bypassed the scheme check | Fixed |
| SR-28 | High | Electron 35 end-of-life; all permissions granted; no fuses | Fixed |
| SR-29 | Medium | Planned dependency updates never applied; unused TLS stack shipped | Fixed |
| SR-30 | Low | `splitty server` printed the process token | Fixed: removed |
| SR-31 | Low | Cross-port GETs with the session cookie safe only by convention | Fixed |
| SR-32 | Low | Bootstrap codes never expired | Fixed |
| SR-33 | Medium | Docs advertised removed features and a same-user boundary that does not exist | Fixed |
| SR-34 | Info | The SR-11 CSP blocked WebAssembly, breaking the ghostty renderer | Fixed |

## Details

**SR-01.** Web-app panes were served through `/proxy/{port}` on Splitty's own
origin, so any embedded page could reach the DOM, the session and the terminal
API. Web panes, the proxy, `/api/favicon`, `/api/frame-check` and `/api/webapps`
are deleted; the paths answer 404
(`webview_routes_are_absent_even_with_a_valid_token`). Links open in the
system browser, and the desktop shells refuse to render other origins (SR-27).

**SR-02.** Removed with SR-01. `openExternalTarget`
(`frontend/src/lib/window.ts`) forwards only `http:`/`https:`; the CSP sets
`frame-src 'none'` as a regression guard (`app_document_carries_a_strict_csp`).

**SR-03.** No subframes remain. Tauri skips its ACL for custom commands unless
the app declares a manifest; `crates/splitty-tauri/build.rs` now declares all of
them, and `capabilities/` grants them per context: `wsl_start_backend`, which
hands out a bootstrap URL, only to the bundled setup page.

**SR-04.** Split into a single-use bootstrap code (120 s, SR-32), an `HttpOnly`
`SameSite=Strict` session cookie, single-use 10 s WebSocket tickets and a
process token for non-browser callers (`crates/splitty-server/src/http/session.rs`).
Tests: `bootstrap_code_cannot_be_replayed`, `session_cookie_is_httponly`,
`query_parameter_token_is_not_accepted`, `legacy_token_cookie_is_not_accepted`,
`websocket_ticket_cannot_be_replayed`, `startup_output_contains_no_reusable_credential`.

**SR-05.** Shells get the path of a 0700/0600 Unix control socket with
multiplexer routes only, never a credential
(`spawn_session_calls_create_with_env_vars`,
`control_socket_cannot_reach_privileged_endpoints`). *Note:* this is not a
boundary against processes of the same user, which can read the 0600 token
file or edit the configuration directly; it keeps credentials out of
environments, logs and crash reports (corrected in SR-33).

**SR-06.** Installers and binaries are not code-signed (macOS builds are
ad-hoc signed). Releases ship `SHA256SUMS` and GitHub build provenance
attestations (`.github/workflows/release.yml`). Paid signing is an open,
accepted decision.

**SR-07.** Removed. A routable bind address aborts startup
(`refuses_to_bind_to_a_routable_address`, `refuses_direct_remote_mode`); remote
access goes through `splitty connect` over SSH.

**SR-08.** The fetchers were deleted with SR-01.

**SR-09.** The proxy is gone; JSON bodies are capped by Axum's 2 MB default.
Residual: an authenticated client can still send large terminal input.

**SR-10.** CI runs `cargo deny check advisories bans sources` and `pnpm audit`
for both frontends on every push and weekly; Dependabot proposes updates
(`.github/workflows/ci.yml`, `deny.toml`).

**SR-11.** Deny-by-default CSP and baseline headers
(`app_document_carries_a_strict_csp`,
`app_document_carries_baseline_security_headers`); DevTools only in a
`devtools` feature build (Tauri) or unpackaged runs (Electron). Accepted
deviation: `style-src 'unsafe-inline'`, because xterm.js and dockview inject
style elements at runtime and there is no HTML injection point.

**SR-12.** Commands in a template are listed and need confirmation before
anything runs; "layout only" strips them (`frontend/src/lib/template_guard.ts`).

**SR-13.** `SshTarget::parse` rejects option-like targets, including Unicode
dashes (`option_like_targets_are_rejected`); `ExitOnForwardFailure` and
`BatchMode` are pinned, the local port stays reserved until `ssh` starts, and
the forward is verified before a URL is printed (`crates/splitty-server/src/tunnel.rs`).

**SR-14.** `SECURITY.md` was rewritten so every claim points at a test. SR-33
found remaining inaccuracies.

**SR-15.** All writers go through `ensure_private_dir` / `write_private_file`
(`ensure_private_dir_creates_and_repairs_0700`,
`write_private_file_is_0600_in_a_0700_directory`,
`reusing_a_token_file_repairs_its_permissions`).

**SR-16.** Theme names are validated and resolved inside the themes directory
(`traversal_and_separators_are_rejected`,
`symlink_out_of_the_themes_directory_is_rejected`).

**SR-17, SR-22.** One comparison remains, `splitty_core::auth::validate`, and
it rejects empty values on both sides (`empty_values_never_validate`).

**SR-18.** `/health` returns `ok` and nothing else; with the loopback-only bind
this is accepted.

**SR-19.** The WSL backend refuses a root default user before provisioning and
before every start (`root_default_user_is_rejected`, `unparseable_uid_is_rejected`).

**SR-20.** OSC 52 reads remain always denied. Since 2026-10-15 writes are
enabled by default so terminal application copy works without setup. This
relaxes the original write opt-in: a terminal process can replace the clipboard.
`terminal.allowOsc52Write=false` durably disables writes; successful writes show a
short notice, historical OSC 52 sequences are stripped on resume, and payloads
are capped at 64 KiB of UTF-8
(`frontend/src/lib/terminal/clipboard_policy.ts`).

**SR-21.** `daemon.json` holds the control socket path, not a token.

**SR-23.** Paths under `SERVER_PATH_PREFIXES` answer 404 instead of the app
shell (`spa_fallback_still_serves_the_app`).

**SR-24.** `/api/bootstrap` exists only on the token-authenticated HTTP path,
never on the control socket.

**SR-25.** CI builds and tests the Tauri shell for Windows and Linux.

**SR-26.** The opener permission had no URL scope, so every link failed and the
frontend fell back to `window.open`, which on Windows opened the target in a
WebView2 popup inside Splitty. The capability now scopes the opener to
`http://*` and `https://*`, and the fallback is gone.

**SR-27.** Terminal output is untrusted, and ghostty-web opens OSC 8 hyperlinks
with a bare `window.open`, bypassing `openExternalTarget`. Both desktop shells
now decide every navigation and new-window request natively: the backend
origin and app popouts stay in the window, other web URLs go to the system
browser, everything else is dropped (`crates/splitty-tauri/src/navigation.rs`
with unit tests; `hardenWebContents` in `frontend-electron/src/main.ts`). App
responses carry COOP and CORP `same-origin`.

**SR-28.** Electron 35.7.5 had 37 open Electron/Chromium advisories. Upgraded
to 44; a permission handler allows only notifications and clipboard writes, and
only for the backend origin; fuses disable `ELECTRON_RUN_AS_NODE`,
`NODE_OPTIONS` and inspector flags and load app code only from an
integrity-checked asar.

**SR-29.** The updates planned in August were never applied. `cargo update` and
`pnpm update` cleared all advisories; reqwest no longer compiles a TLS stack the
product never uses. Remaining exception: GHSA-ch52-4w7c-c8xp in electron-builder's
download tooling (no fixed release published; not applicable to a single-user
cache), documented in `frontend-electron/pnpm-workspace.yaml`. Informational:
`serial` (via `portable-pty` 0.8, unused serial-port support) and
`proc-macro-error` (build time) are unmaintained.

**SR-30.** The API-only `splitty server` subcommand printed the process token on
stdout and logged a prefix of it. No launcher used it; removed.

**SR-31.** Every other `127.0.0.1` port is same-site with Splitty, so the browser
attaches the session cookie to its requests, and no-cors GETs carry no `Origin`.
API and WebSocket requests now have to carry `Sec-Fetch-Site: same-origin` when
the header is present (`api_rejects_same_site_fetch_metadata`,
`websocket_rejects_same_site_fetch_metadata`); GET handlers stay side-effect free
for engines without Fetch Metadata.

**SR-32.** Bootstrap codes expire after 120 seconds and are pruned on issue
(`expired_bootstrap_code_is_rejected`).

**SR-33.** The README advertised webview panes, remote mode, `--addr 0.0.0.0`
and `SPLITTY_TOKEN`; `SECURITY.md` claimed stdout never carries a reusable
credential; code comments claimed panes cannot read the token file. All
corrected.

**SR-34.** `script-src 'self'` also forbids compiling WebAssembly, so the ghostty
renderer and the xterm image addon failed to load. The CSP now adds
`'wasm-unsafe-eval'`, which permits WebAssembly only, not `eval` or inline
script.
