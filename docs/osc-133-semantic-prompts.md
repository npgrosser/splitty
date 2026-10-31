# Enhancement Proposal: OSC 133 Semantic Prompts for Splitty

## Context

Splitty currently detects application state (e.g. "Claude Code is waiting for input") by
watching for icon changes in OSC 0/2 window titles — extracting the leading Unicode character,
counting how often it changes, and inferring "idle" when the icon stops animating or matches a
known static icon. This is a fragile heuristic that only works for apps with specific title
conventions and requires per-app configuration (`AppIdleRule` with glob patterns and known icons).

Terminal emulators like Ghostty solve this properly using **OSC 133 Semantic Prompts**, a
standardized protocol where the shell itself reports prompt/input/output boundaries via escape
sequences. This proposal replaces the icon-sniffing hack with OSC 133 support.

---

## 1. OSC 133 Protocol Overview

OSC 133 (spec: https://gitlab.freedesktop.org/Per_Bothner/specifications/blob/master/proposals/semantic-prompts.md)
defines escape sequences that the **shell** emits to mark regions of terminal output. The terminal
sees a repeating cycle:

```
  \e]133;A\a    ─── prompt start (shell is rendering the prompt)
  \e]133;B\a    ─── input start (user is typing a command)
  \e]133;C\a    ─── output start (command is executing, producing output)
  \e]133;D;N\a  ─── command finished (exit code N)
       │
       └──────── next \e]133;A\a
```

Every character on screen belongs to one of three semantic zones: **prompt**, **input**, or **output**.

`133;D` also carries the **exit code**, so the terminal knows whether a command succeeded or failed.

The protocol has additional actions (L, N, P, I) and options (aid, cl, redraw, etc.) but A/B/C/D
are the core that matters for Splitty's use case.

### What this enables

| Capability                        | How                                                   |
|-----------------------------------|-------------------------------------------------------|
| **Idle detection (universal)**    | `C` → `D` transition = command done, works for any app |
| **Per-command exit codes**        | `D;N` carries exit code for each command, not just session exit |
| **Long-running command detection**| In `Executing` phase for > N seconds = still running   |
| **Command duration tracking**     | Time between `C` and `D`                               |
| **Jump-to-prompt (future)**       | Mark rows as prompt/output for scrollback navigation   |
| **Smart resize (future)**         | Shell can signal it handles prompt redraws             |
| **Prompt/output styling (future)**| Visual distinction between zones in the terminal       |

---

## 2. Current Implementation (What Gets Replaced)

### The icon-sniffing hack

The current idle detection pipeline:

1. `OscScanner` (`pty/osc.rs`) parses OSC 0/2 title sequences from the PTY byte stream
2. Title changes are pushed via a `watch` channel as `OscTitleEvent`
3. `watch_osc_title()` in `protocol/dispatcher.rs:544` reacts to title changes and feeds them
   into `NotificationEngine`
4. `NotificationEngine::on_osc_title_change()` in `protocol/notify.rs:335`:
   - Calls `extract_leading_icon()` to grab the first non-ASCII char from the title
   - Tracks `icon_change_count` — if the icon keeps changing, assumes animation (spinner)
   - Matches process name against `AppIdleRule` globs (e.g. `"claude*"`)
   - If icon matches known `idle_icon` (e.g. `"✻"`) → fires `app.idle` immediately
   - Otherwise, starts a threshold timer → fires `app.idle` when icon stops changing
5. `AppIdleRule` config in `config/settings.rs:224` stores per-app rules with `match_pattern`,
   `threshold_ms`, and `idle_icon`

### Why this is fragile

- **App-specific**: Only works for apps whose title icon conventions are known and configured
- **Hardcoded defaults**: Ships with `"claude*"` / `"✻"` — breaks if Claude Code changes its icons
- **False positives**: Any app changing its title icon rapidly could trigger idle detection
- **Not generalizable**: Can't detect "command finished" for `cargo build`, `npm install`, etc.
- **Complex machinery**: `icon_change_count`, `idle_notified`, `last_idle_notif_at` cooldowns,
  `IDLE_NOTIF_COOLDOWN`, threshold timers — all to approximate what OSC 133 gives for free

### Code to remove

| File | What | Why |
|------|------|-----|
| `config/settings.rs` | `AppIdleRule` struct, `app_idle_rules` field | No longer needed — detection is universal |
| `config/settings.rs` | `app_idle` bool | Replaced by semantic state |
| `protocol/notify.rs` | `IdleRule` struct | Backend equivalent of `AppIdleRule` |
| `protocol/notify.rs` | `extract_leading_icon()` | The core hack |
| `protocol/notify.rs` | `SessionTracker` icon fields: `last_icon`, `last_icon_change`, `icon_change_count`, `idle_notified`, `last_idle_notif_at` | All animation-tracking state |
| `protocol/notify.rs` | `on_osc_title_change()`, `on_idle_timeout()` | Icon-based idle detection logic |
| `protocol/notify.rs` | `match_idle_rule()`, `glob_match()` | Per-app rule matching |
| `protocol/notify.rs` | `IDLE_NOTIF_COOLDOWN` | Workaround for rapid icon flapping |
| `protocol/dispatcher.rs` | `watch_osc_title()` idle timer logic | Replaced by semantic event handling |

The `NotificationEngine` itself stays — it still handles `process.finished` (via proc-polling),
`session.exited`, and `session.crashed`. Only the icon-based idle detection is removed.

---

## 3. Architecture Decisions

### 3.1 Overall data flow

Splitty has a daemon architecture: the backend manages PTY sessions that persist independently
of client connections. Clients connect/disconnect over WebSocket. This is fundamentally different
from native terminals (Ghostty, Alacritty) where the terminal process and PTY lifetime are the
same, and it drives most of the architectural decisions below.

The end-to-end data flow for OSC 133:

```
PTY output bytes
  │
  ├──► OscScanner (backend, byte stream tap)
  │      │
  │      ├──► SemanticState update (backend reducer, per session)
  │      │     └── survives client disconnects, served on attach
  │      │
  │      └──► MSG_OSC133 on WebSocket (parsed event, to connected clients)
  │             └── client reducer builds local UI state
  │
  └──► raw bytes forwarded to xterm.js (untouched)
         └── frontend could ALSO parse via registerOscHandler(133, ...)
             (client's choice — two channels available)
```

Key principle: **events on the wire, state on connect.** The backend sends parsed OSC 133
transitions as lightweight events. On reconnect, it serves the current aggregated state as
a snapshot. The client initializes from the snapshot and applies live events from there —
classic event sourcing with snapshots.

### 3.2 Where does parsing live?

**Decision: Backend — extend the existing `OscScanner`.**

Reasoning:
- The backend already taps the PTY byte stream for OSC 0/2 titles (`session.rs:301-308`).
  OSC 133 is the same pattern — a lightweight FSM scanning for specific byte sequences.
- The raw bytes still flow through to xterm.js untouched. The backend is not consuming or
  modifying the stream — it's tapping it, same as it does for titles today.

**Why the backend must be involved (daemon architecture):**
Splitty sessions outlive client connections. Consider: a user starts `cargo build`, closes
their browser tab, and the build finishes 2 minutes later. `133;D;0` is emitted into the PTY.
If only the frontend parsed OSC 133, nobody would see it — the event is lost. The backend must
watch the stream so it can maintain state and serve it when the client reconnects.

**This is NOT building a terminal emulator on the backend.** A headless terminal emulator
(vte, termwiz, alacritty\_terminal) maintains full screen state: cursor position, cell grid,
character attributes, scroll regions — O(rows x cols) complexity, tens of thousands of lines
of code. `OscScanner` is a byte stream scanner: a small FSM (~120 lines) that picks out
specific escape sequences. Adding OSC 133 is ~50 more lines of FSM logic. The spectrum:

```
  OscScanner (where we are)              headless emulator (not needed)
  ─────────────────────────────────────────────────────────────────────
  Scans for specific sequences           Full screen state tracking
  O(1) memory, ~170 lines               O(rows*cols), ~50,000+ lines
  Title + semantic events                Everything: cursor, scroll, attrs
```

### 3.3 What does the backend send to clients?

**Decision: Forward parsed OSC 133 events as structured messages. No invented protocol.**

The backend sends minimal, standard-faithful event messages:

```json
{"type": "osc133", "id": "session-1", "action": "A"}
{"type": "osc133", "id": "session-1", "action": "B"}
{"type": "osc133", "id": "session-1", "action": "C"}
{"type": "osc133", "id": "session-1", "action": "D", "exitCode": 0}
```

These are just structured representations of the standard OSC 133 sequences — not a new
proprietary protocol layered on top.

Reasoning against alternatives considered:

- **Why not invented notifications?** Translating `133;D` → `Notification { kind: "app.idle" }`
  is redundant — it invents a proprietary protocol on top of something already standardized.
  The client should decide what "idle" means to it. Different clients may want different
  thresholds, different UI treatments, or no notification at all.
- **Why not state snapshots on every change?** Sending full `SemanticState` on each transition
  is just events with extra bytes. The event stream is leaner and lets the client build
  whatever derived state it needs.
- **Why not let the frontend parse the byte stream directly?** It could — xterm.js has
  `parser.registerOscHandler(133, ...)` and the raw bytes are already flowing there. But
  the backend already parses the stream for titles, and having structured messages on the
  WebSocket is easier for UI code (toasts, badges, sounds) than reaching into xterm.js
  parser internals. This is consistent with how OSC 0/2 titles are already forwarded as
  `MSG_TITLE`. **The client can choose which source to use** — both channels are available.

### 3.4 What about reconnection?

**Decision: Backend sends aggregated state snapshot on connect. No event replay.**

When a client attaches to a session, the response includes the current `SemanticState`:

```json
{
  "type": "attached",
  "id": "session-1",
  // ... existing fields ...
  "semanticState": {
    "phase": "idle",
    "phaseSince": 1711799501000,
    "lastExitCode": 0,
    "commandCount": 5
  }
}
```

The client initializes its reducer from this snapshot. From that point forward, live
`MSG_OSC133` events drive state transitions.

Reasoning against event replay:
- **Unbounded history**: How far back do you replay? Last command? Last 100? Since session
  start? There's no obvious cutoff. State is O(1), replay history grows unbounded.
- **Already computed**: The backend maintains `SemanticState` for its own needs (it's the
  backend reducer's output). Serializing it on connect is free — no replay queue, no buffer
  management, no additional storage.
- **Fragility**: If the event log is truncated (ring buffer, memory pressure), replay produces
  wrong state silently. Live-maintained state is always correct because it's updated on every
  event.

### 3.5 Where does the reducer live?

**Decision: Both sides. Backend for durability, client for interpretation.**

```
Backend reducer (Rust, per session):
  SemanticAction events ──► SemanticState { phase, phase_since, last_exit_code, command_count }
  │
  ├── Survives client disconnects
  ├── Serves as snapshot on reconnect
  └── Drives server-side concerns (e.g. desktop notifications when no client connected)

Client reducer (TypeScript, per terminal):
  MSG_OSC133 messages ──► local UI state { show toast? update badge? play sound? }
  │
  ├── Initializes from backend snapshot on connect
  ├── Runs same state machine as backend
  └── Owns all UI/UX decisions
```

The client needs a reducer regardless — it receives events on the WebSocket and must turn
them into UI state. The backend needs a reducer regardless — it must maintain state across
disconnects. Neither is redundant; they serve different purposes.

This is the standard event sourcing with snapshots pattern: don't replay from the beginning
of time, restore the latest snapshot and replay from there.

### 3.6 Relationship to existing proc-polling

**Decision: Keep both. They're complementary.**

- **Proc-polling** (`watch_title` / `on_proc_update`): Watches foreground process changes via
  `/proc` and `tcgetpgrp`. Detects `process.finished` when the foreground process returns to
  the base shell. Works without shell integration scripts.
- **OSC 133**: Shell reports command boundaries. Provides exit codes, precise timing, and
  semantic zones. Requires shell integration scripts to be active.

They serve different purposes and have different availability. OSC 133 is strictly better
when available, but proc-polling is the universal fallback when shell integration isn't active
(e.g. non-bash/zsh shells, SSH sessions, tmux).

---

## 4. Implementation Plan

### Step 1: Extend `OscScanner` to parse OSC 133

**File: `pty/osc.rs`**

Add a new event type and extend the FSM to recognize `ESC ] 133 ; {action} [; params] BEL/ST`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticAction {
    PromptStart,                        // A
    InputStart,                         // B
    OutputStart,                        // C
    CommandFinished { exit_code: Option<i32> }, // D;N
}
```

The scanner needs to:
1. Recognize the digit sequence `1`, `3`, `3` after `GotBracket` (currently only handles single
   digits `0` and `2`)
2. Parse the payload: first char after `;` is the action letter, optional `;`-separated params
3. For action `D`, parse the optional exit code from the params
4. Emit `SemanticAction` events alongside existing title events

Add a separate watch channel for semantic events (like the existing `osc_tx`/`osc_rx` for titles).

### Step 2: Add `SemanticState` to session tracking

**File: `pty/mod.rs`** (new types)

```rust
pub enum SessionPhase {
    Unknown,    // no OSC 133 seen yet (shell integration not active)
    Prompt,     // after A
    Input,      // after B
    Executing,  // after C
    Idle,       // after D
}

pub struct SemanticState {
    pub phase: SessionPhase,
    pub phase_since: Instant,
    pub last_exit_code: Option<i32>,
    pub command_count: u32,
}
```

**File: `pty/session.rs`**

- Add `semantic_state: Mutex<SemanticState>` to `Session`
- Add a watch channel for semantic events (similar to `osc_tx`/`osc_rx`)
- In `Session::read()`, when `OscScanner` emits a `SemanticAction`, update `SemanticState`
  and push through the watch channel
- Add `semantic_state()` and `semantic_watch()` to `SessionHandle` trait

### Step 3: Forward events to clients

**File: `protocol/messages.rs`**

Add a new message type constant:
```rust
pub const MSG_OSC133: &str = "osc133";
```

The event message uses the existing `Response` struct or a lightweight dedicated struct with
`msg_type`, session `id`, `action` (string: "A"/"B"/"C"/"D"), and optional `exit_code`.

**File: `protocol/dispatcher.rs`**

Add a `watch_semantic` background task (similar to `watch_osc_title`) that:
- Watches the session's semantic event channel
- Sends `MSG_OSC133` messages to connected clients
- Updates `NotificationEngine` if needed (for server-side notifications when no client
  is connected)

### Step 4: Include semantic state in attach response

**File: `protocol/dispatcher.rs`** (attach handler)

When building the `MSG_ATTACHED` response, include the current `SemanticState` from the session.
This is the client's initialization snapshot.

### Step 5: Remove the icon-sniffing machinery

See the table in Section 2 for the complete list. Key changes:

- **`protocol/notify.rs`**: Remove `IdleRule`, `extract_leading_icon()`, icon tracking fields
  from `SessionTracker`, `on_osc_title_change()`, `on_idle_timeout()`, `match_idle_rule()`,
  `glob_match()`, `IDLE_NOTIF_COOLDOWN`. Keep `NotificationEngine` for process/session
  notifications — just remove the idle detection path.
- **`config/settings.rs`**: Remove `AppIdleRule`, `app_idle_rules`, `app_idle`. Keep other
  notification settings.
- **`protocol/dispatcher.rs`**: Simplify `watch_osc_title()` — it no longer needs idle timers.
  It may become unnecessary entirely if semantic events are handled by `watch_semantic`.
  The OSC title forwarding (for tab titles) can move into the title watcher or the new
  semantic watcher.

### Step 6: Shell integration scripts

Create minimal shell integration scripts that emit OSC 133 sequences:

**`shell-integration/bash/splitty.bash`** (~25 lines):
- `precmd`: emit `\e]133;D;$?\a` (end previous command) + `\e]133;A\a` (prompt start)
- Patch PS1 to include `\e]133;B\a` (input start) after the prompt
- `preexec`: emit `\e]133;C\a` (output start)

**`shell-integration/zsh/splitty-integration`** (~25 lines):
- Same hooks via `precmd_functions` and `preexec_functions`

**Injection**: Set `SPLITTY_RESOURCES_DIR` environment variable when spawning sessions
(in `session.rs`, next to `SPLITTY_SESSION_ID`), and source the appropriate script.
Follow the same pattern Ghostty uses — set env vars, source from the resources dir.

---

## 5. Verification

### Unit tests

- `pty/osc.rs`: Test OSC 133 parsing for all actions (A, B, C, D with/without exit code),
  BEL and ST terminators, interleaved with regular data and OSC 0/2 sequences
- `pty/session.rs` or `pty/mod.rs`: Test `SemanticState` transitions (A→Prompt, B→Input,
  C→Executing, D→Idle, with exit code tracking and command counting)
- `protocol/notify.rs`: Verify the icon-sniffing tests are removed and existing
  `process.finished`/`session.exited`/`session.crashed` tests still pass

### Integration tests

- Spawn a PTY session, send raw OSC 133 byte sequences through the PTY, verify:
  - `SemanticState` transitions correctly
  - `MSG_OSC133` events arrive at the client
  - Attach response includes correct `semanticState` snapshot
- Verify shell integration scripts emit correct sequences (can test by inspecting
  PTY output from a bash/zsh session with the integration sourced)

### Manual testing

- Start a Splitty session with shell integration active
- Run a command (e.g. `sleep 2 && echo done`)
- Verify semantic events flow to the frontend
- Disconnect and reconnect — verify state snapshot is correct
- Run a failing command — verify exit code is captured
- Run Claude Code — verify idle detection works without any `AppIdleRule` config

---

## 6. Migration / Backwards Compatibility

- `app_idle` and `app_idle_rules` are removed from `NotificationSettings`. Existing config
  files that contain these fields will be silently ignored by serde (the fields simply won't
  be present in the deserialized struct). No migration needed.
- Clients that don't understand `MSG_OSC133` messages will ignore them (standard WebSocket
  message handling). No breaking change.
- Sessions without shell integration (e.g. non-bash/zsh shells, SSH sessions, tmux) will
  have `SemanticState { phase: Unknown }`. The frontend can fall back to proc-polling based
  indicators in this case. This is strictly better than the current situation where those
  sessions also have no idle detection.
