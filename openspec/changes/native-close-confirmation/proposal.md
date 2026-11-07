## Why

Native Screen close already asks before ending busy sessions, but pane close and normal window close currently bypass that protection. One close flow should apply the same local terminal policy and preserve the owned service when a prompt is cancelled or stale. This is a bounded follow-up to [native UI milestone 120](../../../roadmap/README.md) and the [native evaluation](../native-ui-gpui/proposal.md).

## What Changes

- Assess the terminal sessions affected by closing a pane, Screen or the owned local window. Include hidden Screens and unresolved local creations for window close.
- Reuse that assessment for pane buttons, the pane-close shortcut, Screen close and normal window-close requests. Known idle targets close without a prompt; busy or unresolved impact requires confirmation.
- Capture connection and target/session identities, recheck the current known impact in the worker, and reject stale or enlarged impact without ending other work.
- Keep the window alive until a matching worker approval. On approval, stop accepting further user mutations and finish the existing owned-service cleanup and join. Treat an in-flight transport delivery revoked by that shutdown as cancellation, while preserving malformed-frame, foreign-budget and queue-exhaustion errors.

Out of scope: remote or daemon-detachment policy, scratchpad/viewer loss assessment, general quit handling for other apps/probes, forced process termination or non-cancellable platform quit callbacks, persistence, new backend/wire policy, WASM and a browser refactor. Busy metadata is an observation, not a lock on external processes or concurrent service clients.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `client-connections`: consistent local terminal close confirmation, stale-consent rejection and worker-approved owned-window shutdown.

## Impact

Touches the existing UI-free workspace model, native workspace worker and GPUI workspace/window adapter. Existing close operations and service lifetime remain authoritative. No new dependency or public wire message is required. Extend focused model/runtime tests and native interaction evidence; broad CI and release-parity checks remain outside this increment.

## Architecture impact

Keep close assessment under `splitty.client_core.client_workspace` in `architecture/client-core.c4`, orchestration under `native_workspace`, and prompts/window interception under `splitty.gpui.gpui_workspace`/`gpui_main` in `architecture/gpui.c4`; update their descriptions/actual relationships and the Client connections/owned lifetime concept in `docs/architecture.md`, without introducing a new top-level component.

## Decisions

- **NCC-1 — bounded close assessment (2026-11-07, provisional, tbd):** a UI-free `ClosePlan` captures the connection generation, target (pane with Screen, Screen, or owned window), affected pane/session identities and busy/unresolved impact. Assessment uses the canonical snapshots and pending reservations. A single bounded close request is active per window; duplicate gestures reuse or leave it pending. Prompt layout, keyboard handling and focus restoration stay in GPUI. This is a reusable decision boundary, not a claim that the native runtime is browser-portable.
- **NCC-2 — consent revalidation (2026-11-07, provisional, tbd):** submit the captured plan and its request identity through the existing bounded worker queue. Before an operation, compare against the latest worker-known connection, target and affected identities; require renewed review if membership or session identity changed, or if previously unconfirmed impact became busy/unresolved. Title, layout-only and unrelated metadata updates do not invalidate consent. A rejection ends that pending request with a visible reason and leaves the target alive; the next close gesture assesses it again. A failed queue admission also preserves the target and never retries a destructive operation automatically.
- **NCC-3 — explicit window approval (2026-11-07, provisional, tbd):** `on_window_should_close` vetoes the initial close while assessment/confirmation and worker validation run. The UI must not call `stop`, close its update receiver, or quit at confirmation time. Only a matching `WindowCloseApproved` worker update permits quitting. The worker enters a closing state before publishing approval, accepts no further user mutation/input commands, and proceeds directly to its existing client/service shutdown; `main` joins it after the event loop. A rejected or stale request publishes rejection instead and keeps servicing the workspace. Closed channels, a failed approval publication and app teardown retain owned cleanup; they never authorize a different request or generation. The non-cancellable `on_app_quit` hook is for cleanup, not prompting.

Pane and Screen requests use the same validation path and existing service close operations; their views disappear only through authoritative reconciliation. Submission is not settlement: keep one worker-owned request until an ordered catalog response whose mutation watermark covers its operation. Record matching operation failures against that retained request, including failures without a pane ID, and do not admit another close before that barrier. Screen removal is then confirmed by the catalog; pane removal also requires a captured-Screen snapshot following the barrier, unless the Screen itself is absent. Only worker settlement or rejection releases the UI's pending request. A failed admission before any operation was sent can reject immediately. This reuses the existing serial reads and mutation watermarks rather than adding a wire correlation protocol. Window confirmation describes ending the entire owned local workspace, including hidden Screens. The worker's recheck covers observed state and already processed local intents; no new server-side compare-and-swap or guarantee about unobserved external races is introduced.

Startup and failure paths must not trap the window: before discovery completes, assess impact as unresolved and obtain a decision from the same worker lifetime. If that worker has already completed owned cleanup, an explicit completion state permits exit without a live-service approval. A closed update channel or failed command send alone is not proof of completed cleanup. Cover both cases with focused lifetime tests.
