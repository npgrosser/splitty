# Proposal

## Why

Server and native client describe the same messages twice. Capability strings are repeated as literals on both sides and the `mux.welcome` list is put together in two places; the hub's replies (remembered Screens, forwards, reverse forwards, host metadata) are restated in client-core, which decodes host-metadata replies into the server's file records; several server messages are built ad hoc with `json!`; and the client recognises server messages by matching type strings in a `serde_json::Value`. A field renamed on one side is found only at run time.

## What Changes

- Every capability string is defined once in `splitty_core::protocol::capability`; server and client-core use the constants, and the `mux.welcome` list is assembled by one function there.
- The hub's reply types move to `splitty_core::native_api` (`remembered`, `forwards`, `reverse`, `hosts`) and are used by the server to build its replies and by client-core to read them; client-core's copies are deleted, and host-metadata replies get their own types instead of `splitty_remote::hosts` file records.
- The server messages built ad hoc (`hub.changed`, `mux.template_extracted`, `mux.close_assessed`, `editor.request`, `editor.cancelled`, `file.offer`, `file.closed`) are built from types in `splitty_core::protocol`.
- `ServerEvent` and the new message types derive `Deserialize`; client-core decodes them typed, module by module, and keeps ignoring unknown message types and unknown fields.
- No behavior change: golden tests pin the JSON of each message and reply before its construction or parsing changes.

## Capabilities

No requirement changes: the messages, replies and capabilities on the wire stay as they are. `skip_specs: true` applies.

## Impact

`splitty-core` (`protocol`, `native_api`), `splitty-server` (client session, hub, service operations), `splitty-client-core` (hub clients, ports, native workspace event handling). No new dependency between crates; client-core still builds without `splitty-server` on Windows.

## Architecture impact

none (new submodules inside the existing `splitty-core` protocol and native API components; no new crate, top-level module or dependency between components).

## Decisions

- SWT-1 (provisional, tbd): the server's files and its checks of input keep their own records and strictness (`deny_unknown_fields`, `lastError` never read from disk); the shared types describe replies and tolerate unknown fields, so an older client keeps reading a newer service. The server converts its records into the reply types.
- SWT-2 (provisional, tbd): messages formerly built with `json!` now put `type` first and their other fields in declaration order, as `ServerEvent` does; names and values are unchanged and no reader depends on key order.
- SWT-3 (provisional, tbd): a client reads a known message type whose fields do not decode the way it read a malformed message before (ignored, or reported where it was reported); unknown message types are ignored.
