/**
 * Splitty Protocol Specification
 *
 * Defines ALL data types exchanged between Client and Server.
 * This file is the canonical reference — both Go backend and TypeScript
 * frontend are derived from these types.
 *
 * Transport: Single WebSocket connection per client.
 * Encoding: JSON (binary frames for terminal output may come later).
 *
 * Architecture: Single-Attach. One client per screen at a time.
 * The client is autonomous for layout decisions. The server is the
 * source of truth for persistent state.
 */

// ============================================================================
// 0. GLOSSARY
//
// These terms have precise meanings in Splitty. Do not use them interchangeably.
//
// Session    A PTY process (shell, nvim, htop). Lives on the server.
// Pane       A UI viewport in the layout. References a Session.
// Slot       Leaf node in the layout tree. Contains 1..n Panes (stack).
// Split      Branch node in the layout tree (horizontal or vertical).
// Screen     Top-level container. Own layout tree + ownership. ATTACH TARGET.
// Workspace  All Screens together. The complete server-side state.
//
// "Attach" always means attaching a client to a SCREEN, never to a Session.
// A Session is an implementation detail inside a Pane.
// ============================================================================

// ============================================================================
// 1. DOMAIN MODEL
//
// The domain model describes the server-side state. The client renders it
// via dockview but never persists it — the server owns this data.
// ============================================================================

// --- Ownership ---

/**
 * Determines what happens to sessions when the attached client disconnects.
 *
 * - "client-owned": Sessions die with the client (default for desktop app).
 * - "server-owned": Sessions survive client disconnect (default for `splitty serve` / remote).
 *
 * Set per Screen, not per Session.
 */
type OwnershipMode = "client-owned" | "server-owned";

// --- Layout Tree ---

/**
 * A node in the layout tree. Either a Split (branch) or a Slot (leaf).
 *
 * ```
 * Screen "dev"
 *   └── Split H [0.5, 0.5]
 *       ├── Slot [pane-editor]
 *       └── Split V [0.6, 0.4]
 *           ├── Slot [pane-backend, pane-logs]   ← stack (multiple panes)
 *           └── Slot [pane-shell]
 * ```
 */
type LayoutNode = SplitNode | SlotNode;

interface SplitNode {
  type: "split";
  id: string;
  /** Split direction. "horizontal" = children side by side, "vertical" = stacked top/bottom. */
  axis: "horizontal" | "vertical";
  children: LayoutNode[];
  /** Relative sizes, must sum to 1.0. Length matches children. */
  sizes: number[];
}

interface SlotNode {
  type: "slot";
  id: string;
  /** Pane IDs in this slot. Multiple = tabbed stack. At least 1. */
  paneIds: string[];
  /** Which pane is visible in the stack. Defaults to first. */
  activePaneId?: string;
}

// --- Floating ---

/**
 * A floating window overlay. Contains its own layout tree (can be a
 * single Slot or a full Split tree).
 */
interface FloatingEntry {
  id: string;
  root: LayoutNode;
  /** Relative position/size within the screen viewport (0-1). */
  bounds: { x: number; y: number; w: number; h: number };
}

// --- Pane & Session ---

/**
 * A Pane is a UI viewport in the layout. It references a Session (the
 * underlying PTY process). Pane ≠ Session: a Pane is layout, a Session
 * is a process. This separation enables future features like session
 * reuse and detached sessions.
 */
interface PaneState {
  id: string;
  jobId?: string;
  sessionId?: string;
  title?: string;
  cwd?: string;
  role?: "terminal" | "webview" | "viewer";
}

/**
 * A Session is a PTY process managed by the server.
 */
/** Session launch environment (not client-supplied protocol fields):
 * TERM=xterm-256color, COLORTERM=truecolor, SPLITTY_SESSION_ID and, when bound,
 * SPLITTY_PANE_ID/SPLITTY_SCREEN_ID. Ordinary sessions get SPLITTY_SOCK; agent jobs
 * get only SPLITTY_AGENT_SOCK, never reusable credentials. If an installation-local
 * CLI is available, PATH includes its directory and SPLITTY_CLI_DIR names that
 * non-secret directory for supported shell startup. Inherited hints are replaced
 * or removed; no shell config/global install or already-running shell is modified.
 */
interface SessionState {
  id: string;
  jobId?: string;
  restartPolicy?: "shell" | "never";
  shellProfileId?: string;
  command?: string;
  cwd?: string;
  shell?: string;
  status: "starting" | "running" | "exited";
  exitCode?: number;
  /**
   * The session's scratchpad (session-scratchpad), at most 64 KiB of UTF-8;
   * absent while empty. Only screen snapshots over the WebSocket carry it:
   * GET /api/mux/state and /api/mux/screen/{id} leave it out.
   */
  scratchpad?: string;
}

// --- Screen (top-level) ---

/**
 * Complete state of a single Screen. The server holds one ScreenState
 * per screen. This is sent as a snapshot on mux.resync.
 */
interface ScreenState {
  id: string;
  name?: string;
  revision: number;

  root: LayoutNode;
  floating: FloatingEntry[];

  panes: Record<string, PaneState>;
  sessions: Record<string, SessionState>;
  /** Snapshot-only projection; persisted authoritative jobs are workspace-wide. */
  jobs?: Record<string, Pick<ManagedJobRecord, "jobId" | "state" | "error" | "result">>;
}

/**
 * Top-level server state: multiple screens, one active.
 */
interface WorkspaceState {
  screens: Record<string, ScreenState>;
  activeScreenId?: string;
  screenOrder: string[];
  /** Absent in workspaces written before managed jobs. */
  jobs?: Record<string, ManagedJobRecord>;
}

interface JobLaunch {
  requestId: string;
  argv: string[];
  cwd: string;
  name?: string;
  placement:
    | { kind: "screen" }
    | { kind: "tab"; screenId?: string; paneId?: string }
    | { kind: "split"; screenId?: string; paneId?: string; direction: "left" | "right" | "above" | "below" };
  newScreenId?: string;
  newPaneId?: string;
  background?: true;
}

/** Public intent; the server adds trusted principal metadata before reduction.
 * Lifecycle facts and owner changes cannot be submitted as arbitrary mux ops. */
type ManagedJobOp =
  | ({ type: "create_job" } & JobLaunch)
  | { type: "stop_job" | "forget_job"; jobId: string };

/** Persisted metadata only: no running process handles, output or grant sockets. */
interface ManagedJobRecord {
  jobId: string;
  launch: JobLaunch;
  screenId: string;
  paneId: string;
  sessionId: string;
  createdBy: string | null;
  owner: string | null;
  createdAt: number;
  startedAt: number | null;
  endedAt: number | null;
  state: "starting" | "running" | "stopping" | "exited" | "failed" | "interrupted";
  result: { exitCode: number | null; signal: string | null };
  error: string | null;
  forgotten: boolean;
}

/** POST /api/jobs -> 201 after a spawn, 200 for successful idempotent replay.
 * GET /api/jobs -> {capabilities: ["managed-jobs-v1"], jobs: ManagedJob[]}.
 * GET /api/jobs/{jobId}; POST .../stop return ManagedJob.
 * POST .../input takes {data: string}, at most 16 KiB; POST .../forget takes {}.
 * All are available on trusted TCP/owner sockets and filtered agent sockets. */
interface ManagedJob {
  jobId: string; requestId: string; screenId: string; paneId: string; sessionId: string;
  argv: string[]; cwd: string; name: string | null;
  createdBy: string | null; owner: string | null;
  createdAt: number; startedAt: number | null; endedAt: number | null;
  state: ManagedJobRecord["state"];
  exitCode: number | null; signal: string | null; error: string | null;
}

/** GET /api/jobs/{jobId}/logs?cursor=...|tail=...&limit=...&waitMs=...&format=text|raw */
interface JobLogs {
  data: string; encoding: "utf8" | "base64";
  nextCursor: string | null; oldestCursor: string | null; endCursor: string | null;
  lostBytes: number; hasMore: boolean; state: ManagedJobRecord["state"];
  outputComplete: boolean; outputAvailable: boolean;
}

/** Owner Unix socket only: POST /api/agents {name, anchor?}; GET /api/agents
 * returns {agents: AgentGrant[]}; GET /api/agents/{agentId}; POST .../revoke {}.
 * POST /api/jobs/{jobId}/owner {agentId: string|null} transfers/reclaims a job. */
interface AgentGrant {
  agentId: string; name: string; socket: string;
  anchor: {screenId: string; paneId: string} | null; revoked: boolean;
}

interface JobControlError {
  code: string; error: string;
  requestId?: string; jobId?: string; screenId?: string; paneId?: string; sessionId?: string;
}

// ============================================================================
// 2. CLIENT → SERVER MESSAGES
//
// Sent by the client over WebSocket. Two categories:
// - Session messages: PTY I/O (input, resize, create, close)
// - Mux messages: Layout operations (split, move, etc.)
// ============================================================================

// --- Session Messages (existing, unchanged) ---

/** Request a new PTY session. Server responds with SessionCreated. */
interface SessionCreate {
  type: "create";
  /** Preferred backend shell profile. Takes precedence over legacy `shell`. */
  shellProfileId?: string;
  shell?: string;
  cols?: number;
  rows?: number;
  command?: string;
  dir?: string;
  root?: string;
  restoreMode?: string;
  /** Dockview panel ID. Injected as SPLITTY_PANE_ID into the session's env. */
  panelId?: string;
  /** Screen ID. Injected as SPLITTY_SCREEN_ID into the session's env. */
  screenId?: string;
}

