# Electron shell

Run `pnpm install --frozen-lockfile`, then `pnpm build` from this directory.
The build compiles the frontend, compiles the main/preload TypeScript, and copies
only declared frontend resources into `compiled/ui/files`. Its generated
`compiled/ui/manifest.json` records exact paths, content types, byte lengths and
SHA-256 digests. Electron Builder includes this directory through `compiled/**/*`.
`pnpm pack` makes an unpacked application; `pnpm dist` builds the configured package.
`pnpm smoke:assets` runs a hidden sandboxed Electron window against the built
asset bundle, checking CSS, font and WebAssembly loading, denied unknown paths,
absence of renderer Node access and CSP rejection of network scripts/connections.
It needs a graphical session (including WSLg) and the Electron runtime installed.
`pnpm smoke:pipes` runs the compiled Node pipe adapter against the real Rust
executable at `../target/debug/splitty`; pass another executable path as its
argument when testing a release build. It uses isolated settings, checks native
welcome/API ordering and listener-free status, then observes clean child exit
and zero byte/worker counters after EOF. It does not require a graphical session.
`pnpm smoke:owner` checks the embedded service's app lifetime with independent
native clients: shared state, 40 document reconnections, isolated invalid host
opens, cancellation with all data slots occupied, and cleanup despite a failing
document notification. It also checks untouched independent daemon metadata and
clean process/byte/stream counters after the owner exits.
`pnpm smoke:ipc` runs an actual sandboxed Electron window with the compiled
private preload and a real Rust owner. It checks binary MessagePort delivery,
API results, reload revocation without a service change, denial of an unassigned
packaged window, and an old document sending during a deliberately delayed
navigation. It also redelivers that old IPC after commit, including Electron's
reuse of the same frame object. No raw port reaches application JavaScript.
Its Linux extension suspends a separate real renderer with SIGSTOP while a
healthy window uploads and downloads a 1 MiB binary file. Only the frozen
client is disconnected; reattaching confirms that later PTY output was captured.
It also checks byte-exact terminal output, explicit/disconnect upload cleanup,
32 admitted streams in one WebContents, rejection of excess opens and continued
API use at that ceiling. Reported memory values are observed native transport
budget counters, not total process RSS or a cross-platform WebView guarantee.
The watchdog resumes suspended renderers and observes helper exit, with bounded
cleanup escalation.
`pnpm smoke:app` runs the production main entry and built frontend in an isolated
package/configuration fixture against the debug Rust executable. It loads settings
through IPC, executes a command in a real terminal, reloads with a conflicting
host query, and verifies that the UI makes no HTTP/WebSocket request. On Linux it
also inventories actual helper TCP listeners. App shutdown must observe helper
exit and return all process/byte counters to zero. The same smoke saves a real
blob download, rejects its reveal after document replacement, and verifies that
a popout has a parent reference but no own bridge/client. Parent-mediated calls
remain in the explicitly shared trust domain.

`pnpm smoke:remote-lifecycle` runs the production remote-window orchestration
against real delayed-exit child pipes with stubbed Electron windows. It checks
that four cancelled preparations retain all four slots until process exit, that
navigation revokes the last requesting document and discards late progress/ready,
and that cancelling one of two coalesced requesters leaves the other intact.
It also admits 32 actual Node fixture children through the production helper
supervisor, refuses the next spawn and keeps capacity occupied until child exit.
This checks helper lifetime; it does not substitute for 32 real SSH backends or
the real SSH/window smoke.

`pnpm smoke:package [path/to/linux-unpacked/splitty-electron]` launches an
actual Linux distribution package with a private configuration. It waits for a
real terminal created by the packaged UI, checks the bundled helper's listening
sockets, then terminates the application and observes helper/PTY exit. Independent
daemon discovery and keeper identities must remain untouched; the same independent
job must still run a command after app exit. Startup and teardown share a cleanup
scope; failed teardown retains its fixture for investigation. This test uses Linux
`/proc`; the other
platform runtime checks remain separate. The development validation may bundle
the matching debug helper explicitly; shipping packages use the release helper.

Both packaging commands require the server executable at `../target/release/splitty`.

The registered `splitty://app` protocol serves only manifest entries. `/` is the
explicit alias for `/index.html`; `/setup.html` and the empty parent-owned
`/popout.html` document are the other declared entries.
Unknown routes are not a SPA fallback. Authorities, methods, encodings and paths
cannot select arbitrary files or network requests. The build refuses symlinks
and undeclared resource types; the handler verifies size and digest before
serving content. CSP permits packaged scripts, styles, fonts and WebAssembly,
with no network script source, wildcard loopback connection or child frame.
Each ephemeral remote session receives its own protocol handler.

