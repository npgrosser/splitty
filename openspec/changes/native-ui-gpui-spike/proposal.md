# Proposal

## Why

The server prerequisites for [roadmap milestone 120](../../../roadmap/README.md#als-nächstes) are complete. A runnable, isolated prototype must establish native terminal rendering, document selection and offscreen composition before the [full GPUI migration](../native-ui-gpui/proposal.md) can be planned.

## What Changes

- Add an experimental native executable beside the released desktop clients, with an isolated app-owned service and no persisted workspace migration.
- Introduce a small UI-free native client adapter using the existing ordered, budgeted connection, mux, session and viewer operations.
- Display a real service-backed terminal with native input, composition, selection, clipboard and resize handling; display a service-backed Markdown document with native selection and server-stored annotations.
- Provide an optional offscreen scripted-content experiment using a fixed local fixture, native overlays and clipping, with no service authority exposed to its engine. Record actual integration failures explicitly.
- Record reproducible dependency pins, Linux launch results and a platform/interaction matrix distinguishing tested, failed and unverified cases. The result is a migration decision input, not production parity.

Out of scope: replacing released packages; deleting browser, Electron or Tauri code; a complete dock, hosts UI or production client core; new transport messages; arbitrary scripted documents; shipping a web engine; claiming macOS/Windows validation from Linux runs; or declaring the migration approved solely from automated tests.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `client-connections`: distinguish the explicitly experimental native executable from released-client feature parity, and define its isolated lifetime, native session/document paths and authority-free scripted fixture.

## Impact

- New `splitty-client-core` and `splitty-gpui` workspace crates, with platform-conditional embedded-service composition and exact GPUI/terminal dependencies.
- Existing native protocol and server ownership remain authoritative. Windows must not acquire dependencies on the Unix service/core crates.
- Architecture containers/modules/relationships, development documentation, milestone status and the parent draft receive the measured spike results.
- Optional CEF integration and user-local build prerequisites are development-only. Existing release packages and security policy remain unchanged.