/** Attach to an existing PTY session (resubscribe to output). */
interface SessionAttach {
  type: "attach";
  id: string;
  cols?: number;
  rows?: number;
}

/** Detach from a PTY session (stop receiving output, keep session alive). */
interface SessionDetach {
  type: "detach";
  id: string;
}

/** Send keyboard input to a PTY session. */
interface SessionInput {
  type: "input";
  id: string;
  data: string;
}

/**
 * The size at which this connection shows a PTY session. A server announcing
 * "pty-size-v1" records it and sets the PTY to the size its policy derives from
 * all connections that show the session (`terminal.size_policy`: "active",
 * "latest" or "smallest"), and answers every resize with SessionSize; older
 * servers set the PTY to it. cols and rows from 1.
 */
interface SessionResize {
  type: "resize";
  id: string;
  cols: number;
  rows: number;
  /**
   * "pty-size-v1": the first report after an attach. When the PTY size does not
   * change, the server resizes the PTY by one row and back, so its processes
   * redraw after the replay (older servers: the client sends both resizes).
   */
  redraw?: boolean;
}

/**
 * "pty-size-v1": this connection's user acted (a key, a pointer button, the
 * window focused or resized; not scrolling). With `takeover`, every other
 * connection of the server is idle until its own next activity. A connection
 * that never sends activity or idle counts as acting at each of its resizes.
 */
interface SessionActivity {
  type: "activity";
  takeover?: boolean;
}

/** "pty-size-v1": this connection is not in use (its window is hidden) until its next activity. */
interface SessionIdle {
  type: "idle";
}

/** "pty-size-v1": this connection keeps the session but does not show it, until its next resize. */
interface SessionHide {
  type: "hide";
  id: string;
}

/** Close/kill a PTY session. */
interface SessionClose {
  type: "close";
  id: string;
}

/**
 * Store an image a user pasted or dropped into a pane on that pane's own
 * server, which answers ImageSaved or ImageFailed. At most 10 MiB of PNG,
 * JPEG, GIF or WebP. The server picks the file's name and stores it 0600 in
 * `splitty/paste/` of its cache directory; the client pastes the path.
 */
interface SaveImage {
  type: "save_image";
  /** Echoed by the answer. */
  requestId: string;
  /** The image, standard base64. */
  data: string;
}

/**
 * One chunk of a file a user dropped or pasted into a pane, for that pane's
 * own server, which answers every chunk with UploadProgress, UploadSaved or
 * UploadFailed. The client sends the next chunk only after the answer to the
 * last. The first chunk (offset 0) also carries `name` and `size`. The server
 * stores the file 0600 in a new `upload-<UTC time>-<hex>/` directory of
 * `splitty/paste/` in its cache directory, under `name` made safe (last
 * component; characters other than letters, digits, `.`, `-`, `_` become
 * `_`; no leading dots; at most 120 bytes; else `file`). At most 1 MiB of
 * data per chunk, 512 MiB per file, 4 uploads per connection at once; a
 * failed upload, and an unfinished one when its connection closes, is
 * removed. Older servers answer `error` "unknown message type: upload".
 */
interface Upload {
  type: "upload";
  /** Names the upload; echoed by every answer. */
  requestId: string;
  /** Where in the file `data` starts: the bytes received so far. */
  offset: number;
  /** The chunk, standard base64; empty for an empty file. */
  data: string;
  /** First chunk only: the client's name of the file. */
  name?: string;
  /** First chunk only: the file's length in bytes. */
  size?: number;
}

// --- Mux Messages (new in Milestone 1) ---

/**
 * Sent once on WebSocket connect. Initiates the handshake.
 * If clientId is provided (from localStorage), server checks if this
 * client was the last one attached → determines if snapshot is needed.
 */
interface MuxConnect {
  type: "mux.connect";
  /** From localStorage. Omit on first-ever visit. */
  clientId?: string;
}

/**
 * Fire-and-forget layout operation. Client has already applied the
 * change locally — this informs the server so it can update its state.
 *
 * The server does NOT respond to this message (no ACK, no reject).
 * At Single-Attach there are no conflicts to handle.
 */
interface MuxOp {
  type: "mux.op";
  screenId: string;
  op: Operation;
}

/**
 * Attach to a specific screen. Sent after receiving mux.welcome.
 * Server responds with mux.screen_snapshot for this screen.
 *
 * Single-Attach: if another client is already attached to this screen,
 * server responds with mux.busy instead.
 *
 * The client decides which screens to attach to (typically all available
 * ones, but could be selective — e.g. `splitty attach backend`).
 */
interface MuxAttach {
  type: "mux.attach";
  screenId: string;
}

/**
 * Detach from a specific screen. Sessions in server-owned screens
 * keep running. Sessions in client-owned screens are killed.
 */
interface MuxDetach {
  type: "mux.detach";
  screenId: string;
}

/**
 * Explicit request for a full state snapshot of a screen.
 * Used when the client knows its local state is stale (e.g. after
 * receiving a mux.nudge from a CLI-initiated change).
 */
interface MuxResync {
  type: "mux.resync";
  screenId: string;
}

type ClientMessage =
  | SessionCreate
  | SessionAttach
  | SessionDetach
  | SessionInput
  | SessionResize
  | SessionActivity
  | SessionIdle
  | SessionHide
  | SessionClose
  | SaveImage
  | Upload
  | MuxConnect
  | MuxAttach
  | MuxDetach
  | MuxOp
  | MuxResync;

// ============================================================================
// 3. SERVER → CLIENT EVENTS
//
// All server→client messages are part of a single logical event stream,
// discriminated by `type`. Every JSON event has `id` (unique event ID for
// dedup) and `timestamp` (Unix ms). Session-scoped events add `sessionId`.
//
// Terminal output is the only exception: it uses binary WebSocket frames
// for performance (see Section 9: Wire Format).
//
// Design principle: the server sends facts. The frontend decides
// presentation (severity, toast vs badge, display text, etc.).
// ============================================================================

// --- Base types (TypeScript-only, not on the wire) ---

/** Base shape of every JSON server event. */
interface ServerEvent {
  type: string;
  /** Unique event ID for dedup/idempotency. Format: "e-" + 16 hex chars. */
  id: string;
  /** Unix milliseconds when the event was created. */
  timestamp: number;
}

/** Base for events scoped to a terminal session. */
interface SessionEvent extends ServerEvent {
  sessionId: string;
}

// --- Session Lifecycle Events ---

/** PTY session was created successfully. */
interface SessionCreated extends SessionEvent {
  type: "created";
  shellProfileId?: string;
  homeDir?: string;
  /** Always "unknown" for newly created sessions. */
  semanticPhase?: string;
}

/** Reattached to an existing PTY session. */
interface SessionAttached extends SessionEvent {
  type: "attached";
  homeDir?: string;
  /** Current OSC 133 phase: "unknown" | "prompt" | "input" | "executing" | "idle". */
  semanticPhase?: string;
  /** Exit code from the last completed command (OSC 133 D). */
  semanticExitCode?: number;
  /** Total number of commands completed in this session. */
  semanticCommandCount?: number;
}

/**
 * PTY output data.
 *
 * Logically part of the event stream but transported as binary WebSocket
 * frames for performance. See Section 9: Wire Format.
 */
interface SessionOutput {
  type: "output";
  sessionId: string;
  data: Uint8Array;
}

/** PTY session ended (process exited or was killed). */
interface SessionClosed extends SessionEvent {
  type: "closed";
  exitCode?: number;
  signal?: string;
}

/** Error related to a session operation. */
interface SessionError extends ServerEvent {
  type: "error";
  sessionId?: string;
  error: string;
}

/** A SaveImage was stored: the absolute path of its file. */
interface ImageSaved extends ServerEvent {
  type: "image_saved";
  requestId: string;
  path: string;
}

/** A SaveImage was refused (not an image, too large, not stored); nothing was stored. */
interface ImageFailed extends ServerEvent {
  type: "image_failed";
  requestId: string;
  error: string;
}

/** An Upload chunk was stored; `received` bytes of the file have arrived. */
interface UploadProgress extends ServerEvent {
  type: "upload_progress";
  requestId: string;
  received: number;
}

/** The last Upload chunk arrived: the absolute path of the stored file. */
interface UploadSaved extends ServerEvent {
  type: "upload_saved";
  requestId: string;
  path: string;
}

/** The upload ended (out of order, too large, not stored); nothing of it is kept. */
interface UploadFailed extends ServerEvent {
  type: "upload_failed";
  requestId: string;
  error: string;
}

// --- Session Stream Events (derived from PTY) ---

/** Process title/cwd changed (polled ~1Hz via /proc, event-driven via OSC 0/2). */
interface TitleChanged extends SessionEvent {
  type: "title";
  title: string;
  cwd: string;
  cmdline?: string;
  baseShell: boolean;
  /** OSC 0/2 window title (e.g. "✻ Claude Code"). */
  oscTitle?: string;
}

/**
 * OSC 133 semantic prompt event.
 * Emitted when the shell reports a prompt/input/output/command-finished boundary.
 */
interface SemanticBoundary extends SessionEvent {
  type: "osc133";
  /** Semantic action: "A" (prompt), "B" (input), "C" (output), "D" (finished). */
  action: "A" | "B" | "C" | "D";
  /** Exit code of the finished command (only present when action is "D"). */
  exitCode?: number;
}

