# Tasks

## 1. Capability constants

- [x] 1.1 Unit (splitty-core, splitty-server mux_handler): pin the current `mux.welcome` capability list, in order, for a local main connection and for any other connection.
- [x] 1.2 Define every capability string once in `splitty_core::protocol::capability`, assemble the welcome list there, and use the constants in splitty-server and splitty-client-core instead of literals and local copies.

## 2. Hub reply types

- [x] 2.1 Unit: pin the JSON of the remembered Screens, forward, forward preference, reverse forward, reverse preference and host-metadata replies as the server builds them today.
- [x] 2.2 Move the reply types into `splitty_core::native_api` (`remembered`, `forwards`, `reverse`, `hosts`); the server builds its replies from them, client-core decodes them and drops its copies and its use of `splitty_remote::hosts` records.
- [x] 2.3 Unit (client-core): replies with unknown fields still decode.

## 3. Ad hoc server messages

- [x] 3.1 Unit (splitty-core): golden tests of `hub.changed`, `mux.template_extracted`, `mux.close_assessed`, `editor.request`, `editor.cancelled`, `file.offer` and `file.closed` against the `json!` they were built with.
- [x] 3.2 Build them from the types in the server.

## 4. Typed decoding in client-core

- [x] 4.1 Unit (splitty-core): `ServerEvent` and the message types decode their own JSON, ignore unknown fields, and an unknown type decodes as unknown.
- [x] 4.2 Decode server messages typed in client-core module by module; extend each module's existing tests with a message carrying an unknown field.
- [x] 4.3 Update docs/protocol.ts and docs/architecture.md where they name the places.
