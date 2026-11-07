# Proposal

## Why

The native workspace currently has one local source, so several retained view keys and queued intents identify objects only by Screen, pane, session or node ID. Make source identity and captured execution authority explicit before adding remote Screens, while preserving the existing local workflow and its canonical mux projection.

This is a preparatory part of [roadmap milestone 120](../../../roadmap/README.md) and the [native UI direction](../native-ui-gpui/proposal.md), consistent with the shared-client work in [frontend-client-core](../frontend-client-core/proposal.md). The roadmap describes milestone 120 in its numbered overview rather than a separate milestone file.

## What Changes

- Qualify retained Screen, pane and layout identities by their assigned workspace source.
- Capture immutable source/connection authority on commands, deliveries and deferred callbacks; reject stale or foreign authority before routing effects or consuming content.
- Keep terminal routing independent of Screen position so surviving sessions retain their terminal views during same-source Screen transfers.
- Bind geometry, visibility, resize retries, document results and confirmation outcomes to their original owner and relevant existing freshness checks.
- Keep one production local source, one existing workspace worker and the current independent API streams. No new connection or remote feature is activated.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None. `client-connections` already specifies fixed backend/generation identity, ordered delivery, stale callback isolation, local session retention and close-consent scope. This refactor implements those contracts more explicitly without changing their semantics. `skip_specs: true` avoids inventing an architecture-only specification.

The schema remains **spec-driven** because the refactor crosses the shared client runtime, workspace projection and GPUI retained state and needs an implementation design. Skipping behavioral deltas does not remove the design or its regression tests.

## Impact

Primary implementation areas are `crates/splitty-client-core/src/{workspace,native_workspace,connection,document}.rs` and their existing submodules, plus `crates/splitty-gpui/src/workspace.rs`, its feature helpers and the terminal callback seam. Update `architecture/client-core.c4`, `architecture/gpui.c4` and the native workspace explanation in `docs/architecture.md` with the implementation.

There is no wire, persistence, server mux, SSH, web-client or transport-protocol change. The current embedded Unix and owned Windows adapters retain their behavior.

Out of scope: remote connect/browse/open/detach/terminate, reconnect supervision, remembered references, forwarding, settings features, a source registry with remote models, a second layout tree, generalized service abstractions, CI/release work and Windows builds. Full workspace merge checks remain deferred until main integration; targeted tests and architecture/spec validation are required for this branch increment.
