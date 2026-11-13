# Proposal

## Why

The native operation catalog is the one description of the service's API, but several of its properties still live elsewhere:

- The dispatcher picks an operation's error style by its HTTP path prefix (`/api/jobs`), and the viewer operations decode their own bodies to get their style.
- Non-body arguments are strings on the wire; `job.logs` turns `tail`, `limit` and `waitMs` into numbers by a list of names inside the dispatcher.
- The owner control socket serves twelve catalog operations through a hand-maintained route list with its own handlers. Those handlers answer malformed requests in the HTTP framework's style (plain-text rejections, 400 for broken JSON, unknown query fields ignored) while the browser gateway, which derives its routes from the catalog, answers them in the native style; the control API requires an endpoint available on both transports to behave identically.
- The gateway reads JSON bodies whatever their `Content-Type`, so a `text/plain` request reaches the operations; only the Origin, Fetch Metadata and credential checks stand before it.
- `splitty attach` is typed on WebSocket messages even for its native carrier, which converts them back.
- Small leftovers: `Hub::open_forward_with` has no caller; the browser session store keeps a default cookie name only tests use; `control::spawn_service` repeats `control::spawn_at`; the remote-window helper spells the connection steps through their debug names.

## What Changes

- Every catalog entry states its error style (`plain`, `job`, `viewer`) and the type of each argument (text, count, body). The dispatcher decodes bodies and count arguments by these, without path prefixes or name lists. Count arguments stay decimal strings on the wire; error answers keep their current shapes and statuses.
- A `control` scope marks the backend operations the owner control socket serves. The control socket routes them from the catalog through the same request translation as the browser gateway (query and path arguments, JSON body, catalog validation) and the same service entry point. The job and grant-administration routes keep their router, which the agent grant sockets share, and `POST /api/daemon/stop` stays a control-socket adapter route.
- Both transports refuse a catalog operation that takes a JSON body with 415 unless its `Content-Type` is `application/json` or an `application/*+json` type (parameters allowed), before the body is read.
- `docs/native-operations.json` lists each operation's error style and its count arguments, and `control` as a scope.
- `splitty attach` writes session messages as text, and each carrier (WebSocket, native framing) frames them itself.
- Removed: `Hub::open_forward_with`, the session store's unnamed default constructor and default cookie name, `control::spawn_service` (callers use `control::spawn_at`). The connection steps of `splitty connect` and the remote-window helper have one name function.

Out of scope: the operation set (removals belong to another cleanup), hub wire types, capability lists and client-side event parsing; the job router of the grant sockets; typed numbers on the native wire (would need a capability for older services).

## Capabilities

### New Capabilities

### Modified Capabilities
- `control-api`: Two Control Transports adds that malformed requests are answered alike on both transports.
- `access-control`: adds JSON Request Bodies (415 for a body operation without a JSON `Content-Type`).

## Impact

- `crates/splitty-core`: `native_api` (operation `Description`: error style, argument types, `Control` scope).
- `crates/splitty-server`: `service::dispatch`, `service::review` (typed bodies), `http` (shared catalog request translation for gateway and control socket, `routes::control_routes`, session store), `hub` (unused method), `control` (`spawn_service` removed), `native::helper`, `native::client` (`SessionSink`), `cli::attach`, tests.
- `crates/splitty-remote`: `tunnel::Step::name`.
- `crates/splitty-client-core` (`embedded`) and `crates/splitty-tauri` (`lib.rs`, `client_ipc::provider`): call `control::spawn_at` and `Step::name`.
- `docs/native-operations.json`, `docs/protocol.ts`, `docs/architecture.md`.
- Wire: unchanged for existing clients. The control socket's answers to malformed requests change shape (see the spec delta); the CLI reads both shapes and sends well-formed JSON with `Content-Type: application/json`, as the web client does.

## Architecture impact

none: no component or relationship changes; docs/architecture.md's control socket paragraph says its routes come from the catalog.

## Decisions

- **NAH-1** (provisional, tbd): the error style and argument types are catalog properties; count arguments stay strings on the wire, so older and newer peers keep working without a capability. Alternative: accept JSON numbers too, which a newer client could not rely on against an older service.
- **NAH-2** (provisional, tbd): the control socket takes the browser gateway's answers to malformed requests (JSON `{"error"}` objects; 422 for a body that is no JSON or does not decode, 400 for unknown or duplicate query arguments, 413 above the operation's body limit) instead of keeping the HTTP framework's. Alternative: keep per-route handlers and only derive the list.
- **NAH-3** (provisional, tbd): a body operation needs a JSON `Content-Type` on both transports (415 otherwise), as defense in depth: no API body can then be sent as a CORS-simple request. The CLI and the web client already send it.
- **NAH-4** (provisional, tbd): `splitty attach` writes text session messages through a carrier-neutral sink of strings rather than a new message type, since it sends nothing else.
