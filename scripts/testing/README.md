# Remote forwards browser adapter

## Managed-job browser smoke

`managed-jobs-browser.mjs` owns a fresh backend, private HOME/XDG directories and
browser context. It exercises background tabs, all four split directions, new
screens, actual keyboard routing, shared output, retained ended panes and failed
job reload. It never targets an existing daemon.

```sh
node scripts/testing/managed-jobs-browser.mjs \
  --binary /absolute/path/to/target/debug/splitty \
  --playwright-module /absolute/path/to/playwright-core/index.mjs \
  --browser /absolute/path/to/chrome \
  --evidence-dir /tmp/splitty-jobs-evidence
```

Build the frontend and binary first. Full Chrome runs headless with its sandbox
enabled. Optional evidence contains a screenshot of generated fixture output only;
xterm uses a GPU canvas, so the test checks received binary frames plus the visual
artifact rather than pretending terminal text is in the DOM. No paid agent call is
required. CLI/grant restarts and daemon no-replay are separately covered in the
existing Rust `daemon_lifecycle` smoke harness.

## Remote-forward adapter

`remote-forwards-browser.mjs` is a narrow smoke-level adapter for OpenSpec
`remote-forwards-persistence` design D8 and task 4.1. It consumes the existing
`crates/splitty-server/tests/remote_connect.rs` lab. It starts only Chrome, owns
only its browser context and two test mappings, and adds no npm dependencies.
Real browser activation and SSH transport cannot be established by frontend
unit tests.

**Verified on 2026-10-28** in a Debian bookworm arm64 container (OpenSSH 9.2,
OpenSSL 3.0, Chromium 156): all three results passed with Playwright 1.58.1 and
1.60.0, TLS included, cleanup pass (see "Calling it from the Rust lab" for the
container). The controller regression suite is separate from this real-browser
check.

## Exact private fixture contract

Invoke Node with an explicit lab-only acknowledgment:

```sh
node scripts/testing/remote-forwards-browser.mjs --lab-only --config -
```

Write one JSON object to its private stdin pipe, then close the pipe. No cookie
is accepted in argv or environment variables. Alternatively use `--config` with
an owner-only regular file (0600, owned by the invoking uid, no symlink). The
nine fields are mandatory; unknown fields are rejected:

```json
{
  "hubUrl": "http://127.0.0.1:30001",
  "cookie": "splitty-session=<fresh HubServer session value>",
  "target": "splittyhost",
  "remote": 30002,
  "local": 30003,
  "marker": "unique-per-run-real-service-response-marker",
  "playwrightModule": "/absolute/path/to/playwright/index.mjs",
  "executablePath": "/absolute/path/to/full/chrome",
  "apiService": {
    "remote": 30004,
    "local": 30005,
    "marker": "unique-per-run-api-service-marker",
    "tlsCa": null
  }
}
```

The example is a schema illustration, not a fixture to run. Requirements:

- The caller owns a throwaway `Lab`, `HubServer`, session and two namespace
  services for the whole invocation. `--lab-only` acknowledges those ownership
  conditions and authorizes creation and deletion of the supplied mappings.
  A URL alone cannot prove ownership; never supply an existing user daemon or
  preview. Known preview port 44877 is refused for the hub and local forward.
- `hubUrl` is the literal HTTP IPv4 loopback origin with an explicit port;
  paths, bootstrap queries, credentials and redirects are rejected. The hub
  serves the final implementation's built frontend assets and APIs together.
- The hub has **zero active and saved forwards**, fresh private settings and no
  remembered screens/host connections or concurrent clients. The adapter
  checks both forwarding APIs before any mutation and refuses dirty state.
  It does not snapshot, delete or reconstruct pre-existing mappings.
- `target` is the lab's SSH alias (normally `splittyhost`), with letters,
  digits, `_` or `-`, starting with a letter. No public hostname, IP literal,
  `user@host`, SSH configuration or external target is accepted. The caller
  must ensure the alias resolves through the lab's namespace ProxyCommand.
