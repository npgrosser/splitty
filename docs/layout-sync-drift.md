# Design Decision: Layout Sync between Dockview and the Mux Model

- **Status:** Open — decision deferred, gathering data
- **Affects:** `mux.op` protocol, PTY lifecycle, CLI control, multi-window / daemon restore
- **Written:** 2026-08-01, after debugging "split creates a blank pane"

---

## The question

Splitty keeps two models of the same layout:

- **Dockview** in the frontend — what the user sees and manipulates
- **`ScreenState`** in the mux backend — what the CLI, other windows, and a daemon restore read

They are kept in sync by fire-and-forget `mux.op` messages. When an op is lost or rejected, the
two drift apart. Since the server started owning the PTY lifecycle, drift is no longer cosmetic:
a pane the server does not know about is a pane whose terminal never starts.

This document records why the current design is what it is, what changed underneath it, and which
options exist for making drift converge again.

---

## How it works today

Dockview is the local source of truth. Every interactive layout change (split, new tab, close,
move) is mirrored to the server as an op — `split_pane`, `close_pane`, … — with client-generated
pane and screen IDs. The server validates, reduces, and executes the resulting effects.

Since commit `7dee0bd` the server also spawns the PTY (see
[The shift that changed the risk profile](#the-shift-that-changed-the-risk-profile)):

```
client: addPanel(terminal-7)          -> pane exists, terminal waits
client: mux.op split_pane(ref=terminal-6, new=terminal-7, cwd=…)
server: reduce -> Effect::SpawnSession -> PTY
server: mux.pane_spawned(terminal-7, sess-x)
client: attach(sess-x)                -> terminal appears
```

## The design was deliberate

Shadow Mode is documented, not accidental:

- `docs/protocol.ts` §8.2 (line 964) — "Shadow Mode (Phase A)"
- `docs/architecture.md` — "Client Autonomy (Shadow Mode)" (line 21)
- `roadmap/01-multiplexer-backend.md` — lines 20, 315–321, 529–531

The reasoning, in the project's own words:

- **Single-Attach** — one client per screen, so no conflicting writers, no echo detection, no
  revision tracking, no conflict resolution. Multi-Attach (`openspec/changes/multi-attach/proposal.md`) was
  explicitly deferred because that machinery is expensive.
- **Client-set IDs** — the client names the pane before the server has seen the op, so a pane can
  render immediately. No round trip.
- **Fire-and-forget** — named in `roadmap/01:530` as the mitigation against "Performance-Regression
  durch Op-Roundtrips": the client never waits for the server.

And the ops model itself carries its own advantages, independent of Shadow Mode:

- **Ops express intent, not state.** `split_pane(paneId, axis, cwd)` tells the server *what should
  happen*, so it can derive the right effect (spawn a PTY in the right directory). A layout tree is
  only a description — the server would have to infer which pane is new, which is being adopted,
  which session is orphaned, and a wrong guess kills a live PTY.
- **The reducer stays pure and validated.** Every op passes `validate_screen_op`. A "replace the
  layout" message bypasses transition validation by construction.
- **Multiplayer-ready without a format break.** `docs/protocol.ts` §8.3: an op is both intent and
  fact, whether applied optimistically or authoritatively. Moving to a server-authoritative mode
  changes delivery semantics (ACK/REJECT, broadcast), not the wire format. A full-state push
  broadcasts badly — every change would ship the whole tree to every client.

## The shift that changed the risk profile

`docs/protocol.ts` §8.2 used to state:

> PTY lifecycle uses the existing session protocol (create/close messages), **NOT** mux effects —
> effects are skipped for WS-initiated ops to avoid creating duplicate PTYs

Commit `7dee0bd` ("server-driven PTY lifecycle and CLI control") inverted exactly that. WS-initiated
ops now execute effects, and the client waits for `mux.pane_spawned` before attaching. That is a
sound direction — it is what makes CLI control, multi-window and daemon restore possible — but it
moves a dependency the fire-and-forget semantics were never designed to carry:

| | before `7dee0bd` | after |
|---|---|---|
| Rejected / lost op | cosmetic — the client had already created its own PTY, only the shadow drifts | **no PTY at all** — the pane stays blank |
| Documented mitigation | "automated comparison of `dockview.toJSON()` and `ScreenState`" (`roadmap/01:529`) | none — and the comparison was never built |

The same document already names the missing piece:

> Future: When moving to server-authoritative mode, the server would start sending **ACK/REJECT**
> responses … The op FORMAT stays the same — only the delivery semantics change.

`mux.op_failed` (added 2026-08-01) is the REJECT half of that sentence.

---

## What was fixed on 2026-08-01

Symptoms: no terminal on startup, splits producing a permanently blank pane, and
`mux.op split_pane failed: … pane terminal-5 not found` in the server log.

1. **A rejected op wiped the entire workspace state.** `Store::dispatch` passed the state to the
   reducer by value (`std::mem::take`) and returned early on error — the taken state was gone. One
   rejected op (a `move_pane` for a pane dockview had already moved, a split off an unknown pane)
   dropped every screen, after which *everything* failed. Now the dispatch rolls back
   (`crates/splitty-core/src/mux/store.rs:54`, test `failed_dispatch_leaves_state_untouched`).
2. **Several pane-creation paths never registered with the mux** — the `+` new-tab button, the
   split buttons in the group header, the watermark, the pane context menu. They created panes
   without a `screenId` and without an op, so splitting off them could only fail. Registration is
   now central in `addTerminalPane` via a screen registry (`frontend/src/lib/panes.ts:20`). That
   registry looked a layout up by its API object, which dockview does not share with group headers
   and the watermark, so the `+` button, the header's split buttons and the watermark still missed
   it until `connection-scoped-frontend` keyed it by the layout's id
   (`frontend/src/lib/connection/screen_binding.ts`).
3. **The originating client nudged itself.** Every op sent the initiator a `mux.nudge`, which
   triggered a resync and a full snapshot rebuild — closing and re-adding panes whose terminal was
   still attaching (duplicate attaches, two readers on one PTY, blank panes). The initiator now
   receives only `mux.pane_spawned`; other clients still get the nudge.
4. **No recovery path.** A pane waiting for `pane_spawned` hung for 30 s before falling back. Now
   the server reports `mux.op_failed` and the client spawns immediately; the timeout is 8 s
   (`frontend/src/panes/terminal_pane.ts:147`).
5. Side fixes: `split_pane` carries `cwd` so the server-side spawn inherits the working directory;
   a second `attach` cancels the previous output reader; a disposed pane detaches instead of killing
   a session the rebuild is about to reuse.

The fallback means the user sees a working terminal even when an op is rejected — but the pane stays
unknown to the server, and the warning keeps appearing.

## What was fixed on 2026-10-30

Symptom: on a remote screen, a managed job's pane that had been dragged into a tab group came back as a split of its own after the connection to the host dropped.

Cause: a source of drift this document did not list.
- Dockview's `onDidMovePanel` was mirrored as `move_pane` with only the pane's ID. The server rejected that for lack of a target, quietly, since `mux.op_failed` was only sent for ops that create a pane.
- Divider drags were not mirrored at all.

So every drag and every divider move stayed in the window, and the next snapshot undid it: a reconnect, a reload, another client's nudge or a daemon restore.

Fix (change `arrange-panes`): after a drop or a divider drag the window sends its whole grid as `arrange_panes`. Dockview's grid and the server's tree do not match node for node, so `move_pane` and `resize_split` could not carry these gestures. The server accepts the arrangement only when it holds exactly the root layout's panes; unknown pane IDs are left out. The op creates and ends nothing. A rejection is reported, and the window resyncs at once. This is not option B below: the server infers no new panes and adopts or ends no session.

## Remaining sources of drift

- **Cascade (dominant).** Once a pane is unknown, every pane split off it is created by the fallback
  and is therefore unknown too. A single miss produces a whole family of warnings; only the first
  one is diagnostically interesting.
- **Template panes.** They carry `command`/`dir`, which `split_pane` cannot express, so they spawn
  directly by design (`directSpawn` in `template_apply.ts`). Splitting off them fails.
- **Ops dropped during a snapshot rebuild.** A screen's binding drops its ops while its
  `suppressOps` is set (`ScreenBinding.sendOp`, `frontend/src/lib/connection/screen_binding.ts`;
  per screen since `connection-scoped-frontend`, for all screens before), which covers the rebuild
  plus one animation frame (`screen_snapshot.ts`). A pane created on that screen in that window is
  never announced.
- **Webview panes.** `addWebviewPane` only registers where the caller explicitly notifies.
- **Floating and popout groups.** A drag between the grid and a floating group is not part of an
  arrangement; the server rejects it and the window shows the pane where the server keeps it. Popout
  groups are not modeled on the server.

Since the server now logs the panes it does know —
`… err=pane terminal-6 not found, known panes=[terminal-2, terminal-1]` — the next occurrence should
identify which of these it is.

## What drift actually costs

Nothing locally: the pane and its terminal work. It costs correctness for everything that reads the
server's model — `splitty split`/`focus` and friends targeting that pane, layout sync to a second
window or a remote client, and a daemon restore, which will restore a layout missing those panes.

---

## Options

### A — Status quo: ops + `op_failed` + client fallback

The client repairs the *user-visible* symptom immediately and leaves the model diverged.

- **+** No round trip, no new protocol surface, panes never stay blank.
- **−** Drift is permanent and accumulates over a session; the server's model silently degrades.

### B — Reconcile: client pushes its full layout

On `op_failed` (or on reconnect) the client sends its complete dockview layout including the session
IDs it already holds; the server adopts it wholesale.

- **+** Converges in one shot, no matter how the drift arose. Also repairs unknown causes.
- **−** State, not intent: the server must infer which panes are new, which sessions to adopt, and
  which to kill — the one place where a bug takes down a live PTY. Bypasses `validate_screen_op`.
  Scales badly to multiple clients (whole tree per change). Largest implementation.

### C — Narrow `adopt_pane` op (preferred if anything is needed)

```
adopt_pane { paneId, refPaneId, axis, sessionId }
```

Same reducer path as `split_pane`, but it adopts the session the client already spawned instead of
producing a `SpawnSession` effect. The client sends it exactly when it fell back after `op_failed`.

- **+** Heals drift where it originates, one pane at a time. Stays in the ops model: intent,
  validated, small, broadcast-friendly. No PTY is ever at risk.
- **−** New op + reducer path + effect-suppression logic; does not repair drift whose cause never
  produced an `op_failed` (e.g. an op swallowed by `suppressOps`).

### D — Close the remaining holes instead

Register template panes (needs `command`/`cwd` on the split op, or a pane role that spawns nothing),
queue instead of drop ops while `suppressOps` is set, register webview panes. Prevention rather than
repair.

- **+** Removes causes instead of symptoms; no protocol addition for the common paths.
- **−** Cannot cover the unknown-unknowns; the fallback path itself always leaves a hole (C's job).

### Not an option

Making the server tolerant of unknown reference panes (auto-registering them on `split_pane`). It
turns a detectable inconsistency into a silent one and lets the two models disagree about which
pane the new one sits next to.

---

## Recommendation

**Do nothing yet.** With the 2026-08-01 fixes in place, wait for the diagnostic warning to show
whether drift still occurs and which pane it starts with.

- If the log points at a specific creation path → **D** for that path, done.
- If drift persists across paths, or Multi-Attach (`openspec/changes/multi-attach`) becomes real → **C**, and revisit
  ACK/REJECT for all ops as `docs/protocol.ts` §8.2 anticipates.
- **B** only if a future feature needs the client to hand over a whole layout anyway (e.g. adopting
  an externally built layout, or template application becoming server-side).

## Follow-ups regardless of the decision

- The drift check from `roadmap/01:529` (`dockview.toJSON()` vs `ScreenState`) was never built. Even
  a dev-only assertion would have caught all of this before it reached the UI.
