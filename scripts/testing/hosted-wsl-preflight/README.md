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

Passing proves hosted WSL2 and effective NAT localhost forwarding on/off. It
**does not** prove Splitty's native transport under the false setting. The
existing full Windows application fixture is currently orchestrated from WSL
(`wslpath`, Linux `ldd`, `/mnt/c` PowerShell); its guarded PowerShell drivers and
ownership checks can be reused in a subsequent native-Windows adaptation.

Portable preparation (no WSL/configuration operations):

```sh
python3 scripts/testing/hosted-wsl-preflight/test.py
```

A future approved manual dispatch would select this branch explicitly, e.g.
`gh workflow run ci.yml --ref diagnostic/hosted-wsl-forwarding`. This is a
reviewable command only; the diagnostic preparation does not execute it.