- `remote` and `local` are integer ports 1024–65535. The local port is free on
  every supported loopback family and differs from the hub port. Do not keep
  a reservation socket open while invoking the adapter. The real backend's
  exact-local POST must succeed; no alternate port is chosen by this runner.
- The namespace service must return HTTP success and a plain-text body
  containing `marker` at `/`, through the real SSH forward. Use a new marker
  per invocation (16–200 printable ASCII characters), not a generic `ok` or a
  competing listener's marker. The service must not require hub credentials,
  redirect or load remote resources. Its `/hmr?mode=raw%2Fupdate` endpoint must
  implement the bounded fixture exchange in `Lab::marker_on_host`.
- `apiService` supplies the second service's distinct remote/local ports and
  distinct marker (16–200 letters, digits or hyphens). Both local ports must be
  free and differ from the hub and preview ports. Its four fields are required.
  `tlsCa` is either the ephemeral public PEM certificate (maximum 4096 bytes)
  or `null` only when OpenSSL is absent. It never contains a private key or path.
- Supply an existing Playwright module exporting `chromium` with
  `BrowserContext.routeWebSocket` support, and full Chrome, not headless-shell.
  The adapter uses headless full Chrome with its sandbox enabled and removes
  Playwright's default `--disable-popup-blocking` argument. It verifies the
  actual launch arguments with CDP, for which it passes `--enable-automation`
  itself (Playwright 1.60 no longer does). No extra launch options, proxy,
  certificate bypass, executable search, downloads, or fallback browser are
  supported. On Debian, use the binary `/usr/lib/chromium/chromium`, not the
  `/usr/bin/chromium` wrapper, which adds switches of its own.
- Leave `DEBUG`, `PWDEBUG` and `DEBUG_FILE` unset. Reports contain fixed failure
  codes and timings, never raw errors, headers, service bodies, cookies or
  configuration. No traces, screenshots or browser console logs are collected.

## Assertions and response ordering

The adapter establishes exactly one ephemeral mapping with the real API. If the
drawer is closed, it clicks the visible **Toggle drawer (Ctrl+B)** button.
Ordinary web pages omit that desktop titlebar, so the fallback right-clicks the
pane and chooses **Show Drawer** from its menu; the shortcut alone may be
swallowed by terminal autofocus. In the drawer's **Hosts** view it unfolds the
lab host's entry, which browses the host through the hub's `/ws/remote`, as it
does for a user who looks at its ports, then clicks the row's real **Open**
button. A transparent observer delegates to native
`window.open` and records whether it was called while a trusted click was still
being dispatched, with active user activation. It never creates a popup itself
or changes the native return value.

The browser's matching POST goes to the real hub using `route.fetch()` with
redirects and retries disabled. Only after its real HTTP 200 and matching
target/remote/local/URL/up body arrive does the adapter start a response gate
of at least 6500 ms. It releases that exact `APIResponse` using
`route.fulfill({ response: realResponse })`, with no status/header/body overrides.
It compares SHA-256 digests of the backend and browser response bodies to check
byte equality. No success is synthesized and the POST is sent only once.
During the positive hold the new tab must stay `about:blank`, have
`opener === null`, title
`Opening forwarded port…`, and a no-referrer policy. No service request may
occur. After release the browser must see that same matching HTTP 200.
Navigation must follow delivery of that response, retain
a null opener and show the unique real service marker. Service requests are
checked for absence of Cookie, Authorization and Referer headers. Other HTTP
origins and WebSockets other than the hub's `/ws` and its `/ws/remote` browse of
the lab target are blocked and fail these first two scenarios.

