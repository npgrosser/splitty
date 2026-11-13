# Proposal

## Why

A browser reaches Splitty only through the explicit gateway (`http::gateway::router`): `serve` and
`serve --daemon` build it, it derives every `/api/*` route from the native operation catalog and
turns each request into a native request, and its `/ws/remote` opens a native host stream to the
hub. The axum API router from before that gateway is still in the server: `build_router` and
`build_router_with_frontend` with about forty hand-written routes and their handlers, a WebSocket
carrier of its own for `/ws` (`ws.rs`) and, in the hub, a second relay for `/ws/remote` that
speaks WebSocket directly (`Hub::serve` with its setup, relay and message helpers). No production
code calls any of it; only the in-process tests did, so those tests checked a path no browser
takes, and the hub's bookkeeping after a host is reached (`remote.ready`, recent host, catalog
observation, restoring saved forwards) was written twice.

## What Changes

- The in-process tests of the browser routes run through `gateway::router` over the
  in-process native service (`gateway::Backend::embedded`), the composition `serve` builds.
  Their bodies stay; the few assertions that described the old router's extractors now describe
  the gateway's answers (a body that is not JSON is 422, as for every catalog operation).
- Removed: `build_router`, `build_router_with_frontend`, `apply_middleware`, the API route table
  (`routes::api_routes_with`) with the handlers only it registered, the old `/ws` and
  `/ws/remote` upgrades, `ws.rs`, and the WebSocket half of the hub (`Hub::serve`, its setup,
  relay and message helpers and their types).
- The control-socket router keeps its routes and handlers on a state that holds only the service
  (`ControlState`); `AppState` with its browser fields goes away, as it was built in production
  only with empty token, sessions, origins and address.
- What the hub does once a host is reached is one function shared by every relay that remains.
- docs/architecture.md, docs/networking.md and the LikeC4 model describe only the gateway path.

No behavior a client can observe changes: the removed code had no production caller.

## Capabilities

### New Capabilities

### Modified Capabilities

None; no spec names the removed router (`skip_specs: true`).

## Impact

`crates/splitty-server` (`http`, `hub`, `ws`, `control`, `main.rs`) and its in-process tests;
`control::spawn*` take the service instead of `AppState`; docs and the architecture model.

## Architecture impact

Removes the `ws` component of the server container and the arrows only it and the old router
used; the `http` component is described as the browser gateway plus the control-socket router.

## Decisions

- The gateway decodes request bodies by the catalog and does not look at `Content-Type`, so the
  old router's 415 for a non-JSON content type is not carried over; the gateway's origin, Fetch
  Metadata and authentication guards still run before any body is read.
