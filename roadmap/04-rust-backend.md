# Milestone 4: Rust Backend — DONE

> **Voraussetzung:** [Milestone 1 — Multiplexer Backend](01-multiplexer-backend.md) (Protokoll ist stabil, Ops und Sync-Flows werden nicht mehr grundlegend umgebaut)
>
> Das Go-Backend dient als Prototyping-Umgebung für das Multiplexer-Protokoll. Der Rust-Rewrite kommt erst, wenn man anfängt Features draufzubauen statt Grundlagen umzubauen.

## Why Rust

### 1. Cross-platform PTY via `portable-pty`
Currently we use `creack/pty` (Go, Unix-only) and need platform-specific code for process detection (`procinfo_linux.go`, `procinfo_darwin.go`). Windows is not supported.

`portable-pty` (from the wezterm project) provides:
- Unified API for Unix PTY and Windows ConPTY
- `process_group_leader()` — foreground process PID detection built-in
- Battle-tested (powers wezterm)

We still need OS-specific code to read process name/cmdline/cwd from a PID, but the PTY layer itself becomes cross-platform.

### 2. Desktop & Web: Tauri-first, one frontend, multiple shells

Splitty has been built as a server/client architecture from day one — multiplexing, persistent sessions, remote connections, the protocol. The web frontend implements the full feature set and serves as the base for all targets.

#### Three targets, one frontend

