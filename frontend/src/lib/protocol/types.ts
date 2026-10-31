// WebSocket JSON wire format for the Splitty protocol.

// --- Template Types ---

/**
 * How Splitty restores a pane command:
 * - replay: start a normal shell and type the command into it
 * - shell: run via interactive shell init, then drop back into a shell
 * - close: run via interactive shell init, then close when the command exits
 */
export type RestoreMode = "replay" | "shell" | "close";

export interface TemplatePane {
  type?: "terminal" | "webview";
  url?: string;
  command?: string;
  dir?: string;
  restore_mode?: RestoreMode;
  shell_profile?: string;
}

export interface TemplateLayoutNode {
  slot?: string[];
  split?: "horizontal" | "vertical";
  children?: TemplateLayoutNode[];
  ratio?: number;
}

export interface TemplateFloatingEntry {
  layout: TemplateLayoutNode;
  bounds: { x: number; y: number; w: number; h: number };
}

export interface Template {
  name: string;
  panes: Record<string, TemplatePane>;
  layout: TemplateLayoutNode;
  floating?: TemplateFloatingEntry[];
}

export interface TemplateSummary {
  slug: string;
  displayName: string;
}

// --- Client Commands ---

export type ClientCommand =
  | { type: "pane.activate"; paneId?: string }
  | { type: "pane.copy"; paneId?: string }
  | { type: "pane.paste"; paneId?: string }
  | { type: "pane.clear"; paneId?: string }
  | { type: "pane.close"; paneId?: string }
  | { type: "pane.split"; paneId?: string; direction: "right" | "below" };

// --- Client -> Server ---

export type EditorOutcome = "dispatched" | "denied" | "failed" | "expired" | "disconnected";
export interface EditorRequest {
  type: "editor.request";
  requestId: string;
  sessionId: string;
  path: string;
  kind: "file" | "folder";
  editor?: "vscode" | "zed" | null;
  expiresAt: number;
}
export type IncomingMessage =
  | { type: "editor.register" }
  | { type: "editor.result"; requestId: string; outcome: EditorOutcome }
  | { type: "create"; shell?: string; shellProfileId?: string; cols?: number; rows?: number; command?: string; dir?: string; root?: string; restoreMode?: RestoreMode; paneId?: string; screenId?: string }
  | { type: "attach"; id: string; cols?: number; rows?: number }
  | { type: "detach"; id: string }
  | { type: "input"; id: string; data: string }
  // The size this connection shows the session at; `redraw` after an attach (`pty-size-v1`).
  | { type: "resize"; id: string; cols: number; rows: number; redraw?: boolean }
  | { type: "close"; id: string }
  // PTY size policy (`pty-size-v1`): the user acted / the window is hidden / a session kept but not shown.
  | { type: "activity"; takeover?: boolean }
  | { type: "idle" }
  | { type: "hide"; id: string }
  // An image for a pane's paste, base64; answered by `image_saved` or `image_failed`.
  | { type: "save_image"; requestId: string; data: string }
  // A chunk of a file for a pane's drop or paste, at most 1 MiB as base64; the first
  // (`offset` 0) names the file and its size. Answered by `upload_progress`, `upload_saved` or `upload_failed`.
  | { type: "upload"; requestId: string; offset: number; data: string; name?: string; size?: number }
  // File offers (`file-offer-v1`): receive them, take or decline one, read the taken one chunk by chunk.
  | { type: "file.register" }
  | { type: "file.accept"; offerId: string }
  | { type: "file.decline"; offerId: string }
  | { type: "file.read"; offerId: string; offset: number }
  // Mux messages
  | { type: "mux.op"; screenId: string; op: MuxOperation }
  | { type: "mux.resync"; screenId: string };

// --- Server -> Client: Base Types ---

/** Base for all JSON server events. Output excluded (binary frame). */
export interface ServerEvent {
  type: string;
  id: string;
  timestamp: number;
}

/** Base for events scoped to a terminal session. */
export interface SessionEvent extends ServerEvent {
  sessionId: string;
}

// --- Server -> Client: Session Lifecycle ---

export interface SessionCreated extends SessionEvent { type: "created"; shellProfileId?: string; homeDir?: string; semanticPhase?: string }
export interface SessionAttached extends SessionEvent { type: "attached"; homeDir?: string; semanticPhase?: string; semanticExitCode?: number; semanticCommandCount?: number }
export interface SessionClosed extends SessionEvent { type: "closed"; exitCode?: number; signal?: string }
export interface SessionError extends ServerEvent { type: "error"; sessionId?: string; error: string }

// --- Server -> Client: Pasted Images ---

export interface ImageSaved extends ServerEvent { type: "image_saved"; requestId: string; path: string }
export interface ImageFailed extends ServerEvent { type: "image_failed"; requestId: string; error: string }

// --- Server -> Client: Uploaded Files ---

