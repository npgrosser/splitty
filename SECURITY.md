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

- **Web pages and the network.** The terminal service has no TCP listener;
  its local sockets and owned pipes are not reachable by web pages.
- **Other users on the same machine**, as far as file permissions and loopback
  networking allow.
- **Untrusted terminal output**: escape sequences and links printed by build
  tools, remote hosts or package scripts.
- **Shared templates** that carry commands.

Splitty does **not** defend against:

- **Processes running as your user.** They can drive the multiplexer through the
  control socket or edit Splitty's configuration directly, just as they can
  edit `~/.bashrc`. Separating them needs an OS sandbox.
- **Tampered downloads** beyond what checksums and build provenance can show.
  The binaries are not code-signed (see [Releases](#releases)).

## Network boundary

The [connection diagrams](docs/networking.md) show the runtime paths, listeners
and lifetime boundaries summarized here.

- The app's local connections use in-process channels (Linux, macOS) or the
  pipes of its own `wsl.exe` child (Windows). They start no terminal-service TCP listener
  and never adopt an independently started daemon
  or its keeper sessions.
- Explicit daemons accept UI clients on a private owner-only Unix socket.
  Endpoint paths reject symlinks, stale cleanup checks identity, and both ends
  verify peer ownership (`crates/splitty-server/src/native/endpoint.rs`).
  Owned group-writable ancestors such as `~/.local` with mode `0775` are
  supported without changing their permissions. Native endpoint operations
  and daemon discovery/log access use retained private directory handles;
  the final directory stays `0700`, and socket/lock files stay `0600`.
  Symlinks, foreign-owned directories and non-sticky world-writable ancestors
  are refused. A group writer can still disrupt path availability.
- Remote connections use SSH exec stdio to the host's private native socket.
  Validated targets cannot inject SSH options. Native negotiation verifies
  the expected daemon instance and protocol before application input.

Reverse-share admissions coordinate through a private
per-user reservation registry across configuration directories. Ambiguous
worker identity, corrupt records and unsupported namespace views fail closed.
Linux requires the initial PID/user namespaces, a matching root mount view and
verified persistent filesystem semantics for the account-home registry. Unsupported container/mount/filesystem
views fail before binding; an isolated network namespace alone does not establish
shared reservation authority among all processes that can reach its loopback.
Native desktop and daemon transports do not require this registry.
Live internal-listener claims from older processes of the same user remain
reserved against sharing; this version creates only share reservations.
The registry is rooted in the OS account's persistent home, independently of
`HOME`, XDG and Splitty configuration overrides: `.local/state/splitty/exposure`
on Linux or `Library/Application Support/Splitty/exposure` on macOS. Its ancestor
walk rejects symlinks, foreign owners and non-sticky group/world-write bits; the
owned final directory is `0700` and its single-link regular files are `0600`.
Consequently, a `0775` `.local` can be valid for a native client endpoint while
being refused for reverse sharing. Splitty does not change
shared parent modes. Pinning a directory for one process cannot establish that
a later process uses the same registry after an ancestor is renamed; relaxing
this rule requires a shared authority across those processes, not only safer
individual file operations.
A positively different immutable process birth identity in the same verified
machine/boot/PID view permits release of the old worker generation. UID changes,
unknown publication state and elapsed time do not. Cleanup bookkeeping exhaustion
keeps admission closed rather than forgetting unresolved persisted claims.
Windows and WSL refuse reverse sharing until a shared localhost reservation
authority exists; forward-to-local development ports remain supported. This registry does not constrain
other forwarding tools run by the same OS user.

Legacy daemon migration requires an explicit update confirmation. On Linux,
the installed backend verifies the private owner socket, process generation and
compatible keeper coverage before offering migration, then repeats those checks
before signalling a retained pidfd. Daemon-owned PTY masters block migration,
including terminals absent from the workspace snapshot. It never sends a
session-ending command or escalates to a forced kill. This is a bounded preflight,
not a freeze of other clients: the preservation promise covers the checked
keeper-backed sessions. Unsupported identity checks or platforms refuse the
operation; an ambiguous SSH result never automatically replays it. The proof
stays in native code and grants no renderer-selected process or endpoint access.
Implementation and regressions: `crates/splitty-server/src/host/migration.rs`,
`crates/splitty-server/tests/daemon_lifecycle.rs` and
`crates/splitty-remote/src/tunnel/supervisor.rs`.

The owner control router and grant routers require JSON `Content-Type` for JSON
request bodies; other content types are rejected with 415 before mutation
(`crates/splitty-server/src/control/operation.rs` and `control/jobs.rs`). The owner router excludes
host-directory, settings, build and command-dispatch catalog routes; native
operations use their assigned connection role.

## Desktop app

The app draws its own interface and runs no browser engine: there is no page
script, and nothing a host or a program in a pane sends is run as UI code.

- **Viewer panes** render Markdown and HTML as text and layout only: no
  scripts, nothing loaded, and their links open nothing
  (`crates/splitty-gpui/src/workspace/viewer.rs`).
- **Links in terminals** open only as bounded absolute `http(s)` URLs, and only
  on a click; every other scheme is dropped
  (`crates/splitty-client-core/src/external_links.rs`, with unit tests).
- **Editor and folder hand-offs** that a process in a pane requests open only
  after **Open** in the window's notice; preferences choose an editor but never
  allow a launch (`crates/splitty-client-core/src/native_workspace/editors.rs`).
- **WSL** on Windows (`crates/splitty-gpui/src/startup/wsl.rs`). A distribution
  whose default user is root is refused
  (`root_other_architectures_and_other_distributions_are_refused`). The backend's
  traffic uses the owned `wsl.exe` pipes with no port or bootstrap text parsing;
  on exit the app ends only its own bridge, never the distribution or unrelated
  WSL work.

## Terminal output

- **Clipboard (OSC 52).** Reads are always denied; writes are allowed by default and can be disabled with
  `terminal.allowOsc52Write=false`; payloads over 64 KiB are dropped. In
  the app: `crates/splitty-gpui/src/terminal_engine.rs` (tests
  `clipboard_writes_are_refused_until_allowed_and_reads_always`,
  `clipboard_writes_are_bounded_valid_live_and_for_the_standard_clipboard`).
- **Links** are scheme-checked: in the app by
  `crates/splitty-client-core/src/external_links.rs`.
- **Templates** that carry commands show them and need confirmation before
  anything runs: in the app **Run displayed commands**
  (`crates/splitty-gpui/src/workspace/templates.rs`).

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
peer grants or configuration files. Launched programs execute as the server user,
outside any sandbox imposed only on the invoking agent process. Granting a generic
launch tool is therefore command-execution authority. Scoped CLI discovery fails
closed, and agent-owned jobs inherit their grant socket rather than the owner socket;
neither carries a reusable network credential. The UI remains a trusted-user client.
See [managed terminal jobs](docs/cli-jobs.md) for setup and lifetime semantics.

- The configuration directory is 0700, private files 0600, written
  atomically (`ensure_private_dir_creates_and_repairs_0700`,
  `write_private_file_is_0600_in_a_0700_directory`).
- Shells get the path of a Unix control socket, never a credential
  (`spawn_session_calls_create_with_env_vars`). The socket is 0600 inside a 0700
  directory, checks the peer's user id, and offers the owner control route set only
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
  like the owner socket it is outside the scoped agent
  boundary, and any process of the same user can claim a keeper. Keepers open
  no network socket and hold no credential; no
  session's environment names a keeper socket
  (`a_shell_outlives_stop_restart_update_and_crash_of_its_daemon`).

## Releases

- Release binaries are **not code-signed**; macOS builds are ad-hoc signed.
  Each release ships `SHA256SUMS` and GitHub build provenance attestations:

  ```sh
  sha256sum -c SHA256SUMS --ignore-missing   # macOS: shasum -a 256 <file>
  gh attestation verify <file> --repo npgrosser/splitty
  ```

- CI checks dependencies on every push and weekly (`cargo deny`, plus `pnpm audit` for architecture tooling),
  and Dependabot proposes updates. Exceptions are documented with a reason in
  `deny.toml` and `architecture/pnpm-workspace.yaml`.

## Known limitations

- Processes of your own user are not isolated from Splitty (see the threat
  model).
