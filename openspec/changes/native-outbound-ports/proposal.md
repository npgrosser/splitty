# Proposal

## Why

The native workspace can connect to remote Screens but cannot use their development services through Ports, and it deliberately refuses remote loopback links. Complete the outbound forwarding workflow using the existing local service authority as the next increment of [native workspace milestone 120](../../../roadmap/README.md#als-nächstes).

## What Changes

- Retain source-qualified listening-port observations, distinguish unavailable observations from empty lists, and combine detected, active and remembered mappings without opening connections on inspection.
- Provide manual target/port entry, Forward/Open, Stop/Resume, explicit alternate local ports, Remember, Forget and label editing through the existing outbound forwarding operations.
- Keep slow forwarding API jobs independent from metadata reads, host management and terminal events, with bounded admission, identified outcomes, authoritative reconciliation and no replay of uncertain mutations.
- Route deliberate remote terminal loopback links through an exact-target forward and a revocable browser-open continuation; preserve local-link behavior and existing URL validation.
- Validate the complete workflow in an owned Linux native window with an HTTP fixture, and record the distinct native Windows/WSL endpoint and collision evidence needed to claim equivalent browser opening there, fixing or withholding a demonstrably mismatched path.

Out of scope: reverse sharing, automatic forwarding from detection or `remote.autoForwardPorts`, changes to released web/shell clients, SSH configuration or credential management, a second tunnel supervisor, new forwarding wire operations, a speculative cross-platform loopback proxy, broad release/CI work and native cutover. Enabled saved-rule restoration remains the server's existing host-readiness behavior. Native automatic forwarding remains an explicit later parity item; its setting is not exposed as working by this increment.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `remote-screens`: native outbound-port observations and explicit workflow, identified asynchronous mutation ownership, and revocable exact-source browser activation. Existing backend mapping, persistence, Stop/Forget and authorization semantics remain authoritative. The native host-management requirement is not replaced by this change.

## Impact

Extend `splitty-client-core` with a Ports feature seam and source observation state; wire it through the existing native workspace runtime and owned-service API streams. Add native Ports presentation and forward-aware terminal activation in `splitty-gpui`. Reuse the service's forwarding operations and byte-budgeted transport, extracting portable response contracts only where necessary without changing their wire shape. Update the architecture model and native-support documentation. There is no new SSH ownership, persistent format or browser-origin authority.