/** Terminal bell (BEL character, 0x07). */
interface Bell extends SessionEvent {
  type: "bell";
}

/**
 * "pty-size-v1": the session's PTY size as the server's size policy set it.
 * Sent to every connection streaming the session when it changes or the policy
 * is reconfigured, on attach before the replay, after `created`, and in answer
 * to each of a connection's resizes (also when unchanged). Clients show the
 * session at this size and ignore one outside 1–1000 columns or 1–500 rows.
 */
interface SessionSize extends SessionEvent {
  type: "size";
  cols: number;
  rows: number;
  policy: "active" | "latest" | "smallest";
}

// --- Process Events ---
//
// These are first-class event types — NOT wrapped in a notification envelope.
// The frontend derives severity, display text, and presentation from the
// event data. The server sends facts, not presentation decisions.

/** Foreground process returned to shell (baseShell false→true). */
interface ProcessFinished extends SessionEvent {
  type: "process.finished";
  process: string;
  cmdline?: string;
}

/** Shell command finished — app is waiting for input (detected via OSC 133 D). */
interface ProcessIdle extends SessionEvent {
  type: "process.idle";
  process: string;
  exitCode?: number;
}

/**
 * PTY session exited. Covers both clean exits and crashes.
 * Frontend derives severity from exitCode (0 = info, nonzero = error).
 */
interface SessionExited extends SessionEvent {
  type: "session.exited";
  exitCode: number;
  process?: string;
  command?: string;
  userInitiated: boolean;
}

// --- Generic Notification (fallback) ---

type NotificationSeverity = "info" | "success" | "warning" | "error";

/**
 * Generic notification for cases where no specific event type exists.
 * Example: "Server shutting down", "Disk space low", etc.
 *
 * For process/session events, use the specific types above instead.
 */
interface NotificationMessage extends ServerEvent {
  type: "notification";
  severity: NotificationSeverity;
  title: string;
  body?: string;
  sessionId?: string;
}

// --- Mux Events ---

/**
 * Sent on WebSocket connect. Tells the client:
 * - Its assigned clientId (store in localStorage for reconnect)
 * - Available screens with their attach status
 *
 * The client requests mux.resync for the listed screens. Terminal attach is
 * a separate session-protocol operation; there is no screen ownership.
 */
interface MuxWelcome extends ServerEvent {
  type: "mux.welcome";
  clientId: string;
  screens: ScreenInfo[];
  serverMode?: "embedded" | "daemon";
  capabilities?: string[]; // includes "managed-jobs-v1", "editor-open-v1", "file-offer-v1", "pty-size-v1", "scratchpad-v1", "layout-arrange-v1" and "tab-select-v1"; absent on old servers
}

/** Summary of a screen (sent in mux.welcome — NOT the full state). */
interface ScreenInfo {
  id: string;
  name?: string;
}

/**
 * Full state snapshot of a single screen.
 * Sent to the requesting client in response to mux.resync, including on reload.
 * Job panes carry lifecycle metadata and never imply a new shell launch.
 */
interface MuxScreenSnapshot extends ServerEvent {
  type: "mux.screen_snapshot";
  screen: ScreenState;
}

/**
 * Nudge: server tells the client that something changed externally
 * (e.g. a CLI command modified the layout). Client should request
 * a resync (mux.resync) to get the updated state.
 *
 * This is intentionally NOT a full state push — keeps the protocol
 * simple. Client decides when to resync.
 */
interface MuxNudge extends ServerEvent {
  type: "mux.nudge";
  reason: string; // includes cli_op and mux_op
  screenId?: string;
}

/**
 * A WebSocket op was rejected, sent to its client only: for an op that names a
 * pane to create (`paneId`, so the client stops waiting for mux.pane_spawned)
 * and for `arrange_panes` (no `paneId`; the window resyncs `screenId`).
 */
interface MuxOpFailed extends ServerEvent {
  type: "mux.op_failed";
  op: string;
  screenId?: string;
  paneId?: string;
  error: string;
}

/**
 * Reject: another client is already attached to this screen.
 * Only relevant for server-owned screens (Single-Attach enforcement).
 */
interface MuxBusy extends ServerEvent {
  type: "mux.busy";
  screenId: string;
  /** Info about the currently attached client (for UX). */
  attachedSince?: string; // ISO timestamp
}

// --- Listening Ports ---

/** A TCP port a process of a session listens on. */
interface ListeningPort {
  port: number;
  sessionId: string;
  /** The process's name (`/proc/<pid>/comm`), such as "node" or "python3". */
  process: string;
}

/**
 * The TCP ports processes of the server's sessions (each shell and all its
 * descendants) listen on, on loopback (127.0.0.0/8, ::1, v4-mapped loopback)
 * or every address (0.0.0.0, ::): the full list, ordered by port, each port
 * once (for the lowest session id). Sent right after mux.welcome, also when
 * the list is empty, and to every client whenever the list changes; the
 * server checks every 2 seconds while a client is connected.
 *
 * Linux only: other servers never send it. Not a mux.* message. Through the
 * hub (section 10) it is one of the host daemon's relayed messages. A client
 * ignores server messages of a type it does not know.
 */
interface PortsChanged extends ServerEvent {
  type: "ports.changed";
  ports: ListeningPort[];
}

// --- Server-Initiated Commands ---

/** Server-initiated command for the client to execute (e.g. from context menu). */
interface ServerCommand extends ServerEvent {
  type: "command";
  command: ClientCommand;
}

// --- Union ---

type ServerMessage =
  // Session lifecycle
  | SessionCreated
  | SessionAttached
  | SessionOutput
  | SessionClosed
  | SessionError
  | ImageSaved
  | ImageFailed
  | UploadProgress
  | UploadSaved
  | UploadFailed
  // Session stream
  | TitleChanged
  | SemanticBoundary
  | Bell
  | SessionSize
  // Process events
  | ProcessFinished
  | ProcessIdle
  | SessionExited
  // Notification (generic fallback)
  | NotificationMessage
  // Mux
  | MuxWelcome
  | MuxScreenSnapshot
  | MuxNudge
  | MuxBusy
  // Listening ports
  | PortsChanged
  // Commands
  | ServerCommand;

// ============================================================================
// 4. OPERATIONS
//
// Operations describe layout mutations. They are sent inside MuxOp
// messages (client-initiated, fire-and-forget) or triggered by CLI
// commands (server-initiated, results in MuxNudge to client).
//
// Client-initiated ops include concrete layout values (sizes, positions)
// because the client's UI library (dockview) computed them.
// Server-initiated ops (from CLI) may omit these values — the server
// applies its own default policy.
// ============================================================================

interface SplitPanelOp {
  type: "split_panel";
  panelId: string;
  axis: "horizontal" | "vertical";
  /** Client-set ID for the new panel. Server generates one if omitted (e.g. CLI-initiated). */
  newPanelId?: string;
  /** Client provides the resulting sizes after split. Server uses default (equal) if omitted. */
  resultingSizes?: number[];
  /** Position of the new panel in the children array. */
  newPanelIndex?: number;
  /** Explicit profile, otherwise inherit the source pane's profile. */
  shellProfileId?: string;
}

interface ClosePanelOp {
  type: "close_panel";
  panelId: string;
  /** How siblings redistribute space. Server uses default (equal) if omitted. */
  resultingSizes?: number[];
}

/** Create a terminal tab in the source pane's slot, including floating slots. */
interface NewTabOp {
  type: "new_tab";
  paneId: string;
  /** Client-set pane ID; the server generates one when omitted. */
  newPaneId?: string;
  /** Defaults to the source pane's known directory, then its session's directory. */
  cwd?: string;
  /** Explicit profile, otherwise inherit the source session's profile and legacy shell. */
  shellProfileId?: string;
}

interface ResizeSplitOp {
  type: "resize_split";
  nodeId: string;
  /** New sizes for this split node's children. Must sum to 1.0. */
  sizes: number[];
}

/**
 * Make `paneId` the active tab of the slot that holds it, in the root layout
 * or a floating entry (servers with "tab-select-v1"). A window sends it for a
 * group whose shown tab is not the one the server has; nothing else changes.
 * Rejected for a pane in no slot, without mux.op_failed; applied without
 * mux.nudge to other clients.
 */
interface SelectTabOp {
  type: "select_tab";
  paneId: string;
}

/**
 * Move a pane within the root layout: into the slot `targetSlotId`, or beside
 * the node or pane `targetSplitId` in `direction`. Rejected when the target is
 * missing, is no slot (for `targetSlotId`), or is gone once the pane is taken
 * out. Windows send `arrange_panes` for drags instead.
 */
interface MovePaneOp {
  type: "move_pane";
  paneId: string;
  /** Target: existing slot (tab into) */
  targetSlotId?: string;
  /** Target: split direction from a reference node or pane; required with `direction`. */
  targetSplitId?: string;
  direction?: "right" | "below" | "left" | "above";
  /** Resulting sizes at destination. */
  resultingSizes?: number[];
  /** Resulting sizes at source (after panel left). */
  sourceResultingSizes?: number[];
}

/** A layout tree of `arrange_panes`: a LayoutNode without node IDs. */
type Arrangement =
  | { type: "slot"; paneIds: string[]; activePaneId?: string }
  | { type: "split"; axis: "horizontal" | "vertical"; children: Arrangement[]; sizes: number[] };

