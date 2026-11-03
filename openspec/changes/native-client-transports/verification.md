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

## Final integration candidate

The application-owned memory boundary is settled in NCT-8; framework ingress and
total RSS are explicitly excluded without relaxing transport/output accounting or
native authority. Independent security review accepted this boundary. Main specs
are synchronized with the implementation (15 spec validations pass), including the
fail-closed corrupt-discovery behavior. Architecture checks pass in both directions.

Two separate corrections address the macOS core acceptance failures:

- `50470d1` replaces the Apple-incompatible POSIX/ENV bootstrap with startup before
  the first prompt or explicitly interactive command. It preserves login state,
  profile selection, inherited startup variables and first-prompt hooks. History
  file access is deferred until profile settings are applied, avoiding premature
  truncation. Profile-preloaded history is retained without a second automatic
  read; this documented integration boundary avoids emulating every Bash history
  initialization detail. Independent Bash review found no remaining blocker.
- `bff1069` separates process completion from reader EOF using a controlled reader
  through the production output reader, and retains real-PTY final-byte/exit-status
  coverage without requiring a Darwin background writer to outlive its session
  leader. Production EOF behavior is unchanged.

All 27 targeted session tests, core all-target clippy, formatting and diff checks
pass locally. Actual macOS CI and the final integrated suite remain pending; prior
baseline reproductions below are historical evidence, not waived acceptance tests.

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
- Actual macOS [CI at `b88933f`](https://github.com/npgrosser/splitty/actions/runs/41717039859)
  passes GUI all-target clippy and all 90 GUI library tests. All twelve real-child
  native bridge failure/cancellation regressions pass, including the owner
  cleanup and held-stderr deadline cases;
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

The Windows isolation fixture subsequently exposed a fatal native-endpoint
`EAGAIN` during remote preparation. A targeted smoke now reproduces the same
classification gap by holding the real private instance lock before discovery
publication. Native daemon start reports temporary contention with exit 75,
and remote handshake preserves the existing typed discovery-refresh result.
The client retains its existing bounded refresh policy; corrupt discovery
still fails permanently and no lock or live endpoint is replaced. The extended
lifecycle smoke passes and independent source review passes. This establishes
the classification fix, not the identity of the lock holder in the original
Windows run or completion of the outstanding Windows isolation check.

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

## Windows isolation and consumer-pressure fixture

The hosted isolation/consumer run at `6b08775` passes
[run 41720162085](https://github.com/npgrosser/splitty/actions/runs/41720162085).
Setup/Main and two Remote windows pass opener/named-window/storage isolation,
foreign target/handle denial, independent slow-client closure, healthy API/file
completion on the unchanged connection, and 32-handle saturation/refusal/
replacement/recovery. Original write → open → attach ordering is retained:
write/open at 1793740944716 ms precede attach at 1793740944723 ms; the attached
stream delivers 94,084 bytes including the actual post-close marker. Diagnostic
awaits follow attach. Binary PTY and both file SHA checks pass.

The strict listener gate passes with no listener/unknown-state rows and no
provider error. Bounded known non-listening observations are retained separately.
The app exits naturally with code 0; local helpers/PTYs exit, the remote daemon
survives until fixture teardown, independent sentinels preserve identity and
answer fresh nonces, both app distributions are removed, and owned handles reach
zero. The outer control distribution is also removed and the original WSL
configuration absence is restored. This scenario uses forwarding enabled for
its explicit SSH listener; disabled-forwarding evidence is separate below.

Earlier local `a52fde175fc7` remains an overall failure on unclassified listener
rows, despite passing its functional workload. Local native-API state 0 / CIM
null controls and the earlier `c92108c1f378` replay-marker failure remain
unexplained. The hosted result establishes the original-order case on a valid
measurement environment; it does not identify those earlier failure causes.
Copied-handle denial is a capsule check; stale Setup-proof denial is a broker
diagnostic. The slow client withholds receipts rather than freezing an engine;
global/worker ceilings are observed but not independently saturated.
[Fixture instructions and evidence limits](../../../scripts/testing/tauri-windows-native-smoke/README.md)
retain these distinctions.

## Native Windows app with localhost forwarding disabled

The separate diagnostic branch at `dfe1996`, including production revision
`6464f3b`, passes GitHub [run 41712473304](https://github.com/npgrosser/splitty/actions/runs/41712473304).
Linux builds the production assets, backend and private rootfs from the same
revision; the Windows runner verifies their digests and lockfile provenance.
The existing full-UI fixture runs under NAT with `localhostForwarding=false`.
Matching guest/Windows nonce responses first prove forwarding works when
enabled. Guest-positive/Windows-blocked controls then enclose the actual app
run, while configuration bytes remain unchanged.

The actual WebView2 app loads production CSS, font and WASM, renders the Ghostty
canvas, persists settings and passes the bounded shared-popout checks. Binary
PTY traffic and a 1,048,833-byte transfer pass; its independently checked SHA256
is `2863365348a5ef27b6035a51ff1783a1303e28f78045001face85bee49b8caae`.
Duplicate start retains the first owner; distribution switching performs a real
app restart. Both app instances exit naturally, owned helpers/PTYs exit, sampled
app and guest listener inventories are empty, independent fixture workers
survive, and owned handles return to zero. Both app distributions and the
control distribution are removed; configuration restoration and cleanup pass.

This establishes the disabled-forwarding application case of task 7.6. It does
not establish sibling-remote isolation, the remaining consumer-pressure cases,
whole-framework memory bounds or continuous TCP-listener absence. The separate
local SSH listener observation remains unresolved and is not waived by this run.

## Hosted Windows TCP measurement control

The isolated diagnostic at `58ed0ce` passes GitHub
[run 41717658574](https://github.com/npgrosser/splitty/actions/runs/41717658574)
on a fresh Windows 2025 runner. It creates only its own IPv4/IPv6 loopback
listeners and connected peers. `GetExtendedTcpTable`, raw CIM properties and
`Get-NetTCPConnection` agree: listener state 2 and established state 5, with
matching endpoints. Independent decoder controls pass and every owned socket
is closed. The retained helper matches the earlier local candidate exactly
after checkout CRLF normalization.

This validates the decoder on the hosted runner. It does not explain or waive
the local native-API state 0 / CIM null listener control or classify the earlier
SSH observations. The later application run above supplies its own successful
strict inventory; the self-socket control alone is not an application pass.

## Unix window isolation and receipt-pressure diagnosis

The new actual GTK/WKWebView fixture uses the production window builder,
bootstrap, permissions and transport with two fixed embedded Remote backends.
It does not claim SSH deployment coverage. GTK required distinct private
context keys so each incognito Remote WebView registers the packaged protocol;
the production fix preserves private contexts and eliminates the blank-window
failure. Main and both Remote windows subsequently pass opener/named-reference,
storage, target and local-action isolation checks. The fixture now creates its
SessionClient before adopting the channel so the real welcome capabilities
cannot be missed.

The remaining Linux replay failure was reproduced with temporary bounded
first-close diagnostics at source revision `e443ea3`. Both the deliberately
stalled channel and a consuming replay channel exhausted the 64-slot ordinary
control queue. The replay context held approximately 343 KiB, far below its
byte limit; subsequent “invalid queue transfer” errors were consequences of
that closure. Each consumed data frame had enqueued a serialized receipt into
the same queue as ping/cancel, although the admitted data window allows 1024
frames. A separate instrumented run passed, confirming that a single pass is
insufficient to establish a fix. These diagnostics were removed after capture.
The fix stores compact charged receipts in a separate bounded queue and
serializes only the writer's current receipt under the existing control budget.
A deterministic regression using actual frames and a blocked writer fails
before the change and passes afterward. All 51 transport unit, one protocol
integration and four allocation tests pass, as does all-target clippy. The
regressions verify 1024 consumed frames, exact original-credit release,
ping/cancel priority, overflow closure and normal cancellation with zero
retained bytes. Independent source review covers the final allocation ordering.

The uninstrumented actual Linux run with the fix passes: Main/two-Remote
isolation, concurrent healthy API/file work, 94,084 replay bytes including the
post-close marker, 32-stream window saturation/refusal/replacement/recovery,
one reload and natural app exit. All final bytes, carriers, streams, workers,
handles and opening counters are zero; observed shell and terminal processes
exit. The 92 sampled TCP inventories contain no listeners. The healthy work
starts during the stalled channel's lifetime and completes just after its
closure; no stronger overlapping-completion claim is made. This fixture uses
packaged minimal test content and receipt starvation, not the full UI or an
OS-suspended JavaScript engine.

The integrated macOS run at `cb51fa7` passes both actual WKWebView fixtures
([CI run 41727093381](https://github.com/npgrosser/splitty/actions/runs/41727093381)).
The production full UI completes 37 document generations. The separate
Main/two-Remote isolation/consumer run replays 94,112 bytes including the
post-close marker; 258 healthy API replies and the verified file transfer
complete while the slow consumer is still stalled. Window saturation,
refusal/replacement/recovery and final resource counters pass. Both apps exit
naturally with all observed shell/terminal processes gone; 324 full-UI and 151
isolation-run TCP samples contain no listeners. The recorded transport source
hashes match `cb51fa7`. Cookie-positive and whole-engine/RSS claims remain
excluded, as in the other minimal isolation fixtures.

The same run passes 90 GUI tests, 12 real-child carrier cleanup cases,
private-endpoint peer-user checks and native exposure/gateway positive controls.
All non-macOS CI jobs and CodeQL pass. The macOS core suite still reports 389
passed and two failed; both failures reproduce on exact baseline `cc66e97`.
The explicit final gate correctly keeps the overall CI result red. The core
step's continue-on-error status must not be interpreted as a passing core suite.
No new functional platform failure is waived by the passing WebView evidence.

Task 7.6 is complete on the combined platform evidence, not on any single
smoke result. Packaged assets and binary traffic use the full-UI runs; the
separate minimal fixtures establish remote/setup authority, consumer isolation
and admission recovery. Disabled forwarding and distribution switching apply
to Windows/WSL and use its dedicated hosted runs. App exit, real owned-child
failure/cancellation and surviving independent daemons use the lifecycle
fixtures described above. “Frozen client” means withheld output consumption;
no operating-system engine suspension or total-RSS bound is claimed. The
framework allocation limitation remains explicit, and the core-suite gate is not waived.

The documentation revision `8521d6b` has the same transport source hashes.
Its [CI run 41730093388](https://github.com/npgrosser/splitty/actions/runs/41730093388)
again passes every non-macOS job, both WKWebView fixtures and native boundary
checks; only the same two core baseline tests fail. The isolation run replays
94,114 bytes with its marker and zero final resources. CodeQL passes all four
languages. CI executes the required formatting, workspace/GUI clippy and tests,
frontend check/test/build, Electron build and architecture checks. The separate
`openspec validate --specs` command passes all 14 specifications. Final cutover
and task 9.6 remain open pending the macOS corrections and final integration;
this records the checked revision rather than waiving those gates.

## Quantitative memory contract

NCT-8 now explicitly covers application-owned transport data throughout the
path: reassembly, queues, conversions, retained diagnostics and native IPC
source/encoding reservations through platform completion or disposal. All
existing numeric caps, native authority checks and cleanup ownership remain.
Wry-owned inbound invoke bodies allocated before the handler, generic framework
replies, unrelated engine storage and total process/renderer RSS are excluded.
No framework fork is part of this change.

Independent boundary review found no concrete foreign-page or SSH-peer route
to local authority requiring a Wry fork. A compromised packaged renderer can
nevertheless exhaust framework storage before application validation. Remote
windows therefore isolate authority but do not promise availability isolation
from every other window sharing the desktop process. This limitation is now
normative in the client-connections delta and explicit in the security and
architecture documentation, rather than an unfulfilled quantitative claim.

## Open merge gates

- The current Linux/macOS/Windows WebView isolation and consumer fixtures pass,
  including the integrated receipt fix on Linux and macOS. The macOS core
  baseline failures remain blocking independently of those platform fixtures.
- The strict native Linux reservation test passed with an actual surviving
  worker after coordinator SIGKILL, IPv4/IPv6 gateway exclusion and reuse only
  after observed worker exit. The real SSH/hostile-peer crash variant and
  native macOS positive controls and the hosted Windows app/SSH inventory now
  pass. Task 5.9 is complete on this combined evidence; actual window-isolation
  and consumer evidence is recorded separately under task 7.6.
- Migration, conformance and security review must reconcile the remaining task
  checks, including the macOS corrections, before cutover is approved.

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
