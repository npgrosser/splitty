# Proposal

## Why

The native terminal currently treats application mouse input as local text selection and substitutes arrow keys for every alternate-screen wheel event. Terminal applications therefore cannot reliably use clicks, dragging or their requested wheel reports. Legacy mouse formats also expose a UTF-8-only input boundary that cannot carry all terminal bytes.

## What Changes

- Add an optional, bounded byte-preserving session input message with capability negotiation; retain the existing text input contract for existing clients and servers.
- Encode native application pointer events according to terminal-requested tracking and encoding modes, with Shift retaining local selection and local scrolling.
- Keep pointer gestures bound to their original terminal, settle or cancel them on lifecycle changes, and preserve existing bounded input admission without replay.
- Validate the engine, shared input path and real native-window interaction with deterministic terminal fixtures.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `terminal-sessions`: optional raw-byte input with unchanged session authority and ordered delivery.
- `client-connections`: native terminal pointer routing, local override, lifecycle and compatibility.

## Impact

Touches the core session dispatcher, native client input API, native workspace routing and GPUI terminal adapter. The Ghostty binding gains its pinned mouse encoder API. The browser protocol type and `docs/protocol.ts` document the additive input message; existing browser behavior remains unchanged. This is a native UI experiment increment; no numbered roadmap milestone directly specifies it.

Out of scope: web removal/refactoring, search, terminal links, renderer replacement, remote UI, new packaging/CI, driver changes and generalized input remapping.
