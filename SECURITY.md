# Security

Splitty is a terminal: whoever controls it runs commands as you. This document
describes what Splitty protects against, what it does not, and how to report a
problem. Every guarantee names the test or the code that enforces it; the
history of individual findings is in
[`docs/security/findings.md`](docs/security/findings.md).

## Reporting a vulnerability

Please report vulnerabilities privately through
[GitHub's private vulnerability reporting](https://github.com/npgrosser/splitty/security/advisories/new),
not in public issues. Include what you did, what happened and which version
you used. Fixes go into the latest release only.

## Threat model

Splitty defends against:

- **Web pages you visit**, including pages served from other ports on
  `127.0.0.1`, which browsers treat as the same site.
- **The network.** The backend never listens on a routable address.
- **Other users on the same machine**, as far as file permissions and loopback
  networking allow.
- **Untrusted terminal output**: escape sequences and links printed by build
  tools, remote hosts or package scripts.
- **Shared templates** that carry commands.

Splitty does **not** defend against:

- **Processes running as your user.** They can drive the multiplexer through the
  control socket, read the 0600 token file, or edit Splitty's configuration
  directly, just as they can edit `~/.bashrc`. `tmux` and `screen` behave the
  same way; separating them needs an OS sandbox.
- **Tampered downloads** beyond what checksums and build provenance can show.
  The binaries are not code-signed (see [Releases](#releases)).

## Network boundary

- The server binds to loopback only; a routable address or the old `--remote`
  flag aborts startup (`refuses_to_bind_to_a_routable_address`,
  `refuses_direct_remote_mode`).
- A `Host` check blocks DNS rebinding (`rejects_foreign_host_header`).
- Remote machines are reached through an SSH tunnel, `splitty connect user@host`.
  The target is parsed, so it cannot inject `ssh` options
  (`option_like_targets_are_rejected`), and the forward is verified before a URL
  is printed (`crates/splitty-server/src/tunnel.rs`).

## Credentials

| Credential | Held by | Lifetime | Readable by page script |
|---|---|---|---|
| Bootstrap code | the launcher, in the URL it opens | one use, 120 s | yes, worthless once used |
| Session cookie | the browser | server process | no (`HttpOnly`, `SameSite=Strict`) |
| WebSocket ticket | the page, for one handshake | one use, 10 s | yes, worthless once used |
| Process token | CLI and launchers, 0600 file | until deleted | never sent to the browser |

Implementation: `crates/splitty-server/src/http/session.rs`. Tests:
`bootstrap_code_cannot_be_replayed`, `expired_bootstrap_code_is_rejected`,
`session_cookie_is_httponly`, `websocket_ticket_cannot_be_replayed`,
`ws_ticket_requires_a_session`, `query_parameter_token_is_not_accepted`,
`startup_output_contains_no_reusable_credential`, `empty_values_never_validate`.

To rotate the process token, quit Splitty, delete
`~/.config/splitty/auth-token` and start it again. Sessions and tickets are never
persisted and die with the process.

## Browser boundary

- **Origin.** The WebSocket requires the app's own `Origin`; the API rejects any
  other one (`websocket_rejects_foreign_origin`, `websocket_rejects_missing_origin`,
  `api_rejects_foreign_origin`). WebSockets have no CORS protection, so this is
  what keeps even a leaked credential from being used by a web page.
- **Fetch Metadata.** Pages on other loopback ports get the session cookie
  attached and can send GETs without an `Origin`. API and WebSocket requests
  must therefore carry `Sec-Fetch-Site: same-origin` when the browser sends the
  header (`api_rejects_same_site_fetch_metadata`,
  `websocket_rejects_same_site_fetch_metadata`).
- **No CORS headers** at all (`no_response_carries_wildcard_cors`).
- **CSP and headers.** `default-src 'none'`, `script-src 'self' 'wasm-unsafe-eval'`,
  `frame-src` and `frame-ancestors 'none'`, `connect-src` limited to the
  server's own port; `nosniff`, `no-referrer`, a `Permissions-Policy` that blocks
  clipboard reads, and COOP and CORP `same-origin`
  (`app_document_carries_a_strict_csp`,
  `app_document_carries_baseline_security_headers`,
  `static_assets_carry_the_security_headers`).

## Desktop shells

Splitty never renders foreign web content. Both shells decide every
navigation and every new-window request natively, whatever code in the page
asked for it: the backend origin and the app's own popouts stay in Splitty,
other `http(s)` URLs open in the system browser, and every other scheme
(`file:`, `javascript:`, OS protocol handlers) is dropped.

- **Tauri** (`crates/splitty-tauri/src/navigation.rs`, with unit tests). Custom
  commands are declared in an app manifest and granted per context: to the
  bundled Windows setup page in `capabilities/`, and to the main window at
  runtime, only on the exact origin of its own backend (`src/capability.rs`).
  Only the setup page may start the backend. A remote window, which shows a
  host's UI, is in no capability, has a cookie store of its own in memory only,
  stays on its tunnel origin and opens no windows (`src/remote.rs`). The URL
  opener is scoped to `http(s)`. DevTools exist only in builds with
  `--features devtools`.
- **Electron** (`frontend-electron/src/main.ts`). Sandboxed renderer with
  context isolation and no Node integration, a fixed IPC channel list, guards on
  every WebContents, a permission allowlist (notifications and clipboard writes,
  backend origin only), no DevTools when packaged, and fuses that disable
  `ELECTRON_RUN_AS_NODE`, `NODE_OPTIONS` and inspector flags and load the app
  only from an integrity-checked asar.
- **WSL** (`crates/splitty-tauri/src/windows_wsl.rs`). A root default user is
  refused (`root_default_user_is_rejected`), credentials are redacted from the
  backend log (`backend_stdout_credentials_are_redacted_in_logs`), and the
  control socket is refused on Windows drive mounts
  (`windows_drive_mounts_are_refused`).

## Terminal output

- **Clipboard (OSC 52).** Reads are always denied; writes are off unless
  `terminal.allowOsc52Write` is enabled; payloads over 64 KiB are dropped
  (`frontend/src/lib/terminal/clipboard_policy.test.ts`).
- **Links** are scheme-checked in the UI (`openExternalTarget`) and enforced again
  by the shells, because a renderer library can bypass the UI.
- **Templates** that carry commands show them and need confirmation before
  anything runs (`frontend/src/lib/template_guard.test.ts`).

## Local files and the control socket

### Managed-job agent scopes

`splitty agent create` provisions a separate private Unix socket whose listener
identifies an agent grant. It exposes job operations only, filters every read by
current ownership, and authorizes writes at execution time. An anchor permits adding
a tab/split beside a terminal, never reading or typing into that terminal. Grants do
not inspect `sudo` prompts or infer authority from process names, pane IDs or cwd.
Reclaim a job before privileged manual use; revoke/transfer closes the previous
grant's access, including pending readers and queued input, without killing the job.

This boundary does not isolate processes of the same OS user from owner sockets,
peer grants or token/config files. Launched programs execute as the server user,
outside any sandbox imposed only on the invoking Claude process. Granting a generic
launch tool is therefore command-execution authority. Scoped CLI discovery fails
closed, and agent-owned jobs inherit their grant socket rather than the owner socket;
neither carries a reusable network credential. The UI remains a trusted-user client.
See [managed terminal jobs](docs/cli-jobs.md) for setup and lifetime semantics.

- The configuration directory is 0700, credential files 0600, written
  atomically (`ensure_private_dir_creates_and_repairs_0700`,
  `write_private_file_is_0600_in_a_0700_directory`,
  `reusing_a_token_file_repairs_its_permissions`).
- Shells get the path of a Unix control socket, never a credential
  (`spawn_session_calls_create_with_env_vars`). The socket is 0600 inside a 0700
  directory, checks the peer's user id, and offers multiplexer operations only
  (`control_socket_is_private_to_the_owner`,
  `control_socket_cannot_reach_privileged_endpoints`). As described in the
  threat model, this keeps credentials out of environments and logs; it is not
  a boundary against your own processes. A daemon's control socket is
  `daemon.sock` in its run directory (`$SPLITTY_CONFIG_DIR/run` or
  `<user data dir>/splitty/run`), every other server's `<pid>.sock`.
- In daemon mode each session runs in a keeper process with one Unix socket in
  `keepers/` below the run directory, under the control socket's rules: a 0700
  directory, a 0600 socket, the peer's user id checked by the keeper and by its
  controllers (`the_directory_is_private`,
  `peers_of_another_or_an_unknown_user_are_refused`). A keeper socket carries the
  authority of the owner socket over its terminal (input, output, hang-up);
  like the owner socket and the token file it is outside the scoped agent
  boundary, and any process of the same user can claim a keeper. Keepers open
  no network socket, hold no credential and never read the token file; no
  session's environment names a keeper socket
  (`a_shell_outlives_stop_restart_update_and_crash_of_its_daemon`).

## Releases

- Release binaries are **not code-signed**; macOS builds are ad-hoc signed.
  Each release ships `SHA256SUMS` and GitHub build provenance attestations:

  ```sh
  sha256sum -c SHA256SUMS --ignore-missing   # macOS: shasum -a 256 <file>
  gh attestation verify <file> --repo npgrosser/splitty
  ```

- CI checks dependencies on every push and weekly (`cargo deny`, `pnpm audit`),
  and Dependabot proposes updates. Exceptions are documented with a reason in
  `deny.toml` and the `pnpm-workspace.yaml` files.

## Known limitations

- Processes of your own user are not isolated from Splitty (see the threat
  model).
- `style-src` allows `'unsafe-inline'`, because xterm.js and dockview inject
  style elements at runtime. There is no HTML injection point, and scripts stay
  restricted to `'self'`.
- A token passed with `--token` is visible in the process list; prefer the
  token file.
- `splitty serve --open` hands the bootstrap URL to the system's URL opener, so
  it briefly appears in that process's arguments. On a machine shared with
  other users, use the desktop app instead.
