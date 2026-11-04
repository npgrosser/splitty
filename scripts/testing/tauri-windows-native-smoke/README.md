# Native Windows and isolated WSL smoke

The latest hosted isolation/consumer run at `6b08775` **passes overall**
([run 41720162085](https://github.com/npgrosser/splitty/actions/runs/41720162085)).
Setup/Main/two-Remote isolation, slow-client closure, healthy same-connection
API/file work, post-close API, original-order replay, 32 handles, excess refusal,
replacement and baseline recovery all pass. The replay includes the actual
post-close marker in 94,084 bytes, with write/open preceding attach and no added
diagnostic awaits before attach. Both file SHA checks and binary PTY pass.

The strict raw-CIM inventory reports no listener/unknown rows or query error;
known non-listening rows are retained separately. Natural app exit, zero owned
handles, independent sentinel preservation, exact distribution removal and WSL
configuration restoration pass. The remote daemon survives app exit until
fixture teardown. This uses forwarding enabled for the explicit SSH fixture.

Earlier local runs retain their failed status: `a52fde175fc7` had unclassified
listener rows, and `c92108c1f378` lacked the replay marker. The hosted result does
not explain those failures or the local raw-API/CIM state anomaly. See the
[verification record](../../../openspec/changes/native-client-transports/verification.md)
for the evidence and its remaining limits.

## Entrypoints and evidence

Build `frontend/dist` and the Linux debug `splitty` payload first. From WSL, with
the installed Windows MSVC toolchain, the enabled fixture entrypoints are:

```sh
python3 scripts/testing/tauri-windows-native-smoke.py --build-only
python3 scripts/testing/tauri-windows-wsl-smoke.py --disposable-distros
python3 scripts/testing/tauri-windows-wsl-smoke.py --disposable-distros --full-ui
python3 scripts/testing/tauri-windows-wsl-smoke.py --disposable-distros --isolation-consumer
```

`--disposable-distros` is mandatory and scopes in-memory execution approval with
`try/finally`; imported helpers deny execution by default. Each run copies fresh
source and drivers to marked Windows TEMP. Optional `--cache-workspace` reuses
only marked Cargo target artifacts. Never execute an old TEMP driver manually.
Setup-only and standalone remote GUI entrypoints remain hard-stopped, as does
the console-spawn suite. Runtime changes and new scenarios require review and
explicit authorization; the commands above are not authorization to run them.

Evidence stays in the fresh `%TEMP%\splitty-native-windows-*` workspace:
source/payload hashes, build logs, `owned-distros.json`, `distro-cleanup.json`,
`runtime-evidence/processes.json`, checkpoints, `runtime-result.json` and
`wsl-verification.json`. Isolation and pressure checkpoints survive later test
failures. Replay diagnostics contain bounded events/errors, byte counts and
first/last 96-byte hex previews. Private fixture bytes are the only previews.

## Ownership and guest guards

`fixture-job.cs` creates a private unnamed kill-on-close Job before launching a
suspended root, assigns and verifies its exact process handle, then resumes it.
There are no breakaway flags. Assignment failure terminates only the exact new
suspended handle; cleanup targets retained Job handles. STARTUPINFOEX inherits
only the fixture's explicit stdin/stdout/stderr handles. Membership inventory
comes from the Job and same-handle checks, never parent-PID traversal.

The WSL workflow imports exactly two fresh random distributions at recorded
TEMP paths, with UID1000, private HOME, and disabled guest systemd, automount and
interop. The driver checks the marked root, pinned executable, exact registration
names/base paths and its explicit permit before Job launch. Every actual guest
command first compares `WSL_DISTRO_NAME` with the recorded expected name, including
the copied application's WSL executor and owner paths. Only newly owned matching
registrations may be terminated/unregistered in `finally`. No user distribution,
default selection, global `.wslconfig`, profile or clipboard is changed.

Earlier PID-based cleanup misclassified unrelated user processes. Those process
isolation and cleanup claims are invalid. Retained synthetic ancestry helpers
are not a destructive ownership policy: mixed-time snapshots and PID reuse can
still misclassify descendants. An earlier always-quoted WSL option also executed
`-d` in the default distro; the conditional formatter and first-action identity
gate replace that path. These incidents and invalidated evidence remain in the
verification record. Do not restore PID kills, `taskkill /T`, or check-then-kill.

## Scenario scope and limits

The default WSL scenario checks A/restart/B selection, duplicate starts, backend
identity, binary PTY/file traffic, heartbeat, owned helper/PTY death and independent
guest sentinel preservation. `--full-ui` copies real packaged production assets
and reuses shared App/CSS/font/Ghostty/settings assertions plus three bounded
popout checks. The `94f8368a656b` full-UI run passed; its final child was cleaned
at app exit, not by parent reload. Window inventory covers Tauri-managed HWNDs.
Modern and legacy clipboard operations are blocked and recorded in owned main,
setup and popout documents; clipboard feature parity is not tested.

`--isolation-consumer` implies production assets, uses two aliases for one private
SSH host in fixture A, and starts local B without the default restart sequence.
All native SSH command forms use a temporary `-F` config, private keys/known_hosts
and an explicitly owned loopback SSH listener. A dedicated foreground sshd Job
must pass a bounded native Windows SSH HOME/UID/nonce authentication transaction
before app launch. At most four readiness attempts are admitted before the
20-second between-attempt deadline, each with a five-second wait and cleanup
bound. Guest teardown owns any Linux lifetime remaining after Windows Job stop.

Its reference/storage/target checks use production roles and capsule paths.
Copied-handle denial is a capsule check; stale Setup-proof denial is a native
broker diagnostic, not a raw forged-IPC attempt. The temporary external-opener
effect recorder must stay empty; this is not a positive OS-opener test. The slow
client actually withholds receipts; the JavaScript engine is not suspended.
Healthy operations may complete after immediate slow-client closure; precise
timestamps and unchanged connection identity are retained. Global/worker quota
saturation and whole-framework/RSS bounds are not established. Local B must exit;
remote A's daemon intentionally survives app exit until exact fixture teardown.

Listener collection freezes live identities from retained handles before/after
sampling and records socket creation/state/endpoints. Missing, older or future
creation times stay UNCLASSIFIED. The raw CIM query filters only owned PIDs;
provider errors fail rather than becoming an empty result. Null, zero and unknown
TCP states also fail; known non-listening observations are retained separately.
Every listener row still fails. Sampling does not prove continuous absence. The separate native TCP-table candidate failed its
owned-listener control and remains unwired, preserved outside this commit for a
fresh hosted control. That control now passes on Windows 2025 for IPv4/IPv6
([run 41717658574](https://github.com/npgrosser/splitty/actions/runs/41717658574));
it validates the decoder there but does not resolve the local measurement or
prove application listener absence. No guessed state mapping is accepted. Disabled localhost
forwarding and packaged installer compatibility remain separate validation.

## Pure checks

```sh
python3 scripts/testing/tauri-windows-guard.test.py
node scripts/testing/tauri-windows-native-smoke/pressure.test.mjs
```

```powershell
& .\scripts\testing\tauri-windows-native-smoke\fixture-commandline.test.ps1
& .\scripts\testing\tauri-windows-native-smoke\fixture-registration.test.ps1
& .\scripts\testing\tauri-windows-native-smoke\fixture-listener.test.ps1
```

These use synthetic environments, temporary marker files, argument parsing and
in-memory metadata. They launch no GUI, query no registrations and invoke no WSL
command. The separate Job console suite remains paused.