- **Web** — `splitty-server` serves the frontend over WebSocket. This is the product.
- **Tauri** — First-class desktop app. Backend runs in-process (direct function calls instead of WebSocket — same protocol, but "native transport"). Custom window title bar, desktop-specific features (file explorer etc.) are built here first.
- **Electron** — Desktop wrapper for the web app. Bundles `splitty-server` as a sidecar, connects via WebSocket — identical to the web target, just with its own window. No custom title bar, no desktop-specific features initially. Exists because of a concrete tradeoff: WebKitGTK (Tauri's webview on Linux) renders scrolling and animations noticeably less smoothly. Electron ships Chromium: bigger bundle, slower startup, but at runtime **significantly** smoother. For a terminal app where every frame counts, that's the deciding factor.

**Priority:** Tauri gets the attention. Desktop-specific features are Tauri-only in the early phase. Electron catches up later but is not highest priority — most features are part of the protocol anyway and therefore automatically available everywhere.

#### Transport abstraction

Same protocol, two transports:

| Target | Transport | Latency |
|--------|-----------|---------|
| Web | WebSocket via `splitty-server` | Network |
| Electron | WebSocket via `splitty-server` (localhost sidecar) | ~0 |
| Tauri | Direct function calls into `splitty-core` | Zero (same process) |

The frontend needs a thin abstraction layer that hides the transport — Tauri uses `invoke()`, Web/Electron use WebSocket. Same messages, same protocol, just a different channel.

### 3. Unified codebase, no network boundary (nur relevant falls native Client kommt)
Currently: Go backend (PTY, sessions, templates) ↔ WebSocket ↔ TypeScript frontend

Falls ein nativer Client jemals gebaut wird, ermöglicht Rust:
- **Local mode**: Backend traits called directly via method calls. No serialization, no WebSocket, no port.
- **Remote mode**: Same traits, but behind an RPC/WebSocket boundary for SSH multiplexing or multi-window.

## Key crates

| Crate | Purpose | Published |
|-------|---------|-----------|
| `portable-pty` | PTY creation, resize, fg process PID | Yes |
| `termwiz` | Escape sequence parser, input parser, screen buffer | Yes |
| `serde` + `serde_yaml` | Template serialization (replacing Go `yaml.Marshal`) | Yes |
| `serde` + `toml` | Config serialization (settings) | Yes |
| `tokio` | Async runtime for multiplexed I/O | Yes |
| `axum` | HTTP/WS server (splitty-server only) | Yes |
| `tracing` | Structured logging | Yes |
| `thiserror` | Error type derivation | Yes |
| `uuid` | ID generation | Yes |

## What doesn't exist

- **No multiplexing crate.** Wezterm's `mux` is internal and tightly coupled. We build our own split-tree + session management (as we do now in Go/TS).
- **No cross-platform process info crate.** Reading process name/cmdline/cwd from a PID remains OS-specific (`/proc` on Linux, `sysctl`/`libproc` on macOS, `NtQueryInformationProcess` on Windows).

---

## Architecture

### Crate structure

```
crates/
├── splitty-core/          # Everything without framework dependencies
│   │                      # No axum, no network. Only tokio (channels/spawn), serde, portable-pty.
│   │                      # Tauri imports core directly. Server imports core and exposes it over HTTP/WS.
│   └── src/
│       ├── lib.rs
│       ├── ids.rs                    # PaneId, ScreenId, SessionId, NodeId, ClientId, FloatingId (newtypes)
│       │
│       ├── mux/
│       │   ├── mod.rs
│       │   ├── state.rs              # WorkspaceState, ScreenState, LayoutNode, SessionEntry, PaneState
│       │   ├── ops.rs                # Op, ScreenOp, Ownership, Axis, Direction, FloatBounds
│       │   ├── reducer.rs            # reduce_workspace(), reduce_screen()
│       │   ├── validate.rs           # validate()
│       │   ├── effects.rs            # Effect enum
│       │   ├── effect_runner.rs      # EffectRunner (concrete struct, calls PtyManager)
│       │   ├── store.rs              # Store (std::sync::RwLock, dispatch, queries, attach/detach)
│       │   └── grid.rs              # project_grid(), find_neighbor() (2D spatial)
│       │
│       ├── pty/
│       │   ├── mod.rs
│       │   ├── session_handle.rs     # SessionHandle trait
│       │   ├── session.rs            # Session (impl SessionHandle)
│       │   ├── backend.rs            # Backend trait
│       │   ├── manager.rs            # Manager (registry, impl Backend)
│       │   ├── procinfo.rs           # process_info() with #[cfg(target_os)] per platform
│       │   ├── osc.rs                # OscScanner (title extraction state machine)
│       │   ├── ringbuffer.rs         # RingBuffer (scrollback)
│       │   └── startup.rs            # Shell command injection, restore modes
│       │
│       ├── protocol/
│       │   ├── mod.rs
│       │   ├── messages.rs           # IncomingMessage, OutgoingMessage, ClientCommand, Notification
│       │   ├── dispatcher.rs         # Dispatcher (routes messages to pty/mux)
│       │   └── notify.rs             # NotificationEngine, SessionTracker, idle detection
│       │
│       ├── template/
│       │   ├── mod.rs
│       │   ├── model.rs              # Template, Pane, TemplateLayoutNode, validation
│       │   ├── store.rs              # TemplateStore trait
│       │   └── file_store.rs         # FileStore (YAML on disk)
│       │
│       ├── config/
│       │   ├── mod.rs
│       │   └── settings.rs           # Settings structs, TOML load/save, defaults
│       │
│       └── auth/
│           ├── mod.rs
│           └── token.rs              # Token generation, validation, file persistence
│
└── splitty-server/
    └── src/
        ├── main.rs                   # CLI, server startup, signal handling, graceful shutdown
        ├── http/
        │   ├── mod.rs
        │   ├── routes.rs             # Axum router, all route registration
        │   ├── middleware.rs          # Auth (token), CORS, host validation
        │   ├── api.rs                # REST endpoints (settings, templates, webapps, mux state, stats)
        │   └── proxy.rs              # Reverse proxy for localhost:{port}
        │
        └── ws/
            ├── mod.rs
            ├── handler.rs            # WebSocket upgrade, client lifecycle, message routing
            ├── mux_handler.rs        # Per-client mux state (attach/detach/op/resync/disconnect cleanup)
            ├── codec.rs              # Binary frame encode/decode
            └── sender.rs             # ClientSender (WebSocket message writer)
```

### ID types (newtype wrappers)

All IDs are newtype wrappers over `String` for type safety. Prevents accidentally passing a `PaneId` where a `ScreenId` is expected.

```rust
// ids.rs — macro-generated newtypes
// PaneId, ScreenId, SessionId, NodeId, ClientId, FloatingId
// All: Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Display
// All: ::new() generates UUID v4
```

### Mux layer: Nested Reducer

The Go backend's mux layer has a structural problem: workspace-level ops (`create_screen`, `close_screen`, `rename_screen`) contain reducer logic that lives inside Store methods instead of pure functions. The Store mixes locking, state transitions, and map mutations in the same methods. Screen-level ops go through a clean pure reducer, but workspace-level ops bypass it entirely.

In Rust, we fix this from day one with a **nested reducer** architecture (à la Redux `combineReducers`).

#### State types

```rust
pub struct WorkspaceState {
    pub screens: HashMap<ScreenId, ScreenState>,
    pub active_screen_id: Option<ScreenId>,
}

pub struct ScreenState {
    pub id: ScreenId,
    pub name: String,
    pub revision: u64,
    pub ownership: Ownership,
    pub root: LayoutNode,
    pub floating: Vec<FloatingEntry>,
    pub panes: HashMap<PaneId, PaneState>,
    pub sessions: HashMap<SessionId, SessionEntry>,
    pub last_client_id: Option<ClientId>,
}

// Rust enum with serde tag — serializes as {"type": "slot", ...} on the wire
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayoutNode {
    Slot { id: NodeId, pane_ids: Vec<PaneId>, active_pane_id: PaneId },
    Split { id: NodeId, axis: Axis, children: Vec<LayoutNode>, sizes: Vec<f64> },
}

pub struct PaneState {
    pub id: PaneId,
    pub session_id: SessionId,
    pub title: String,
    pub cwd: String,
    pub role: PaneRole,  // Terminal, Webview, Viewer
}

// "Entry" not "State" — it's a data record in the session table, not a stateful object
pub struct SessionEntry {
    pub id: SessionId,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub shell: Option<String>,
    pub status: SessionStatus,  // Starting, Running, Exited
    pub exit_code: Option<i32>,
}
```

#### Operations

Two-level enum hierarchy. All variants are imperative commands.

```rust
pub enum Op {
    // Delegates to reduce_screen() — "modify this screen with this operation"
    ModifyScreen { screen_id: ScreenId, op: ScreenOp },

    // Workspace-level ops — handled directly in reduce_workspace()
    CreateScreen { name: Option<String>, ownership: Option<Ownership>,
                   screen_id: Option<ScreenId>, new_pane_id: Option<PaneId> },
    CloseScreen { screen_id: ScreenId },
    RenameScreen { screen_id: ScreenId, name: String },
    SetActiveScreen { screen_id: ScreenId },
    MovePaneToScreen { pane_id: PaneId, source: ScreenId, target: ScreenId,
                       direction: Option<Direction> },
}

pub enum ScreenOp {
    SplitPane { pane_id: PaneId, axis: Axis, new_pane_index: u8,
                new_pane_id: Option<PaneId>, resulting_sizes: Option<Vec<f64>> },
    ClosePane { pane_id: PaneId, resulting_sizes: Option<Vec<f64>> },
    ResizeSplit { node_id: NodeId, sizes: Vec<f64> },
    MovePane { pane_id: PaneId, target_slot_id: Option<NodeId>,
               direction: Option<Direction>, target_split_id: Option<NodeId>,
               source_resulting_sizes: Option<Vec<f64>>, resulting_sizes: Option<Vec<f64>> },
    FloatPane { pane_id: PaneId, bounds: Option<FloatBounds> },
    DockPane { floating_id: FloatingId, pane_id: PaneId },
    MoveFloat { floating_id: FloatingId, bounds: FloatBounds },
    SetOwnership { ownership: Ownership },
}
```

#### Reducer (pure functions)

```rust
fn reduce_workspace(state: WorkspaceState, op: Op) -> Result<(WorkspaceState, Vec<Effect>)> {
    match op {
        Op::ModifyScreen { screen_id, op } => {
            // delegate to screen sub-reducer
            let screen = state.screens.remove(&screen_id)?;
            let (new_screen, effects) = reduce_screen(screen, op)?;
            state.screens.insert(screen_id, new_screen);
            Ok((state, effects))
        }
        Op::CreateScreen { .. } => { /* workspace-level, pure */ }
        Op::CloseScreen { .. } => { /* workspace-level, pure */ }
        Op::MovePaneToScreen { .. } => {
            // Cross-screen: has access to both screens, just another match arm
        }
        // ...
    }
}

fn reduce_screen(state: ScreenState, op: ScreenOp) -> Result<(ScreenState, Vec<Effect>)> {
    match op {
        ScreenOp::SplitPane { .. } => { /* ... */ }
        ScreenOp::ClosePane { .. } => { /* ... */ }
        // ...
    }
}
```

#### Store (pure infrastructure)

Store's only job: lock → reduce → store → unlock. Zero business logic.

```rust
pub struct Store {
    state: std::sync::RwLock<WorkspaceState>,  // std, not tokio — reducer is fast, CPU-bound
    attached_clients: std::sync::RwLock<HashMap<ScreenId, ClientId>>,
    default_ownership: Ownership,
}

impl Store {
    pub fn dispatch(&self, op: Op) -> Result<Vec<Effect>> {
        let mut state = self.state.write().unwrap();
        let (new_state, effects) = reduce_workspace(std::mem::take(&mut *state), op)?;
        *state = new_state;
        Ok(effects)
    }

    // Queries (read lock)
    pub fn get_screen(&self, id: &ScreenId) -> Option<ScreenState>;
    pub fn get_workspace_state(&self) -> WorkspaceState;
    pub fn session_id_for_pane(&self, pane_id: &PaneId) -> Option<SessionId>;

    // Single-attach policy
    pub fn attach_client(&self, screen_id: &ScreenId, client_id: &ClientId) -> Result<()>;
    pub fn detach_client(&self, screen_id: &ScreenId, client_id: &ClientId);
    pub fn detach_all_screens(&self, client_id: &ClientId);
}
```

#### Effects

Effects are pure data, not executables. The EffectRunner interprets them.

```rust
pub enum Effect {
    SpawnSession { session_id: SessionId, pane_id: PaneId, screen_id: ScreenId,
                   command: Option<String>, cwd: Option<String>, shell: Option<String> },
    KillSession { session_id: SessionId },
}

// Concrete struct, not a trait — server and tauri both just call pty::Manager
pub struct EffectRunner {
    pty: Arc<pty::Manager>,
    server_addr: Option<String>,  // None in Tauri
    token: Option<String>,
}
```

### PTY layer

```rust
// pty::SessionHandle — trait for a single PTY session
pub trait SessionHandle: Send + Sync {
    fn id(&self) -> &str;
    fn read(&self, buf: &mut [u8]) -> io::Result<usize>;
    fn write(&self, data: &[u8]) -> io::Result<()>;
    fn resize(&self, cols: u16, rows: u16) -> io::Result<()>;
    fn close(&self) -> io::Result<()>;
    fn done(&self) -> &tokio::sync::watch::Receiver<Option<i32>>;
    fn title(&self) -> TitleInfo;
    fn osc_title(&self) -> OscTitleInfo;
    fn scrollback(&self) -> Vec<u8>;
}

// pty::Backend — trait for session registry
pub trait Backend: Send + Sync {
    fn create(&self, opts: CreateSessionOpts) -> Result<Arc<dyn SessionHandle>>;
    fn get(&self, id: &str) -> Option<Arc<dyn SessionHandle>>;
    fn close(&self, id: &str) -> Result<()>;
    fn close_all(&self);
}

// pty::Manager — concrete implementation of Backend
pub struct Manager { .. }

// pty::Session — concrete implementation of SessionHandle
// Uses portable-pty for cross-platform PTY
// RingBuffer for scrollback, OscScanner for title extraction
pub struct Session { .. }

// Process info: one file, #[cfg(target_os)] per platform
pub fn process_info(pid: u32) -> Option<ProcessInfo>;
```

### Protocol layer

Wire protocol is **100% identical** to the Go backend. Same JSON message types, same fields, same binary frame format. The Svelte frontend works without changes.

```rust
// Incoming (client → server), tagged by "type" field
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IncomingMessage {
    Create { cols: u16, rows: u16, command: Option<String>, dir: Option<String>, .. },
    Attach { id: SessionId, cols: u16, rows: u16 },
    Detach { id: SessionId },
    Input { id: SessionId, data: String },  // base64-encoded
    Resize { id: SessionId, cols: u16, rows: u16 },
    Close { id: SessionId },
    #[serde(rename = "mux.connect")] MuxConnect { client_id: Option<ClientId> },
    #[serde(rename = "mux.attach")] MuxAttach { screen_id: ScreenId },
    #[serde(rename = "mux.detach")] MuxDetach { screen_id: ScreenId },
    #[serde(rename = "mux.op")] MuxOp { screen_id: ScreenId, op: serde_json::Value },
    #[serde(rename = "mux.resync")] MuxResync { screen_id: ScreenId },
}

// Outgoing (server → client)
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutgoingMessage {
    Hello { client_id: ClientId },
    Created { id: SessionId, home_dir: String },
    Output { id: SessionId, data: String },
    Closed { id: SessionId, exit_code: Option<i32> },
    Title { id: SessionId, title: String, cwd: String, cmdline: String, base_shell: bool, osc_title: String },
    Error { id: Option<SessionId>, error: String },
    Command { command: ClientCommand },
    Notification { kind: String, severity: String, title: String, body: String, .. },
    #[serde(rename = "mux.welcome")] MuxWelcome { client_id: ClientId, screens: Vec<ScreenInfo> },
    #[serde(rename = "mux.screen_snapshot")] MuxScreenSnapshot { screen: ScreenState },
    #[serde(rename = "mux.nudge")] MuxNudge { reason: String, screen_id: ScreenId },
    #[serde(rename = "mux.busy")] MuxBusy { screen_id: ScreenId },
}

// Binary frame (terminal output, high throughput)
// [1 byte: session ID length][N bytes: session ID][payload]
```

### Notification engine (in core)

Must be in core because Tauri calls core directly — no server in between.

```rust
pub struct NotificationEngine {
    trackers: HashMap<SessionId, SessionTracker>,
    buffers: HashMap<ScreenId, NotificationBuffer>,  // last 50, 1h TTL
    idle_rules: Vec<AppIdleRule>,
}

impl NotificationEngine {
    pub fn on_title_update(&mut self, session_id: &SessionId, info: &TitleInfo) -> Vec<Notification>;
    pub fn on_session_exit(&mut self, session_id: &SessionId, exit_code: i32) -> Option<Notification>;
    pub fn replay(&self, screen_id: &ScreenId, since: Option<&ClientId>) -> Vec<Notification>;
}
```

### Transport abstraction (ClientSender)

```rust
// In core — transport-agnostic
pub trait ClientSender: Send + Sync {
    fn send_message(&self, msg: &OutgoingMessage) -> Result<()>;
    fn send_binary(&self, session_id: &str, data: &[u8]) -> Result<()>;
}

// In splitty-server: WebSocket implementation
// In splitty-tauri: Tauri IPC implementation (later)
```

### Error handling

Per-layer error types with `thiserror`:

```rust
pub enum MuxError {
    PaneNotFound(PaneId),
    ScreenNotFound(ScreenId),
    NodeNotFound(NodeId),
    InvalidSizes { expected: usize, got: usize },
    LastPane,
    AlreadyFloating,
    ScreenBusy(ClientId),
    // ...
}

pub enum PtyError {
    SpawnFailed(io::Error),
    SessionNotFound(String),
    WriteFailed(io::Error),
    // ...
}
```

### Config

- **Settings**: TOML (`~/.config/splitty/config.toml`) — flat structure, Rust ecosystem convention
- **Templates**: YAML (`~/.config/splitty/templates/*.yaml`) — deeply nested, better fit

---

## Migration strategy

The Go backend is ~6.5k lines production code (~8k including tests), with the bulk in `internal/mux/` (~3k lines). Migration is incremental — Go backend stays running, Svelte frontend connects to whichever backend is active.

### Phase 1: Core foundation (parallel to Go backend)

```
Cargo.toml + go.mod coexist at repo root.
Go backend remains the active backend while Rust is built.
```

1. ~~**IDs + state types** — `ids.rs`, `mux/state.rs`, `mux/ops.rs`, `mux/effects.rs`~~ ✅
2. ~~**Reducer + validation + grid** — `mux/reducer.rs`, `mux/validate.rs`, `mux/grid.rs`~~ ✅
3. ~~**Store** — `mux/store.rs`~~ ✅
4. **PTY layer** — `pty/session.rs`, `pty/manager.rs`, `pty/procinfo.rs`, `pty/osc.rs`, `pty/ringbuffer.rs`, `pty/startup.rs` (using `portable-pty`)
5. **Template system** — `template/model.rs`, `template/file_store.rs` (straightforward serde)
6. **Config + auth** — `config/settings.rs`, `auth/token.rs`
7. **Notification engine** — `protocol/notify.rs`
8. **Protocol messages** — `protocol/messages.rs` (serde types matching wire format)
9. **Effect runner** — `mux/effect_runner.rs` (bridges reducer to pty)
10. **Dispatcher** — `protocol/dispatcher.rs` (wires protocol to pty + mux)

> **Status (2026-03-26):** Phase 1 + Phase 2 complete. 170 tests passing. Go backend removed. Tauri desktop app scaffolded. Switchover done.

### Phase 2: Server (replaces Go backend)

1. **HTTP layer** — Axum routes, auth middleware, REST endpoints
2. **WebSocket layer** — Handler, MuxHandler, binary codec
3. **Integration testing** — Svelte frontend connects to Rust backend
4. **Switchover** — Remove Go backend

### Phase 3: Desktop targets (later)

1. **splitty-tauri** — Tauri commands wrapping core, window management
2. **electron** — Thin shell spawning splitty-server
