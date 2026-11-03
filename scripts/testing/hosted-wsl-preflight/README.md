# Disposable hosted WSL forwarding preflight

This directory belongs to a diagnostic branch, not the normal PR CI. Its
manual-only replacement of the existing `ci.yml` permits dispatching the
already-registered workflow ID with an explicit diagnostic `--ref` after review.
Nothing here runs on push or pull requests. Do not dispatch without approval.

The Windows driver **refuses local and self-hosted machines**. It requires all
three GitHub-hosted Windows environment markers, an explicit switch, fresh
RUNNER_TEMP evidence directory, and zero pre-existing WSL registrations. It
writes `.wslconfig` only on that disposable VM, saves its bytes or absence, and
restores them. No default distro is selected or changed.

The Linux job builds a minimal rootfs from the trusted Ubuntu runner's shell,
identity tools and matching shared libraries, with a static C nonce TCP tool.
The artifact records source commit, image version, rootfs and individual tool
SHA256s. The Windows job verifies the rootfs digest and source revision before
importing **one** fresh random distribution explicitly as WSL2. No arbitrary
public rootfs download or package installation is required.

Windows verifies the exact registry name/BasePath/version, guest target guard,
UID1000, private HOME, actual WSL2 kernel and fresh nonce execution. Commands use
the existing suspended-create/kernel-Job fixture with bounded waits. Server
lifetime is 180 seconds and 32 accepts; client I/O is bounded. The script has a
15-minute work deadline, an 18-minute cleanup ceiling and the Windows job a
20-minute cap.

The networking controls run in this order:

1. Independent real Windows loopback connect/accept positive control.
2. Explicit NAT + `localhostForwarding=true`, complete WSL shutdown/relaunch.
   Guest loopback and Windows localhost must return the same fresh nonce.
3. Explicit NAT + `localhostForwarding=false`, full shutdown/relaunch and a
   new nonce. Guest loopback succeeds before and after the Windows probe;
   **any established Windows TCP connection fails**, regardless of its payload.

Only the known new registry entry may be terminated/unregistered; any identity
change refuses control. Exact Windows Jobs/handles are cleaned and configuration
bytes/absence restored. Evidence includes command logs, registrations, config
phases, runner/WSL/kernel/source identity, positive/negative nonce controls, and
cleanup results. A virtualization refusal, import failure, missing positive
control, timeout or cleanup error fails: none is a skipped proof. Cleanup on
abrupt hosted-job cancellation is ultimately the disposable VM teardown; this
script does not claim Windows Job membership owns Linux guest processes.

Without `-RunNativeApplication`, passing proves only hosted WSL2 and effective
NAT localhost forwarding on/off. It does not prove the application transport.

## Optional actual application proof (prepared, not yet executed)

The diagnostic workflow now incorporates the committed product snapshot
`6464f3b9a0e35347d6f02309c523b148dd5aaee4` and builds its production frontend and Linux backend from the same
workflow source revision. The additional application manifest pins Cargo.lock,
raw/stripped backend, existing full fixture rootfs, and production asset archive.
Windows verifies every prepared digest before extracting assets into its fresh
hosted checkout. No Linux tools execute on Windows.

The existing `tauri-windows-wsl-smoke.py --disposable-distros --full-ui` remains
the only app fixture: same Rust instrumentation, TypeScript assertions,
PowerShell runtime driver, kernel Jobs and exact registry/guest guards. Its host
path/PowerShell utilities and Vite invocation now support native Windows. Two
optional entrypoints reuse that flow: `--build-only` warms MSVC dependencies and
cleans its temporary distributions without starting GUI; prepared artifact input
replaces only Linux `strip`/`ldd` preparation. The subsequent run copies fresh
source and reuses only the marked build target cache.

The full-UI run occurs after the true positive and false negative TCP controls,
while the false config bytes remain unchanged. Fresh false controls run again
after its two distro contexts finish. Actual success additionally requires the
production UI report, binary PTY bytes, 1,048,833-byte file transfer, app restart,
owned backend/session cleanup and no guest TCP listener. Source/runtime hashes,
probe bundle, app reports and private distro cleanup are retained separately.
`NativeApplicationPassed` stays false until both app and post-run controls pass.

The Windows job reserves up to 25 minutes for a cold MSVC prebuild. The actual
controlled-runtime step still has a 20-minute cap (15-minute work deadline plus
cleanup reserve). On timeout the exact parent Job is stopped, and the existing
fixture's recorded-only cleanup entrypoint handles any remaining private distro
registrations. A bad/missing ownership record fails closed; abrupt VM teardown
remains the final boundary on hosted cancellation. No installer, remote SSH
isolation, or hard OS-failure shutdown guarantee is added by this fixture.

Portable preparation (no WSL/configuration operations):

```sh
python3 scripts/testing/hosted-wsl-preflight/test.py
```

A future approved manual dispatch would select this branch explicitly, e.g.
`gh workflow run ci.yml --ref diagnostic/hosted-wsl-forwarding`. This is a
reviewable command only; the diagnostic preparation does not execute it.
