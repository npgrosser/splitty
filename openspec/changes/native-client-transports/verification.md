# Native transport verification status

This change is under review and is not ready to merge. Unchecked tasks retain
all their requirements; the evidence below does not narrow their scope.

## Implemented behavior

| Entry | Transport | Session lifetime |
| --- | --- | --- |
| Local desktop | Owned in-process service or inherited child pipes | Ends with the app |
| Independent local daemon | Private Unix socket | Independent of the app |
| SSH host | SSH exec/stdio to the host's private socket | Survives client disconnect |
| Explicit browser gateway | HTTP/WebSocket to the selected backend | Follows that backend |

Desktop documents load packaged assets and receive an assigned backend through
native authority tied to the window and document generation. Browser access
requires an explicit gateway. Local desktop startup does not adopt a separately
running daemon. Pending upload destination/cancellation work is a prerequisite
and retains cancellation and generation semantics across the transport change.

## Evidence collected

- Linux CI workspace tests excluding the GUI: 1,024 reported passing; optional
  OS tests can skip at runtime, so individual platform evidence is listed
  separately below. Workspace clippy passed.
  Subsequent shared-budget CAS changes passed their targeted tests. Rust 1.99
  workspace and GUI all-target clippy also pass after constant-chunk lint fixes.
  The dependency audit passes after updating yanked async-executor 1.14.0 to
  1.14.1.
- Frontend: 1,502 tests passed (also repeated from a fresh checkout with both frontend
  and Electron adapter dependencies installed); check completed with zero errors and eight existing
  warnings; production build passed. Electron build, packaged-asset execution and
  real IPC smoke passed, including frozen-client quotas and final zero counters.
- Final Linux GUI source checks: 89 library tests, all-target clippy, 19 private
  bootstrap regressions and formatting pass. The remote admission tests include
  coalesced request floods, cancellation and repeated document replacement.
- Linux Tauri/WebKit: actual packaged full UI passed 37 document generations,
  binary traffic, heartbeat, stale-proof denial, popout lifecycle and independent
  daemon/keeper coexistence. Application resource counters reached zero; sampled
  app/WebKit TCP inventories were empty. GUI runs use an isolated Xvfb display.
- Native Windows MSVC tests and WebView2 setup-role checks passed. Actual WSL
  fixtures passed binary PTY/API/file traffic, distribution switch, restart and
  independent guest-worker preservation. Fixtures override configuration and
  distribution discovery; they do not establish ordinary host inventory.
  Fresh setup and remote-only runs using the reviewed kernel Job driver now pass:
  fixed remote authority, actual Windows OpenSSH, binary PTY/API traffic, daemon
  replacement/reconnect, reattachment to the same shell, rejected offline input
  and shell survival after app exit. Both runs end naturally with zero remaining
  Job members before cleanup and zero owned handles afterward. Listener checks
  are sampled, not proof of continuous absence. A fresh guarded WSL run also
  passes distribution A, production switch/restart to B, binary PTY traffic and
  a byte-checked 1,048,833-byte file transfer. Both app instances exit naturally
  with zero remaining Job members; their helper/PTY ends, while independent
  guest sentinels preserve their start identities and answer fresh nonces. Both
  temporary distributions are removed and owned handle counts return to zero.
- Actual Windows WebView2 also passes the production full-UI mode: packaged
  CSS/font/WASM, Ghostty canvas, native settings and three bounded popout
  lifecycle checks. Direct popout commands are denied while parent-mediated
  calls remain allowed in the shared trust domain. The existing binary PTY and
  1,048,833-byte upload checks pass alongside the real UI. UI-error, backend
  HTTP/WebSocket and clipboard-attempt observations are empty. Both app
  instances exit naturally; their Jobs are empty before cleanup, owned handles
  return to zero, independent guest sentinels answer fresh nonces, and both
  temporary distributions are removed. The last popout closes at app exit;
  this is not Windows parent-reload or sibling-remote isolation evidence.
- Actual macOS CI at `4eb7e72` passes GUI all-target clippy and all 90 GUI library tests.
  All twelve real-child native bridge failure/cancellation regressions pass at
  `5d25bf9`, including the new owner cleanup and held-stderr deadline cases;
  these exercise controlled carriers, not permanent system-SSH reconnect failure.
  Private endpoint peer-user checks, native forwarding reservation, IPv4/IPv6
  listener positive controls, owned-helper lifetime and explicit gateway
  replacement also pass. Two core failures (Bash CLI PATH restoration and the
  background-writer PTY fixture) reproduce on exact baseline `cc66e97`; they
  remain failing checks and are recorded in the baseline findings. The earlier
  managed-stop failure recurs intermittently: the new diagnostic identifies
  `killpg` on the owned leader's group returning `EPERM`. That remains open;
  no blanket permission-error suppression was added.
