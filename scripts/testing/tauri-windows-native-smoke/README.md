# Native Windows and isolated WSL smoke

## Windows GUI validation is blocked

Windows GUI runs are disabled in the shared Python runtime, the WSL GUI entry
point, the direct PowerShell WSL driver and the importable sentinel cleanup
helper. Do not remove these guards or invoke
the retained legacy cleanup helpers. Previously copied TEMP drivers are not
retroactively guarded and must not be executed or reused. Previous process-inventory/cleanup results
do not establish isolation from unrelated applications. Compile and pure unit
results remain separate evidence; GUI verification is pending independent review
of a replacement ownership and cleanup mechanism.

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

No destructive candidate helper is provided. A future implementation must use
OS job membership or equivalent proven process lifetimes, fail closed on unknown
ancestry, and bind final identity verification and termination to the same OS
handle. Previously observed process IDs and executable names alone are not
cleanup authority. Legacy cleanup code remains unreachable through the guarded
GUI entry points and must not be called directly. No further GUI run is approved.

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

## Disabled runtime scenarios

The default `tauri-windows-native-smoke.py` GUI probe,
`tauri-windows-wsl-smoke.py --disposable-distros`, the remote script's `--gui`
path, and direct `wsl-runtime.ps1` execution are disabled. These are retained
scenario definitions, not runnable validation instructions. Do not reuse old
TEMP copies or cached runtime drivers to bypass the repository guards.

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
Its process attribution and cleanup are blocked. Assertions that independent
fixture workers survive while owned helpers exit must be revalidated after a
safe ownership mechanism exists.

## Disposable WSL fixture boundaries (design only)

The retained setup code builds a minimal root filesystem from local shell,
coreutils and shared libraries. It imports newly generated exact distribution
names into recorded TEMP folders, uses a non-root user with private HOME, and
disables systemd, Windows drive automounts and interop inside those fixtures.
The copied Tauri configuration points at TEMP; a guard rejects `wsl.exe -d`
targets outside the recorded fixture names. These boundaries do not repair the
Windows process classifier or authorize runtime execution.

Distribution cleanup checks each newly generated exact name against the
original registration snapshot and its recorded TEMP base path before
unregistering that distribution. This narrow distribution-name policy is
separate from the unsafe Windows process cleanup, which remains disabled.
Recorded creation timestamps alone do not authorize terminating a process.
Evidence files include `owned-distros.json`, `distro-cleanup.json`, source hashes
and the prior runtime reports; the latter retain the limitations above.

The source's read-only networking-configuration snapshot and a run with mirrored
networking or default localhost forwarding do not establish the separate
localhost-forwarding-disabled check. No claim is made here for macOS runtime,
Windows remote-host isolation, exhaustive frozen-client admission limits or
packaged installer behavior.