/**
 * Replace the screen's root layout with the window's grid after a drag or a
 * divider move (servers with "layout-arrange-v1"). Pane IDs the screen does not
 * hold are left out; then it must hold every pane of the root layout exactly
 * once and no floating pane, every slot a pane, every split two or more
 * children with sizes as for resize_split. The server assigns new node IDs;
 * no pane, session or floating entry changes. Rejected: mux.op_failed to the
 * sender, which resyncs. Applied: mux.nudge for the screen to other clients.
 */
interface ArrangePanesOp {
  type: "arrange_panes";
  root: Arrangement;
}

interface CreateScreenOp {
  type: "create_screen";
  /** Client-set screen ID. Server generates one if omitted. */
  screenId?: string;
  name?: string;
  /** Client-set ID for the initial pane. Server generates one if omitted. */
  newPanelId?: string;
  /** Explicit profile, otherwise use the backend default. */
  shellProfileId?: string;
  ownership?: OwnershipMode;
}

// --- Shell Profile HTTP API (`GET|PUT /api/shell-profiles`) ---

type ShellTarget = "local" | "windowsInterop";

interface ShellProfile {
  id: string;
  name: string;
  command: string;
  args: string[];
  enabled: boolean;
  target: ShellTarget;
  source: "detected" | "custom";
  available: boolean;
  reason?: string;
  experimental: boolean;
}

interface ShellProfilesState {
  defaultProfileId: string;
  profiles: ShellProfile[];
}

interface CloseScreenOp {
  type: "close_screen";
  screenId: string;
}

interface SetActiveScreenOp {
  type: "set_active_screen";
  screenId: string;
}

interface RenameScreenOp {
  type: "rename_screen";
  screenId: string;
  name: string;
}

/**
 * Move a pane with its running session from the envelope's screen to another
 * screen of the same server, or to a new one placed right after the source.
 * No session starts or ends; a source left without panes is removed. Not for
 * a managed job's pane. Other clients get `mux.nudge` for both screens.
 */
interface MovePaneToScreenOp {
  type: "move_pane_to_screen";
  paneId: string;
  targetScreenId: string;
  /** Create `targetScreenId` instead of moving into an existing screen. */
  createScreen?: boolean;
  /** The new screen's name; only with `createScreen`. */
  screenName?: string;
}

/**
 * Replace a session's scratchpad with `text` (empty clears it). Only a window
 * sends it, over the WebSocket; POST /api/mux/op answers 400. Refused for an
 * unknown session or more than 64 KiB of UTF-8. Other clients get `mux.nudge`
 * for the screen; the sender gets nothing.
 */
interface SetScratchpadOp {
  type: "set_scratchpad";
  sessionId: string;
  text: string;
}

interface SetScreenOwnershipOp {
  type: "set_screen_ownership";
  screenId: string;
  ownership: OwnershipMode;
}

type Operation =
  | SplitPanelOp
  | NewTabOp
  | ClosePanelOp
  | ResizeSplitOp
  | SelectTabOp
  | MovePaneOp
  | ArrangePanesOp
  | CreateScreenOp
  | CloseScreenOp
  | SetActiveScreenOp
  | RenameScreenOp
  | MovePaneToScreenOp
  | SetScratchpadOp
  | SetScreenOwnershipOp;

// ============================================================================
// 5. CLIENT COMMANDS
//
// Server-initiated actions that the client should execute locally.
// Two use cases:
//
// 1. Context menu actions triggered via native menus (Wails desktop app).
// 2. CLI layout commands (split, close, focus) — the CLI sends a
//    ClientCommand via POST /api/command, the server broadcasts it to
//    the attached WS client, and the client applies it in dockview then
//    mirrors the result as a mux.op. See Flow 4a.
//
// Why CLI uses ClientCommands instead of direct mux ops:
// The client owns dockview panel IDs. A split needs dockview to create
// the new panel and assign its ID. The client then tells the server the
// resulting panel ID via a mux.op with newPanelId (client-set ID pattern).
// ============================================================================

type ClientCommand =
  | { type: "pane.activate"; panelId?: string }
  | { type: "pane.copy"; panelId?: string }
  | { type: "pane.paste"; panelId?: string }
  | { type: "pane.clear"; panelId?: string }
  | { type: "pane.close"; panelId?: string }
  | { type: "pane.split"; panelId?: string; direction: "right" | "below" };

// ============================================================================
// 6. CONNECTION LIFECYCLE & FLOWS
//
// Not types — behavioral spec. Describes the sequences of messages
// for key scenarios.
// ============================================================================

/**
 * FLOW 1: Fresh Connect (first visit, no existing screens)
 *
 *   Client                          Server
 *     │                               │
 *     │──── [WebSocket open] ────────>│
 *     │<─── hello { clientId }        │  (legacy, kept for compat)
 *     │──── mux.connect {}            │  (no clientId — first visit)
 *     │<─── mux.welcome {            │
 *     │       clientId: "abc",        │
 *     │       screens: []             │  (no screens exist yet)
 *     │     }                         │
 *     │                               │
 *     │  Client stores clientId in localStorage.
 *     │  Client creates screens locally via mux.op { create_screen }.
 *     │  Client sends mux.ops fire-and-forget as user works.
 *     │                               │
 *     │──── mux.op { create_screen }  │
 *     │──── create { ... }            │  (PTY session)
 *     │<─── created { id }            │
 *     │──── mux.op { split_panel }    │  (fire-and-forget)
 *     │     ...                       │
 */

/**
 * FLOW 2: Reconnect — same client, no one else attached in between
 *
 *   Client                          Server
 *     │                               │
 *     │──── [WebSocket open] ────────>│
 *     │<─── hello { clientId }        │
 *     │──── mux.connect {             │
 *     │       clientId: "abc"         │  (from localStorage)
 *     │     }                         │
 *     │<─── mux.welcome {            │
 *     │       clientId: "abc",        │
 *     │       screens: [              │
 *     │         { id: "s1",           │
 *     │           lastClientId: "abc",│  ← same! client can trust local state
 *     │           attached: false }   │
 *     │       ]                       │
 *     │     }                         │
 *     │                               │
 *     │  Client sees lastClientId == own clientId for screen "s1".
 *     │  → Keeps local dockview state, no snapshot needed.
 *     │  → Just re-attaches to the screen and reconnects PTY streams.
 *     │                               │
 *     │──── mux.attach { screenId: "s1" }
 *     │                               │  (server sees lastClientId match → no snapshot)
 *     │──── attach { id: "sess-1" }   │  (re-subscribe to PTY output)
 *     │<─── attached { id: "sess-1" } │  (scrollback replay follows)
 *     │<─── output { data: ... }      │  (buffered output since disconnect)
 */

/**
 * FLOW 3: Reconnect — different client was attached in between
 *
 *   Client                          Server
 *     │                               │
 *     │──── mux.connect {             │
 *     │       clientId: "abc"         │
 *     │     }                         │
 *     │<─── mux.welcome {            │
 *     │       clientId: "abc",        │
 *     │       screens: [              │
 *     │         { id: "s1",           │
 *     │           lastClientId: "xyz",│  ← different! local state is stale
 *     │           attached: false }   │
 *     │       ]                       │
 *     │     }                         │
 *     │                               │
 *     │  Client sees lastClientId != own clientId for screen "s1".
 *     │  → Must load server state.
 *     │                               │
 *     │──── mux.attach { screenId: "s1" }
 *     │<─── mux.screen_snapshot {     │  (server sends full screen state)
 *     │       screen: { ... }         │
 *     │     }                         │
 *     │                               │
 *     │  Client discards local state for this screen.
 *     │  Client rebuilds dockview layout from snapshot.
 *     │  Client attaches to all PTY sessions listed in the screen.
 */

/**
 * FLOW 4a: CLI layout commands (split, close, focus) via ClientCommand
 *
 * Layout commands go through the client because the client owns panel IDs
 * (dockview assigns them). The CLI tells the client WHAT to do, and the
 * client does it autonomously, then mirrors the result as a mux.op.
 *
 *   Client                CLI               Server
 *     │                    │                   │
 *     │                    │── POST /api/command
 *     │                    │   { type: "pane.split",
 *     │                    │     direction: "right" }
 *     │                    │                   │── broadcasts to WS client
 *     │                    │<── 200 { sent: 1 }│
 *     │<── command {       │                   │
 *     │      type: "pane.split",               │
 *     │      direction: "right"                │
 *     │    }               │                   │
 *     │                    │                   │
 *     │  Client executes split in dockview.
 *     │  Client generates new panel ID ("terminal-5").
 *     │  Client mirrors result to server:
 *     │                    │                   │
 *     │── mux.op {         │                   │
 *     │     split_panel,   │                   │
 *     │     panelId: "terminal-3",             │
 *     │     newPanelId: "terminal-5",          │
 *     │     axis: "horizontal"                 │
 *     │   }                │                   │
 */

/**
 * FLOW 4b: CLI workspace commands (new-screen, switch-screen) via mux.op
 *
 * Workspace commands don't need the client to execute first — the server
 * can create screens and apply effects (SpawnSession) directly.
 *
 *   CLI                              Server
 *     │                                │
 *     │── POST /api/mux/op            │
 *     │   { screenId: "_",            │
 *     │     op: { type: "create_screen",
 *     │           name: "backend" } } │
 *     │                                │── Reducer creates screen
 *     │                                │── SpawnSession effect
 *     │                                │── Nudge attached clients
 *     │<── 200 { ok, revision }       │
 */