The second scenario opens a second real UI page in the same browser context
(sharing the session), then clicks Open in the first page. After its POST has
**completed at the backend**, the runner holds delivery of that success. The
second page chooses **Stop** from its row's **More** menu; a real DELETE 204 and
empty active/saved APIs must confirm Stop. The first page receives focus and
at least 6500 ms to refresh/poll before the old success is delivered unchanged.
This ordering is essential: holding the POST before backend execution instead
would let a later explicit POST legitimately resume the mapping.

The negative assertion requires that the pending blank be closed by completion,
that zero service navigation requests are attempted throughout the scenario,
and that the backend remain stopped. It does not require cancellation before
the delayed response arrives. `closedBeforeRelease` and
`convergedBeforeRelease` are informational observations only. An attempted
stale navigation is recorded and blocked at the
route, then reported as **failure**, never a pass merely because interception
prevented contact. Evidence includes backend-success, Stop-acknowledgment and
response-delivery times, whether the first page converged before release, and
service request attempts. A one-second observation after operation completion
also checks queued continuations. No DOM force or internal controller calls
are used.

### Two manual services and raw transport

After the D8 scenarios leave the fixture empty, the same adapter uses **Forward a
port on another host…** in the Hosts view twice. It enters the target and exact
remote/local ports, verifies Remember starts unchecked, then opts in and labels
the mappings Frontend and API. It verifies both UI rows, each named by its label
as only a remembered mapping is, and both independent saved/active API entries.
Neither service belongs to a host pane; the browser opens no host screen, and
its only host connection is the browse of the unfolded entry.

The Frontend URL returns its exact marker over HTTP in the real browser holding
the hub's `127.0.0.1` cookie. The browser then connects directly to the forwarded
`ws://localhost:<port>/hmr?mode=raw%2Fupdate`. This one service WebSocket is excluded
from Playwright routing: the browser performs the actual upgrade, subprotocol
negotiation, masking and close. The Python fixture validates Host, Origin, query,
subprotocol and absence of a Splitty cookie. The test checks a connected message,
a client text change request and server HMR-style update, a 256-byte binary
round trip (including zero/high bytes and extended frame length), and a clean
1000/`checked` close. This proves transport of an HMR-style exchange, not Vite's
module replacement or filesystem watching.

The API service is contacted by Node using `localhost`, an exact path/query and
a 256-byte POST. An unauthenticated request must reach the service's own 401;
its fixture-only application bearer credential must yield the exact marker and
unchanged bytes. No Splitty credential is sent to either service.

TLS is conditional on `openssl` being in PATH. The Rust fixture invokes
`openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj /CN=localhost -addext
subjectAltName=DNS:localhost -keyout key.pem -out cert.pem` inside its private
temporary directory. This requires OpenSSL 1.1.1 or later and Python's standard
`ssl` module. The API listener then uses TLS; Node trusts only that public
certificate, keeps hostname verification and `rejectUnauthorized: true`, and
asserts an authorized TLS 1.2/1.3 connection and the same API responses. This is
HTTPS passthrough evidence from Node, not browser certificate-trust evidence.
There is no certificate bypass or new package dependency. Missing OpenSSL runs
the API checks over HTTP and explicitly reports TLS **unrun**. Present but
unusable OpenSSL, missing Python SSL, certificate/handshake errors or changed
service responses fail the opted-in test; they never become runtime skips.

### Validation boundary

The controller validates each successful POST and optional Remember PUT against
a fresh passive active-forward GET before publishing or navigating. If another
window's acknowledged Stop is visible in that snapshot, the pending tab closes
without navigating. The same rule rejects a down or remapped forward and a failed
validation read. The current key's generation still guards the final asynchronous
read; other mappings are not overwritten by an older whole-list snapshot.

The negative scenario covers POST completion, then Stop, then delayed delivery
of the old POST success. A POST accepted after Stop is instead a valid explicit
Resume. The adapter requires closure by completion and zero navigation attempts;
pre-delivery closure is informational. It allows up to 15 seconds for passive
validation after response delivery, then observes queued continuations for one
second. This snapshot check cannot prevent changes made by another process after
validation. Controller mutation tests prove these distinctions in-process; this
adapter must still prove the actual browser and SSH behavior.

