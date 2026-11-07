# Spec Delta

## MODIFIED Requirements

### Requirement: Detected Ports In The Window
For each connected host, the client SHALL list the ports of its last `ports.changed`. Each port shows its process, and either its local address with "Open in browser" and "Stop" when it is forwarded, or "Forward" when it is not. Saved preferences and active forwards SHALL remain available independently of discovery; an absent discovery entry SHALL NOT imply that a manually entered service is not listening. The client SHALL join entries by target and remote port and distinguish a paused saved rule from a failed restore and from an active forward being rebuilt. A port that appears on a connected host SHALL be offered in the host's port list, not announced in a notice. A URL whose host is `localhost`, `127.0.0.1`, `0.0.0.0` or `[::1]`, opened from a remote pane's output, SHALL open the forwarded local address with the same scheme, path, query and fragment. When that port is not forwarded yet, the client SHALL forward it first, since the click counts as the user's consent, including explicit resumption of a paused saved rule. A saved-port conflict SHALL be shown without opening an unrelated local service or silently remapping the URL. Local panes SHALL keep opening such URLs as they are.

The experimental native workspace SHALL retain observations for the exact current host connection, distinguish not-yet-reported from a reported empty list, and discard live observation authority on disconnect or replacement. Invalid or over-limit replacements SHALL NOT become an empty successful observation. Passive inspection SHALL neither start a forward nor restore a saved rule; existing server-owned restoration on successful host readiness is unchanged.

#### Scenario: Link in a remote pane
- **WHEN** a remote pane on `vm1` prints `http://localhost:5173/app` and the user opens it, with port 5173 not forwarded
- **THEN** the client forwards port 5173 of `vm1` and opens `http://localhost:<local>/app` in the browser

#### Scenario: New port notice
- **WHEN** `vite` on `vm1` starts listening on port 5173 while the window is connected to `vm1`
- **THEN** no notice appears, and `vm1`'s port list offers 5173 with `vite` to forward and open

#### Scenario: Saved ports without discovery
- **WHEN** saved frontend and API mappings exist but the host has sent no discovery list or no longer lists those ports
- **THEN** both mappings remain available with their saved local ports and preferences, joined with any active forwards without duplicates

#### Scenario: First list and explicit paused link
- **WHEN** a reconnected host sends its initial port list containing a paused saved mapping
- **THEN** it is listed without being resumed automatically
- **AND** a later explicit link click resumes using the saved local port, preserving the link's scheme, path, query and fragment, or reports the saved-port conflict without opening that URL


#### Scenario: Native observation belongs to one connection
- **WHEN** a native host reconnects or a delayed `ports.changed` belongs to its previous connection
- **THEN** the replacement has no current observation until its own valid list arrives, and the delayed list cannot populate it
- **AND** a valid empty list clears detected suggestions while active and saved mappings remain available

#### Scenario: Native discovery absent or invalid
- **WHEN** the native workspace has received no valid port list, or receives a malformed or over-limit replacement
- **THEN** it distinguishes missing discovery from a reported empty list, preserves only valid current-connection observations, and still permits deliberate manual mappings
- **AND** inspection and rejected observations submit no forwarding mutation

## ADDED Requirements

### Requirement: Native Outbound Port Workflow
The experimental native main workspace SHALL offer manual and detected outbound forwards through its assigned local service, with explicit local-port selection, Stop/Resume, opt-in Remember, metadata-only Save, label editing and Forget. Existing mapping, conflict, persistence and restoration rules SHALL apply. Screen detachment and closing the Ports surface SHALL NOT implicitly stop forwards.

#### Scenario: Native manual mapping without a Screen
- **WHEN** a user enters a valid target and port without discovery or an open Screen, and explicitly starts it with Remember
- **THEN** the assigned local service opens that exact mapping and saves its actual local port only after confirmed opening
- **AND** another mapping can progress independently, while starting without Remember creates no saved preference

#### Scenario: Native save and label without connection
- **WHEN** a native user saves a mapping without starting it, or edits or clears its saved label
- **THEN** only the selected mapping's metadata is changed, omitted fields and paused state are preserved, and no SSH connection or forward starts

#### Scenario: Native Stop Forget and alternate port
- **WHEN** a native user stops or forgets a mapping, or explicitly selects an alternate local port after a conflict
- **THEN** Stop durably pauses it, Forget removes only saved metadata and leaves a live forward running, and an alternate updates the saved port only after confirmed success
- **AND** no conflicting listener or open Screen is terminated

