# Proposal

## Why

The native workspace can use local Screens but cannot yet inspect the host directory already owned by its local service. A passive Hosts surface makes known hosts and their last observed Screens available while establishing a reusable client boundary for later remote workflows.

This is a bounded continuation of the native client work in [roadmap/README.md](../../../roadmap/README.md), following the completed host-directory work from Milestone 7; it does not reopen the historical SSH implementation in Milestone 5a.

## What Changes

- Read the existing merged directory through a dedicated, bounded local API stream in the UI-free client core, independently of terminal events and settings requests.
- Refresh on initial local readiness and `hub.changed`, coalescing invalidations and retaining the last valid directory with explicit loading/error state.
- Bind directory results to the local connection owner and refresh generation, retain allocation charges through presentation, and join owned stream cleanup.
- Show authoritative host targets, aliases, cached Screen names/pane counts and observation freshness in a native Hosts destination added by this change after `native-workspace-chrome` settles. Cached observations do not establish host reachability or active remote connections.
- Share the existing directory response shape between its Rust producer and consumer without changing HTTP/native payloads or persistence formats.

Out of scope: connecting or browsing a live remote host, opening/creating/terminating remote Screens, remembered-screen restoration, host metadata edits, forwards/reverse shares, port discovery, automatic SSH, updates/migration, a second layout tree, a browser rewrite or WASM binding, CI/release changes, and Windows builds.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `remote-screens`: extend Native Host Management Surface with passive native presentation, source-scoped refresh outcomes and failure/cleanup behavior. Existing Merged Host Directory and Hub Changes On The Local Connection guarantees remain the authority for data and invalidation.

## Impact

`splitty-core::native_api` gains shared directory response records; `splitty-server::hub::directory` projects existing metadata into those records. `splitty-client-core` gains a dedicated hub reader and charged presentation state, composed through its existing embedded/owned service and native workspace runtime. `splitty-gpui` presents that state through the native Hosts surface after the separate chrome change.

The operation remains `remote.directory.get`, available only through the assigned main service; the browser API and server-side merge remain unchanged. Architecture relationships and the corresponding client/host-directory sections of `docs/architecture.md` will be updated during implementation. No new crate or dependency on a server implementation type is required in the client.