/**
 * FLOW 5: Two clients, different screens (Single-Attach per Screen)
 *
 *   Client A is attached to screen "backend".
 *   Client B connects and wants screen "frontend".
 *
 *   Client B                         Server
 *     │                               │
 *     │──── mux.connect {             │
 *     │       clientId: "xyz"         │
 *     │     }                         │
 *     │<─── mux.welcome {            │
 *     │       clientId: "xyz",        │
 *     │       screens: [              │
 *     │         { id: "backend",      │
 *     │           attached: true },   │  ← Client A is here
 *     │         { id: "frontend",     │
 *     │           attached: false }   │  ← free
 *     │       ]                       │
 *     │     }                         │
 *     │                               │
 *     │──── mux.attach {              │
 *     │       screenId: "frontend"    │  ← attaches to free screen, works fine
 *     │     }                         │
 *     │<─── mux.screen_snapshot {}    │
 *     │                               │
 *     │──── mux.attach {              │
 *     │       screenId: "backend"     │  ← tries busy screen
 *     │     }                         │
 *     │<─── mux.busy {               │  ← rejected
 *     │       screenId: "backend"     │
 *     │     }                         │
 *     │                               │
 *     │  Client B works on "frontend". Client A works on "backend".
 *     │  Both are connected to the same server simultaneously.
 */

/**
 * FLOW 6: Client disconnect behavior (Ownership Model)
 *
 *   Client disconnects (WebSocket closes)
 *     │
 *     Server checks each screen the client was attached to:
 *     │
 *     ├── client-owned screens:
 *     │     → Kill all PTY sessions in these screens
 *     │     → Remove screens from state
 *     │     → Same as today's behavior (Connection.Close())
 *     │
 *     └── server-owned screens:
 *           → Detach PTY output streams (stop sending to this client)
 *           → Sessions keep running
 *           → Scrollback continues to buffer in RAM
 *           → Screen state preserved, marked as not attached
 *           → lastClientId updated to this client's ID
 */

// ============================================================================
// 7. HTTP API (for CLI)
//
// CLI commands use HTTP, not WebSocket (one-shot, no persistent connection).
// All endpoints require token auth.
// ============================================================================

/**
 * POST /api/mux/op
 *
 * Submit a layout operation. Same Operation types as MuxOp.
 * Server applies it through the Reducer, then sends mux.nudge
 * to any attached client.
 *
 * Request:
 *   { screenId: string, op: Operation }
 *
 * Response:
 *   200 { ok: true, revision: number }
 *   400 { ok: false, error: string }     // validation error
 *   404 { ok: false, error: string }     // screen/panel not found
 */

/**
 * GET /api/mux/state
 *
 * Returns the full WorkspaceState. Used by `splitty list-panes`,
 * `splitty list-screens`, etc.
 *
 * Response:
 *   200 WorkspaceState
 */

/**
 * GET /api/mux/screen/:id
 *
 * Returns a single ScreenState.
 *
 * Response:
 *   200 ScreenState
 *   404 { error: "screen not found" }
 */

/**
 * POST /api/command
 *
 * Send a ClientCommand to all connected WS clients. Used by CLI for
 * layout commands (split, close, focus) that need client-side execution.
 * The client applies the change in dockview and mirrors the result as
 * a mux.op. See Flow 4a.
 *
 * Request:
 *   ClientCommand (e.g. { type: "pane.split", direction: "right" })
 *
 * Response:
 *   200 { ok: true, sent: number }    // number of clients that received it
 */

/**
 * GET /api/mux/neighbor?screen=ID&pane=ID&dir=left|right|up|down
 *
 * Spatial neighbor query using the 2D grid projection of the layout tree.
 * Used by `splitty focus` to resolve directional pane navigation.
 *
 * Response:
 *   200 { paneId: string }            // "" if no neighbor found
 */

// ============================================================================
// 8. ARCHITECTURAL PATTERNS
//
// Not types — design decisions that shape the protocol's semantics.
// ============================================================================

/**
 * 8.1 Client-Set IDs
 *
 * Panel and screen IDs are generated by the client (dockview assigns them,
 * e.g. "terminal-1", "screen-3"). The client includes them in mux.ops via
 * the newPanelId and screenId fields. The server uses them as-is.
 *
 * If omitted (e.g. CLI-initiated ops), the server generates IDs.
 *
 * This eliminates round-trip latency: the client creates a dockview panel
 * and assigns its ID before the server even processes the op.
 */

/**
 * 8.2 Shadow Mode (Phase A)
 *
 * Current operational mode. Dockview remains the client's source of truth.
 * The server tracks layout state in parallel via fire-and-forget mux.ops.
 *
 * Key properties:
 * - No ACK for accepted mux.ops; the server answers only with the
 *   mux.pane_spawned events an op causes, and with mux.op_failed when it
 *   rejects an op that would create a pane or an arrange_panes
 * - Every op runs its effects, from the WebSocket as from the CLI
 *   (POST /api/mux/op): the server spawns and kills pane sessions, and the
 *   client attaches to a spawned session after mux.pane_spawned
 * - Every connected client may change every screen; ops apply in arrival
 *   order and other clients learn of them through mux.nudge
 *
 * Future: When moving to server-authoritative mode (multi-client), the
 * server would start sending ACK/REJECT responses and broadcasting ops
 * to other clients. The op FORMAT stays the same — only the delivery
 * semantics change.
 */

/**
 * 8.3 Ops Model (not Events/Commands)
 *
 * Layout changes are Operations — typed structs that serve as both intent
 * and fact. There is no CQRS-style separation between Commands and Events.
 *
 * The flow is Redux-like:
 *   Op → Validate → Reduce (pure) → new State + Effects
 *
 * An op is always "I want this to happen." Whether it's applied optimistically
 * (client) or authoritatively (server) doesn't change the wire format.
 * A rejected op is simply an op that wasn't applied — no separate event type.
 */

/**
 * 8.4 2D Grid Projection
 *
 * The layout tree is projected into a flat list of PaneRects with relative
 * coordinates (0-1). Used by the server for spatial neighbor queries
 * (GET /api/mux/neighbor) which power `splitty focus left/right/up/down`.
 *
 * Pure function: LayoutNode → PaneRect[]. Computed on demand, not stored.
 * Uses the same 90° cone filter as the frontend's findNeighborGroup().
 */

/**
 * 8.5 Unified Event Stream
 *
 * All server→client messages are part of one logical event stream,
 * discriminated by `type`. Every JSON event carries:
 * - `id`: unique event ID (format "e-" + 16 hex chars) for dedup/idempotency
 * - `timestamp`: Unix milliseconds when the event was created
 *
 * Session-scoped events additionally carry `sessionId`.
 *
 * The server sends facts — the frontend decides presentation (severity,
 * toast vs badge, display text). Process events like `process.finished`
 * and `session.exited` are first-class event types, NOT wrapped in a
 * generic notification envelope.
 *
 * The generic `notification` type (with severity, title) is reserved as
 * a fallback for messages that have no specific semantic event type
 * (e.g. "Server shutting down").
 */

// ============================================================================
// 9. WIRE FORMAT
//
// How messages are encoded on the WebSocket transport.
// ============================================================================

/**
 * 9.1 Two Frame Types
 *
 * The WebSocket carries two types of frames:
 *
 * - Text frames: JSON-encoded ServerEvent objects.
 *   Dispatched by the frontend via `JSON.parse()` → switch on `type`.
 *
 * - Binary frames: Raw terminal output (the hot path).
 *   Format: [1 byte: session ID length] [N bytes: session ID] [payload bytes]
 *   Bypasses JSON entirely for zero-overhead PTY output forwarding.
 *
 * The frontend distinguishes them trivially:
 *
 *   ws.onmessage = (e) => {
 *     if (typeof e.data === "string") {
 *       dispatch(JSON.parse(e.data));   // ServerEvent
 *     } else {
 *       const [sessionId, bytes] = decodeBinaryFrame(e.data);
 *       terminal.write(sessionId, bytes);
 *     }
 *   };
 *
 * 9.2 Why Not All Binary?
 *
 * Binary frames work because output has no structure — raw bytes go 1:1
 * into the terminal emulator. Structured events (title, bell, osc133)
 * need JSON serialization regardless. Wrapping JSON inside a binary frame
 * adds complexity with no performance benefit.
 *
 * 9.3 Event ID and Timestamp
 *
 * Generated server-side. Event IDs use `generate_event_id()` (random,
 * not sequential). Timestamps use `SystemTime::now()` as Unix ms.
 * Output frames do NOT carry id/timestamp (binary, no JSON).
 */