The native pipe adapter negotiates framing and service readiness separately,
checks the helper's fixed direct/host purpose, and holds data charges until
physical writes and receipt consumption complete. Framing, preparation,
attachment and partial-frame deadlines are bounded; heartbeat controls have
reserved capacity even when a renderer stops consuming. The helper supervisor
retains process slots until observed exit, bounds stderr, and closes inherited
diagnostic pipes before releasing their reservation. The production launcher
connects the packaged frontend through the document broker and private MessagePorts.
`NativeOwner` uses the helper's `--owner` mode to retain one embedded runtime
across independent client connections. Each client negotiates its own native
stream, while the owner's private pipe carries bounded chunks and reserved
cancellation controls. Closing a client keeps the service alive; closing the
owner ends it. Host substreams use this same service's Hub. The owner carrier
is never handed to a renderer.

`NativeBroker` grants an actual main frame a fresh native document challenge.
Navigation blocks opens until commit and invalidates the preceding challenge;
handles, deliveries and cancellation remain tied to that document. Frame
identity or packaged origin alone is insufficient. The private preload keeps
the MessagePort, charges bounded outgoing queues and incoming reassembly, and
permits one 64 KiB IPC chunk in each direction until acknowledged. IPC completion
and native delivery receipts retain independent references to shared payloads.
Broker admission remains occupied until the owner confirms stream cleanup, or
the helper actually exits after carrier failure.

The production launcher uses an app-owned embedded helper for local windows and
fixed-host SSH helpers for separate remote windows. Window settings and known
hosts use bounded native API reads. Frontend startup reads use a short-lived
client before the application graph loads; the terminal client starts only after
its welcome handlers are installed. Root roles come from the broker assignment,
not a host query or a bridge global. Main-window host streams use the embedded
service's Hub and retain its progress/ready/error protocol without browser tickets.

All shell commands carry the private preload's document challenge. Pending
remote opens retain each requesting document, stop when their last requester is
revoked, and retain opening capacity until the helper's exit is observed. Closing
a host view cancels preparation through the same broker. Quitting prevents a
second reentrant quit until all owned helpers have exited.

Remote roots retry transient failures for at most ten minutes, preserving the
packaged document across SSH loss and daemon replacement. The native process
sets titles from its fixed target and document-checked connection state. Opens
and deliberate retries are coalesced until the workspace connects. Cancellation
also gates delayed renderer retries before any helper can spawn; capacity remains
held through observed child exit. Only an explicit open passes `--remember-target`;
automatic recovery preserves hidden host metadata. Structured, bounded SSH
failures retain the host-key verification hint.

The pre-review transport acceptance matrix is recorded in
`openspec/changes/native-client-transports/verification.md`. Follow-up review
corrections and their validation are tracked there separately; this local smoke
does not by itself establish the complete platform matrix.

## Browser assignment smoke

After building `frontend/` and the debug `splitty` executable,
`pnpm smoke:browser-assignment` starts an isolated, explicitly requested browser
gateway and loads its production frontend in Chromium with no native preload.
It verifies manual login, invalid-token handling, bootstrap removal and terminal
startup after assignment. It then suspends the first document, restarts its
gateway on the same port and authenticates a second tab before resuming the first.
The first tab must reject the changed gateway instance and reload before opening
another workspace connection. Both the gateway and browser profiles belong to
the fixture; no existing daemon is used. Remote role restrictions are separately
covered by the gateway and frontend assignment tests.

The production-entry `smoke:app` also creates two actual isolated remote
WebContents using native registration hooks exposed only in its temporary test
copy. It verifies no opener or named-window reference into main/popout/another
remote, private ephemeral storage, denied editor/reveal/window IPC, blocked
clipboard reads and HTTP(S)-only system-browser handoff without a child window.
Navigation stays on each role's assigned packaged document, and a rejected
navigation leaves the main document's existing native connection usable. This
policy probe does not open SSH; helper/session survival remains separate
transport smoke coverage.

An admitted navigation that is later aborted remains a revocation: the IPC
smoke confirms a new document challenge, rejected old requests and unchanged
embedded service identity. The app offers Reload for that revoked assignment;
the production smoke checks that action recovers the workspace. Ordinary
connection loss continues to offer Reconnect.


## Real SSH remote-window smoke

The opt-in `electron_remote_windows_reconnect_without_navigation_through_real_ssh`
case in `crates/splitty-server/tests/remote_connect.rs` uses the existing isolated
OpenSSH namespace lab and the compiled production Electron entry/preload/UI.
Build the Electron frontend first, then set `SPLITTY_TEST_ELECTRON` to the absolute
Electron executable and, when using an unpacked sshd, `SPLITTY_TEST_SSHD_ROOT` to
its root. Run the named cargo test with `--nocapture`.

It checks unknown-host-key hints, coalesced opens, recent/hidden hosts, native
titles, actual terminal input before and after SSH loss and daemon restart,
discarded offline input, same-document deliberate recovery, no HTTP/WebSocket
requests, keeper survival after window closure and zero transport counters at
quit. A missing opt-in reports UNRUN; it is not runtime evidence. The separate
`smoke:remote-lifecycle` harness retains the delayed-retry cancellation,
child-exit-before-UI-handoff and multi-helper startup cleanup regressions.
