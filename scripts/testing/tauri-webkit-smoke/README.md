# Actual Linux WebKit smoke

From the repository root, with Xvfb available (or pass `--xvfb /path/to/Xvfb`):

```sh
python3 scripts/testing/tauri-webkit-smoke.py --evidence-dir /tmp/splitty-webkit-minimal
pnpm --dir frontend build
python3 scripts/testing/tauri-webkit-smoke.py --full-ui --coexist-daemon --reloads 36 --evidence-dir /tmp/splitty-webkit-full
```

`--coexist-daemon` also needs the current `target/debug/splitty` binary. It starts
a real daemon and keeper job under the same private fixture configuration, then
checks unchanged discovery, daemon/keeper generations and working input to the
same job shell after app exit. Fixture cleanup runs on success and failure.

Both modes compile a temporary instrumented composition root against the actual
production Rust modules, Tauri capabilities and CSP, using the existing shared
Cargo target. They isolate HOME/config/data/cache. Every GUI run starts a private authenticated
Xvfb display, disables its TCP listener, forces GTK X11 and removes Wayland from
the application environment; the user desktop is never used. No HTTP UI server
is started. A daemon exists only with the explicit fixture coexistence option. The minimal mode keeps a small native protocol reproducer.
Full mode copies `frontend/dist` unchanged, mounts the production Svelte App,
drives the renderer setting through the UI, and checks packaged CSS, font,
Ghostty WASM and rendered canvas. Its separate native protocol probe uses a
reserved request-ID range so independently bundled adapter counters cannot
collide with the production application's counters.

Evidence includes source and packaged asset hashes, native bootstrap/reload
proof checks, settings/workspace and byte-exact terminal output, heartbeat
survival in the first/last document, native counters for every document, observed shell exit, and sampled TCP listener
inventories across the process subtree's observed network namespaces. Fetch and
WebSocket instrumentation rejects HTTP/API fallback. Reload success requires a stable native handle quota with no stale handles,
independent of whether retained handles have delivery backing. Current in-flight
deliveries remain visible as diagnostics. The 36-reload example exceeds the
32-handle domain ceiling without allocating additional probe PTYs in intermediate
documents. Full mode opens popouts only in the first document. It requires a usable
parent reference, direct-command denial, explicit parent mediation, successful
`window.close()` and actual native GTK/window inventory returning to baseline.
The scoped parent close command rejects main/remote/unknown labels. One final
child deliberately survives until parent navigation; the next document requires
native teardown. These are assertions against production lifecycle behavior,
not a test-only close implementation.

Full mode destroys the actual WebView before checking final native counters;
application shutdown then closes owned PTYs. It never simulates destruction by
purging the broker while a WebView still owns framework buffers. This harness
adds only test instrumentation to temporary source copies; it does not modify
production startup or authentication.

This is Linux WebKit runtime coverage. It does not establish macOS/WebView2
behavior or a bound on framework allocations performed before application
validation. Sampling listeners is evidence about the observed run, rather than
proof that no arbitrarily brief listener could exist.

## Optional isolation and consumer case

`--isolation-consumer` is a separate mode from `--full-ui`, available in both
the Linux and macOS scripts. It uses actual Main and two Remote WebViews with
production bootstrap, roles, capabilities, navigation and ephemeral storage.
Only the two native-fixed remote backend locations are substituted with distinct
real embedded services; `backend-substitution.json` records the generated source
hashes. This is no SSH, host-filesystem or additional production-UI proof.
Editor/navigation OS effects are intercepted and must remain unused. Copied
public handle checks describe private-capsule rejection, not raw IPC authority.

A receipt-starved client runs beside concurrent healthy API/file work. Existing
pressure chronology is retained without requiring completion before the slow
client closes. Replay verifies continued capture; a 32-handle window flood must
refuse excess work, release a slot, admit a replacement and recover to baseline.
The final report additionally verifies file bytes and closes both fixture
runtimes/windows before requiring zero native bytes, handles and admission.
This is not an engine freeze, renderer RSS or independent global/worker ceiling
test. Source preparation and compile checks alone leave runtime unverified.