// ============================================================================
// 10. REMOTE SCREEN HUB
//
// `GET /ws/remote?ticket=<ticket>&target=<[user@]host[:port]>[&update=1]`
// gives the window a WebSocket to a host's daemon through the local server.
// It needs the same Origin, Sec-Fetch-Site and single-use ticket as `/ws` and
// is not served on the control socket.
//
// `update=1` (only this value) updates the host: when its daemon already ran
// and runs another version than the local server, the hub stops it with
// `daemon stop` on the host, which ends all its sessions, and the handshake
// then starts the local server's version. A daemon of the local server's
// version, or one this socket's handshake started, is left alone. A client
// sends it only after the user confirmed what ends on the host.
//
// Before the relay, the hub sends only RemoteHubMessage text messages:
// RemoteProgress per step, then exactly one RemoteReady, or a RemoteError
// after which it closes the socket (code 1000, the error code as reason).
// After RemoteReady every message is the host daemon's own, relayed
// unchanged in both directions (text as text, binary as binary): its
// mux.welcome first, then the messages of sections 2 and 3 in the wire
// format of section 9.
// The hub sends nothing of its own any more; a client drops any `remote.*`
// message after RemoteReady. Messages the client sends before RemoteReady
// are relayed after it (at most 64).
//
// When the host side ends (the bridge or its ssh), the hub closes the socket
// with code 4001 (REMOTE_HOST_LOST): retry. Closing the socket ends the
// socket's ssh; the host's daemon and its sessions keep running. Every socket
// has an SSH connection of its own.
// ============================================================================

/** Close code of a relaying `/ws/remote` socket whose host side ended. */
const REMOTE_HOST_LOST = 4001;

/** A step before the relay, in this order. `install` only when the server is
 *  uploaded now; `update` only with `update=1` when the daemon of another
 *  version is stopped; `started` only when this socket's handshake started
 *  the daemon (after `update`, always). */
type RemoteStep = "install" | "daemon" | "update" | "started" | "bridge";

interface RemoteProgress {
  type: "remote.progress";
  step: RemoteStep;
  /** For the user, e.g. "Installing Splitty 0.1.0 on vm1 (x86_64)..." or
   *  "Stopping Splitty 0.0.9 on vm1...". */
  message: string;
}

/** Sent once, right before the host daemon's first message. */
interface RemoteReady {
  type: "remote.ready";
  /** The target as the hub validated it. */
  target: string;
  daemon: {
    version: string;
    /** Within the hub's protocol version and the one before. */
    protocol: number;
    /** Its sessions outlive it in keepers, so an update ends none; absent from an older hub: false. */
    keepsSessions?: boolean;
    /** Whether this socket's handshake started the daemon. */
    started: boolean;
  };
  /** The host's screens when the handshake ran (the reopen offer). Untrusted host data. */
  screens: { id: string; name?: string; panes: number }[];
}

/**
 * Retrying can help for `unreachable`, `daemon`, `bridge` and `internal`
 * (and close code 4001); the other codes need the user.
 */
type RemoteErrorCode =
  | "invalid_target" //   not a valid [user@]host[:port]; no ssh was started
  | "host_key" //         unknown or changed host key (hint: run `ssh <target>` once)
  | "auth" //             the host accepted no key; the hub never prompts
  | "unreachable" //      ssh could not connect (status 255)
  | "unsupported_host" // not Linux, no POSIX sh, or no server for its architecture
  //                      (with update=1: none of the local server's version to update
  //                      to; the daemon keeps running)
  | "install" //          the upload, its checksum or the unpacked server failed
  | "daemon" //           the handshake or the daemon start failed (with update=1 also:
  //                      the stop failed, or the old daemon had not ended yet)
  | "incompatible" //     the daemon's protocol is outside the window; it keeps running
  | "bridge" //           remote-bridge ended before the daemon's first message
  | "forward" //          only from /api/remote/forwards: the forward failed, or the
  //                      host's SSH configuration forwards ports itself
  | "internal"; //        a local problem (no ssh client, no free port)

interface RemoteError {
  type: "remote.error";
  code: RemoteErrorCode;
  message: string;
  /** What the user can do, where there is something. */
  hint?: string;
  /** For `incompatible`: the host daemon's version and protocol. */
  version?: string;
  protocol?: number;
  /** For `incompatible`: its sessions outlive it, so an update ends none. */
  keepsSessions?: boolean;
  /** For `incompatible`: the daemon's screens, as in `remote.ready`, which an update would end unless it keeps them. */
  screens?: { id: string; name?: string; panes: number }[];
}

type RemoteHubMessage = RemoteProgress | RemoteReady | RemoteError;

/**
 * GET /api/remote/hosts
 *
 * The known hosts of the user the server runs as, for a host picker: the
 * recent hosts (recorded when a `/ws/remote` socket or `splitty connect`
 * became ready), most recent first, then the `Host` names of `~/.ssh/config`
 * not already listed. Authenticated like the other `/api` routes; not on the
 * control socket.
 *
 * Response:
 *   200 KnownHost[]
 */
interface KnownHost {
  target: string;
  source: "recent" | "ssh-config";
  /** Display only; SSH always receives target unchanged. */
  alias?: string;
  /** For a recent host: seconds since the Unix epoch; may be absent for an
   * alias saved after the host aged out of the recent list. */
  lastConnected?: number;
}

/**
 * GET /api/remote/host-preferences
 * PUT /api/remote/host-preferences
 *     { target: string, alias?: string | null, hidden?: false }
 * DELETE /api/remote/hosts  { target: string }
 *
 * Local display names and removal tombstones in remote-host-preferences.json,
 * shared by local server and desktop shells. All routes require API auth and
 * origin/Fetch Metadata checks and are absent from the control socket.
 * GET has no side effects; missing/corrupt files yield an empty document,
 * invalid saved rows are skipped. Files are owner-only and atomically replaced.
 *
 * PUT requires at least alias or hidden:false. Omitted fields preserve their
 * previous value. Aliases are trimmed, at most 200 characters, with no control
 * characters; null or an empty trimmed string clears the alias. Targets pass
 * SSH target validation unchanged. Unknown fields are rejected. Aliases are
 * display metadata and never affect SSH routing or user selection. An alias
 * keeps a visible target discoverable after it ages out of the recent list.
 *
 * DELETE removes the target's recent entry, alias and cached screen directory,
 * and persists a hidden tombstone so ~/.ssh/config cannot reintroduce it.
 * It does not modify SSH config, remembered open screens, forwards, running
 * connections, remote sessions or processes. Hidden targets are omitted by
 * GET /api/remote/hosts and /api/remote/catalog. Catalog updates for a hidden
 * host return 409 and cannot resurrect its directory. A successful explicit
 * CLI/desktop connection unhides its target; PUT hidden:false reveals it for
 * an intentional hub connection, including when it is already running.
 * Automatic hub reconnects preserve tombstones and do not record recents.
 *
 * Metadata writes share an OS file lock across local processes, including
 * recording recents, preference changes, removal and catalog merges.
 *
 * Response:
 *   GET 200 RemoteHostPreferences
 *   PUT 200 RemoteHostPreference
 *   DELETE 204
 *   400 { error: string }   (invalid body or malformed JSON)
 *   415                     (not application/json)
 *   500 { error: string }   (metadata could not be written)
 */
interface RemoteHostPreferences {
  version: 1;
  hosts: RemoteHostPreference[];
}

interface RemoteHostPreference {
  target: string;
  alias?: string;
  hidden?: boolean;
}

/**
 * GET /api/remote/host-order
 * PUT /api/remote/host-order  { order: string[] }
 *
 * The order in which the user arranged the client's host groups in the screen
 * list, by SSH target, in remote-host-order.json beside the preferences (a
 * file of its own: the preference reader rejects unknown fields). Local
 * screens always come first; hosts not listed follow the listed ones in the
 * order their screens first appear. Same authentication, origin checks,
 * control-socket absence, lock and private atomic writes as host preferences;
 * never contacts a host.
 *
 * PUT replaces the whole order: at most 256 targets, each passing SSH target
 * validation unchanged, a repeated target kept once; anything else, a value
 * that is not a list of strings or an unknown field is refused without a
 * write. Forgetting a host (DELETE /api/remote/hosts) removes it from the
 * order. A missing or corrupt file reads as an empty order.
 *
 * Response:
 *   GET 200 RemoteHostOrder
 *   PUT 200 RemoteHostOrder (as stored)
 *   400 { error: string }   (invalid body or malformed JSON)
 *   415                     (not application/json)
 *   500 { error: string }   (the order could not be written)
 */
interface RemoteHostOrder {
  version: 1;
  order: string[];
}

/**
 * GET /api/remote/screens
 * PUT /api/remote/screens
 *
 * The client's list of open remote screens, remembered across restarts by
 * the local server in `remote-screens.json` in its configuration directory
 * (owner-only, replaced atomically), so every window and browser profile on
 * the machine and an embedded server share it. Authenticated like the other
 * `/api` routes; not on the control socket.
 *
 * GET returns the list; a missing, unreadable or corrupted file is an empty
 * list, and entries of a hand-edited file that break the rules below are
 * left out.
 *
 * PUT replaces the whole list with the body (`Content-Type:
 * application/json`) and answers with the list as stored. A body that breaks
 * a rule is refused as a whole and nothing is written:
 *   - `version` is 1, and there are no other fields;
 *   - at most 200 screens;
 *   - `target` passes the SSH target validation as it is (no blanks around it);
 *   - `screenId` has 1 to 256 characters, `name` at most 200;
 *   - `position` is a non-negative integer;
 *   - each (`target`, `screenId`) once.
 *
 * Response:
 *   200 RememberedScreens
 *   400 { error: string }   (a body without the shape, or not JSON)
 *   415                     (not application/json)
 *   500 { error: string }   (the file could not be written)
 */
interface RememberedScreens {
  version: 1;
  /** In the client's order. */
  screens: RememberedScreen[];
}

interface RememberedScreen {
  target: string;
  /** The screen's id on the host. */
  screenId: string;
  /** Its name when last seen, to show it before the host is connected. */
  name?: string;
  /** Its place among the window's screens, local ones included. */
  position?: number;
}

