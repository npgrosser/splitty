# Proposal

## Why

Whether a close loses something is decided twice, in each client, and differently: the native client never counts scratchpad text and passes no annotations when its window closes, the web client never asks about terminals that are still starting, and only the native client checks what a template replacement of a Screen ends. The server holds every fact (program state, scratchpads, annotations); it should give the answer once, for local and host Screens alike.

## What Changes

- New session message `mux.assess_close` with a `scope` (panes of one Screen, whole Screens, or the whole workspace) answered by `mux.close_assessed`: per affected pane or session whether a program runs, is still starting, its scratchpad holds text, and how many annotations it holds, plus totals and whether to ask. Announced as `close-assessment-v1` in `mux.welcome`.
- Both clients decide and word their confirmations from that answer only and drop their own rules: pane, tab, Screen and window close, and replacing a Screen by a template. Against a server without the capability, or when the answer does not come, a client always asks a generic confirmation.
- **BREAKING** (for older windows on a newer server): `apply_template` onto an existing Screen that holds panes or sessions is refused unless the op carries `replaceConfirmed: true`.
- The web client now asks before a template replaces a Screen whose programs, scratchpads or annotations would end; the native window counts scratchpads and annotations on every close, including its own window close.

Out of scope: what a close does (unchanged ops), the native consent binding to identities and connection generations (stays client-side, it is about the client's own pending state), managed job panes (still not counted as running programs), the confirmation texts' wording beyond the new facts.

## Capabilities

### New Capabilities
None.

### Modified Capabilities
- `mux-protocol`: the close assessment message, its capability and the older-server behavior.
- `templates`: replacement needs `replaceConfirmed`; both windows ask from the assessment before replacing.
- `session-scratchpad`: Confirmation Before Text Is Lost comes from the server's assessment.
- `viewer-annotations`: Confirmation Before Annotations Are Lost comes from the server's assessment.
- `client-connections`: the native close assessment takes busy, starting, scratchpad and annotation impact from the server.

## Impact

`crates/splitty-core` (mux: assessment over the workspace state, template replacement check), `crates/splitty-server` (session message, welcome capability), `crates/splitty-client-core` (connection request, source runtime, close plan), `crates/splitty-gpui` (close prompts, template review), `frontend/` (mux client, close guard, quit and template flows), `docs/protocol.ts`, `docs/architecture.md`.

## Architecture impact

none (a new module inside the existing `mux` component; no new crate, top-level module or dependency between components).

## Decisions

- SCA-1: the request is a session message on the Screen's own connection, like template extraction, so a hub relays it to a host unchanged; the field is `scope`, since session messages may not carry `target`.
- SCA-2: what is lost is decided per pane: a terminal pane (or a Screen's session without a pane) whose program runs or is still starting, a session whose scratchpad holds text, and a viewer pane holding annotations, resolved ones included. Managed job panes are not counted as running programs (provisional, tbd).
- SCA-3: without `close-assessment-v1`, or when the assessment fails or times out, every close asks a generic confirmation, idle ones included; the clients keep no second rule for older servers (provisional, tbd).
- SCA-4: the server refuses a replacement of a non-empty Screen without `replaceConfirmed: true`, so no client can skip the question; an older window that replaces a Screen on a newer server is refused and has to be updated (provisional, tbd).
- SCA-5: closing a browser tab ends no sessions, so the web page no longer asks on `beforeunload`; the desktop shell's own quit still asks from the assessment (provisional, tbd).
- SCA-6: the native window waits for the server's answer before it prompts or closes; the execution-time check compares the identities and the client's own pending state as before and keeps the assessed server impact, without asking the server a second time (provisional, tbd).
