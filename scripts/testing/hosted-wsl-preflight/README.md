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

## Application scenarios

The existing `-RunNativeApplication` mode retains its actual full production UI
run under `localhostForwarding=false`, with positive/negative controls before
and a fresh negative control afterward. Its previously recorded result remains
separate from the new scenario below.

The diagnostic workflow now selects `-RunIsolationConsumer`, a mutually exclusive
scenario under explicit NAT `localhostForwarding=true`. After its positive nonce
control it runs the existing guarded `--disposable-distros --isolation-consumer`
fixture once, verifies its app/cleanup evidence and unchanged true config bytes,
then finishes and restores the original configuration. It does not run false
controls or a second GUI in this scenario. The fixture's private guest-loopback
SSH host requires this forwarding-enabled setting. No false-mode claim is made.

Current fixture source includes the original post-close write → open replay →
attach ordering. Diagnostic catalog/checkpoint awaits follow attach. Success
requires persisted write/open/attach timestamps in order and the actual replay
marker; a portable source check rejects extra pre-attach awaits. Earlier local
missing-marker evidence remains unexplained; a later pass is not its diagnosis.
The strict raw-CIM listener/unknown/error gate remains enabled.

Both modes reuse the same suspended-create kernel Jobs, exact registration and
first-action guest guards, fresh marked source/driver copies and recorded-only
cleanup fallback. They do not change existing distributions, user SSH files or
clipboard. Windows paths, Vite entrypoints and prepared inputs work without Linux
commands on Windows. A separate prebuild warms only Cargo artifacts and removes
its two private distributions before the one runtime begins.

The Linux job builds production assets and backend at the workflow revision. It
stages `openssh-server` and `libwrap0` with `apt-get download`/`dpkg-deb` in
RUNNER_TEMP, without installing packages or running service/key-generation hooks.
The existing rootfs tool/dependency catalog packages these explicit files and
fails on missing libraries. Package/tool, Cargo.lock, payload, asset and rootfs
hashes accompany the same-source manifest. Windows verifies all digests before
using the prepared inputs.

The prepared SSH rootfs contains no per-run host or client keys. Windows generates
fresh fixture keys and a private loopback SSH config, then overlays only its host
key (0600), public authorized key (0644) and sshd config (0600). Base/final rootfs
and public-key hashes are retained. Evidence upload enumerates reports and hashes;
it does not upload private keys, key directories, generated rootfs or WebView data.
The Windows app/SSH readiness and authentication remain actual runtime checks,
not claims established by portable archive tests.

The cold MSVC prebuild has a 25-minute step cap; controlled runtime has a
20-minute cap with a 15-minute work deadline and cleanup reserve. The outer Job
is stopped on timeout, and recorded exact-target cleanup handles remaining app
fixtures before the preflight distro/config finally cleanup. Abrupt hosted-job
cancellation ultimately relies on disposable VM teardown. Neither mode is an
installer or whole-framework/RSS bound test. Source preparation does not dispatch
this manual workflow.

Portable preparation (no WSL/configuration operations):

```sh
python3 scripts/testing/hosted-wsl-preflight/test.py
```

A future approved manual dispatch would select this branch explicitly, e.g.
`gh workflow run ci.yml --ref diagnostic/hosted-wsl-forwarding`. This is a
reviewable command only; the diagnostic preparation does not execute it.