export interface UploadProgress extends ServerEvent { type: "upload_progress"; requestId: string; received: number }
export interface UploadSaved extends ServerEvent { type: "upload_saved"; requestId: string; path: string }
export interface UploadFailed extends ServerEvent { type: "upload_failed"; requestId: string; error: string }

// --- Server -> Client: File Offers (`file-offer-v1`) ---

/** A file a pane's process offers for download (`splitty download`). Untrusted host data. */
export interface FileOffer extends ServerEvent { type: "file.offer"; offerId: string; sessionId: string; name: string; size: number; expiresAt: number }
/** A chunk of the taken offer at `offset`, base64; `eof` on the last. */
export interface FileChunk extends ServerEvent { type: "file.chunk"; offerId: string; offset: number; data: string; eof: boolean }
export type FileClosedReason = "taken" | "expired" | "declined" | "disconnected" | "failed";
export interface FileClosed extends ServerEvent { type: "file.closed"; offerId: string; reason: FileClosedReason }

// --- Server -> Client: Session Stream Events ---

export interface TitleChanged extends SessionEvent { type: "title"; title: string; cwd: string; cmdline?: string; baseShell: boolean; oscTitle?: string }
export interface SemanticBoundary extends SessionEvent { type: "osc133"; action: string; exitCode?: number }
export interface Bell extends SessionEvent { type: "bell" }
export type SizePolicy = "active" | "latest" | "smallest";
/** The session's PTY size as its server's size policy set it (`pty-size-v1`). */
export interface SessionSize extends SessionEvent { type: "size"; cols: number; rows: number; policy: SizePolicy }

// --- Server -> Client: Process Events ---

export interface ProcessFinished extends SessionEvent { type: "process.finished"; process: string; cmdline?: string }
export interface ProcessIdle extends SessionEvent { type: "process.idle"; process: string; exitCode?: number }
export interface SessionExited extends SessionEvent { type: "session.exited"; exitCode: number; process?: string; command?: string; userInitiated: boolean }

// --- Server -> Client: Notification (generic fallback) ---

export type NotificationSeverity = "info" | "success" | "warning" | "error";

export interface NotificationMessage extends ServerEvent {
  type: "notification";
  severity: NotificationSeverity;
  title: string;
  body?: string;
  sessionId?: string;
}

// --- Server -> Client: Mux ---

export interface MuxWelcome extends ServerEvent { type: "mux.welcome"; clientId: string; screens: ScreenInfo[]; serverMode?: "embedded" | "daemon"; capabilities?: string[] }
export interface MuxScreenSnapshot extends ServerEvent { type: "mux.screen_snapshot"; screen: ScreenState }
export interface MuxNudge extends ServerEvent { type: "mux.nudge"; reason: string; screenId?: string }
export interface MuxPaneSpawned extends ServerEvent { type: "mux.pane_spawned"; paneId: string; sessionId: string }
export interface MuxOpFailed extends ServerEvent { type: "mux.op_failed"; op: string; screenId?: string; paneId?: string; error: string }

// --- Server -> Client: Listening Ports ---

/** A TCP port a process of a session listens on. Untrusted host data. */
export interface ListeningPort { port: number; sessionId: string; process: string }
/** The server's full list of listening ports, after `mux.welcome` and on change (Linux only). */
export interface PortsChanged extends ServerEvent { type: "ports.changed"; ports: ListeningPort[] }

// --- Server -> Client: Commands ---

export interface ServerCommand extends ServerEvent { type: "command"; command: ClientCommand }

// --- Server -> Client: Union ---

export type OutgoingMessage =
  | EditorRequest
  | { type: "editor.cancelled"; requestId: string }
  // Session lifecycle
  | SessionCreated
  | SessionAttached
  | SessionClosed
  | SessionError
  // Pasted images
  | ImageSaved
  | ImageFailed
  // Uploaded files
  | UploadProgress
  | UploadSaved
  | UploadFailed
  // File offers
  | FileOffer
  | FileChunk
  | FileClosed
  // Session stream
  | TitleChanged
  | SemanticBoundary
  | Bell
  | SessionSize
  // Process events
  | ProcessFinished
  | ProcessIdle
  | SessionExited
  // Notification
  | NotificationMessage
  // Mux
  | MuxWelcome
  | MuxScreenSnapshot
  | MuxNudge
  | MuxPaneSpawned
  | MuxOpFailed
  // Listening ports
  | PortsChanged
  // Commands
  | ServerCommand;

// --- Mux Domain Model ---

export interface LayoutNode {
  type: "split" | "slot";
  id: string;
  // Split fields
  axis?: "horizontal" | "vertical";
  children?: LayoutNode[];
  sizes?: number[];
  // Slot fields
  paneIds?: string[];
  activePaneId?: string;
}

/**
 * A layout tree as `arrange_panes` carries it: like {@link LayoutNode}, but
 * without node ids, which the server assigns.
 */
export type Arrangement =
  | { type: "slot"; paneIds: string[]; activePaneId?: string }
  | { type: "split"; axis: "horizontal" | "vertical"; children: Arrangement[]; sizes: number[] };

