# Native Windows and isolated WSL smoke

## Runtime entrypoint status

The full WSL A/restart/B workflow requires explicit CLI opt-in:

```sh
python3 scripts/testing/tauri-windows-wsl-smoke.py --disposable-distros
```

That required flag enables fixture execution only inside a `try/finally` scope;
imported helpers remain denied by default and approval resets after success or
failure. The workflow always copies fresh current source and drivers. Optional
`--cache-workspace` reuses only a marked TEMP Cargo target, never old source or
runtime drivers. It imports exactly two new random distributions into recorded
TEMP paths and removes only those matching names/paths in `finally`.

The copied PowerShell driver requires its internal `-PermitIsolatedFullRun`
switch and revalidates the marked root, exactly two new A/B registrations and
their recorded paths under the expected TEMP directory before creating Jobs.
Its guest identity gates, Job ownership, stdio allowlist and timeouts remain
mandatory. Do not invoke copied drivers manually or reuse old TEMP drivers.

Setup-only and remote GUI entrypoints remain hard-stopped. Console-spawn tests
remain paused; pure formatter, gate and registration tests may run. Historical
PID-based cleanup reports do not establish isolation from unrelated applications;
the later Job-owned results remain separate, source-versioned observations.

The old inventory seeded numerical root PIDs and followed `ParentProcessId`
without checking the parent's process generation. An older unrelated child can
retain a parent PID after that number is reused by a fixture. Checking the
child's creation time immediately before killing it only confirms the wrongly
classified child; it does not establish fixture ownership. `taskkill /T` repeats
an unverified descendant traversal, while a separate CIM check followed by
`Stop-Process -Id` also has a PID-reuse race.

The retained `owned-processes.ps1` functions and seven pure snapshot checks reject
the original older-child case and basic identity/ancestry mismatches. They are
not an approved destructive cleanup policy. A mixed-time enumeration can still
contain a fixture root born at T20, after which it exits at T30 and its PID is
reused at T50, together with an unrelated child born at T51. Comparing only
`child.creation >= parent.creation` accepts that child against the obsolete root
row. An enumeration is not assumed to be an atomic process-lifetime snapshot.

The replacement `fixture-job.cs` uses a private, unnamed Windows Job with
kill-on-close and no breakaway flags. It creates each root suspended, assigns
its exact process handle to that Job, verifies membership, then resumes it.
Failure before assignment terminates only the exact newly created suspended
handle. If Job setup fails, no root is launched. Windows application descendants
remain in the Job, and separate fixture sleep workers receive separate Jobs.
Cleanup targets only retained Job handles. There is no PID-based termination or
parent-PID traversal in the drivers.

Membership snapshots come from `QueryInformationJobObject`; each opened member
handle is checked again using `IsProcessInJob` before its creation time and image
are recorded. These snapshots are inventory, never cleanup authority. The
fixture retains opened process handles to record their eventual exit status.
Windows listener inventory samples `Get-NetTCPConnection` and joins sampled Job
member IDs later. A PID reused between samples can falsely attribute a listener,
and polling cannot prove absence of transient listeners. Reports must retain
that sampled scope.

Fresh console-only checks use `fixture-job.test.ps1` and `fixture-console.cs`.
They cover owned child/grandchild cleanup, preservation of a separate fixture,
breakaway refusal, real assignment failure before resume, and zero remaining
tracked owned handles. They launch no GUI and inspect no existing application.
The Job helper and driver integration received independent source review.
Earlier runtime evidence does not establish the new mechanism. Only explicitly
authorized fresh hidden scenarios may run; there are no popout or reload loops.

## WSL target parsing and first-action guard

An isolated sentinel attempt exited 127 with `zsh:1: command not found: -d`:
the always-quoted `"-d"` token was interpreted as a command in the default Ubuntu
distribution. Its exact input arguments, reconstructed generated command line,
stdio, exit result and source hashes were preserved. No termination targeted
that distribution. The earlier missing-stdio explanation was disproved by the
captured stderr; explicit stdio isolation remains useful independently.

