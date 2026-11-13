# Proposal

## Why

The native client links the server only to start it in-process on Linux and macOS (the embedded
service) and then talks to it over the same framed pipe and handshake a remote client uses. The
code holds that split today, but nothing keeps it: the architecture check counts test and
production references alike, so a new production dependency of the client on server internals
passes as soon as someone adds an arrow to the model. Two places also reach into the server
beyond that link: the terminal font comes from the server's embedded web build, and the embedded
shutdown closes the server's client registry entries itself.

## What Changes

- The architecture check tells test code (`#[cfg(test)]` and `#[test]` items, and module files
  declared under them) from production code, and gains a boundary rule: production code of
  `splitty-client-core` names `splitty-server` only from the modules that start and connect the
  embedded service (`embedded`, `connection`, `api_stream`); `splitty-gpui` names no crate of the
  workspace but `splitty-client-core`, tests included, as its manifest allows. Unit tests of the
  checker show that a forbidden reference fails. The rule is documented in `architecture/README.md`.
- The model's descriptions catch up with the native app being the default desktop app, and the
  container relationships gain what is real: the native app starts its backend in WSL on
  Windows and reads the themes directory, and client-core links the server in-process on Unix.
- The terminal font is compiled into client-core from `frontend/public` on every platform instead
  of being read from the server's embedded web build on Linux and macOS.
- A default feature `web-assets` of `splitty-server` embeds `frontend/dist`; client-core depends on
  the server without it, so the native app builds without a frontend build. The `splitty` command
  and `splitty serve` keep embedding the web UI.
- The server's `Runtime::close` closes every client's stream before it shuts down jobs and
  terminals; the embedded service calls only that.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None: no behavior a spec states changes (`skip_specs: true`).

## Impact

- `architecture/check` (scanner and checks), `architecture/*.c4`, `architecture/README.md`.
- `crates/splitty-client-core` (`appearance`, `embedded`, its manifest), `crates/splitty-server`
  (`embed`, `http::routes`, `service::runtime`, its manifest).
- `docs/development.md`, `docs/architecture.md`, CI and release workflow notes.

## Architecture impact

Removes the relationship `appearance -> embed`; changes descriptions of `client_core`, `gpui`
and several relationship titles that only tests back; adds the container relationships
`gpui -> wsl`, `gpui -> config_files`, `client_core -> server` and `user -> gpui`; the check gains
the client/server boundary rule.

## Decisions

- Test code is told apart in the scanner, not by tagging relationships in the model: a
  relationship backed by both kinds of reference needs no second arrow, and the rule reads the
  references themselves, so a test-only arrow cannot hide a production reference.
- Code under a `cfg` that also compiles without `test` (`any(test, feature = "ui-test-support")`)
  counts as production code; only a predicate that requires `test` makes code test code.
- The boundary rule lives in `architecture/check/tests/model.rs` beside the mux purity rule, with
  its allowed modules spelled out there, rather than as model metadata.
- `web-assets` is a default feature; a build without it answers every page request with
  "frontend not built", as a server with an empty `frontend/dist` does. Cargo unifies features per
  invocation, so a build of `splitty-gpui` together with `splitty-server` (or the workspace)
  still embeds the assets in both.