export interface FloatingEntry {
  id: string;
  root: LayoutNode;
  bounds: { x: number; y: number; w: number; h: number };
}

export interface PaneState {
  id: string;
  /** Managed identity survives exit; never replace a missing job with a shell. */
  jobId?: string;
  sessionId?: string;
  title?: string;
  cwd?: string;
  role?: "terminal" | "webview" | "viewer";
}

export interface SessionState {
  id: string;
  jobId?: string;
  restartPolicy?: "shell" | "never";
  shellProfileId?: string;
  command?: string;
  cwd?: string;
  shell?: string;
  status: "starting" | "running" | "exited";
  exitCode?: number;
  /** The session's scratchpad (`session-scratchpad`); absent while empty. */
  scratchpad?: string;
}

export interface ScreenState {
  id: string;
  name?: string;
  revision: number;
  root: LayoutNode;
  floating: FloatingEntry[];
  panes: Record<string, PaneState>;
  sessions: Record<string, SessionState>;
  /** Lifecycle projection only, not the persisted launch/authority registry. */
  jobs?: Record<string, { jobId: string; state: JobState; error?: string | null; result?: JobResult }>;
}

export interface ScreenInfo {
  id: string;
  name?: string;
}

// --- Mux Operations ---

export type JobState = "starting" | "running" | "stopping" | "exited" | "failed" | "interrupted";
export interface JobResult { exitCode: number | null; signal: string | null }
export type JobPlacement =
  | { kind: "screen" }
  | { kind: "tab"; screenId?: string; paneId?: string }
  | { kind: "split"; screenId?: string; paneId?: string; direction: "left" | "right" | "above" | "below" };
/** POST /api/jobs. Ownership comes from the transport, never this object. */
export interface JobLaunch {
  requestId: string; argv: string[]; cwd: string; name?: string | null;
  placement: JobPlacement; newScreenId?: string; newPaneId?: string; background?: true;
}
export interface ManagedJob extends JobResult {
  jobId: string; requestId: string; screenId: string; paneId: string; sessionId: string;
  argv: string[]; cwd: string; name: string | null; createdBy: string | null; owner: string | null;
  createdAt: number; startedAt: number | null; endedAt: number | null; state: JobState; error: string | null;
}
/** Opaque cursors are bound to one job, output generation and representation. */
export interface JobLogs {
  data: string; encoding: "utf8" | "base64";
  nextCursor: string | null; oldestCursor: string | null; endCursor: string | null;
  lostBytes: number; hasMore: boolean; state: JobState; outputComplete: boolean; outputAvailable: boolean;
}
/** Owner-socket administration only; these IDs are not network credentials. */
export interface AgentGrant {
  agentId: string; name: string; socket: string; anchor: {screenId: string; paneId: string} | null; revoked: boolean;
}
export interface JobControlError {
  code: string; error: string; requestId?: string; jobId?: string; screenId?: string; paneId?: string; sessionId?: string;
}

export type MuxOperation =
  | ({ type: "create_job" } & JobLaunch)
  | { type: "stop_job" | "forget_job"; jobId: string }
  | { type: "split_pane"; paneId: string; axis: "horizontal" | "vertical"; newPaneId?: string; resultingSizes?: number[]; newPaneIndex?: number; cwd?: string; shellProfileId?: string }
  | { type: "new_tab"; paneId: string; newPaneId?: string; cwd?: string; shellProfileId?: string }
  | { type: "close_pane"; paneId: string; resultingSizes?: number[] }
  | { type: "resize_split"; nodeId: string; sizes: number[] }
  | { type: "move_pane"; paneId: string; targetSlotId?: string; targetSplitId?: string; direction?: "right" | "below" | "left" | "above"; resultingSizes?: number[]; sourceResultingSizes?: number[] }
  // The window's grid after a drag; only the panes the root layout holds (`arrange-panes`).
  | { type: "arrange_panes"; root: Arrangement }
  | { type: "select_tab"; paneId: string }
  | { type: "create_screen"; screenId?: string; name?: string; newPaneId?: string; shellProfileId?: string }
  | { type: "close_screen"; screenId: string }
  | { type: "set_active_screen"; screenId: string }
  | { type: "rename_screen"; screenId: string; name: string }
  | { type: "reorder_screens"; screenOrder: string[] }
  // The pane and its running session go to another screen of the same server, or a new one.
  | { type: "move_pane_to_screen"; paneId: string; targetScreenId: string; createScreen?: boolean; screenName?: string }
  | { type: "float_pane"; paneId: string; bounds?: { x: number; y: number; w: number; h: number } }
  | { type: "dock_pane"; floatingId: string; paneId?: string }
  | { type: "move_float"; floatingId: string; bounds: { x: number; y: number; w: number; h: number } }
  // A session's whole scratchpad; only a window sends it (`session-scratchpad`).
  | { type: "set_scratchpad"; sessionId: string; text: string };