The formatter now leaves plain option/name tokens unquoted and quotes only
arguments that require Windows escaping. Pure Windows argument-parser tests
cover whitespace, quotes, trailing backslashes, empty values and plain WSL
options without launching a process. This does not itself establish a new WSL
runtime result.

Every actual guest command is wrapped by `fixture-guest-guard.sh`. Its first
shell action compares `WSL_DISTRO_NAME` with the exact pre-recorded expected
fixture name before executing a probe, script or mutation. Missing or different
identity refuses execution. The same guard covers Python helpers, PowerShell
sentinel/probe/transaction paths, and both the copied application's executor and
native backend owner path. Fixture names and recorded registration base paths
remain independently checked. WSL import/terminate/unregister operate only on
new exact names with matching recorded TEMP paths. No existing distro or user
profile may be queried to test this fix.

Pure synthetic-environment tests verify that wrong or absent identity performs
zero child writes. The full WSL CLI opt-in described above is the enabled workflow. Setup-only
and remote GUI entrypoints remain paused; enabling them is separate work.

## Available compile-only check

From WSL, with the installed Windows MSVC Rust toolchain, built
`frontend/dist` and Linux debug `splitty` executable:

```sh
python3 scripts/testing/tauri-windows-native-smoke.py --build-only
```

This creates a fresh marked Windows TEMP source copy and runs native GUI crate
unit tests without starting a GUI probe. It installs no application, toolchain,
Node, pnpm or system package. Source SHA-256 manifests and build logs remain
under that temporary workspace. The bundled Linux executable is a test payload,
not a release package or a static compatibility claim.

## Runtime scenario status

The full WSL implementation uses explicit opt-in and a fresh Job driver.
Setup and remote implementations retain hard stops. Do not invoke PowerShell
drivers outside their fresh authorized fixture.
Do not reuse old TEMP copies or cached runtime drivers. A prior Cargo target
cache may supply build artifacts to a fresh source and driver snapshot.

The setup scenario targets production Windows Tauri startup, capabilities,
private bootstrap and broker with a small packaged setup document. Its intended
assertions include setup-role stream/editor denial, invalid-target refusal and
no WSL owner startup. Earlier reports' process-tree, TCP-listener and surviving
child results used the invalid ownership classifier; they are not accepted
runtime-isolation or process-cleanup evidence. Native IPC observations and
compile/unit results must be evaluated separately against their recorded source
versions. Full Svelte UI and successful remote SSH coverage remain separate.

The extended WSL scenario targets duplicate starts, distribution switching and
app restart, backend identity, native API calls, binary PTY output, a
1,048,833-byte upload with an independent SHA-256 check and a heartbeat interval.
Its process attribution and cleanup use the reviewed Job mechanism. Independent
fixture workers must retain both their guest PID and `/proc` start tick and
answer a fresh nonce transaction after app exit; a live Windows WSL launcher
alone is insufficient. The app-owned helper and PTY must exit.

## Disposable WSL fixture boundaries

The retained setup code builds a minimal root filesystem from local shell,
coreutils and shared libraries. It imports newly generated exact distribution
names into recorded TEMP folders, uses a non-root user with private HOME, and
disables systemd, Windows drive automounts and interop inside those fixtures.
The copied Tauri configuration points at TEMP; a guard rejects `wsl.exe -d`
targets outside the recorded fixture names. These boundaries complement kernel Job ownership and do not authorize accessing
an existing distribution.

Distribution cleanup checks each newly generated exact name against the
original registration snapshot and its recorded TEMP base path before
unregistering that distribution. This narrow distribution-name policy is separate from kernel Job process
ownership and from the guest first-action identity guard.
Recorded creation timestamps alone do not authorize terminating a process.
Evidence files include `owned-distros.json`, `distro-cleanup.json`, source hashes
and the prior runtime reports; the latter retain the limitations above.

