# Tasks

## 1. Server

- [x] 1.1 Mux: a first-screen create that is applied only to a workspace without screens and rejected otherwise; the `screen-names-v1` capability constant
- [x] 1.2 Session and HTTP `create_screen` take `onlyIfEmpty: true`; `mux.welcome` lists `screen-names-v1`
- [x] 1.3 Tests (unit, reducer): first screen on an empty workspace named Screen 1; refused on a workspace with a screen, unchanged and without effects; two in a row leave one
- [x] 1.4 Tests (in-process, mux handler): the welcome lists the capability; a refused first-screen create yields `mux.op_failed` to its sender
- [x] 1.5 Program status notices title an unnamed Screen "Unnamed Screen" instead of by its place; unit test

## 2. Native client

- [x] 2.1 Client core: connection sends a first-screen create with `onlyIfEmpty`; the catalog records `screen-names-v1`, creates without a name with it and with the smallest free "Screen N" without it; the first local Screen goes through the first-screen create; `CreateScreen` carries an optional name
- [x] 2.2 Native window: "New Screen" sends no name; the positional fallback labels (drawer, move-to-Screen menu, program status) become "Unnamed Screen", a pending local creation "Creating Screen…"
- [x] 2.3 Tests (unit, client core): create without a name with the capability, fallback name without it, first-screen message shape
- [x] 2.4 Tests (in-process, embedded service): two clients send the first Screen against an empty workspace and the service holds exactly one; the loser's catalog drops its pending Screen
- [x] 2.5 Tests (view, `ui-test-support`): the empty-page test expects a create without a name

## 3. Web client

- [x] 3.1 Mux client: `screen-names-v1` flag; `createScreenOp` with `onlyIfEmpty`
- [x] 3.2 Screens store: new Screens without a name get a placeholder label marked provisional, never sent; `nextScreenName` only for older servers; local snapshots set the name
- [x] 3.3 Workspace: create and move-to-new-Screen omit the name with the capability and resync to learn it, otherwise name at send time; the pending host Screen rename (`avoidHostNames`) removed; the first Screen goes out with `onlyIfEmpty`
- [x] 3.4 Screen startup: a refused first Screen is dropped once the server's Screen is shown
- [x] 3.5 Tests (vitest): screens store naming, screen startup refusal, mux client op shape

## 4. Docs

- [x] 4.1 `docs/protocol.ts`: `onlyIfEmpty`, the capability
- [x] 4.2 `docs/architecture.md`: ID Ownership and the naming paragraphs; `docs/native-ui-handoff.md`
- [x] 4.3 `openspec/open-decisions.md`: SSN-1 to SSN-3

## 5. Checks

- [x] 5.1 Targeted tests of the touched modules
- [x] 5.2 Full check as in CLAUDE.md, including `cargo test -p splitty-gpui --features ui-test-support` and `pnpm check`, `pnpm test`, `pnpm build` in `frontend/`