- Actual macOS WKWebView now passes the full packaged UI through 37 document
  generations: styles/fonts, Ghostty WASM/canvas, binary terminal data, settings,
  direct popout denial, parent-mediated operations and native window cleanup.
  All generations keep the same owned service instance. There are no observed
  HTTP/WebSocket backend requests or TCP listeners (309 inventory samples).
  Final application byte/stream/carrier/worker/handle/open counters are zero;
  owned shells exit. A real separate daemon keeps unchanged discovery and
  keeper sockets, and the same original shell accepts input after app exit.
  Fixture fixes canonicalize the private home and restrict an inherited test
  probe to the top packaged workspace; production authority checks remain
  unchanged. These passing runtime checks do not waive the core failure gate.

- Subsequent isolation-fixture preparation found a Windows navigation effect
  mismatch: a rejected packaged HTTP-origin target could reach the system
  opener. The shared fallback now blocks internal asset authorities before
  external HTTP(S) handling, while preserving exact role entry permissions and
  genuine external links. Six focused navigation tests and GUI library clippy
  pass; independent source review found no authority expansion. The new
  Windows isolation run will verify actual named-window behavior separately.

These results describe the runs performed, not proof that every requirement is
complete. Final source checks and platform results must be reconciled before
marking the change ready.

## CodeQL review

The pull-request findings were checked against the actual data flow, with an
independent security review. The WebSocket trace now uses a constant format
string; keeper-stop diagnostics omit the session identifier. The Electron IPC
smoke encodes script values as JSON literals with escaped HTML delimiters and
Unicode line separators. Its real renderer checks a hostile literal roundtrip;
the full IPC smoke still passes binary traffic, frozen-client isolation and zero
final resource counters. Eleven focused socket tests and the server check pass.

The subsequent CI run at `312cb04` passes Rust, Linux Tauri, frontend,
Electron, aarch64 and dependency checks. All four CodeQL analyses pass and the
PR comparison reports no new alerts. The pre-existing shell-profile allocation
alert remains separate; the comparison result is not a claim that the entire
repository has no findings. macOS again passes the actual WKWebView and native
network/lifetime checks, while its mandatory core gate still fails on the two
reproduced baseline cases.

