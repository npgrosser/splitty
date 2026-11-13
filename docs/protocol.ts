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
  /** Viewport units: all components finite, w/h positive. Negative origins
   * and sizes above 1 are valid; clients may clamp only their presentation. */
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
  /** A host of an earlier release may still send the removed "webview"; windows read it as "terminal". */
  role?: "terminal" | "viewer";
  /** Only on a pane of the role "viewer", which has no sessionId. */
  viewer?: ViewerState;
}

/**
 * The document of a viewer pane (servers with "viewer-v1", see `viewer-panes`).
 * The pane's title is the file's name or the text's title, its cwd the file's
 * directory or the text's `cwd`. Windows give the viewer panes they create IDs
 * of the form `viewer-<16 hex>`. The content is read with `viewer.read`.
 */
interface ViewerState {
  source: ViewerSource;
  /**
   * The terminal pane it was opened from, or that showed it again last; kept
   * when that pane closes.
   */
  originPaneId?: string;
  /**
   * Starts at 1; rises when `open_viewer` shows the same file again or gives
   * the pane a new text with `replace`.
   */
  revision: number;
  /**
   * The pane's annotations with their threads, in creation order, absent
   * while there are none (servers with "viewer-annotations-v1", see
   * `viewer-annotations`). Snapshots carry them; GET /api/mux/state and
   * GET /api/mux/screen/{id} leave them out, with the two fields below; an
   * agent reads what the user handed over with GET /api/viewer/comments.
   * They live and end with the pane, and stay when the pane gets a new text.
   */
  annotations?: ViewerAnnotation[];
  /**
   * The highest number the server gave an annotation of the pane, absent
   * while 0; a number is never given twice in a pane.
   */
  lastAnnotationNumber?: number;
  /**
   * The process waiting for the user's verdict on the document (`splitty
   * review`), absent while none waits. Set and cleared by the server alone,
   * never in state.json.
   */
  reviewWait?: ReviewWait;
}

/**
 * A remark a window made on a viewer pane's document, with its thread. The
 * server stores the window's fields as they came and never interprets their
 * text. A pane holds at most 100, with at most 262,144 bytes of quote,
 * prefix, suffix, body, replacement, replies and reply titles over all. The
 * kinds "question" and "delete" of earlier builds are read as "comment" and
 * as "change" with an empty replacement, and never written.
 */
interface ViewerAnnotation {
  /** "ann-" and 16 lowercase hex characters, chosen by the window; unique within the pane. */
  id: string;
  /**
   * Given by the server when it adds the annotation, lastAnnotationNumber + 1,
   * and kept when a window replaces it; never in put_annotations.
   */
  number?: number;
  /** "change" suggests a replacement of the passage; an empty one, its deletion. */
  kind: "comment" | "change";
  /** The remark, at most 16,384 bytes of UTF-8; a comment needs one that is not only whitespace. */
  body?: string;
  /** The suggested text of a "change": required, at most 16,384 bytes; "" suggests deleting the passage. */
  replacement?: string;
  /** The passage; absent for a general comment, which is a comment. */
  target?: AnnotationTarget;
  /** Absent while open ("open" is read as absent); only a window resolves or reopens. */
  status?: "resolved";
  /**
   * The user handed the annotation over; only mark_sent sets it. A window that
   * changes it puts it without, and the server drops it from a changed or new one.
   */
  sent?: true;
  /**
   * The thread in the order its replies were added, at most 50, absent while
   * empty. Kept by the server: never in put_annotations, kept when a window
   * replaces the annotation, gone with it.
   */
  replies?: AnnotationReply[];
}

/** A reply in an annotation's thread. */
interface AnnotationReply {
  /** "rep-" and 16 lowercase hex characters: the window's for the user's, the server's for an agent's. */
  id: string;
  /**
   * A window writes the user's replies only (put_replies); an agent's come
   * through POST /api/viewer/comments/reply and stay as written.
   */
  author: "user" | "agent";
  /** At most 16,384 bytes of UTF-8, not only whitespace. */
  body: string;
  /** The user handed the reply over, by mark_sent alone; the user's replies only. */
  sent?: true;
  /**
   * An agent's reply only: the caller's pane and the title the server knew
   * for it, at most 256 bytes (the pane's title, else its process's, else its
   * OSC title, else "agent").
   */
  from?: { paneId: string; title: string };
}

/** A process waiting for the user's review of a viewer pane's document. */
interface ReviewWait {
  /** "wait-" and 16 lowercase hex characters, chosen by the server. */
  id: string;
  /** The pane of the waiting process. */
  paneId: string;
  /** That pane's title, chosen as an agent reply's `from.title`. */
  title: string;
}

/** A passage, found again by its quote; the lines only order and label it. */
interface AnnotationTarget {
  /** Which text the quote was taken from. */
  view: "source" | "rendered";
  /** The selected text: not empty, at most 16,384 bytes. */
  quote: string;
  /** Context before and after the quote in that text, each at most 256 bytes ("" for none). */
  prefix: string;
  suffix: string;
  /** 1-based source lines when annotated: 1 <= startLine <= endLine <= 10,000,000. */
  startLine: number;
  endLine: number;
}

type ViewerSource =
  /** A file on the server's machine, read when a window shows it. */
  | { kind: "file"; path: string }
  /**
   * A text the server keeps in the pane record (and state.json). Snapshots,
   * GET /api/mux/state and GET /api/mux/screen/{id} leave `text` out.
   * `format` "markdown" renders it as a Markdown document; plain text without
   * it. `splitty review --text` sets it unless `--format text`; terminal
   * selections never do. Any other value rejects the op.
   */
  | { kind: "text"; title: string; text?: string; format?: "markdown" };

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
  /** Server-derived display text; available with serverDerivedPaneState. */
  displayTitle?: string;
  /** Known non-base-shell foreground process; false after exit. */
  busy?: boolean;
  restoreMode?: "replay" | "shell" | "close";
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
  /**
   * Raised by every accepted set_scratchpad, also an emptying one
   * ("scratchpad-versions-v1"); absent while 0, and left out where
   * `scratchpad` is.
   */
  scratchpadVersion?: number;
  /**
   * The session's root program status record (ProgramStatusRecord), absent
   * while it holds none. Only GET /api/mux/state and /api/mux/screen/{id}
   * carry it; screen snapshots over the WebSocket leave it out, where
   * ProgramStatusChanged carries the records.
   */
  programStatus?: ProgramStatusRecord;
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

/** Send the UTF-8 bytes of data unchanged to a PTY session. */
interface SessionInput {
  type: "input";
  id: string;
  data: string;
}

/**
 * Byte-preserving input, available with terminal-input-bytes-v1. Standard padded
 * base64, at most 64 KiB decoded (87,384 encoded bytes). Malformed/oversized
 * data produces a session-scoped error and no write. Session authority, ordering
 * and write errors match SessionInput; a streaming attachment is not required.
 */
