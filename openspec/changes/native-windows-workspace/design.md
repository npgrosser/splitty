# Design

## Context

See proposal.md. The native session client already accepts an owned-helper connection, and the released Windows launcher already starts a native-v1 embedded owner through WSL pipes. GPUI's workspace worker, auxiliary API jobs and API catalog imports are currently Unix-only. The existing owner supervisor observes helper exit but discards cleanup errors, which is insufficient for the native window's cleanup-complete guard.

## Goals / Non-Goals

Use one workspace/appearance/profile/template worker on both platforms. Keep platform bootstrap and process ownership outside GPUI. Preserve typed request validation, bounded replies, cancellation, identity and mutation uncertainty. Deliver an optimized development executable with a launcher that names an existing distribution, matching backend and isolated preview configuration. Provisioning, distribution discovery/switching and a production installer remain separate.

## Decisions

- Move the existing pure service catalog into `splitty-core::native_api`; retain the server's catalog path through reexports. Its operation names, roles, argument validation, limits and deadlines remain the same. This avoids parallel client/server catalogs and does not move effect execution or authority to the client.
- Extract the existing API-only request transaction into `splitty-transport::api_request`, operating directly on `Peer`. Server native clients and owned connections delegate to it. It retains increasing request IDs, cancellation-on-drop, deadline handling, charged matching replies and uncertain lost-mutation errors. The transport receives wire names/deadlines and has no dependency on the domain catalog. A second request implementation and a generic transport framework are unnecessary.
- Add a concrete client-core API stream wrapper and an owned-service wrapper. The API wrapper validates the portable catalog and wraps either embedded or owned connections. The service wrapper opens independent session/API streams and joins ownership cleanup. Existing auxiliary scheduling and workspace orchestration compile on both platforms.
- Windows startup explicitly names `--wsl-distro`, `--wsl-backend` and `--wsl-config-dir`; the latter two are absolute paths in the selected distribution. Validate complete, bounded configuration before spawning. Pass arguments directly to `wsl.exe`, using `env` to set only the child's configuration directory, followed by `desktop-backend --transport native-v1 --backend embedded --owner`. There is no shell interpolation, implicit backend substitution, install, daemon discovery or TCP fallback. Unix's existing default embedded startup remains unchanged.
- Extend owned-helper cleanup with an additive result API. Keep existing consumers source-compatible; native cleanup-complete requires successful observed helper cleanup. Nonzero exit, transport cleanup failure or forced bridge termination is reported, not equated with graceful Linux session cleanup. Cancellation retains ownership until the existing supervisor observes exit; never terminate a distribution.
- Build Windows dependencies and output in an isolated Windows directory/cache. The development package contains the optimized executable and explicit launcher; its matching Linux backend stays at the configured WSL path. Record source/binary hashes and graphics/startup/shutdown evidence. This is a local evaluation package, not a self-contained installer.

## Architecture impact

Add `core.native_api` and redirect the server service catalog relationship to it in architecture/core.c4 and server.c4. Add `transport.api_request` with direct peer/wire/budget relationships; server native and remote owner delegate to it in transport.c4, server.c4 and remote.c4. Add client-core API-stream and owned-service components and update the connections of local_service, appearance, shell_profiles, templates, embedded and native_workspace in client-core.c4. Update GPUI composition/startup descriptions in gpui.c4. Document Client connections, Windows ownership and native development launch commands in docs/architecture.md, docs/development.md and docs/gpui-spike.md. Existing docs/protocol.ts and generated native-operation descriptions retain their shapes and are checked for equivalence.

## Risks / Trade-offs

- Sharing request code affects existing clients → preserve server reexports and validate request matching, cancellation, charged replies and the operation inventory before native integration.
- Windows shutdown can observe a reaped bridge without proving Linux graceful cleanup → expose the actual outcome and only claim normal cleanup for successful termination.
- Windows paths, executable lookup and SDK dependencies vary → use an isolated native MSVC build, explicit tested tool paths and arguments, and the pinned Ghostty bootstrap; do not overwrite an existing checkout or install toolchains globally.
- A preview launcher references its WSL backend/configuration → report missing paths clearly and ship the exact launcher with the executable. Self-contained packaging is deferred.
- Rendering/input may still differ by platform → verify an actual Windows window and record GPU identification; do not infer smoothness or all-feature parity from a successful cross-compile.

## Provisional decisions

- **NWW-1** (2026-11-07, provisional, tbd): explicit existing-distribution/backend/configuration arguments for the development executable; no automatic provisioning or distribution selector.
- **NWW-2** (2026-11-07, provisional, tbd): move the pure catalog and existing request transaction to shared lower layers, using concrete stream/service wrappers and one worker rather than platform-specific orchestration copies.
- **NWW-3** (2026-11-07, provisional, tbd): deliver a local optimized Windows preview plus launcher and matching WSL backend; releases, installers and full merge CI remain separate.

## Migration Plan

Keep the accepted Linux preview and released desktop clients available. Build in a separate worktree while the template window acceptance runs. Merge the Windows increment back into the evaluation branch after relevant Linux/Windows checks. Reverting the Windows launcher/shared adapter changes requires no data migration; existing protocol operation names and template files remain compatible.