The source's read-only networking-configuration snapshot and a run with mirrored
networking or default localhost forwarding do not establish the separate
localhost-forwarding-disabled check. No claim is made here for macOS runtime,
Windows remote-host isolation, exhaustive frozen-client admission limits or
packaged installer behavior.

## Paused isolated Job console check

The repository console-spawn test remains guarded; do not run it until the
current review is complete. Its intended invocation after authorization is:

```powershell
& .\scripts\testing\tauri-windows-native-smoke\fixture-job.test.ps1
```

It creates a fresh marked Windows TEMP directory and retains its exact helper
sources, hashes, member snapshots and result JSON. Every launched console root
is assigned to its private Job before it executes. All test Jobs are stopped
and disposed in `finally`; the unrelated process assertion refers to a second
explicitly created console fixture, never a user application.

## Currently allowed pure checks

```sh
python3 scripts/testing/tauri-windows-guard.test.py
```

```powershell
& .\scripts\testing\tauri-windows-native-smoke\fixture-commandline.test.ps1
& .\scripts\testing\tauri-windows-native-smoke\fixture-registration.test.ps1
```

The first uses only synthetic environment values and local temporary marker
files. The PowerShell checks compile the formatter, parse strings and validate
synthetic registration metadata in memory. They launch no fixture process,
query no registry and call no WSL executable.

## Opt-in production UI mode

Add `--full-ui` to the guarded `--disposable-distros` workflow to copy the real
built `frontend/dist` into the packaged fixture. The production index, CSS,
fonts, Ghostty WASM and popout document remain real; a separate probe module is
appended to the index. The existing setup document still controls the isolated
A/restart/B backend selection. Default fixture behavior is unchanged.

The mode reuses the shared production DOM/settings/Ghostty assertions and three
sequential, bounded popout lifecycle checks. Fixture windows stay hidden. Its
window inventory counts Tauri-managed HWND entries, not all OS windows. The
probe uses a separate native-open request namespace from the production App.
Clipboard methods are blocked and attempts recorded; no global clipboard read
or write is part of this test. SSH host-list reads use an empty fixture folder.
Production asset hashes and UI observations are retained with the evidence.

The reviewed mode passed an actual bounded Windows MSVC/WebView2 run in
`%TEMP%\splitty-native-windows-94f8368a656b`. The production App mounted with
CSS and SplittyMono loaded; Ghostty WASM came from the packaged origin with
`application/wasm`, rendered glyphs into a 900×540 canvas, and the renderer
setting persisted through the native API. Observed UI errors, network fallback
attempts and modern/legacy clipboard attempts were empty.

The three sequential popout checks verified direct command denial, allowed
parent-domain calls/shared opener, ordinary close, parent-mediated close and
foreign-label refusal. The final child survived until app exit; this is app-exit
cleanup evidence, not a Windows parent-reload result. Window inventory covers
Tauri-managed HWNDs only. Both app instances exited naturally with no remaining
Job members before cleanup; owned handle count reached zero. Independent guest
sentinels retained their exact start identities and answered fresh nonces.
Owned helper/PTY exit, binary PTY traffic and the 1,048,833-byte upload hash also
passed. Both newly registered distributions were removed.

Evidence includes `production-ui-assets.json`,
`runtime-evidence/runtime-result.json`, `runtime-evidence/processes.json`, the
sentinel before/after records, `wsl-verification.json` and `distro-cleanup.json`.
The earlier `5990d77041bd` attempt stopped before any GUI because the pinned
executable guard correctly refused a junction-resolved cache path. Launch path
construction now preserves the fresh root's lexical target path; the exact
guard was not relaxed.

This result does not establish sibling remote/setup reference isolation, frozen
client or admission-flood behavior, a noncooperative bridge shutdown, disabled
localhost forwarding, global OS-window enumeration, clipboard feature parity,
or whole-framework/RSS memory bounds. TCP inventories remain sampled. No global
clipboard operation or WSL configuration change was part of this run.