#### Scenario: Native Remember partly fails
- **WHEN** opening succeeds but the separately requested Remember operation fails
- **THEN** the live mapping remains visible and usable, the failed save is reported separately, and the client does not claim the preference was saved

### Requirement: Native Forwarding Request Ownership
Native forwarding requests SHALL retain the assigned local owner, exact mapping and request identity with bounded admission. Slow work for one mapping SHALL NOT block terminal delivery, host metadata, passive reads or admitted work for unrelated mappings. Unsubmitted work SHALL be discarded on owner shutdown, submitted work SHALL retain cleanup ownership, and no mutation SHALL be replayed after an uncertain result.

#### Scenario: Native slow mapping and ordered Stop
- **WHEN** one mapping's opening is pending and the user requests Stop for it while another mapping and local terminal remain active
- **THEN** Stop is ordered for the same mapping, older browser-open consent is revoked, and unrelated admitted work continues
- **AND** duplicate pending requests or exhausted admission produce no duplicate side effects

#### Scenario: Native owner or window closes
- **WHEN** the owner changes or its window closes while forwarding setup, a request or cleanup is pending
- **THEN** obsolete results cannot affect the replacement, unsent mutations and browser continuations are discarded, and owned cleanup finishes without resubmitting work
- **AND** the client does not treat loss of a caller as proof that a service-owned forward stopped

### Requirement: Native Forwarding Reconciliation
Native active and saved forwarding reads SHALL preserve their independent last-good values on failure and refresh passively on supported `hub.changed` notifications. Read invalidation SHALL NOT erase a valid mutation acknowledgement. Superseded snapshots SHALL NOT overwrite newer state, and reconciliation after an uncertain mutation SHALL report effective state without claiming that lost request succeeded.

#### Scenario: Native invalidation overlaps acknowledgement
- **WHEN** `hub.changed` arrives before or after a mapping acknowledgement or during its refresh
- **THEN** the valid acknowledgement remains identified, invalidations coalesce into a fresh passive observation, and stale responses do not restore removed or paused state

#### Scenario: Native lost reply and partial read failure
- **WHEN** a mutation's reply is lost and either its active or saved refresh fails
- **THEN** the request remains explicitly uncertain, each successfully read resource updates while the other's last-good state remains distinguishable, and no mutation is replayed

### Requirement: Native Forwarded Browser Activation
Native browser opening for a remote loopback link SHALL require a deliberate captured source action, a validated opening acknowledgement and subsequent passive confirmation of the same up mapping. Only the admitted HTTP(S) scheme, path, query and fragment SHALL survive replacement with `localhost` and the confirmed local port. Browser consent SHALL be separately revocable and SHALL NOT follow a replacement terminal or owner. These checks do not reserve the listener across OS browser dispatch.

#### Scenario: Native exact-source loopback link
- **WHEN** a remote terminal link is deliberately activated while another Screen becomes selected during forwarding
- **THEN** only the original target and mapping can authorize the rewritten URL, and the original scheme, path, query and fragment are preserved
- **AND** a malformed acknowledgement, conflict, down mapping or failed verification opens no browser URL

#### Scenario: Native stale browser continuation
- **WHEN** Stop, a superseding mapping action, explicit cancellation, initiating-surface closure, source detachment or replacement, or owner shutdown revokes a pending open
- **THEN** a late reply cannot open a browser or restart forwarding from that continuation
- **AND** closing only the initiating surface does not imply that an already admitted forward was stopped

#### Scenario: Native cross-window Stop before open
- **WHEN** another client stops a mapping and passive reconciliation observes that stop before browser dispatch
- **THEN** the observed absent or changed mapping prevents browser dispatch from the old request
- **AND** verification does not repeat the opening mutation to undo the Stop

### Requirement: Native Forwarded Browser Endpoint Scope
Native desktop opening SHALL reuse the validated HTTP(S) opener and confirmed localhost mapping contract. A reproduced mismatch between the service's forwarding endpoint and the desktop browser endpoint SHALL be corrected or the affected opening path SHALL be unavailable with an explanation. Backend forwarding success alone SHALL NOT be reported as observed browser reachability, and the client SHALL NOT fall back to the original remote loopback URL.

#### Scenario: Native Windows with a mismatched WSL endpoint
- **WHEN** a Windows/WSL endpoint probe demonstrates that the forwarded localhost URL reaches a competing Windows listener rather than the owned remote fixture
- **THEN** the affected opening path is corrected or explicitly unavailable, without terminating the competing listener or claiming successful browser reachability
- **AND** the authoritative mapping remains visible and an explicit alternate-port action retains the service's conflict semantics
