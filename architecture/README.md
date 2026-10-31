# Splitty architecture model

The module-level map of Splitty in the [LikeC4](https://likec4.dev) DSL: the people and external
systems, the containers (the Rust crates, the web frontend, the desktop shells, the stores), and
the components of each crate with the dependencies allowed between them. `check/` compares the
model with the code in both directions, so it can be trusted without reading imports. Concepts
(ops model, shadow mode, the two protocol paths, server modes) are explained in
`../docs/architecture.md`; behavior lives in `../openspec/specs/`.

| File | Content |
| --- | --- |
| `specification.c4` | Element kinds (`actor`, `system`, `container`, `store`, `component`) and tags (`planned`, `external`) |
| `landscape.c4` | People, external systems, the system `splitty` with its containers and stores |
| `core.c4` | The components of `splitty-core` and their relationships |
| `server.c4` | The components of `splitty-server` (the `splitty` binary) and their relationships, also into the core |
| `remote.c4` | The components of `splitty-remote` (SSH, install, handshake, bridge, tunnel, known hosts) and their relationships |
| `desktop.c4` | The components of `splitty-gui` (the Tauri app) and their relationships, also into the core and the server |
| `views.c4` | Context, containers, the components of each crate, and one view per composition root |
| `check/` | The workspace crate `splitty-architecture`, whose test runs the check |

## Setup

Requires Node ≥ 22.22.3 and [pnpm](https://pnpm.io/). LikeC4 is pinned in `package.json`.
`cargo test` runs the check, so install it once:

```sh
pnpm install --dir architecture   # from the repository root
```

Without it, the tests of `splitty-architecture` fail with that instruction; they never skip.

## Viewing the diagrams

```sh
pnpm --dir architecture dev        # http://localhost:5173, reloads on every edit
pnpm --dir architecture validate   # syntax and references, without layout
```

The views use auto-layout only. The server's component view leaves out the relationships of
`main`; the two composition-root views show what `main` and `app` use.

## Querying the model

The `.c4` files are short enough to read. For questions over the whole model, export it as JSON,
which holds every element with its metadata and every relationship, and query it. For example,
these are the components that depend on `mux`:

```sh
architecture/node_modules/.bin/likec4 export json --skip-layout -o /tmp/model.json architecture
jq -r '.relations[] | select(.target.model == "splitty.core.mux")
       | "\(.source.model | split(".") | last)  (\(.title))"' /tmp/model.json
```

Element ids are qualified (`splitty.core.mux`). `.elements[id].metadata` holds `module` and
`spec`. So `jq '.elements | to_entries[] | select(.value.metadata.spec == "terminal-sessions") | .key'`
finds the components of a capability.

## Conventions

**Containers are the crates.** `splitty-core`, `splitty-server` and `splitty-gui` are containers,
next to the web frontend, the Electron shell and the stores. The core is a container of its own,
although it is only ever linked into the `splitty` binary and the desktop app. That way each
crate's components sit in one place, and a relationship into the core reads as the use of a
library.

**Components are modules.** A component names its code in `metadata { module 'splitty_core::mux' }`,
the module path as Rust spells it, starting with the target's crate name: `splitty_core`,
`splitty_server`, `splitty` (the server's binary), `splitty_tauri_lib` and `splitty_gui`. Where one
capability covers a component, it also names its spec in `metadata { spec 'workspace-model' }`
(a directory under `../openspec/specs/`). Components stop at top-level modules. The one exception
is `mux::effect_runner`, which makes "within the mux, only the effect runner touches the PTY
layer" a checked rule.

**Ownership by longest prefix.** A file belongs to the component whose module is the longest prefix
of its module path. So `splitty_core::mux::effect_runner` belongs to `effect_runner`, and
`splitty_core::mux::reducer` belongs to `mux`. There are two exceptions:
- A component that names a crate root (`main` for `splitty`, `app` for `splitty_tauri_lib`, `app_main` for `splitty_gui`) owns only the root file, so a new module in that crate still needs a component of its own.
- A root that holds nothing but `mod` declarations, attributes and comments, such as the library roots of `splitty-core` and `splitty-server`, needs no owner.

**Flat siblings.** All components are direct children of their container, also where one module
lies inside another's directory. LikeC4 rejects relationships between parent and child, and
`effect_runner` depends on `mux`. The check names components by the last part of their id, so
names are unique across the model.

**Relationships are references.** `effect_runner -> pty 'Backend, SessionHandle, …'` means that
code of `effect_runner` may name modules of `pty`. Its title names what is used. A relationship
covers exactly its two components. Naming counts in these places:
- every path in a `use` tree, re-exports included;
- every path of two or more segments in code and types;
- paths inside macro invocations;
- code under any `cfg`, so Windows-only code counts on Linux too;
- inline `#[cfg(test)]` modules.

Doc comments, integration tests under `tests/`, build scripts and the check's own crate do not
count. The check works in both directions:
- a reference without a relationship fails;
- a relationship that no reference backs fails too, since a stale arrow would tell a reader that a dependency exists.

Relationships to stores, external systems and between containers are documentation and are not
checked.

**Re-exports count where they are written.** If `mux` re-exported the effect runner, every
importer would name `mux`, and `mux` would depend on `effect_runner`. So the boundaries the model
draws are not bridged by `pub use`.

**`planned`.** An element tagged `#planned` stands for something a draft change will build, such as
the MCP server. A planned component names a module that does not exist yet,
and its relationships are not compared with the code. The tag is removed when the module is built,
and the check fails if it is not.

## What the test checks

`check/tests/model.rs` runs `likec4 validate` and `likec4 export json` once per run, scans the
library and binary targets that `cargo metadata` lists, and reports every violation of each
check:

1. The model validates (file, line, message).
2. Every module file is found: every `mod` declaration has a file, and no `#[path]` is used.
3. Every file with code has a component.
4. Every component not tagged `planned` names a module file. A `planned` one names a module that does not exist yet, and no two components name the same module.
5. Every reference between components has a relationship. Example: `crates/splitty-core/src/auth/token.rs:52 names splitty_core::config::ensure_private_dir: no relationship auth -> config`.
6. Every relationship between built components is backed by a reference.
7. Every `spec` names a directory under `../openspec/specs/` (`planned` elements are exempt).
8. The mux stays pure: no relationship from `mux` to `pty` or `effect_runner`.

A change that adds a crate, a top-level module or a dependency between components updates the
`.c4` files in the same change. The failure message names the missing relationship.
