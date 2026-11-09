# Native UI checkpoint — 2026-11-08

Branch: **`feat/native-ui-gpui`**. Latest functional/demo source:
**`2ab058554ddb7f7f48293d7cc36c77eb17820bcb`**. Later checkpoint commits only
record acceptance and this handoff. No additional feature work is in flight at
this cut. The native client is experimental alongside the existing web clients;
there is no completed cutover or release.

## Resume on another machine

```sh
git fetch origin
git switch feat/native-ui-gpui
git pull --ff-only
```

Read `CLAUDE.md`, this file, [architecture](architecture.md), the existing
`architecture/client-core.c4` and `architecture/gpui.c4` components, and the
selected OpenSpec change before editing. Run `openspec list --json` from this
checkout and confirm its reported root. Build prerequisites and platform setup
are in [development](development.md#running-from-source) and the
[GPUI runbook](gpui-spike.md#build-and-run).

A new checkout needs generated frontend assets even for client-core tests,
because the embedded service includes the server's asset embed. Prepare them
before Rust compilation:

```sh
pnpm install --dir frontend
pnpm --dir frontend build
pnpm install --dir architecture
export GHOSTTY_VT_DIR="$(python3 scripts/bootstrap-libghostty-vt.py)"
```

Install the platform native dependencies listed in the runbook first. The
Ghostty bootstrap verifies pinned source/toolchain archives; do not reuse an
unverified library prefix. Private caches, build outputs, agent skills and local
test screenshots are not prerequisites shipped by Git. Use the repository's
commands rather than paths from an earlier session's temporary environment.
Keep temporary fixture paths short enough for Unix sockets.

## Implemented and verified checkpoint

- Real local terminals, Screens, splits, tabs, pane movement and existing local
  floating/transfer/template tools; the native model projects the shared mux.
- Drawer navigation, wheel gestures, pin/autohide, Screen placement, transitions,
  live full titles and leading application symbols, notifications and settings.
- Exact-host Drawer navigation/Ports, explicit remote Screen creation with
  host-owned default names, bounded canonical observation and revocable follow.
  Detach preserves host work; reconnect never replays creation.
- One captured local/remote rename editor: double-click/F2/context menu;
  Enter/Blur commit, Escape cancels. Names change only after canonical readback.
  Hover actions have a fixed lane, preserving title hitboxes and a full tooltip.

Current checks: **268 client-core tests, 190 GPUI tests, 17 architecture tests**,
strict touched-crate all-target Clippy, formatting and affected strict OpenSpec
validation. The Linux platform patch additionally has eight targeted event
tests (four added) and strict platform Clippy. These are scoped checks, not the
full workspace CI gate. Full commands/evidence are in
[the GPUI acceptance record](gpui-spike.md#completed-rename-and-windows-preview-checkpoint-2026-11-08).

Actual Linux windows prove creation on B while A remains usable during delayed
confirmation, real Detach/Reopen with the same session, and B rename via
Enter/Escape/Blur without changing A's selection or any session/process/SSH
inventory. The fixture uses private `.invalid` hosts and an owned SSH-shape shim.
It is not real OpenSSH installation/update acceptance.

## Architecture boundaries and recent regression lessons

- Server mux state, session lifecycle and host launch defaults remain canonical.
  `splitty-client-core` owns UI-free source collections, transport/runtime policy
  and bounded resource admission; `splitty-gpui` owns interaction/projection.
  The web frontend still has TypeScript orchestration; no WASM client-core
  rollout or web cleanup was performed.
- Keep logical host-group interest separate from current execution authority.
  Capture window owner, source/connection and Screen identity in callbacks;
  aliases, selection and raw IDs cannot retarget them. Default remote close is
  Detach; assessed Terminate is separate. Never replay input or mutations.
- New remote commands must pass **both** outer worker source admission and
  remote dispatch. `RenameScreen` was accidentally still local-only at ingress;
  `validate_worker_command_source` now supplies that same gate to the regression
  and production worker. A driver-only test had missed it.
- Rename target validity survives a transient mutation barrier so editor input
  stays intact; start/commit use stricter admission. Active-platform-window
  state is required for a meaningful GPUI Blur test. Background readback must
  not steal editor or terminal focus.
- Linux X11 previously drained native events indefinitely and starved foreground
  updates, filling both bounded queues and rejecting input. The explicit pinned
  [platform patch](../vendor/gpui-pre-linux/SPLITTY-PATCH.md) bounds raw batches
  and schedules one woken weak continuation, preserving repeat/XIM ordering.
  Keep its version/archive/file provenance and Apache license. Do not replace
  this with larger application queues, input replay or hidden registry edits.
- The Ghostty engine boundary remains separate from GPUI rendering. Existing
  web/xterm workarounds are not automatically native feature requirements.

## Demo and reproducibility

Windows preview folder: `Splitty-Native-Preview-2026-11-08-Remote-Rename-2ab058554ddb`
in Downloads. Start `Splitty starten.cmd`; the launcher checks executable/backend
hashes and uses its own Ubuntu WSL runtime/configuration. `build.json`,
`source-sha256.txt`, `SHA256SUMS.txt`, licenses and Linux/Windows evidence are
included. The native MSVC release build passes the 18-stage local workspace
smoke on RTX 4080, template interaction, normal close and owned cleanup while an
unrelated sentinel survives. Separate Windows remote workflows remain untested.

The pre-ingress-fix `4f60b30` candidate is **not ready** and its starter is blocked.
Use `2ab0585`. Earlier previews are separate. Always create new package, source,
runtime and smoke destinations exclusively; an existing destination is an error.
Dependency build caches may be reused with proven source/output identities.
Never merge files into an older preview or place executables in a home root.
No new release tag, installer or CI rollout was created.

On another machine rebuild the GUI/backend using the Windows runbook; local
Downloads packages and cached static backends are not Git artifacts. The latest
demo's static Linux backend was reused only after verifying unchanged production
sources and all 271 reachable resolved dependency records. It bundles remote
self-install only for Linux x86_64.

## Open work and next checkpoint

- macOS runs and the chrome follows the web client's look; the gaps and
  deliberate differences are listed in
  [the GPUI runbook](gpui-spike.md#macos-preview-and-web-look-alignment-2026-11-09).
- `native-remote-screen-create`: **13/14 tasks**; only deferred full workspace
  checks remain. `native-remote-screen-rename`: **9/10 tasks**; same gate remains.
  Do not archive either as fully checked yet.
- The broader `native-remote-screens` change retains separate unfinished
  obligations. Its checklist is not superseded wholesale by these two changes.
  Inspect source and each task before accepting additional parity claims.
- Run the full pre-merge checks from `CLAUDE.md` when preparing a merge. CI was
  deliberately deferred during MVP iteration. Vendored platform tests are
  workspace-excluded and require their explicit documented regression command.
- Real SSH daemon installation/update, individual Windows remote UX, broader
  terminal/IME/image parity and remaining remote-only/local-only capabilities
  still need their own acceptance. Local profiles/templates/transfers are not
  implicitly remote features. Native host-group reorder UI and remembered
  reference persistence remain unfinished.
- Scratchpad, document/viewer and `splitty open` parity were deferred. Preserve
  the working web client and native branch while deciding subsequent scope;
  there is no approved requirement to remove web bridges or adopt full WASM.

This checkpoint finishes the work already in flight. Start the next feature
only with a fresh scope decision; avoid treating old planning ideas or every
unchecked task as an instruction to implement all of them automatically.
