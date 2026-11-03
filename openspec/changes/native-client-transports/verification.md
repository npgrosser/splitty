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

- Workspace tests excluding the GUI: 1,020 tests passed. Workspace clippy passed.
  Subsequent shared-budget CAS changes passed their targeted tests.
- Frontend: 1,502 tests passed; check completed with zero errors and eight existing
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
- macOS harness preparation and portable tests passed. The new CI job is intended
  to run the actual WKWebView, endpoint and lifecycle checks. It has not yet
  established macOS runtime results.

These results describe the runs performed, not proof that every requirement is
complete. Final source checks and platform results must be reconciled before
marking the change ready.

## Open merge gates

- Whole-path resource limits: Wry allocates inbound IPC request bodies before
  application validation. Application-owned quotas do not establish a bound on
  framework allocation, generic invoke replies, JavaScript engine memory or RSS.
- Native macOS endpoint identity, WKWebView runtime, and the complete Windows
  remote-only/reconnect/isolation matrix remain unverified.
- Native Linux/macOS durable forwarding reservation positive cases and the full
  listener/resource inventory remain open. WSL refusal checks are not positive
  native-platform coverage.
- Disabled WSL localhost forwarding has not been exercised; the pipe tests ran
  with the existing host networking configuration unchanged.
- Windows remote SSH smoke reached daemon replacement/reconnect but failed the
  PTY reattach assertion. Whether this is fixture subscription logic or product
  behavior remains unresolved.
- Migration, conformance and security review must reconcile the remaining task
  checks, including framework-bound scope, before cutover is approved.

## Windows harness cleanup incident

The process inventory followed historical parent PIDs without validating the
parent generation and lifetime. PID reuse caused unrelated Firefox processes
to be classified as fixture descendants, and cleanup attempted to terminate
them. All 28 recorded Firefox/crash-helper PIDs were subsequently absent.
Individual termination results were not captured, so direct termination cannot
be distinguished from descendants exiting with their parent.

Windows GUI drivers are disabled. Prior Windows process-tree cleanup inventories
are not evidence of safe isolation. Pure snapshot regressions cover the original
PID reuse case, but a mixed-time process enumeration also needs a proven parent
lifetime: matching a child's creation time and executable at termination does
not establish ownership. Any replacement cleanup must establish ownership and
bind termination to that exact OS process identity. No PID-only or recursive
`taskkill /T` cleanup is acceptable. Runtime verification remains blocked until
the ownership implementation receives independent review.
