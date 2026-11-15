# Test notes

What two of the smoke suites need and prove beyond what `cargo test` prints.
The scripts in this directory drive the desktop app's real window; they are
described in [`docs/gpui-spike.md`](../../docs/gpui-spike.md).

## Native endpoint identity smoke

`cargo test -p splitty-server --test native_endpoint -- --nocapture` connects as
an actual different OS user (`nobody`), using WSL interop or noninteractive sudo.
It first verifies private directory/socket permissions, then loosens only the
temporary fixture's modes and verifies that the server rejects peer credentials
before any application bytes. Without a usable alternate-user launcher it prints
an unavailable diagnostic; such a run does not establish OS enforcement.

The endpoint unit tests separately cover both connection directions, unavailable
credentials, unsafe paths and mounts, stale sockets, descriptor adoption and the
instance lock. Linux runtime success does not substitute for a macOS run.

## Listener inventory

The daemon and remote-screen process smokes (`tests/daemon_lifecycle.rs`,
`tests/remote_connect.rs`) share `crates/splitty-server/tests/support/listeners.rs`.
On Linux it joins each process's socket descriptors to both TCP listener tables;
on macOS it uses `lsof` and requires a live process and a valid inspection
result. An unavailable inspection is a failure, not proof of zero listeners. The
detector has a real IPv4/IPv6 listener check. Daemons, helpers and SSH carriers
must own no TCP listener; a port forward's `ssh` owns exactly its loopback
listener. These checks are verified on Linux; macOS runtime execution remains
outstanding.

`tests/support/processes.rs` inspects every live member of an isolated command
process group. Group membership survives reparenting; exited zombies are not
live workers. Failure fixtures write diagnostics to regular files and bound
leader exit separately, so an orphan holding inherited descriptors cannot hide
from the subsequent group check. Independent daemon survival requires a
non-zombie state and the same process birth identity. The fixtures use known SSH
and proxy commands that remain in that group; they do not prove cleanup of
arbitrary user commands that deliberately detach into another session.