/**
 * GET /api/remote/catalog
 * PUT /api/remote/catalog   { target: string, screens: CatalogScreen[] }
 *
 * Last observed screen directories, separate from remembered open screens.
 * These routes only read/write local metadata; neither contacts a host nor
 * creates or opens a screen. Authentication and origin/Fetch Metadata guards
 * are the same as for the other API routes; absent from the control socket.
 *
 * GET returns the last 20 updated hosts, most recent update first (including
 * ties within a second). A missing, unreadable, malformed or wrong-version
 * file reads as an empty catalog. Invalid/repeated host rows are skipped,
 * preserving the other rows; an invalid screen invalidates its host row.
 *
 * PUT atomically replaces only this host's directory, including an empty
 * list, and returns its HostCatalog with a server-generated Unix timestamp.
 * Updates share a local-process OS file lock across the read/merge/write, so concurrent updates
 * to different hosts are retained. For one host, the last write wins. The
 * oldest entry is evicted when a new host would exceed the 20-host limit.
 * The file is remote-catalog.json in the configuration directory, owner-only
 * (0600 file, 0700 directory), written via an atomic rename.
 *
 * Invalid PUTs change nothing. Unknown fields at any level are refused.
 * Targets must pass SSH validation unchanged (no surrounding whitespace).
 * Each host has at most 200 screens with unique ids of 1–256 characters,
 * optional names of at most 200 characters, and nonnegative integral pane
 * counts. A client cannot supply lastSeen or a document version in a PUT.
 *
 * Response:
 *   GET 200 RemoteCatalog
 *   PUT 200 HostCatalog
 *   409 { error: string }   (target was removed; explicitly re-add it first)
 *   400 { error: string }   (invalid body; malformed JSON also returns 400)
 *   415                     (not application/json)
 *   500 { error: string }   (the file could not be written)
 */
interface RemoteCatalog {
  version: 1;
  hosts: HostCatalog[];
}

interface HostCatalog {
  target: string;
  /** Server-generated seconds since the Unix epoch. */
  lastSeen: number;
  screens: CatalogScreen[];
}

interface CatalogScreen {
  /** The screen's id on the host, never a local screen identifier. */
  id: string;
  name?: string;
  panes: number;
}

/**
 * GET    /api/remote/forwards -> 200 RemoteForward[] (active up/down only)
 * POST   /api/remote/forwards OpenRemoteForward -> 200 RemoteForward
 * DELETE /api/remote/forwards RemoteForwardKey -> 204
 *
 * Ordinary API authentication and Host/Origin/Fetch Metadata guards apply;
 * neither forwarding endpoint is exposed on the control socket.
 *
 * One owned, foreground ssh -N -T -a -x per mapping, from
 * 127.0.0.1:<local> here to localhost:<port> on the host. Saved/explicit local
 * ports are exact. Otherwise use the remote port if free, or a free alternate.
 * Every candidate is checked on IPv4 and IPv6 loopback; only unavailable IPv6
 * is exempt. No dummy IPv6 listener is left behind. Configured LocalForward,
 * RemoteForward and DynamicForward are refused (code "forward").
 *
 * OpenSSH must confirm local binding through a fixed local readiness command,
 * then stay alive and answer the local TCP probe; IPv6 is rechecked before and
 * after setup. Configuration lookup and readiness each have a 15-second bound.
 * Multiplex reuse and backgrounding are disabled for these owned processes.
 * Service traffic is raw TCP: HTTP, TLS and WebSockets carry no injected Splitty
 * authentication. Labels never affect commands, destinations or URLs.
 *
 * POST reuses an active/opening forward; a different explicit local port is
 * refused. Explicit success enables an existing saved rule and persists a new
 * selected local; failure preserves prior durable values and closes any new
 * uncommitted tunnel. POST alone never remembers a mapping. automatic:true
 * refuses a disabled rule, cannot select local, and never changes preferences.
 * Unknown fields retain legacy POST/DELETE behavior (ignored).
 *
 * DELETE (Stop) persists enabled:false first when saved, then waits for earlier
 * opening/rebuild work and port release. A failed durable write leaves the live
 * tunnel alone. Saved entries already stopped return 204; only absent unsaved
 * entries return 404. These operations and preference mutations are ordered per
 * target/remote key, including cleanup when a caller disconnects.
 *
 * A forward survives screen detachment and Forget until Stop/server exit.
 * After SSH exits it is down while rebuilding on the SAME reserved local port
 * with 1, 2, 4, 8, then 10-second backoff. At most 32 active/opening per target.
 * All returned lists are ordered by canonical target and remote port.
 *
 * Both APIs' error bodies are RemoteForwardFailure:
 *   400 {error}                  invalid body/target/port/label
 *   404 {error}                  Stop of an absent unsaved forward
 *   409 {error,code}             local_port_conflict, forward_paused,
 *                                active_mapping_conflict, forward_limit,
 *                                preference_limit (see types below)
 *   500 {error,code}             storage_failure; prior preferences unchanged
 *   500 {error,code:"internal"}  other local failure (e.g. no ssh client)
 *   502 {error,code,hint?}       existing SSH/forward failure and hint
 * Existing 401, 403 and 415 content-type handling is unchanged.
 */
interface RemoteForwardKey {
  target: string;
  /** Integer from 1 through 65535. Target uses existing SSH canonicalization. */
  port: number;
}

interface OpenRemoteForward extends RemoteForwardKey {
  /** Exact local port, integer 1..65535. Omitted: saved local, else best effort. */
  local?: number;
  /** Omitted/false is explicit consent; true cannot be combined with local. */
  automatic?: boolean;
}

interface RemoteForward {
  target: string;
  /** The host's port. */
  remote: number;
  /** The port here. */
  local: number;
  /** `http://localhost:<local>`, never `127.0.0.1`, whose cookies are Splitty's. */
  url: string;
  state: "up" | "down";
}

/**
 * GET    /api/remote/forward-preferences -> 200 RemoteForwardPreferences
 * PUT    /api/remote/forward-preferences SaveRemoteForward -> 200 SavedRemoteForward
 * DELETE /api/remote/forward-preferences RemoteForwardKey -> 204 (idempotent Forget)
 *
 * Metadata only: never connects, reserves a port or starts/retries a forward.
 * PUT merges supplied fields; unknown fields and null values are invalid.
 * Create: enabled=true, omitted local uses the active forward's actual port or
 * the remote port, omitted label is "". Update: omitted fields and enabled are
 * preserved. Supplying a local different from an active up/down forward yields
 * active_mapping_conflict. Only successful explicit POST resumes a saved rule.
 * Forget removes saved metadata/errors, leaving an active forward supervised.
 *
 * Persisted atomically as remote-forwards.json in the private config directory:
 * {version:1,forwards:[{target,remote,local,label,enabled}]}; modes 0700/0600 on
 * Unix. Max 32 saved entries per target and 256 overall, disabled included;
 * overflow is preference_limit, without eviction. Invalid/missing/unreadable
 * files read empty without rewrite. No credential or lastError is stored.
 *
 * After successfully sending remote.ready, the hub attempts that target's
 * enabled inactive mappings independently of relay/discovery. Startup, reads,
 * authentication and edits never restore. Initial failures retry only on later
 * readiness or explicit action; no new RemoteHubMessage is introduced.
 */
interface SaveRemoteForward extends RemoteForwardKey {
  local?: number;
  /** At most 200 Unicode characters. "" clears; omitted preserves/defaults. */
  label?: string;
}

interface SavedRemoteForward {
  target: string;
  remote: number;
  local: number;
  label: string;
  enabled: boolean;
  /**
   * Runtime only. Latest saved-opening/restore failure, never imported from disk.
   * Clears on successful opening, Stop, Forget or actual saved-local change;
   * label-only edits preserve it. Established recovery is active state:"down".
   */
  lastError?: { code: string; message: string; hint?: string };
}

interface RemoteForwardPreferences {
  version: 1;
  forwards: SavedRemoteForward[];
}

type RemoteForwardConflictCode =
  | "local_port_conflict"     // Exact local unavailable on either supported family.
  | "forward_paused"          // Automatic action cannot resume disabled consent.
  | "active_mapping_conflict" // Stop active mapping before changing its local port.
  | "forward_limit"           // 32 active/opening for this target.
  | "preference_limit";       // 32 saved for this target / 256 total.

interface RemoteForwardFailure {
  error: string;
  code?: RemoteForwardConflictCode | ReverseForwardConflictCode
    | "storage_failure" | "internal"
    | "host_key" | "auth" | "unreachable" | "unsupported_host" | "forward";
  hint?: string;
}