The later run at `4eb7e72` passes frontend, Electron, Linux Tauri, aarch64,
dependency and CodeQL checks. Its Rust lifecycle suite fails
`rust_embedded_owner_shares_runtime_across_reload_and_reaps_its_jobs`: the
managed job PID remained visible after owner shutdown. The cause is a 500 ms
helper termination grace racing its own 500 ms PTY cleanup grace. Owner helpers
now use a shared shutdown deadline selected from trusted policy before
negotiation; stderr draining consumes that same deadline. Ordinary SSH relay
policy is unchanged. The new real-child regression fails before the fix and
passes afterward; a held-stderr case, the five owner tests, all twelve bridge
tests and the actual helper/job-reaping smoke pass. Targeted all-target clippy
and independent source review pass. Process/worker ownership is retained until
actual exit; these timing tests do not establish a hard deadline under OS
failure. The complete Rust CI job passes at `5d25bf9` in
[run 41709472218](https://github.com/npgrosser/splitty/actions/runs/41709472218),
alongside frontend, Electron, Linux Tauri, aarch64 and dependency checks. Its
CodeQL analyses also pass. macOS passes all 90 GUI tests, all twelve bridge
regressions and its native endpoint/lifetime/gateway and packaged UI checks.
Its core gate still fails the same two baseline cases, both reproduced against
the merge base in this run; those failures remain open.

Three findings do not indicate a vulnerability in their reported contexts:

- `rust/hard-coded-cryptographic-value` in `client_ipc/bootstrap.rs` is a
  deterministic JSON-escaping fixture under `#[cfg(test)]`. It is not used by a
  running app. Production document nonces use `crypto.getRandomValues`;
  native bootstrap/challenge/proof values use `rand::random`.
- The same query at `tauri::generate_handler!` identifies generated `"nonce"`
  argument lookup keys. Tauri's `CommandArg::from_command` deserializes the
  actual value from the invocation payload; the static key is not a nonce.
- `rust/path-injection` in `service/operations.rs::post_editor_open` identifies
  an intentionally owner-selected absolute host path. The operation checks the
  live session and path syntax, obtains metadata, and sends a confirmation
  request to a registered attached client. It does not read file contents or
  launch an editor. Browser authentication, private owner-socket access and
  native assigned-backend authority apply before dispatch; restricted agent
  sockets do not expose this operation. The client still requires a deliberate
  local confirmation and derives the host from its connection. Confining this
  operation to a repository root would remove its specified behavior.

These dispositions apply to the specific findings, not to all uses of nonces or
filesystem paths. They do not waive the remaining resource or platform gates.

## Migration evidence

| Case | Evidence | Limit |
| --- | --- | --- |
| Compatible native daemon | Private endpoint/negotiation tests and actual daemon client smoke | Remaining desktop platform cells are tracked separately. |
| Older kept daemon update | `the_hub_updates_a_host_daemon_of_another_version` passed through actual isolated OpenSSH and real daemon replacement, preserving keeper socket identity and the original shell PID/private state through fresh nonce-bearing output | The old version/protocol is simulated in discovery metadata; this is not execution of a historical release binary. |
| Incompatible non-kept/newer daemon | Supervisor and HTTP policy tests plus the extended real-SSH `the_hub_updates_a_host_daemon_of_another_version`: explicit updates refuse both cases without replacing discovery, daemon PID or keeper sockets; fresh native input/output proves the original shell remains usable | Version/protocol differences are simulated in valid discovery metadata; remaining desktop platform checks stay open. |
| Supported client downgrade | The real-SSH test reuses a compatible newer-version daemon and proves original shell state; the historical rollback smoke runs current → actual `cc66e97` server → current and proves the same shell PID, private variable, session ID and keeper inode | Version-policy simulation and actual previous-implementation rollback are distinct checks; the historical artifact is locally built from the pinned revision, not a downloaded release package. |
| Stale/corrupt/legacy discovery | Daemon lifecycle `native_discovery_loss_corruption_and_concurrent_start_cannot_replace_a_live_daemon` and `a_live_legacy_record_is_diagnostic_and_never_selects_its_tcp_port` | Legacy ownership uncertainty refuses implicit replacement. |
| Explicit restart and keeper adoption | `a_shell_outlives_stop_restart_update_and_crash_of_its_daemon`, explicit daemon gateway replacement and the pinned historical rollback smoke | Keepers are real processes; the historical check covers the exact prior implementation listed below. |
| Local app with independent daemon | Actual Linux Tauri/Electron and macOS WKWebView coexistence smokes; owned-child pipe lifecycle smoke | The complete Windows matrix remains a separate gate. |

The Linux historical rollback smoke uses an artifact built from exact revision
`cc66e97024f1013cc2abad1727ba9378db2200af`, with SHA256
`3af8b58d33d3408c9b89a32ffac97d51f1fe6838a2e6e5a64be0564c8ebb82b5`.
Source archive, lockfile, compiler and binary provenance were recorded separately.
The strict run passes in a private Bubblewrap PID/user/network namespace with
read-only executable mounts and private temporary storage. It fixes the fixture
shell to `/bin/sh` and checks the actual executable; inherited interactive-shell
startup is not part of this lifecycle test. All captured process generations
exit and no keeper sockets remain. The old implementation's explicit ephemeral
browser listener stays inside the private network namespace; current native
clients never fall back to it. This verifies an ordinary supervised run, not
supervisor death during Bubblewrap's early pre-exec setup.

The catalog/role audit found no remaining native HTTP/token fallback. All 53
operations (31 backend and 22 local hub) and 12 adapter-only routes have checked
classifications. Remaining TCP binds and browser credentials belong to explicit
browser gateways or requested development forwards; native commands retain
assigned-window/document checks. Static classification and native/browser
conformance tests cover enumerated operations,
credential-route refusal and assigned-host restrictions. The expanded framed
Remote-role test rejects all 22 LocalHub operations with 403, verifies nine
management-state views after each refusal and observes zero connector calls.
Allowed backend reads, a real template mutation and missing-object errors
match browser results on that same Remote stream; all 15 conformance cases
pass. This does not close unchecked platform conformance cells.

Retained SSH diagnostics now keep source-byte charges through missing-command,
old-CLI, installation-status, invalid-probe and unsupported-kernel conversions.
Drop/exhaustion tests pass with 149 remote unit tests and 11 hub failure tests;
remote/server all-target clippy passes. Fixed-size setup metadata is separately
bounded by length and setup admission; the transport ledger is not a total
process RSS or trusted deployment-archive bound.

## Hosted Windows forwarding preflight

A separate diagnostic branch at `6acdbdd` passes GitHub run
[41709278261](https://github.com/npgrosser/splitty/actions/runs/41709278261).
On a fresh hosted Windows VM, actual WSL 2 guest execution and matching guest/
Windows nonce responses establish the forwarding-enabled positive control.
After disabling localhost forwarding and restarting WSL, the guest still
answers before and after the refused Windows connection. The private distro
is removed, the original configuration is restored, cleanup reports no errors
and owned Windows handles return to zero. This proves the availability of a
disposable test environment; it does not yet prove Splitty's native app works
with forwarding disabled. That application run remains part of task 7.6.

## Open merge gates

- Whole-path resource limits: Wry allocates inbound IPC request bodies before
  application validation. Application-owned quotas do not establish a bound on
  framework allocation, generic invoke replies, JavaScript engine memory or RSS.
- The complete platform isolation/stress matrix remains open. Actual macOS
  full-UI, native endpoint and coexistence checks pass; core failures remain. Windows remote-only/reconnect now passes
  its fresh kernel Job fixture; that does not fill every platform matrix cell.
- The strict native Linux reservation test passed with an actual surviving
  worker after coordinator SIGKILL, IPv4/IPv6 gateway exclusion and reuse only
  after observed worker exit. The real SSH/hostile-peer crash variant and
  native macOS positive controls now pass in CI. The complete listener/resource
  inventory matrix remains open; WSL refusal checks alone are not positive
  native-platform coverage.
- Disabled WSL localhost forwarding has not been exercised; the pipe tests ran
  with the existing host networking configuration unchanged.
- Migration, conformance and security review must reconcile the remaining task
  checks, including framework-bound scope, before cutover is approved.

## Windows harness cleanup incident

The process inventory followed historical parent PIDs without validating the
parent generation and lifetime. PID reuse caused unrelated Firefox processes
to be classified as fixture descendants, and cleanup attempted to terminate
them. All 28 recorded Firefox/crash-helper PIDs were subsequently absent.
Individual termination results were not captured, so direct termination cannot
be distinguished from descendants exiting with their parent.

Prior Windows process-tree cleanup inventories are not evidence of safe
isolation. The first replacement candidate was rejected: even birth-time checks
cannot prove parent ownership from a mixed-time process enumeration. It was
removed before use.

The reviewed replacement creates private kernel Jobs, launches each fixture root
suspended, assigns its exact handle before resuming, and disables breakaway.
Cleanup uses only retained Job handles or the exact newly created suspended
process handle on assignment failure. No PID-only or recursive `taskkill /T`
cleanup remains. Fresh console tests cover descendants, unrelated fixture
preservation, breakaway refusal, assignment failure and zero retained handles.
Fresh setup, remote-only and guarded WSL switch GUI runs then passed with
natural app exit before cleanup. Old TEMP driver copies must not be
executed. Socket inventories remain sampled observations.

The earlier Windows SSH PTY assertion failure was a fixture omission: it did not
reattach its SessionClient after reconnect. Adding the same explicit attach used
by the client restored the assertion for the original shell, without a product
behavior change or weaker assertion.

## Fixture target and cleanup hardening

A subsequent Windows sentinel preflight exposed unconditional quoting of WSL
flags: WSL attempted the literal `-d` command in the default distribution and
failed before the intended script ran. This invalidates that preflight; it is
not a successful guest lifecycle check. Conditional Windows argument quoting
and first-action exact `WSL_DISTRO_NAME` checks now guard every guest command,
including the copied app's deployment and backend launch paths. Independent
source review and eight pure guard regressions pass. A fresh single-distribution sentinel then passed actual WSL argument parsing
and exact guest identity/nonce checks. Its retained Job was intentionally
stopped, with no remaining members or handles, and its sole temporary
distribution was removed. The independently cleared full WSL rerun also passes, including the production
switch/restart path and independent guest preservation. The earlier interrupted
build attempt never reached GUI execution and is not counted as a runtime pass.

The Linux SSH fixture now pins its private network namespace and resolves each
cleanup candidate to a pidfd before checking namespace membership. Signals use
that same process handle, never a numeric PID fallback. Its regression passes:
an unrelated owned process survives, while a detached fixture process exits
even after the original namespace holder is gone. Independent source review
found no stale-PID signaling path.
