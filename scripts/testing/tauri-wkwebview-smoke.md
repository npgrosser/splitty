# Actual macOS WKWebView smoke

Status: prepared and checked on Linux; **actual macOS build and execution remain
unverified until the macOS CI job or a Mac run passes**. Source generation and
observer unit tests are not runtime evidence.

On a dedicated macOS GUI session with Xcode command-line tools, Rust, Node 24,
pnpm 11 and the repository dependencies:

```sh
pnpm --dir frontend install --frozen-lockfile
pnpm --dir frontend build
cargo build --locked -p splitty-server
python3 scripts/testing/tauri-wkwebview-smoke-test.py
python3 scripts/testing/tauri-wkwebview-smoke.py \
  --full-ui --coexist-daemon --reloads 36 \
  --evidence-dir /tmp/splitty-macos-native-evidence
```

The real Tauri/Wry/WKWebView app compiles in a temporary harness from production
modules. It embeds `frontend/dist`, uses the existing production initialization
capsule and native adapters, and reuses the Linux smoke's `entry.ts`, `full-ui.ts`
and protocol assertions from `report.rs`. Temporary read-only instrumentation
adds reporting commands, bootstrap observations and native counters. The only
platform replacement enumerates actual AppKit NSWindows as well as Tauri's
managed windows. It checks both parent-mediated `window.close()` and the native
parent close command, including the absence of leftover native child shells.
It does not change production sources or enable a mock runtime or HTTP UI server.

The fixture checks packaged CSS/fonts/Ghostty WASM/canvas, private bootstrap,
settings/workspaces, raw binary PTY output, real reloads with stable embedded
instance, revoked old proofs, steady handle quotas and final zero byte/admission
counters. The 36 reloads exceed the 32-handle domain ceiling. App-owned shells
must exit. With `--coexist-daemon`, an independent real daemon and keeper shell
under the same **private fixture configuration** must retain discovery,
generation and working input after app exit; final fixture teardown then ends
only those saved processes. HOME, XDG paths and SPLITTY_CONFIG_DIR are isolated.
Short `/tmp` fixture paths avoid macOS Unix-socket path limits.

`result.json`, bootstrap events, epochs, source/asset hashes, build/runtime logs
and platform scope are written into the evidence directory. `ps` inventories
owned descendants; `lsof` samples their IPv4/IPv6 listeners and newly observed
WebKit XPC candidates. macOS can parent XPC helpers to launchd, so candidate
attribution is observational and pre-existing shared helpers are not attributed.
Listener sampling cannot rule out an arbitrarily brief listener. Exact process
cleanup uses Darwin libproc PID start times including microseconds, not just a
PID or seconds-only `ps` timestamp. The smoke neither bounds framework inbound
body preallocation nor generic command-reply/JavaScript-engine RSS.

Portable preparation, without claiming a Mac build or runtime pass:

```sh
python3 scripts/testing/tauri-wkwebview-smoke-test.py
python3 scripts/testing/tauri-wkwebview-smoke.py --prepare-only \
  --harness-dir /tmp/splitty-wkwebview-prepared \
  --evidence-dir /tmp/splitty-wkwebview-preparation
```

The added `tauri-macos` job in `ci.yml` uses existing pinned actions and unchanged
workflow triggers. It builds/tests the actual macOS GUI and core, runs the
alternate-user native endpoint probe after a mandatory passwordless-sudo
preflight, and runs listener positive controls, actual native-child exposure and
explicit browser-gateway lifecycle tests. It then runs the full WKWebView smoke
and uploads evidence even on failure. No release, tag, publish or CI dispatch is
part of this preparation.
