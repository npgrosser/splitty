# Proposal

## Why

The native Drawer identifies remote host groups but offers no direct route to their host details or development ports. The existing Hosts/Ports workflow already owns the operations and verification; the Drawer needs convenient actions over that same state and carefully scoped browser intent.

## What Changes

- Make a host caption navigate passively to that exact host, and offer a bounded per-host Ports menu with an entry to its full management section.
- Render existing detected, saved and active mapping observations honestly, distinguish unknown from empty information, and disable pending actions.
- Let an explicit port choice ensure forwarding and open through the existing acknowledged and passively verified mapping contract. Preserve opt-in Remember and saved-port conflict semantics.
- Capture canonical host-group and mapping identity independently of the selected Screen or display alias. Keep admitted browser intent across ordinary popup dismissal, Drawer autohide and reconnect; revoke it when its logical group disappears, its target or local owner is replaced, or Stop/cancellation/window close invalidates it.
- Prevent an old unsubmitted popup from becoming valid again after the same source is detached and reopened.

This advances [Milestone 120](../../../roadmap/README.md). Out of scope: creating new remote Screens, host-group reorder/persistence, reverse forwarding, new Core forwarding policy or transport/server operations, remote settings, and changes to the web client.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `remote-screens`: Native Drawer access to exact host/port management and a host-group-scoped deliberate browser continuation, reusing the existing forwarding contract.

## Impact

Only native GPUI presentation and its interaction lifetime change: Drawer composition, the existing host-row projection/navigation and forwarding-origin tracking. The shared client Core continues to own bounded forwarding requests, acknowledgements, passive mapping verification and one-shot browser handles. Existing server, wire and persistence contracts remain unchanged. Architecture documentation records the new initiating surface without introducing another host catalog or forwarding controller.