This adapter does not cover notices/fallback Open, terminal links, competing
listeners, Forget/disposal, multi-window convergence, native shells, a real dev
framework's hot replacement, or the wider task 4.1 matrix. It cannot close D7,
task 4.1 or final verification by
itself.

## Calling it from the Rust lab

The existing Rust smoke test
`the_browser_verifies_open_and_cross_window_stop_through_real_ssh` owns the
throwaway lab, hub, session and two namespace services with unique markers.
It passes the nine-field object over Node's private stdin and waits at most
120 seconds, with cleanup restricted to the owned process group.

Build `frontend/dist` before compiling the server, then set the paths to existing
full Chrome and Playwright installations and enable the SSH lab:

```sh
SPLITTY_TEST_PLAYWRIGHT_MODULE=/absolute/path/to/playwright/index.mjs \
SPLITTY_TEST_CHROME=/absolute/path/to/full/chrome \
cargo test -p splitty-server --test remote_connect \
  the_browser_verifies_open_and_cross_window_stop_through_real_ssh -- --nocapture
```

If sshd is unpacked instead of installed at `/usr/sbin/sshd`, also set
`SPLITTY_TEST_SSHD_ROOT` to its package root. The existing lab prerequisites
(`ssh`, `ssh-keygen`, `unshare`, `nsenter`, `ip` and permitted local sockets/user
namespaces) must work, and the browser hook additionally requires `node` and
`python3`. Use private test XDG directories; no user preview is an acceptable
fixture.

The lab needs Linux. On macOS, a container serves: Debian bookworm with
`openssh-server openssh-client iproute2 python3 openssl nodejs chromium`, the
tests run as an unprivileged user with `--security-opt seccomp=unconfined` (for
user namespaces) and `--init`. Without an init process the test binary is PID 1:
the host daemons and `ssh` processes the lab leaves to it are never reaped, and
three lab tests then wait in vain for processes that are already zombies. Behind
a proxy that intercepts TLS, `cargo fetch` in the container needs its CA
(`CARGO_HTTP_CAINFO`).

With both browser variables absent the test explicitly prints `browser UNRUN`;
that result is not browser coverage. Partial opt-in, unavailable opted-in lab or
dependencies, adapter failure/blocked status, and cleanup failures fail the test.
Success requires exit 0, JSON `status: "pass"`, all three `verified-open`,
`cross-window-stale-response` and `manual-services-transport` results,
`cleanup.status: "pass"`, and independent
empty forwarding APIs/no remaining hub SSH. No cookie or configuration is printed.
The Rust test also requires the transport assertions and a TLS pass when OpenSSL
was present. A pass with `tls.status: "unrun"` does not establish TLS coverage.

On ordinary success/failure the adapter aborts an outstanding response gate, closes its
browser, stops both mappings, forgets any preference for those owned keys, checks
that both forwarding APIs are empty, and removes its private browser XDG files.
The lab owner remains responsible for hub/service teardown after a process kill
or infrastructure failure. No attempt is made to restore unrelated application
state; the contract requires an otherwise disposable fixture.

## Checks that need no browser or sockets

```sh
node --check scripts/testing/remote-forwards-browser.mjs
node --test --test-isolation=none scripts/testing/remote-forwards-browser.test.mjs
node scripts/testing/remote-forwards-browser.mjs --check-module /absolute/path/to/playwright/index.mjs
```

`--check-module` only imports the specified module and validates its Chromium
entry point. Its result explicitly says `browserLaunched: false` and
`featureTestsRun: false`. The guard tests run in-process
(`--test-isolation=none`) and cover input rejection/redaction only, not browser
behavior. Use a Node version that supports `--test-isolation=none`, or invoke the
test file directly.