/**
 * Reverse forwards: a port of this machine shared with a host.
 *
 * GET    /api/remote/reverse-forwards -> 200 ReverseForward[] (active up/down only)
 * POST   /api/remote/reverse-forwards OpenReverseForward -> 200 ReverseForward
 * DELETE /api/remote/reverse-forwards ReverseForwardKey -> 204
 * GET    /api/remote/reverse-forward-preferences -> 200 ReverseForwardPreferences
 * PUT    /api/remote/reverse-forward-preferences SaveReverseForward -> 200 SavedReverseForward
 * DELETE /api/remote/reverse-forward-preferences ReverseForwardKey -> 204 (idempotent Forget)
 *
 * Authentication and guards as the forward APIs; none is on the control
 * socket. Unknown fields and null values are refused (400) on every body;
 * there is no `automatic`.
 *
 * Per start, the hub runs the install's prepare (a Darwin host is refused
 * with unsupported_host), `<server> remote-relay prepare` over a stdio-only
 * ssh (one JSON line {version:1, dir}), then one owned `ssh -T -a -x` with
 * the hardening options, no multiplex reuse or backgrounding,
 * `-R <dir>/<16 hex>.sock:localhost:<local>` and the command
 * `<server> remote-relay serve --port <port> --socket <that path>`. The
 * relay prints SPLITTY_RELAY_READY_V1, or SPLITTY_RELAY_FAILED_V1 <code>
 * <message> (host_port_conflict, listen, socket, unsupported), within 20 s.
 * It listens on 127.0.0.1:<port> and [::1]:<port> (when the host has it)
 * and relays only connections whose client socket has its own uid in
 * /proc/net/tcp(6). This side writes a newline to its stdin every 10 s; it
 * ends after 45 s without one, or at end of input, and removes its socket.
 * Configured LocalForward/RemoteForward/DynamicForward are refused (code
 * "forward"), as for forwards.
 *
 * POST returns an active mapping when `local` matches (active_mapping_conflict
 * otherwise), enables an existing saved rule and saves its local on success,
 * and never creates a rule. The local server's own port is refused with
 * local_port_reserved. DELETE persists enabled:false of a saved rule first,
 * then ends the ssh; 404 only for an absent unsaved mapping. A reverse forward
 * whose ssh ends is `down` while it is rebuilt with the same ports (backoff 1,
 * 2, 4, 8, then 10 s). At most 32 active/opening per target, apart from
 * forwards.
 *
 * Preferences are metadata only: PUT creates an enabled rule (local from the
 * request, else the active mapping's, else port) or changes local; DELETE
 * removes it and leaves an active mapping running. Persisted atomically as
 * remote-reverse-forwards.json in the private config directory:
 * {version:1,forwards:[{target,remote,local,enabled}]}; 32 per target, 256
 * in total (preference_limit); invalid files read empty without rewrite.
 * After remote.ready the hub opens that target's enabled inactive rules;
 * failures are the runtime lastError.
 *
 * Error bodies are RemoteForwardFailure, with the forward APIs' statuses.
 */
interface ReverseForwardKey {
  target: string;
  /** The host's port, integer 1..65535. */
  port: number;
}

interface OpenReverseForward extends ReverseForwardKey {
  /** The port here, integer 1..65535; only localhost here is reached. */
  local: number;
}

interface ReverseForward {
  target: string;
  /** The host's port. */
  remote: number;
  /** The port here. */
  local: number;
  state: "up" | "down";
}

interface SaveReverseForward extends ReverseForwardKey {
  local?: number;
}

interface SavedReverseForward {
  target: string;
  remote: number;
  local: number;
  enabled: boolean;
  /** Runtime only, as for saved forwards. */
  lastError?: { code: string; message: string; hint?: string };
}

interface ReverseForwardPreferences {
  version: 1;
  forwards: SavedReverseForward[];
}

type ReverseForwardConflictCode =
  | "host_port_conflict"    // The host's port is in use there.
  | "local_port_reserved";  // The local server's own port is never shared.

// ============================================================================
// 11. LOCAL EDITOR HANDOFF (desktop IPC, not a host/server command)
//
// Explicit "Open Folder in" submenu action on a hub terminal, never a server
// command:
//   Tauri:    invoke("open_remote_editor", { target, cwd, editor? })
//             invoke("open_local_editor", { editor, cwd, wslDistro? })
//   Electron: invoke("editor:open-remote", { target, cwd, editor? })
//             invoke("editor:open-local", { editor, cwd, wslDistro? })
// Editor is "vscode" (remote default), "zed" or, for remote folders only,
// "sftp" (the OS's handler for sftp: links); unknown values are rejected.
// Both resolve after OS dispatch or reject with an error. Resolution is no
// acknowledgement of installation, successful opening or SSH connection.
//
// Target comes from the owning connection; cwd from that pane's session.
// The UI rechecks their identity at click time. Native handlers independently
// validate inputs and generate only fixed folder URIs, never accepting an
// arbitrary URI, executable or command string. Native access is restricted to
// the exact-backend main UI (its top frame in Electron); remote windows, bundled
// setup pages and other origins gain no permission. Generic openers stay HTTP(S).
// Browsers generate and dispatch the URI synchronously during the user click.
// No new HTTP endpoint or WebSocket message.
interface RemoteEditorFolder {
  /** SSH alias/hostname/IPv4 with optional username. For VS Code and Zed, ports/IPv6 need an alias and VS Code also rejects all-hex aliases; sftp takes ports and bracketed IPv6 as given. */
  target: string;
  cwd: string;
  editor?: "vscode" | "zed" | "sftp";
}
interface LocalEditorFolder {
  editor: "vscode" | "zed";
  /** Supported absolute Linux/macOS directory; Windows local sessions run in WSL. */
  cwd: string;
  /** From /api/build; 1–64 ASCII letters, digits, dots, underscores or hyphens. */
  wslDistro?: string;
}
// VS Code: vscode://file/<path>/ or vscode://vscode-remote/wsl+<distro>/<path>/
// SSH: vscode://vscode-remote/ssh-remote+<encoded-target>/<path>/
// Zed: zed://file/<path>/ or zed://ssh/<target>/<path>/
// Zed with wslDistro is disabled/rejected: its URL handler has no WSL form.
// SFTP: sftp://<target>/<path>/ (remote only; the target unchanged).

// Settings API: GET/PATCH /api/home/settings includes terminal.allowOsc52Write
// (boolean, default true). PATCH false persists an opt-out. The on-disk TOML
// key is terminal.allow_osc52_write. This permits writes only, never OSC 52 reads.


// Settings API: GET/PATCH /api/home/settings includes drawer.pinned (boolean,
// default false), persisted as pinned under [drawer] in config.toml. Clients
// restore it before mounting the workspace; it is independent of the page origin
// and of the OS-assigned backend port used by desktop app restarts.


// Confirmed external-editor requests (editor-open-v1 in mux.welcome).
// POST /api/editor/open, authenticated HTTP or owner socket only, not grants.
// Resolves on the originating server; only regular files/folders are accepted.
export interface EditorOpenRequest {
  sessionId: string;
  path: string;
  editor?: "vscode" | "zed" | null;
}
// 200 { outcome: "dispatched" } means OS handoff, not successful connection.
// Errors: 400 invalid_path, 404 session_not_found / unsupported route,
// 409 no_client | ambiguous_client | denied | disconnected,
// 429 busy, 408 expired, 502 dispatch_failed; JSON {code,error}.
// Invalid JSON / unknown fields use the framework's 4xx rejection.
export type EditorClientMessage =
  | { type: "editor.register" }
  | { type: "editor.result"; requestId: string; outcome: "dispatched" | "denied" | "failed" | "expired" | "disconnected" };
export type EditorServerMessage =
  | { type: "editor.request"; requestId: string; sessionId: string; path: string; kind: "file" | "folder"; editor: "vscode" | "zed" | null; expiresAt: number }
  | { type: "editor.cancelled"; requestId: string };
// expiresAt is UTC Unix milliseconds; requests last at most 30 seconds.
// These messages are transient, connection-bound, and have no generic event ID.

// Files offered to the windows of a session (file-offer-v1 in mux.welcome),
// by `splitty download <file>` in a pane. POST /api/file/offer, authenticated
// HTTP or owner socket only, not grants. The server opens the regular file
// (links followed, at most 512 MiB) and keeps it open: the window reads the
// file as it was at offer time.
export interface FileOfferRequest {
  sessionId: string;
  path: string;
}
// The request waits: 200 { outcome: "saved" } once a window read the last
// chunk. Errors: 400 invalid_path, 404 session_not_found / unsupported route,
// 413 too_large, 409 no_client | declined | disconnected, 408 expired (no
// accept within 60 seconds), 429 busy (8 offers pending, or 2 for the
// session), 500 read_failed; JSON {code,error}.
// The offer goes to every connection that sent file.register and is attached
// to the session. The first file.accept takes it (the others get file.closed
// "taken"); only that connection reads, at the next offset only, 1 MiB per
// chunk. file.decline from every recipient declines it; from the accepting
// connection it cancels the transfer. An accepted offer whose reader sends
// nothing for 60 seconds, or detaches, ends as "disconnected".
export type FileClientMessage =
  | { type: "file.register" }
  | { type: "file.accept"; offerId: string }
  | { type: "file.decline"; offerId: string }
  | { type: "file.read"; offerId: string; offset: number };
export type FileServerMessage =
  | { type: "file.offer"; offerId: string; sessionId: string; name: string; size: number; expiresAt: number }
  | { type: "file.chunk"; offerId: string; offset: number; data: string; eof: boolean }
  | { type: "file.closed"; offerId: string; reason: "taken" | "declined" | "expired" | "disconnected" | "failed" };
// offerId is 32 hex digits; name is the file's last path component as the
// server has it (untrusted: the window shows it as text and saves the file
// under a safe name); data is standard base64; expiresAt is UTC Unix
// milliseconds. Transient, connection-bound, no generic event ID.

// Dedicated desktop commands remain open_local_editor / open_remote_editor
// (Electron editor:open-local / editor:open-remote) with {cwd,editor,target?,
// wslDistro?,kind?:"file"|"folder"}; kind defaults to folder for old callers.