interface SessionInputBytes {
  type: "input_bytes";
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
/** upload-control-v1: dir selects an existing absolute directory. The server
 * stages privately there, then publishes without overwriting any entry. */
interface Upload {
  type: "upload";
  /** First chunk only, omitted for temporary storage. */
  dir?: string;
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

/** upload-control-v1: idempotently discard this connection's unfinished upload.
 * No response. Completed files remain. Never queued across reconnects. */
interface UploadCancel {
  type: "upload_cancel";
  requestId: string;
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

/** Saved YAML/JSON template format (unchanged). Web panes are skipped, never rendered. */
interface Template {
  name: string;
  /** Reading a template drops panes of the removed type "webview" (with their "url"). */
  panes: Record<string, { type?: "terminal"; command?: string; dir?: string; restore_mode?: "replay" | "shell" | "close"; shell_profile?: string }>;
  layout: TemplateLayout;
  floating?: { layout: TemplateLayout; bounds: { x: number; y: number; w: number; h: number } }[];
}
interface TemplateLayout { slot?: string | string[]; split?: "horizontal" | "vertical"; children?: TemplateLayout[]; ratio?: number }
/**
 * POST /api/templates and native template.create ("template-names-v1", see
 * `templates`, Saved Template Names): save under the template's name. The
 * server derives the file name (ASCII letters and digits of the trimmed name,
 * lowercased, other runs as one "-", at most 128 bytes). Without `overwrite`
 * it saves only when no file of that name exists; check and save are one
 * step. 200 TemplateSaved; 409 { error: "template exists"; slug } and nothing
 * changes; 400 { error } for an invalid template or a name without letters
 * or digits. POST /api/templates/{slug} (template.save) still saves under a
 * caller's slug without these checks.
 */
interface SaveTemplateByName { template: Template; overwrite?: boolean }
interface TemplateSaved { slug: string; replaced: boolean }
/** Window-only intent; the envelope screenId names the replaced/new screen.
 * Server registers all panes atomically and runs their launch effects. Invalid
 * references, reused IDs and non-slot floating layouts reject without mutation. */
interface ApplyTemplateOp {
  type: "apply_template";
  template: Template;
  paneIds?: Record<string, string>; // windows always supply every terminal's ID
  root?: string;
  createScreen?: boolean; // default false; true requires a new screen ID
  /** Required when the screen exists and holds panes or sessions; without it
   * the op fails with mux.op_failed (servers with "close-assessment-v1"). A
   * window sets it only after the user agreed to the replacement. */
  replaceConfirmed?: true;
}
/** How both windows extract a Screen, local or on a host: on the Screen's own
 * connection. The same read as the deprecated POST /api/templates/extract/screen
 * and native template.extract, kept for windows of earlier releases; their body
 * omits type/requestId and the response is a Template (404 missing, 400 empty). */
interface ExtractTemplate { type: "mux.extract_template"; requestId: string; screenId: string; name?: string; root?: string }
interface TemplateExtracted { type: "mux.template_extracted"; requestId: string; template?: Template; error?: string }

/**
 * What a close would end ("close-assessment-v1", see `mux-protocol`, Close
 * Assessment). Read-only, answered on the same connection after the messages
 * sent before it; a hub relays it to a host unchanged. Screens and panes the
 * server does not have add nothing; a scope naming more than 512 items or of
 * an unknown kind is answered with `error`.
 */
interface AssessClose { type: "mux.assess_close"; requestId: string; scope: CloseScope }
type CloseScope =
  | { kind: "panes"; screenId: string; paneIds: string[] }
  | { kind: "screens"; screenIds: string[] } // also a screen a template replaces
  | { kind: "workspace" }; // every screen, as the end of an embedded server ends them
interface CloseAssessed { type: "mux.close_assessed"; requestId: string; assessment?: CloseAssessment; error?: string }
interface CloseAssessment {
  /** Whether the window asks first: some entry loses something. */
  confirm: boolean;
  running: number; // terminals whose program runs (managed job panes do not count)
  starting: number; // terminals whose session has not started yet
  scratchpads: number; // sessions whose scratchpad holds text
  annotations: number; // annotations of viewer panes, resolved ones included
  /** Only panes, and sessions of a named screen without a pane, that lose
   * something: tiled, then floating, then other panes, screen by screen. */
  entries: {
    screenId: string;
    paneId?: string;
    sessionId?: string;
    title?: string; // display title, else command, else the pane's title
    running: boolean;
    starting: boolean;
    scratchpad: boolean;
    annotations: number;
  }[];
}

/**
 * Fire-and-forget layout operation. Client has already applied the
 * change locally — this informs the server so it can update its state.
 *
 * Successful launches produce mux.pane_spawned; rejected creation/layout
 * intents produce mux.op_failed, and other windows receive a nudge.
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

/**
 * Request the ordered workspace catalog on this same session stream.
 * The response follows preceding mux operations from this connection.
 * Available when mux.welcome advertises workspace-catalog-v1.
 */
interface MuxWorkspaceResync {
  type: "mux.workspace_resync";
}

type ClientMessage =
  | SessionCreate
  | SessionAttach
  | SessionDetach
  | SessionInput
  | SessionInputBytes
  | SessionResize
  | SessionActivity
  | SessionIdle
  | SessionHide
  | SessionClose
  | SaveImage
  | Upload
  | UploadCancel
  | MuxConnect
  | MuxAttach
  | MuxDetach
  | MuxOp
  | ExtractTemplate
  | AssessClose
  | MuxResync
  | MuxWorkspaceResync;

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
  shellProfileFallbackReason?: "missing" | "disabled" | "unavailable";
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
  /** Same display policy as the mux session snapshot and ordinary pane title; plain text, at most 500 characters. */
  displayTitle?: string;
  busy?: boolean;
  /** OSC 0/2 window title (e.g. "✻ Claude Code"). */
  oscTitle?: string;
}

// OSC 133 prompt boundaries are not sent as events: the server uses them for
// `process.idle`, the semantic snapshot of `created`/`attached` and program
// status. Earlier servers also sent each boundary as an `osc133` event, which
// no window read.

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
// These are first-class event types, not wrapped in a notification envelope.
// A server whose mux.welcome lists "notification-notices-v1" classifies each
// one for the windows in `notice` (mux-protocol, Classified Notification
// Events); a window keeps only unread state, which pane it shows, its own
// `notifications.*` switches and the host's name in the title.

type NotificationSeverity = "info" | "success" | "warning" | "error";

/**
 * What a window shows for a notification event. Untrusted host data: the
 * title is shown as plain text; an unknown `severity` reads as `info`, an
 * unknown `setting` governs nothing, and any `suppressed` means no
 * notification. Without `notice` (an older server) a window shows an `info`
 * notification "Activity in <process>" or "Session activity", governed by
 * `notifications.enabled` only.
 */
interface Notice {
  kind: "process.finished" | "process.idle" | "session.exited" | "session.crashed" | "program-status";
  severity: NotificationSeverity;
  /** Plain text, at most 500 characters. */
  title: string;
  /** The `notifications.*` switch, besides `enabled`, that governs it. */
  setting?: "processFinished" | "processExited" | "processCrashed" | "programStatus";
  /**
   * Why no window notifies: `user-initiated` for a session the server closed
   * while its program ran, `program-status` for a session that holds a root
   * program status record.
   */
  suppressed?: "user-initiated" | "program-status";
}

/**
 * Foreground process returned to shell (baseShell false→true).
 * notice: kind "process.finished", info, "<process> finished", setting
 * "processFinished".
 */
interface ProcessFinished extends SessionEvent {
  type: "process.finished";
  process: string;
  cmdline?: string;
  notice?: Notice;
}

/**
 * Shell command finished — app is waiting for input (detected via OSC 133 D).
 * notice: kind "process.idle", info, "<process> is waiting" or "Command
 * finished"; suppressed "program-status" while the session holds a root
 * program status record.
 */
interface ProcessIdle extends SessionEvent {
  type: "process.idle";
  process: string;
  exitCode?: number;
  notice?: Notice;
}

/**
 * PTY session exited. Covers both clean exits and crashes. `userInitiated`:
 * the server closed the session while its program ran (a connection's
 * `close`, a closed pane or screen).
 * notice: exit code 0 kind "session.exited", info, "<process> exited",
 * setting "processExited"; otherwise kind "session.crashed", error,
 * "<process> failed", setting "processCrashed" ("session" when no process
 * is known); suppressed "user-initiated" with `userInitiated`.
 */
interface SessionExited extends SessionEvent {
  type: "session.exited";
  exitCode: number;
  process?: string;
  command?: string;
  userInitiated: boolean;
  notice?: Notice;
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
  /**
   * "viewer-annotations-v1" covers annotations with their numbers, statuses
   * and threads, the ops put_annotations, remove_annotations, put_replies,
   * remove_replies and mark_sent, viewer.review_answer, and the endpoints
   * under /api/viewer; a window offers none of them without it.
   */
  capabilities?: string[]; // includes "workspace-catalog-v1", "screen-placement-v1", "hub-host-decisions-v1", "ssh-prompts-v1", "hub-auto-forward-v1" and "hub-host-state-v1" (local main connections only), "server-templates-v1", "serverDerivedPaneState", "managed-jobs-v1", "editor-open-v1", "file-offer-v1", "upload-control-v1", "pty-size-v1", "scratchpad-v1", "layout-arrange-v1", "tab-select-v1", "tab-placement-v1", "viewer-v1", "viewer-annotations-v1", "program-status-v1", "close-assessment-v1", "notification-notices-v1", "screen-names-v1", "template-names-v1", "scratchpad-versions-v1" and "viewer-reviews-v1"; absent on old servers
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
 * One coherent ordered catalog, without pane layouts. Duplicate ordered IDs
 * occur once at their first position; missing IDs are omitted. The active hint
 * is null unless it names a returned screen. Names retain the welcome format.
 */
interface MuxWorkspaceSnapshot extends ServerEvent {
  type: "mux.workspace_snapshot";
  screens: ScreenInfo[];
  activeScreenId: string | null;
}

/**
 * Nudge: server tells the client that something changed externally
 * (e.g. a CLI command modified the layout). Client should request
 * a resync (mux.resync) to get the updated state.
 *
 * Accepted shared-layout ops notify other clients even without process effects.
 * Successful pane-spawn events precede at most one nudge per affected screen.
 * IDs name the actual changed, added or removed screens. The originating client
 * receives no success nudge; select_tab retains its window-local exception.
 * A workspace-only order/activation change uses a nudge without screenId.
 * This is an invalidation, not a workspace-directory snapshot; current web
 * clients only resync nudges that identify a screen.
 */
interface MuxNudge extends ServerEvent {
  type: "mux.nudge";
  reason: string; // includes cli_op and mux_op
  screenId?: string;
}

/** One successful PTY creation; failures never advertise an absent session. */
interface MuxPaneSpawned extends ServerEvent {
  type: "mux.pane_spawned";
  paneId: string;
  sessionId: string;
  shellProfileId?: string;
  shellProfileFallbackReason?: "missing" | "disabled" | "unavailable";
}

/**
 * A parsed native/browser layout op was rejected, sent to its client only.
 * The window resyncs screenId for every reported operation, including resize,
 * move, float and workspace operations without a new pane. No failure nudge
 * is sent to others. select_tab retains its documented silent exception.
 * paneId correlates newPaneId with a pending spawn, or identifies the viewer
 * for annotation operations; an ordinary existing-pane target is not a spawn
 * waiter. apply_template also names paneId for an individual spawn failure.
 * Malformed/unknown input remains ignored except for the existing template and
 * annotation parse failures, which already report through this event.
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
 * hub (section 10) it is one of the host daemon's relayed messages; a hub that
 * announces hub-auto-forward-v1 also reads it there, for remote.autoForwardPorts
 * (see HubChanged). A client ignores server messages of a type it does not know.
 */
interface PortsChanged extends ServerEvent {
  type: "ports.changed";
  ports: ListeningPort[];
}

// --- Program Status ---

/**
 * A session's root program status record: the last report without `id` of
 * the Program Status Protocol (OSC 7501, revision 0.4) that a program in the
 * session wrote, as Claude Code and Codex do. Child records (reports with an
 * `id`) stay on the server. `working`, `blocked` and `idle` records end at
 * an OSC 133 prompt mark, when the session stops being busy and when its
 * output ends; `done` and `error` stay until the program replaces or clears
 * them, a full reset or the end of the session.
 */
interface ProgramStatusRecord {
  state: "idle" | "working" | "done" | "blocked" | "error";
  /** Only with "blocked". */
  kind?: "permission" | "question" | "auth";
  /** 0..100, only with "working" and "blocked". */
  progress?: number;
  app?: string;
  /**
   * Decoded and sanitized plain text (no control characters, bidirectional
   * controls, invisible characters or line separators): untrusted program
   * text, shown as plain text and naming its pane.
   */
  title?: string;
  msg?: string;
  /** Unix milliseconds of the report, server clock. */
  updatedAt: number;
}

/**
 * The root record of a session, with the session's pane and screen when it
 * has one; `status` null when the record went (cleared, ended) or the
 * session was closed or removed. Sent right after mux.welcome (after
 * PortsChanged) for each session that holds a root record, and to every
 * connection, streaming the session or not, when a root record changes,
 * goes, or its pane moves to another screen. The server checks every
 * 100 ms while a client is connected, so a session's events are at least
 * 100 ms apart and carry its latest record.
 *
 * `notify` marks an event a window may notify for: the record turned
 * `blocked`, `done` or `error` or changed its kind or message, from live
 * output, and the session's last marked event with that state is at least
 * 10 seconds old. The greeting, the event for a moved pane and records
 * rebuilt from output a session keeper had buffered are never marked.
 * Exactly a marked event carries `notice` ("notification-notices-v1"): kind
 * "program-status", warning for `blocked`, success for `done`, error for
 * `error`, setting "programStatus", titled with the screen (its name, else
 * "Unnamed Screen"), the state in words and the record's `msg`,
 * else its `title`.
 *
 * Only servers whose mux.welcome lists "program-status-v1" send it. Not a
 * mux.* message. Through the hub (section 10) it is one of the host daemon's
 * relayed messages. A client ignores server messages of a type it does not
 * know.
 */
interface ProgramStatusChanged extends SessionEvent {
  type: "program_status.changed";
  paneId?: string;
  screenId?: string;
  status: ProgramStatusRecord | null;
  notify?: true;
  notice?: Notice;
}

// --- Server-Initiated Commands ---

/** Server-initiated command for the client to execute (e.g. from context menu). */
interface ServerCommand extends ServerEvent {
  type: "command";
  command: ClientCommand;
}

interface SettingsChanged extends ServerEvent {
  type: "settings.changed";
  settings: unknown;
}

// --- Union ---

type ServerMessage =
  | SettingsChanged
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
  | Bell
  | SessionSize
  // Process events
  | ProcessFinished
  | ProcessIdle
  | SessionExited
  // Mux
  | MuxWelcome
  | TemplateExtracted
  | CloseAssessed
  | MuxPaneSpawned
  | MuxOpFailed
  | MuxScreenSnapshot
  | MuxWorkspaceSnapshot
  | MuxNudge
  | MuxPaneSpawned
  | MuxBusy
  // Listening ports
  | PortsChanged
  // Program status
  | ProgramStatusChanged
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

interface SplitPaneOp {
  type: "split_pane";
  paneId: string;
  axis: "horizontal" | "vertical";
  /** Client-set ID for the new pane. Server generates one if omitted (e.g. CLI-initiated). */
  newPaneId?: string;
  /** One finite nonnegative value per resulting child, sum 1 within 0.01.
   * Same-axis parent: existing child count + 1; a new wrapper: 2.
   * Invalid arrays reject without changing state or spawning; omission uses equal sizes. */
  resultingSizes?: number[];
  /** 0 inserts before the source slot; other values or omission insert after. */
  newPaneIndex?: number;
  cwd?: string;
  /** Explicit profile, otherwise inherit the source pane's profile. */
  shellProfileId?: string;
}

/**
 * Closing the last pane of a screen (root layout and floating entries, any
 * pane kind) removes the screen with the same op, as close_screen does. A
 * close_screen for it afterwards fails like one for any unknown screen. Servers
 * of earlier versions keep the screen without panes; create_screen with its ID
 * fills it again.
 */
interface ClosePaneOp {
  type: "close_pane";
  paneId: string;
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

/** Float one root pane, leaving at least one pane in the tiled layout.
 * No session is recreated. Omitted bounds use {x:.25,y:.25,w:.5,h:.5}. */
interface FloatPaneOp {
  type: "float_pane";
  paneId: string;
  bounds?: FloatingEntry["bounds"];
}

/** Dock all panes of the identified float into the root slot containing
 * paneId, falling back to the first root slot (including an empty one).
 * The first floated pane becomes active; there are no process effects. */
interface DockPaneOp {
  type: "dock_pane";
  floatingId: string;
  paneId: string;
}

/** Replace bounds of an existing float, subject to the shared finite/positive
 * rule. Invalid float/move bounds reject atomically with mux.op_failed. */
interface MoveFloatOp {
  type: "move_float";
  floatingId: string;
  bounds: FloatingEntry["bounds"];
}

/**
 * Move a pane within the root layout: into the slot `targetSlotId`, or beside
 * the node or pane `targetSplitId` in `direction`. Rejected when the target is
 * missing, is no slot (for `targetSlotId`), or is gone once the pane is taken
 * out. Browser windows send `arrange_panes` for drags; native windows use
 * precise move operations. A target-slot move appends and activates, even
 * within the same slot.
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

/**
 * Exact root-layout tab placement (requires "tab-placement-v1"). The pane
 * must still belong to sourceSlotId; targetSlotId must be a root slot.
 * beforePaneId must name another tab of the destination; omission appends.
 * Same-slot reorder preserves effective selection, including an implicit
 * first tab. Cross-slot placement activates the moved pane and collapses an
 * empty source, preserving current surviving sibling proportions. Existing
 * pane/session records and surviving layout IDs remain unchanged; no process
 * effects. Invalid source/target/anchor rejects atomically with mux.op_failed;
 * accepted placement nudges other clients through ordinary layout propagation.
 */
interface MoveTabOp {
  type: "move_tab";
  paneId: string;
  sourceSlotId: string;
  targetSlotId: string;
  beforePaneId?: string;
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
  /**
   * Omitted: the server assigns its first unused Screen N (from 1)
   * atomically. Clients omit it when mux.welcome lists "screen-names-v1" and
   * learn the name from mux.workspace_snapshot or a screen snapshot; for an
   * older server they send the smallest free Screen N they know of.
   */
  name?: string;
  /** Client-set ID for the initial pane. Server generates one if omitted. */
  newPaneId?: string;
  /** Explicit profile, otherwise use the backend default. */
  shellProfileId?: string;
  /**
   * The client's first screen of an empty workspace ("screen-names-v1"):
   * applied only while the workspace has no screen, otherwise refused with
   * mux.op_failed ("the workspace already has screens") and nothing changes,
   * so windows that start together end with one. Older servers ignore it.
   */
  onlyIfEmpty?: boolean;
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
  /** The effective default profile. */
  defaultProfileId: string;
  profiles: ShellProfile[];
  /** True while the default follows the system default (empty stored default).
   *  On update, true stores the empty value; absent means false. */
  followSystemDefault?: boolean;
  /** The profile detected for the login shell (`$SHELL`); ignored on update. */
  systemDefaultProfileId?: string;
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
  /** The enclosing mux.op screenId identifies the Screen. */
  name: string;
}

/** Legacy replacement order: unknown IDs are filtered; omitted Screens stay omitted. */
interface ReorderScreensOp {
  type: "reorder_screens";
  screenOrder: string[];
}

/**
 * Relative placement (requires "screen-placement-v1" and an ordered catalog).
 * The enclosing mux.op screenId, or the control API body's screenId, names the
 * moved Screen. beforeScreenId names an existing ordered anchor; omission or
 * null places the Screen at the current end. Other entries, including legacy
 * duplicates and omitted Screen records, remain as they are. Only duplicate
 * entries of the moved Screen collapse. Invalid identities or a non-string,
 * non-null anchor reject atomically. No pane, session or active Screen changes.
 * A changed order nudges other clients without a screenId; a no-op does not.
 */
interface MoveScreenOp {
  type: "move_screen";
  beforeScreenId?: string | null;
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
 * for the screen; the sender gets nothing. With `ifVersion`
 * ("scratchpad-versions-v1"), only while the session's `scratchpadVersion`
 * still equals it; otherwise mux.op_failed and nothing changes. A window
 * sends the version its text is based on and expects its save to become
 * that version plus one.
 */
interface SetScratchpadOp {
  type: "set_scratchpad";
  sessionId: string;
  text: string;
  ifVersion?: number;
}

/**
 * Show a document in a new viewer pane, without a session or any effect
 * (servers with "viewer-v1"). With `paneId`, a directional placement puts it
 * beside that pane of the root layout as split_pane does (left/above before
 * it); "tab" appends it to the pane's slot as the active tab. A floating
 * entry holds no splits, so beside a floating pane every placement is "tab".
 * Without `paneId`: the only pane of an empty root layout, else the right
 * half of a new horizontal split around the root layout. Without `newPaneId`,
 * a file a viewer pane of the screen already shows is not opened again: the
 * first such pane in layout order becomes its slot's active pane, its
 * `revision` rises by one, and a `paneId` that yields an origin becomes its
 * `originPaneId`. Rejected: a file path that is relative, starts with "//",
 * is over 4096 bytes, or holds a backslash, a "." or ".." segment, control
 * characters or U+200B..U+200F, U+202A..U+202E, U+2060..U+206F, U+FEFF; a
 * text over 256 KiB of UTF-8, a title outside 1 to 200 characters or with
 * control characters, a relative `cwd`, an unknown `paneId`, a `newPaneId` in
 * use, `replace` with a file or with `newPaneId`. Other clients get
 * `mux.nudge` for the screen; the sender gets nothing.
 */
interface OpenViewerOp {
  type: "open_viewer";
  /**
   * The anchor. A terminal anchor is the origin; a viewer anchor passes on
   * its own origin.
   */
  paneId?: string;
  /** Default "right". */
  placement?: "right" | "left" | "above" | "below" | "tab";
  newPaneId?: string;
  source: ViewerSource;
  /** A text's working directory; absolute. */
  cwd?: string;
  /**
   * With a `text` source and without `newPaneId`: the first viewer pane of
   * the screen in layout order that shows a text of the same title and
   * origin gets the text (and `cwd`, when given), a raised `revision`, and
   * becomes its slot's active pane, keeping its annotations; without one, a
   * new pane. `splitty review --text` sets it; a window never does.
   */
  replace?: true;
}

/**
 * Add annotations to a viewer pane, numbered from lastAnnotationNumber + 1,
 * or replace those with the same `id` in their place, keeping their number
 * and replies (servers with "viewer-annotations-v1"). Only a window sends it,
 * over the WebSocket; POST /api/mux/op answers 400. Refused as a whole, with
 * nothing changed, for a pane that is not a viewer pane, fewer than 1 or more
 * than 100 annotations, an `id` of another form or twice in the op, an
 * annotation with a `number` or `replies` (0 and [] read as absent) or that
 * breaks the rules of ViewerAnnotation, or a pane that would hold more than
 * 100 annotations or 262,144 bytes of their text; the sender then gets
 * mux.op_failed with the pane. No effect. Applied: other clients get
 * `mux.nudge` for the screen; the sender gets nothing, so a window that added
 * an annotation asks for the screen to learn its number.
 */
interface PutAnnotationsOp {
  type: "put_annotations";
  paneId: string;
  annotations: ViewerAnnotation[];
}

/**
 * Remove annotations of a viewer pane by `id`, with their threads; ids the
 * pane does not hold are ignored. Only a window sends it, over the WebSocket;
 * POST /api/mux/op answers 400. Refused, with mux.op_failed and the pane to
 * the sender, for a pane that is not a viewer pane, fewer than 1 or more than
 * 100 ids, or an id of another form. No effect. Applied: other clients get
 * `mux.nudge` for the screen; the sender gets nothing.
 */
interface RemoveAnnotationsOp {
  type: "remove_annotations";
  paneId: string;
  ids: string[];
}

/**
 * Add the user's replies to the thread of annotation `annotationId`, or
 * replace those with the same `id` in their place. Only a window sends it,
 * over the WebSocket; POST /api/mux/op answers 400. Refused as a whole, with
 * mux.op_failed and the pane to the sender, for a pane that is not a viewer
 * pane, an annotation it does not hold, fewer than 1 or more than 50 replies,
 * a reply whose author is not "user" or that names `from`, an `id` of another
 * form, twice in the op, of an agent's reply or of a reply in another thread,
 * a body that is only whitespace or over 16,384 bytes, a thread that would
 * hold more than 50 replies, or a pane over its byte limit. No effect.
 * Applied: other clients get `mux.nudge` for the screen; the sender gets
 * nothing.
 */
interface PutRepliesOp {
  type: "put_replies";
  paneId: string;
  annotationId: string;
  replies: { id: string; author: "user"; body: string; sent?: true }[];
}

/**
 * Remove replies of either author from the thread of `annotationId` by `id`;
 * ids the thread does not hold, and an annotation the pane no longer holds,
 * are ignored. Only a window sends it, over the WebSocket; POST /api/mux/op
 * answers 400. Refused, with mux.op_failed and the pane to the sender, for a
 * pane that is not a viewer pane, fewer than 1 or more than 50 ids, or an id
 * of another form. No effect. Applied: other clients get `mux.nudge` for the
 * screen; the sender gets nothing.
 */
interface RemoveRepliesOp {
  type: "remove_replies";
  paneId: string;
  annotationId: string;
  ids: string[];
}

/**
 * Mark annotations and the user's replies as handed over: 1 to 5,100
 * entries, each an "ann-" or "rep-" id with the content the review held (an
 * annotation's kind, body, replacement and target; a reply's text as
 * `body`). The server marks an entry only while it still reads so, compared
 * field by field, so an edit another window saved before the mark stays
 * pending; agents' replies and ids the pane does not hold are ignored. No
 * put ever marks an entry: a put keeps the mark of an entry that reads as
 * before and drops it otherwise, and a new entry starts unmarked. A window
 * sends it after it handed a review to a terminal, a file or a scratchpad,
 * and after the server accepted its answer to a wait. Only a window sends
 * it, over the WebSocket; POST /api/mux/op answers 400. Refused, with
 * mux.op_failed and the pane to the sender, for a pane that is not a viewer
 * pane, fewer than 1 or more than 5,100 entries, or an id of another form.
 * No effect. Applied: other clients get `mux.nudge` for the screen; the
 * sender gets nothing.
 *
 * An agent's reply and the start and the end of a wait for a review are the
 * server's own operations: they have no wire form here, so no client submits
 * them, over the WebSocket or POST /api/mux/op. Each nudges every client
 * (reason "mux_op") and raises the screen revision.
 */
interface MarkSentOp {
  type: "mark_sent";
  paneId: string;
  entries: {
    id: string;
    kind?: "comment" | "change";
    body?: string;
    replacement?: string;
    target?: AnnotationTarget;
  }[];
}

interface SetScreenOwnershipOp {
  type: "set_screen_ownership";
  screenId: string;
  ownership: OwnershipMode;
}

type Operation =
  | ApplyTemplateOp
  | SplitPaneOp
  | NewTabOp
  | ClosePaneOp
  | ResizeSplitOp
  | SelectTabOp
  | FloatPaneOp
  | DockPaneOp
  | MoveFloatOp
  | MovePaneOp
  | MoveTabOp
  | ArrangePanesOp
  | CreateScreenOp
  | CloseScreenOp
  | SetActiveScreenOp
  | RenameScreenOp
  | ReorderScreensOp
  | MoveScreenOp
  | MovePaneToScreenOp
  | SetScratchpadOp
  | OpenViewerOp
  | PutAnnotationsOp
  | RemoveAnnotationsOp
  | PutRepliesOp
  | RemoveRepliesOp
  | MarkSentOp
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
 *     │──── mux.op { split_pane }     │  (fire-and-forget)
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
 *     │  Client generates new pane ID ("terminal-5").
 *     │  Client mirrors result to server:
 *     │                    │                   │
 *     │── mux.op {         │                   │
 *     │     split_pane,    │                   │
 *     │     paneId: "terminal-3",              │
 *     │     newPaneId: "terminal-5",           │
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
 *   200 { ok: true, revision: number, screenId: string,
 *         created: { paneId: string; sessionId: string }[],
 *         paneId?: string, reused?: boolean,
 *         viewerRevision?: number }   // paneId, reused, viewerRevision: open_viewer only
 *   400 { ok: false, error: string }     // validation error
 *   404 { ok: false, error: string }     // screen/panel not found
 *
 * For open_viewer, `paneId` is the viewer pane that shows the document,
 * `reused` is true when that pane existed before, and `viewerRevision` is the
 * revision of its viewer record after the operation. set_scratchpad,
 * put_annotations, remove_annotations, put_replies, remove_replies and
 * mark_sent are a window's alone: answered 400 without a change.
 */

/**
 * GET /api/mux/state
 *
 * Returns the full WorkspaceState. Used by `splitty list-panes`,
 * `splitty list-screens`, etc. No session carries its scratchpad, no `text`
 * viewer document its text and no viewer pane its annotations,
 * `lastAnnotationNumber` or `reviewWait`, here and in GET /api/mux/screen/:id.
 * Each session that holds a root program status record carries it as
 * `programStatus`, here and in GET /api/mux/screen/:id.
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
 * GET /api/viewer/comments?screenId=ID&paneId=ID[&document=D][&all=true]
 *
 * What an agent may read of the annotations on the viewer panes of the
 * caller's screen (`splitty comments`): `screenId` and `paneId` name the
 * caller; a pane that moved is followed to the screen that holds it now.
 * `document` is a viewer pane's ID ("viewer-...") or an absolute path that
 * file viewer panes of the screen show; without it, every viewer pane of the
 * screen in layout order. An annotation is listed when the user handed it
 * over (`sent`), open ones only unless `all` is "true" ("false" or absent:
 * open only); its replies are the agents' and the user's handed-over ones.
 * Never on a grant socket.
 *
 * Response:
 *   200 { documents: { paneId: string;
 *           source: { kind: "file"; path: string } | { kind: "text"; title: string; format?: "markdown" };
 *           annotations: { number: number; kind: "comment" | "change";
 *             status: "open" | "resolved"; body?: string; replacement?: string;
 *             target?: { view: "source" | "rendered"; quote: string; startLine: number; endLine: number };
 *             replies: { author: "user" | "agent"; body: string; from?: { paneId: string; title: string } }[];
 *           }[] }[] }   // annotations by number
 *   400 invalid     // an empty screenId or paneId, `all` neither true nor false, a
 *                   // document that is neither an absolute path nor a viewer pane's ID
 *   404 not_found   // the caller's pane on no screen, a document the screen does not show
 *   409 ambiguous   // a path several viewer panes show; the error names them
 *
 * POST /api/viewer/comments/reply
 *
 * Add an agent's reply to annotation `number` of `document` (as above), with
 * a server-chosen ID, author "agent" and `from` the caller's pane and its
 * title (`splitty comments reply`). A reply does not reopen a resolved
 * annotation. Other clients get `mux.nudge` (reason "mux_op"). Never on a
 * grant socket.
 *
 * Request:
 *   { screenId: string, paneId: string, document: string, number: number, body: string }
 *
 * Response:
 *   200 { ok: true, paneId: string, number: number }
 *   400 invalid     // as above, a malformed body, a reply body that is empty, only
 *                   // whitespace or over 16,384 bytes
 *   404 not_found   // as above, a number the pane does not hold, an annotation not handed over
 *   409 ambiguous   // as above
 *   409 limit       // 50 replies in the thread, or the pane's 262,144 bytes
 *
 * POST /api/viewer/review/wait
 *
 * Wait for the user's verdict on viewer pane `viewerPaneId` of the caller's
 * screen (`splitty review`): the server names the wait in the viewer record
 * (`reviewWait`), nudges every client, and holds the request until the first
 * window's viewer.review_answer, without a deadline. The wait ends with the
 * answer, with the pane, or when the caller's connection ends, which answers
 * nothing; each end clears `reviewWait` and nudges every client. At most one
 * wait per viewer pane and 16 per server, in memory only. Never on a grant
 * socket.
 *
 * Request:
 *   { screenId: string, paneId: string, viewerPaneId: string,
 *     viewerRevision?: number }   // the open_viewer answer's, when the caller showed the document
 *
 * Response:
 *   200 { outcome: "approved" }                       // approved without notes
 *     | { outcome: "approved", review: string }        // approved, the review as notes
 *     | { outcome: "changes_requested", review: string } // the user requests changes
 *   400 invalid     // an empty screenId or paneId, a malformed body
 *   404 not_found   // the caller's pane on no screen, no viewer pane `viewerPaneId` on it
 *   409 busy        // another process waits for that pane
 *   409 changed     // the pane shows another revision than `viewerRevision`; no wait
 *   410 closed      // the pane closed before an answer
 *   429 busy        // 16 waits on the server
 *
 * Errors of the three endpoints are JSON { code, error }.
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
 *   rejects an op that would create a pane, an arrange_panes or an
 *   annotation op
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
 * Process events like `process.finished` and `session.exited` are
 * first-class event types, NOT wrapped in a generic notification envelope.
 * The server classifies them for the windows (`notice`: kind, severity,
 * title, governing switch, suppression); a window decides only what depends
 * on it: unread state, the pane it shows, its switches and presentation
 * (toast, badge, system notification).
 *
 * There is no generic `notification` type: a window notifies only for the
 * classified events above.
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
 * into the terminal emulator. Structured events (title, bell, size)
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
// RemoteProgress exists only on this browser socket: the gateway translates
// the native host stream's transport Progress reports (Native transport v1
// below), each step once and in order, and forwards no RemoteProgress text of
// the host stream. A native host stream carries the steps only as Progress
// frames before acceptance; its first text is RemoteReady. A hub of the
// previous release repeats them as RemoteProgress text before RemoteReady,
// which every reader ignores.
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

/** A step before the relay, in this order; also the `phase` of a native
 *  Progress report of a host stream. `install` only when the server is
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

/**
 * The hub's plan for an ordinary update of the host's daemon. An available
 * update keeps every session (the hub offers none that ends one), so a window
 * offers one only when `available` and `keepsSessions` are both true, names
 * `toVersion` as the version the host gets, and counts no screens or panes
 * that end. `toVersion` is present whenever `available` is true; older hubs
 * omit it, and the window names the local server's version instead.
 */
interface UpdatePlan {
  available: boolean;
  keepsSessions: boolean;
  toVersion?: string;
}

/** Sent once, right before the host daemon's first message. */
interface RemoteReady {
  type: "remote.ready";
  /** Hub decision; absent only on older hubs. */
  update?: UpdatePlan;
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
  | "unsupported_environment" // local loopback/process reservation authority is unavailable; no SSH started
  | "install" //          the upload, its checksum or the unpacked server failed
  | "daemon" //           the handshake or the daemon start failed (with update=1 also:
  //                      the stop failed, or the old daemon had not ended yet)
  | "incompatible" //     the daemon's protocol is outside the window; it keeps running
  | "migration_unavailable" // legacy migration cannot be verified; no update is offered
  | "bridge" //           remote-bridge ended before the daemon's first message
  | "forward" //          only from /api/remote/forwards: the forward failed, or the
  //                      host's SSH configuration forwards ports itself
  | "internal"; //        a local problem (no ssh client, no free port)

interface RemoteError {
  type: "remote.error";
  retryable: boolean;
  update?: UpdatePlan;
  code: RemoteErrorCode;
  message: string;
  /** What the user can do, where there is something. */
  hint?: string;
  /** For `incompatible`: the host daemon's version and protocol. */
  version?: string;
  protocol?: number;
  /** For `incompatible`: its sessions outlive it, so an update ends none. */
  keepsSessions?: boolean;
  /** For `incompatible`: the last observed screen directory, as in `remote.ready`.
   * Current maintenance eligibility never permits deliberately ending sessions. */
  screens?: { id: string; name?: string; panes: number }[];
  /** Positive, native-verified offer; needs a new explicit confirmation.
   * Conditional host proof tokens never become renderer-selected authority.
   */
  migration?: LegacyMigrationOffer;
}

interface LegacyMigrationOffer {
  available: true;
  fromVersion: string;
  toVersion: string;
  /** Keeper-backed sessions observed by the host's read-only preflight. */
  sessions: number;
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
 * PUT /api/remote/screens/host   (native remoteScreens.updateHost; hub-host-state-v1)
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
 * PUT /api/remote/screens/host is how windows write: the body
 * (RememberedHostScreens) names one host, and the server replaces only that
 * host's entries, in the place of its first one (a new host goes last), with
 * `closed` on each when the body sets it; an empty `screens` forgets the host.
 * The replacement runs under the server's lock on the list it holds, so
 * windows that show different hosts never undo each other's entries. At most
 * 200 entries are kept. A window writes only the hosts it shows or showed: a
 * host whose last screen leaves the window is forgotten, a host the window
 * closed keeps its screens as `closed`. No window brings back `closed`
 * entries at start. Without hub-host-state-v1 a window writes nothing.
 * The body is refused (400) under the rules below for its target and screens.
 *
 * PUT /api/remote/screens replaces the whole list with the body
 * (`Content-Type: application/json`) and answers with the list as stored; it
 * remains for older clients. A body that breaks a rule is refused as a whole
 * and nothing is written:
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
  /** Its host was closed in the client; offered again, not reopened unasked. */
  closed?: boolean;
}

/** The body of PUT /api/remote/screens/host; no other fields. */
interface RememberedHostScreens {
  target: string;
  screens: { screenId: string; name?: string; position?: number }[];
  /** The host was closed in the window. */
  closed?: boolean;
}

/**
 * GET /api/remote/catalog
 * PUT /api/remote/catalog   { target: string, screens: CatalogScreen[] }
 *
 * Last observed screen directories, separate from remembered open screens.
 * Windows report observations with hub.catalog.observe and read the merged
 * directory (remote.directory.get); these routes remain for older clients,
 * under the same rules. They only read/write local metadata; neither contacts a host nor
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
 * An observation lists the host's screens as they are now. A screen without
 * `panes` (hub-host-state-v1) keeps the count stored for it; one without a
 * stored count is left out until an observation gives one.
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
 * selected local; preserveRestore:true keeps the existing enabled choice instead.
 * Failure preserves prior durable values and closes any new
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
  /** Explicit opening preserves the saved restore choice instead of enabling it. */
  preserveRestore?: boolean;
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
 * Create: omitted enabled defaults to true for legacy clients; native supplies false
 * until restoration is explicitly chosen. Omitted local uses the active forward's actual port or
 * the remote port, omitted label is "". Update: omitted fields are preserved.
 * Supplied enabled changes restoration only, never starts or stops a live tunnel. Supplying a local different from an active up/down forward yields
 * active_mapping_conflict. Explicit POST may resume a saved rule; native sends
 * preserveRestore:true so one-off forwarding does not change restoration.
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
  /** Local restore consent. Omitted preserves the prior value or legacy true default. */
  enabled?: boolean;
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
    | "host_key" | "auth" | "unreachable" | "unsupported_host" | "unsupported_environment" | "forward";
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
// Editor is "vscode" (remote default), "zed" or, for remote folders only,
// "sftp" (the OS's handler for sftp: links); unknown values are rejected.
// Both resolve after OS dispatch or reject with an error. Resolution is no
// acknowledgement of installation, successful opening or SSH connection.
//
// Target comes from the owning connection; cwd from that pane's session.
// The UI rechecks their identity at click time. Native handlers independently
// validate inputs and generate only fixed folder URIs, never accepting an
// arbitrary URI, executable or command string. Native access is restricted to
// the exact-backend main UI; remote windows, bundled
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
// POST /api/editor/open or native editor.open; authenticated browser/native UI
// or owner socket, never grants.
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
// Native file.offer, authenticated HTTP or owner socket, never grants.
// The server opens the regular file
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

// The documents of viewer panes (viewer-v1 in mux.welcome). A window reads
// one over the connection whose screen holds the pane, never through the
// control API: offset 0 first, then each next offset with the `version` of
// the first answer. The server keeps nothing between chunks and runs reads off
// the connection's message loop, at most 4 per connection (`busy` beyond, and
// while the connection's output has no room for a chunk). A file is opened anew
// for every read (links followed, regular files of at most 8 MiB, never
// waiting on a FIFO); its version is f-<size>-<mtime ns>-<inode> of the opened
// file, so a file changed between two chunks fails with `changed`. A text
// document is read from the pane record (at most 256 KiB, version
// t-<size>-<16 hex of its SHA-256>). Reading changes no state and reaches no
// other client.
export type ViewerClientMessage = {
  type: "viewer.read";
  /** At most 128 bytes; echoed in the answer. */
  requestId: string;
  paneId: string;
  offset: number;
  /** Required when offset > 0. */
  version?: string;
  /** Offset 0 only: answer viewer.unchanged when the document has it. */
  ifVersion?: string;
};
export type ViewerServerMessage =
  /** At most 1 MiB of the document as standard base64; size is the document's. */
  | { type: "viewer.chunk"; requestId: string; offset: number; size: number; version: string; data: string; eof: boolean }
  | { type: "viewer.unchanged"; requestId: string; version: string }
  | {
      type: "viewer.failed";
      requestId: string;
      code: "unknown_pane" | "not_found" | "not_a_file" | "too_large" | "unreadable" | "changed" | "busy" | "invalid";
      error: string;
    };
// Transient, connection-bound, no generic event ID. A malformed read is
// answered `invalid` with its requestId when that is a string that fits, else "".

// The anchors and the review of a viewer pane's annotations ("viewer-reviews-v1"
// in mux.welcome; `mux-protocol`, Review Texts And Anchors). Sent over the
// connection whose screen holds the pane, beside viewer.read and sharing its
// four reads in flight; a hub relays them to a host unchanged. The server
// takes the pane's record after the connection's earlier messages, reads the
// whole document as viewer.read does, anchors every annotation in its source
// and answers to the sender only; failures are viewer.failed with the codes
// above (`unknown_pane`, `not_found`, `too_large`, `busy`, `invalid`, …).
// Ranges count UTF-8 bytes of the source text: the document's bytes as UTF-8
// with U+FFFD for invalid sequences, without a leading byte order mark, every
// CRLF and lone CR as LF. Lines are 1-based; a passage ends on the line of its
// last character. Transient, connection-bound, no generic event ID.
export type ViewerReviewClientMessage =
  | { type: "viewer.anchor"; requestId: string; paneId: string }
  | {
      type: "viewer.review";
      requestId: string;
      paneId: string;
      /** The verdict of an answer to a waiting process; none for a review handed to a terminal. */
      verdict?: "approved" | "changes_requested";
      /** The window saw the document change while the process waited. */
      changedDuringWait?: boolean;
      /**
       * At most 100: what the window found in its rendered view for
       * annotations taken from it. Used only for annotations whose target's
       * view is `rendered`; passages of annotations the pane does not hold
       * are ignored.
       */
      passages?: RenderedPassage[];
    };
export interface RenderedPassage {
  id: string;
  state: "anchored" | "changed" | "lost";
  /** 1 to 10,000,000, with anchored and changed only; startLine <= endLine. */
  startLine?: number;
  endLine?: number;
  /** Where it starts in the window's rendered text, ordering passages of one line. */
  offset?: number;
  /** A page that names no source lines (a rendered HTML page): the review names none. */
  unlined?: true;
}
export type ViewerReviewServerMessage =
  | {
      type: "viewer.anchored";
      requestId: string;
      paneId: string;
      /** The document version anchored in (viewer.read's). */
      version: string;
      /** Every annotation, in the order of the pane's list. */
      anchors: AnnotationAnchor[];
    }
  | {
      type: "viewer.reviewed";
      requestId: string;
      paneId: string;
      version: string;
      /** The review's Markdown, ending with a line break. */
      text: string;
      /** How many annotations it holds. */
      count: number;
      /** The user's entries it holds that were not handed over, as mark_sent names them. */
      entries: MarkSentOp["entries"];
    };
export interface AnnotationAnchor {
  id: string;
  state: "anchored" | "changed" | "lost" | "general";
  /** Bytes of the source text and current lines; anchored and changed only. */
  start?: number;
  end?: number;
  startLine?: number;
  endLine?: number;
}

// Answers to a wait for a review (`reviewWait` in a viewer record, see
// POST /api/viewer/review/wait): a window sends viewer.review_answer, and the
// server answers each one to its sender only with viewer.review_answered.
// The first answer that names the wait and its pane settles it (`accepted`
// true); an answer to a wait that has ended or of another pane, or one that
// breaks these rules (a request for changes without a review, a review that
// is empty or over 1 MiB, an unknown outcome, a field of the wrong type)
// settles nothing. The acknowledgement
// echoes `paneId` and `waitId` when they are strings of at most 128 bytes,
// else "". A window marks the review's entries (mark_sent) only after
// `accepted` true. Transient, connection-bound, no generic event ID.
export type ReviewClientMessage = {
  type: "viewer.review_answer";
  screenId: string;
  /** The viewer pane. */
  paneId: string;
  waitId: string;
  /** The user's verdict. */
  outcome: "approved" | "changes_requested";
  /**
   * The review, 1 byte to 1 MiB of UTF-8: optional with `approved` (the
   * pending annotations as notes), required with `changes_requested`.
   */
  review?: string;
};
export type ReviewServerMessage = {
  type: "viewer.review_answered";
  paneId: string;
  waitId: string;
  accepted: boolean;
};

// Dedicated desktop commands remain open_local_editor / open_remote_editor
// with {cwd,editor,target?,wslDistro?,kind?:"file"|"folder"}; kind defaults to
// folder for old callers.

// GET /api/client-assignment: browser-gateway-only, authenticated by its local
// session cookie or owner bearer after Host/Origin/Fetch Metadata guards.
// Cache-Control: no-store. Available while the fixed backend reconnects; no
// upstream request. Queries cannot change this assignment. The browser reads
// it before constructing its workspace. All subsequent API/ticket requests send
// X-Splitty-Gateway with this public ID (the isolated cookie name, not its secret).
// A mismatch returns 409 + X-Splitty-Gateway-Changed: 1 before dispatch; the page
// revokes pending delivery and reloads. CLI callers without the header retain
// ordinary authentication. Tickets are valid only at their issuing gateway.
export type BrowserClientAssignment =
  | { gateway: string; role: "main" }
  | { gateway: string; role: "remote"; target: string };

/** Public desktop document assignment, returned only after native caller and
 * readiness checks. The private shell proof/capsule is not part of this value.
 * handle/document are opaque; query parameters never construct them or select
 * a role. Requests, replies and credits belong to this document and generation.
 * Main-window host streams also carry role=remote and their fixed target.
 */
export interface NativeDocumentAssignment {
  handle: string;
  document: string;
  generation: number;
  instance: string;
  role: "main" | "remote";
  target?: string; // required for role=remote; absent for a direct main stream
}

// Native service operations. The checked method/path, role, size, deadline and
// error-style mapping lives in native-operations.json. Unknown operations/arguments
// and authority fields (URL, socket, method, headers, role, backend) are rejected.
// Argument strings are data for the named operation, never routing authority.
// Every argument but body is a string; countArguments are decimal strings of a
// non-negative integer (job.logs: tail, limit, waitMs). body has the same domain
// schema/validation as the mapped browser endpoint; over HTTP it needs a JSON
// Content-Type (application/json or application/*+json), else 415. An argument
// or body that does not decode is answered in the operation's errors style:
// plain 422 {error}, job 400 {code: "invalid_request", error}, viewer
// 400 {code: "invalid", error}. Scope "control" marks backend operations the
// owner control socket serves too.
/** Data returned by theme.get and GET /api/home/theme.
 * id identifies the actual resolved source, including a fallback source;
 * name remains an authored display name. Older replies may omit id.
 * terminal holds the colors of the terminal palette the stored settings choose
 * for this theme's variant: terminal.theme unless "default", else
 * terminal.themeDark / terminal.themeLight unless "default", else this theme's
 * own. terminalPalette is the id of the theme they come from; a chosen name the
 * server has no theme for gives this theme's own colors and id. Older replies
 * omit terminalPalette and always carry the theme's own terminal colors.
 */
export interface ResolvedNativeTheme {
  id?: string;
  name: string;
  variant: "light" | "dark";
  ui: Record<string, string>;
  terminal: Record<string, string>;
  terminalPalette?: string;
  effects?: Record<string, string>;
  layout: {
    "dnd-overlay-scope": string;
    "dnd-overlay-target": string;
  };
}

export type NativeCall =
  | { operation: "home.get"; args?: { } }
  | { operation: "home.metadata"; args?: { } }
  | { operation: "home.update"; args: { body: unknown; } }
  | { operation: "theme.get"; args?: { name?: string; } }
  | { operation: "theme.list"; args?: { } }
  | { operation: "shellProfiles.get"; args?: { } }
  | { operation: "shellProfiles.update"; args: { body: unknown; } }
  | { operation: "template.extract"; args: { body: { screenId: string; name?: string; root?: string } } }
  | { operation: "template.list"; args?: { } }
  | { operation: "template.get"; args: { slug: string; } }
  | { operation: "template.save"; args: { slug: string; body: unknown; } }
  | { operation: "template.create"; args: { body: SaveTemplateByName; } }
  | { operation: "template.delete"; args: { slug: string; } }
  | { operation: "build.get"; args?: { } }
  | { operation: "command.send"; args: { body: unknown; } }
  | { operation: "mux.state"; args?: { } }
  | { operation: "mux.screens"; args?: { } }
  | { operation: "mux.screen"; args: { id: string; } }
  | { operation: "mux.op"; args: { body: unknown; } }
  | { operation: "mux.neighbor"; args?: { screen?: string; pane?: string; dir?: string; } }
  | { operation: "mux.status"; args?: { } }
  | { operation: "session.send"; args: { id: string; body: unknown; } }
  | { operation: "editor.open"; args: { body: unknown; } }
  | { operation: "file.offer"; args: { body: unknown; } }
  | { operation: "viewer.comments"; args?: { screenId?: string; paneId?: string; document?: string; all?: string; } }
  | { operation: "viewer.commentReply"; args: { body: unknown; } }
  | { operation: "viewer.reviewWait"; args: { body: unknown; } }
  | { operation: "job.list"; args?: { } }
  | { operation: "job.launch"; args: { body: unknown; } }
  | { operation: "job.inspect"; args: { id: string; } }
  | { operation: "job.logs"; args: { id: string; cursor?: string; tail?: string; limit?: string; waitMs?: string; format?: string; } }
  | { operation: "job.input"; args: { id: string; body: unknown; } }
  | { operation: "job.stop"; args: { id: string; } }
  | { operation: "job.forget"; args: { id: string; } }
  | { operation: "host.list"; args?: { } }
  | { operation: "host.forget"; args: { body: unknown; } }
  | { operation: "hostPreferences.get"; args?: { } }
  | { operation: "hostPreferences.update"; args: { body: unknown; } }
  | { operation: "hostOrder.get"; args?: { } }
  | { operation: "hostOrder.update"; args: { body: unknown; } }
  | { operation: "remoteScreens.get"; args?: { } }
  | { operation: "remoteScreens.update"; args: { body: unknown; } }
  | { operation: "remoteScreens.updateHost"; args: { body: unknown; } }
  | { operation: "catalog.get"; args?: { } }
  | { operation: "catalog.update"; args: { body: unknown; } }
  | { operation: "forward.list"; args?: { } }
  | { operation: "forward.open"; args: { body: unknown; } }
  | { operation: "forward.stop"; args: { body: unknown; } }
  | { operation: "forwardPreferences.get"; args?: { } }
  | { operation: "forwardPreferences.save"; args: { body: unknown; } }
  | { operation: "forwardPreferences.forget"; args: { body: unknown; } }
  | { operation: "reverse.list"; args?: { } }
  | { operation: "reverse.open"; args: { body: unknown; } }
  | { operation: "reverse.stop"; args: { body: unknown; } }
  | { operation: "reversePreferences.get"; args?: { } }
  | { operation: "reversePreferences.save"; args: { body: unknown; } }
  | { operation: "reversePreferences.forget"; args: { body: unknown; } };
export type NativeOperation = NativeCall["operation"];

/** `remote-handshake --json [--ensure-daemon]`, executed only on the SSH host.
 * Binary and running service identity are independent. A host path is never a
 * local destination; the connector uses its own prepared remote executable.
 * Legacy TCP fields may be read for diagnosis but confer no native capability.
 * Exit 75 means discovery changed during the read and can be refreshed.
 */
export interface RemoteHandshakeV2 {
  handshake: 2;
  server: {
    version: string;
    protocol: number;
    path: string;
    transportVersions: number[];
  };
  daemon:
    | { running: false }
    | ({
        running: true;
        pid: number;
        version: string;
        protocol: number;
        startedAt: string;
        keepsSessions: boolean;
        started: boolean;
      } & (
        | { legacy?: false; instance: string; transportVersion: number; legacyMigration?: never }
        | { legacy: true; instance?: never; transportVersion?: never; legacyMigration?:
            | { status: "ready"; token: string; sessions: number }
            | { status: "blocked"; message: string } }
      ));
  screens: { id: string; name?: string; panes: number }[];
}

/** `remote-migrate --expected <token>` is an internal, explicitly confirmed
 * host command using the installed replacement binary and its configuration.
 * The 64-character lowercase hexadecimal token binds the observed legacy
 * daemon process generation and record; it is not a bearer credential.
 * The host revalidates identity and keepers before signaling. Exit 75 means
 * stale proof; any potentially executed migration is never replayed by the
 * client. A fresh native handshake verifies successful replacement.
 */

/** Owner Unix socket only, excluded from the browser and native UI catalog.
 * POST /api/daemon/stop compares this immutable instance with the accepting
 * daemon before asking that daemon to shut itself down. A mismatch is 409;
 * success is {stopping: instance}. No PID signal or alternate backend fallback.
 */
export interface StopDaemonInstance { instance: string; }

/** Native transport v1 (independent of the mux version).
 * Stream carriers begin with 53 50 4c 49 54 54 59 01 (SPLITTY + 01), then
 * a Hello from the client; the server returns the same magic and Accept.
 * Every following frame is [kind:u8, payloadLength:u32be, payload]. The 16 MiB
 * ceiling includes this five-byte header; control frames are at most 4096 bytes.
 * Text/Binary/Request/Reply wrap their payload as
 * [generation:u32be, application bytes, receipt:16 random bytes]. Text and API envelopes are UTF-8 JSON. Binary is the existing
 * opaque session binary message, including NUL and non-UTF-8 terminal bytes.
 * Other kinds contain only the JSON shape below. Unknown kinds/fields fail.
 *
 * Accept completes framing, not admission. A helper can send Progress during
 * bounded backend preparation. Ready identifies a verified service; ReadyAccept
 * acknowledges that exact instance/generation before any application input.
 * Each framing/service handshake has a five-second deadline. Mux compatibility
 * is checked independently. A host purpose describes an already assigned host;
 * receiving it never grants the peer permission to select a local backend.
 *
 * Receipt credits acknowledge consumption, once, within their generation.
 * Unpredictable trailing receipts prevent guessed credits or a partial first
 * IPC chunk from releasing undelivered bytes. Charges cover reassembly, queues and in-flight data until consumption;
 * peers cannot increase limits. IPC chunks are at most 64 KiB. Application order
 * is preserved; bounded reserved control capacity carries credit/cancel/ping.
 * The first direct event remains mux.welcome. Host streams report preparation
 * steps as Progress frames before acceptance (`phase` a RemoteStep, or a
 * transport phase such as `connecting`, repeated as a pulse), then send
 * remote.ready before mux.welcome. Liveness is the carrier's: Ping every 15 s
 * while none is unanswered, the connection ends at a check that finds no
 * matching Pong for 30 s; session messages carry no heartbeat.
 * Golden bytes: native-transport-v1.json.
 */
export const NativeFrameKind = {
  Hello: 0, Text: 1, Binary: 2, Accept: 3, Ready: 4, Request: 5, Reply: 6,
  Cancel: 7, Ping: 8, Pong: 9, Credit: 10, Progress: 11, Closed: 12, ReadyAccept: 13,
} as const;
export interface NativeHello {
  transportVersion: 1;
  // Only a main broker or authenticated local gateway may request this on the
  // private owner endpoint. Direct/SSH remote carriers reject it before effects.
  // ownerInstance pins the local hub before it opens the separate host stream.
  openHost?: { ownerInstance: string; target: string; update: boolean };
}
export interface NativeLimits {
  frameBytes: number; // <= 16 MiB, includes header
  apiBytes: number; // <= 1 MiB, smaller per-operation limits still apply
  pendingRequests: number; // <= 64
  connectionBytes: number; // <= 32 MiB, process aggregate <= 128 MiB
}
export interface NativeReady {
  generation: number; // nonzero u32, replaced on backend reconnect
  instance: string; // 32 lowercase hex characters; verified against discovery
  muxProtocol: number;
  purpose: { kind: "direct" } | { kind: "host"; target: string } | { kind: "owner" };
  limits: NativeLimits;
}
export interface NativeReadyAccept { generation: number; instance: string }
// Only `desktop-backend --backend embedded --owner` admits the launcher-owned
// owner purpose. It never appears on a daemon socket, SSH bridge or renderer
// port. Its Text frames carry the commands/results below, not mux operations.
// Binary application bytes are a big-endian u32 stream id followed by 1..65536
// opaque bytes of that stream's independently negotiated native-v1 connection.
// Each id increases across the app lifetime and is never reused. At most 32
// streams, including cleanup, exist; each outgoing stream chunk waits for its
// normal unpredictable native delivery receipt. Allocated duplex/scratch
// buffers are charged as well as framed queued/in-flight bytes.
// The owner always serializes both fields. A local/direct stream uses target:null
// and update:false; a deliberate host stream carries its fixed target and
// update:false. An explicit daemon update carries update:true instead. The
// receiver also accepts omitted target/update as null/false for compatibility.
export interface NativeOwnerOpen { type: "open"; id: number; target?: string | null; update?: boolean }
export interface NativeOwnerClosed { type: "closed"; id: number; message: string; retryable: boolean }
// Closing uses NativeCancel on RESERVED CONTROL capacity, with id identifying
// the substream. Up to 32 cancellations may precede their queued Open; such an
// Open is rejected without service/SSH effects. Closing one stream never ends
// the owned service. Only closing the outer owner pipe ends local sessions/jobs.
// Owner commands are <=4096 UTF-8 bytes, including their metadata. Invalid host
// arguments reject only their Open; they never shut down other local streams.
export type NativeRequest = NativeCall & { id: number }; // increasing nonzero u32 per generation
export interface NativeReply {
  id: number;
  status: number;
  contentType: "json" | "text" | "empty";
  body: string; // serialized JSON or UTF-8 text, no arbitrary response headers
  uncertain?: boolean; // cancellation/loss does not undo an admitted mutation
}
export interface NativeCancel { generation: number; id: number }
// Reserved controls can overtake an API frame. Up to 64 future cancellations
// are remembered within their generation; they receive 499/uncertain:false and
// their later request never starts. An already admitted mutation receives
// uncertain:true; its capacity remains occupied until the task completes.
export interface NativePing { nonce: number } // Pong echoes the nonce
export interface NativeCredit { generation: number; receipt: string } // 32 lowercase hex characters
export interface NativeProgress { phase: string; message: string }
export interface NativeClosed {
  code: "incompatible" | "busy" | "timeout" | "protocol" | "backendLost" | "closed" | "internal";
  message: string; // may contain serialized remote.error for a host preparation failure
  retryable: boolean; // authoritative even when message contains remote.error.retryable
}

// GET /api/mux/status and native mux.status describe the terminal service.
// A browser gateway reports its own address at startup, not in this object.
export interface ServiceStatus {
  serverMode: "embedded" | "daemon";
  capabilities: string[];
  version: string;
  screens: number;
  sessions: number;
  activeScreen: string | null;
  listenAddr: null;
  instance: string;
  transportVersion: 1;
}

/** hub-host-decisions-v1: passive local reads and coalesced invalidation.
 * GET /api/remote/directory (native remote.directory.get, localHub only).
 * Reading never connects a host. Hidden hosts are excluded. Ordered entries
 * combine known hosts, cached screens and active/saved forward/reverse hosts.
 */
interface HubDirectory {
  version: 1;
  order: string[];
  hosts: { target: string; alias?: string; source?: "recent" | "ssh-config";
    lastConnected?: number; lastSeen?: number;
    screens: { id: string; name?: string; panes: number }[] }[];
}
/** Sent only on the local service connection; no application frames are
 * injected into a host relay after remote.ready. Refresh passive reads. */
interface HubChanged { type: "hub.changed" }

/** hub-auto-forward-v1 (local main connections): with remote.autoForwardPorts
 * on, the hub forwards a port that appears in a host's ports.changed after
 * the first list of a relay, as an automatic request (a paused saved rule
 * stays paused, a saved local port is kept, nothing is remembered), once for
 * all relays of the host while one runs, and once per port and process. The
 * forward shows up in GET /api/remote/forwards and is announced by
 * hub.changed; there is no message of its own. A client sends no automatic
 * POST; without the capability nothing is forwarded automatically. */
/** ssh-prompts-v1: the questions OpenSSH asks the local service's askpass
 * helper (passwords, key passphrases, host key confirmations).
 * GET /api/remote/ssh-prompts (native ssh.prompts.get, localHub only) lists
 * them; POST /api/remote/ssh-prompts (native ssh.prompts.answer, body
 * SshPromptAnswer) settles one, once. A question waits at most 110 s. */
interface SshPrompt {
  id: string; // 64 hex digits
  target: string; // the SSH target, at most 4096 bytes
  prompt: string; // OpenSSH's question, untrusted, at most 16 KiB
  confirm: boolean; // a host key to trust: answer "yes"
}
interface SshPromptAnswer {
  id: string;
  answer: string | null; // one line, at most 16 KiB; null cancels
}
interface SshPromptAnswered { accepted: boolean } // false: unknown or settled
/** Sent on local main connections whose welcome lists ssh-prompts-v1, when a
 * question appears or goes and once after the greeting while questions are
 * pending. Carries no question; read them with ssh.prompts.get. Never sent to
 * remote-window connections or into a host relay. */
interface SshPromptsChanged { type: "ssh.prompts.changed" }
/** Ordered follow-up observations; the hub owns validation and persistence.
 * Native remote-window roles and restricted sockets cannot submit this. */
interface HubCatalogObservation {
  type: "hub.catalog.observe";
  target: string;
  /** `panes` may be omitted with hub-host-state-v1 (see PUT /api/remote/catalog). */
  screens: { id: string; name?: string; panes?: number }[];
}
/** hub-host-state-v1 (local main connections): the hub writes remembered
 * screens per host (PUT /api/remote/screens/host, remoteScreens.updateHost)
 * and takes observations without pane counts. */
/**
 * serverDerivedPaneState: ordinary terminal panes follow the owning server's
 * terminal.closeOnExit policy (always / graceful / never); managed jobs remain.
 * A screen whose last pane closes this way is removed with it, announced by a
 * mux.nudge without screenId.
 * Clients omit cwd/profile for inherited splits/tabs unless explicitly selected.
 * Replay bytes before attached are filtered at the server for terminal report
 * queries and clipboard operations. Live bytes and trailing incomplete sequences
 * retain normal behavior. Older-host client replay protection remains enabled.
 */
// Settings authority: GET /api/home/settings/metadata and native home.metadata
// return { defaults: HomeSettings, limits: Record<dottedFieldPath,
// { min?: number; max?: number; step?: number; values?: (string | number)[] }> }.
// PATCH /api/home/settings remains a partial settings request and returns the
// complete effective settings. Validation failure is HTTP 400 with
// { error: string, field: dottedFieldPath }; no change is persisted.
// A field with `values` takes only a listed value; numbers compare by value.
// terminal.xterm.scrollSensitivity and fastScrollSensitivity accept fractions.
// appearance.zoom lists the zoom levels clients step through and offer as its
// `values` (with min and max, no step); older servers publish
// { min: 0.5, max: 3, step: 0.1 } and accept any level in that range.
export interface SettingsChangedEvent {
  type: "settings.changed"; id: string; timestamp: number; settings: unknown;
}
// `created.shellProfileFallbackReason` is "missing", "disabled" or
// "unavailable" when requested shellProfileId could not be used.
// Mux-managed sessions report the same effective shellProfileId and
// shellProfileFallbackReason on `mux.pane_spawned` before a client attaches.
