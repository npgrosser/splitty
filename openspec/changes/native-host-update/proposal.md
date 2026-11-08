# Proposal

## Why

The experimental native workspace can connect and retry hosts but drops the hub's structured update offers and always opens hosts without update intent. Complete the explicit update workflow using the shared backend's existing keeper-preserving replacement checks and the UI-free native client's source ownership.

This is a bounded follow-up to roadmap [120, Native UI with GPUI](../native-ui-gpui/proposal.md), listed in [the roadmap](../../../roadmap/README.md#als-nächstes). It builds on the implemented portions of `native-remote-screens` without completing or replacing that change's outstanding runtime acceptance.

## What Changes

- Retain bounded, typed ordinary update and legacy migration offers from the assigned host's readiness or preparation failure.
- Offer explicit native Update after confirming the exact host, observed and replacement versions, keeper preservation and temporary connection interruption.
- Bind the client confirmation to its local owner, host source, connection attempt and accepted offer; reject obsolete or duplicate confirmation before effects.
- Execute one confirmed update through the existing host adapter, retaining cleanup and open remote references. Connect, Retry and automatic recovery never inherit update intent.
- Distinguish cancellation and uncertain completion from confirmed success; reconcile through an ordinary connection without replaying maintenance.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `remote-screens`: extend Host Update with the experimental native offer, captured confirmation, retained connection replacement and uncertain-outcome contract.

## Impact

The main changes are in `splitty-client-core` host presentation, diagnostic decoding, native workspace orchestration and concrete owned/embedded service adapters, with thin GPUI controls. The server hub, remote installer, version policy, conditional stop/migration commands and existing native update wire flag are reused. The existing local `build.get` operation supplies the assigned backend version where an ordinary offer needs it; no new maintenance authority or receipt is introduced.

Architecture documentation and component descriptions will describe these owners. Tests cover typed metadata, stale consent, cleanup, one-use update intent, backend rechecks and actual native confirmation interaction.

## Out of Scope

Exact-observed-instance consent receipts, a new installer or version policy, protocol redesign, destructive updates, shared WASM/web orchestration, changing the web client's warning copy, broad remote parity, remembered-screen restoration, reverse forwarding, payload packaging, releases and cutover are out of scope. Existing `native-remote-screens` acceptance and platform obligations remain open. Full merge CI is deferred to the merge checkpoint rather than run during this slice.
