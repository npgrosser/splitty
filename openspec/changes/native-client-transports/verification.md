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
- Actual macOS CI passes GUI all-target clippy and all 89 GUI library tests.
  Private endpoint peer-user checks, native forwarding reservation, IPv4/IPv6
  listener positive controls, owned-helper lifetime and explicit gateway
  replacement also pass. Two core failures (Bash CLI PATH restoration and the
  background-writer PTY fixture) reproduce on exact baseline `cc66e97`; they
  remain failing checks and are recorded in the baseline findings. The earlier
  managed-stop failure did not recur in this run, so its diagnostic remains.
  Resolving the fixture home to its canonical private path allows WKWebView
  startup. The next run reaches the actual UI and popout, where WKWebView
  inherits the test initialization script into the child: the child correctly
  lacks native authority but incorrectly reports the parent-only capsule
  assertion. The probe now gates on the top packaged index document and adds
  document metadata to reports. Three Node regressions and six portable
  checks pass; the full actual rerun remains open. Production bootstrap and
  endpoint rejection are unchanged. Independent runtime evidence does not
  waive the mandatory core failure gate.

These results describe the runs performed, not proof that every requirement is
complete. Final source checks and platform results must be reconciled before
marking the change ready.

## Migration evidence

| Case | Evidence | Limit |
| --- | --- | --- |
| Compatible native daemon | Private endpoint/negotiation tests and actual daemon client smoke | Remaining desktop platform cells are tracked separately. |
| Older kept daemon update | `the_hub_updates_a_host_daemon_of_another_version` passed through actual isolated OpenSSH and real daemon replacement, preserving keeper socket identity | The old version/protocol is simulated in discovery metadata; this is not execution of a historical release binary. |
| Incompatible non-kept/newer daemon | Supervisor and HTTP policy tests plus the extended real-SSH `the_hub_updates_a_host_daemon_of_another_version`: explicit updates refuse both cases without replacing discovery, daemon PID or keeper sockets | Version/protocol differences are simulated in valid discovery metadata; remaining desktop platform checks stay open. |
| Supported client downgrade | The same real-SSH test connects to a compatible newer-version daemon without restarting it or changing discovery/keepers | Version metadata simulation, not execution of a historical client binary. |
| Stale/corrupt/legacy discovery | Daemon lifecycle `native_discovery_loss_corruption_and_concurrent_start_cannot_replace_a_live_daemon` and `a_live_legacy_record_is_diagnostic_and_never_selects_its_tcp_port` | Legacy ownership uncertainty refuses implicit replacement. |
| Explicit restart and keeper adoption | `a_shell_outlives_stop_restart_update_and_crash_of_its_daemon` and explicit daemon gateway replacement smoke | Keepers are real processes; historical release binaries are not covered. |
| Local app with independent daemon | Actual Linux Tauri and Electron coexistence smokes; owned-child pipe lifecycle smoke | macOS and the complete Windows matrix remain separate gates. |

The catalog/role audit found no remaining native HTTP/token fallback. Static
classification and native/browser conformance tests cover enumerated operations,
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

## Open merge gates

- Whole-path resource limits: Wry allocates inbound IPC request bodies before
  application validation. Application-owned quotas do not establish a bound on
  framework allocation, generic invoke replies, JavaScript engine memory or RSS.
- Full macOS WKWebView runtime and the complete platform isolation matrix
  remain unverified; native macOS endpoint identity checks pass. Windows remote-only/reconnect now passes
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
