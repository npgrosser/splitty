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
  slot?: string | string[];
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
  | { type: "hub.catalog.observe"; target: string; screens: readonly {id: string; name?: string; panes: number}[] }
  | { type: "mux.extract_template"; requestId: string; screenId: string; name: string; root?: string }
  | { type: "mux.assess_close"; requestId: string; scope: CloseScope }
  | { type: "editor.register" }
  | { type: "editor.result"; requestId: string; outcome: EditorOutcome }
  | { type: "create"; shell?: string; shellProfileId?: string; cols?: number; rows?: number; command?: string; dir?: string; root?: string; restoreMode?: RestoreMode; paneId?: string; screenId?: string }
  | { type: "attach"; id: string; cols?: number; rows?: number }
  | { type: "detach"; id: string }
  | { type: "input"; id: string; data: string }
  | { type: "input_bytes"; id: string; data: string }
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
  | { type: "upload"; requestId: string; offset: number; data: string; name?: string; size?: number; dir?: string }
  | { type: "upload_cancel"; requestId: string }
  // File offers (`file-offer-v1`): receive them, take or decline one, read the taken one chunk by chunk.
  | { type: "file.register" }
  | { type: "file.accept"; offerId: string }
  | { type: "file.decline"; offerId: string }
  | { type: "file.read"; offerId: string; offset: number }
  // A viewer pane's document (`viewer-v1`), read in chunks: offset 0 first, every later
  // chunk with the `version` of the first answer; `ifVersion` (offset 0 only) asks for
  // `viewer.unchanged` when the document still has that version.
  | { type: "viewer.read"; requestId: string; paneId: string; offset: number; version?: string; ifVersion?: string }
  // The window's answer to a process that waits for a review of a viewer pane
  // (`viewer-annotations-v1`): a verdict, with a review of at most 1 MiB that an
  // approval may and a request for changes must carry.
  // Answered by `viewer.review_answered` to this connection only.
  | ViewerReviewAnswer
  // Mux messages
  | { type: "mux.op"; screenId: string; op: MuxOperation }
  | { type: "mux.resync"; screenId: string }
  // The ordered screen catalog (`workspace-catalog-v1`), answered by `mux.workspace_snapshot`.
  | { type: "mux.workspace_resync" };

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

export interface SessionCreated extends SessionEvent { type: "created"; shellProfileId?: string; shellProfileFallbackReason?: "missing" | "disabled" | "unavailable"; homeDir?: string; semanticPhase?: string }
export interface SettingsChanged extends ServerEvent { type: "settings.changed"; settings: import("../settings.ts").SplittySettings }
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

// --- Server -> Client: Viewer Documents (`viewer-v1`) ---

/** A chunk of a viewer pane's document at `offset`: base64, at most 1 MiB decoded; `size` is the whole document's. */
export interface ViewerChunk extends ServerEvent { type: "viewer.chunk"; requestId: string; offset: number; size: number; version: string; data: string; eof: boolean }
/** The document still has the version the read named in `ifVersion`. */
export interface ViewerUnchanged extends ServerEvent { type: "viewer.unchanged"; requestId: string; version: string }
export type ViewerFailureCode = "unknown_pane" | "not_found" | "not_a_file" | "too_large" | "unreadable" | "changed" | "busy" | "invalid";
export interface ViewerFailed extends ServerEvent { type: "viewer.failed"; requestId: string; code: ViewerFailureCode; error: string }

// --- Viewer Reviews (`viewer-annotations-v1`) ---

/** The user's verdict on a document a process waits for. */
export type ReviewOutcome = "approved" | "changes_requested";
/** A window's answer to the wait `waitId` on viewer pane `paneId`. */
export interface ViewerReviewAnswer {
  type: "viewer.review_answer";
  screenId: string;
  paneId: string;
  waitId: string;
  outcome: ReviewOutcome;
  /** The review: optional with `approved` (its notes), required with `changes_requested`. */
  review?: string;
}
/** The server's acknowledgement of one `viewer.review_answer`; `accepted` only for the answer that settled the wait. */
export interface ViewerReviewAnswered extends ServerEvent { type: "viewer.review_answered"; paneId: string; waitId: string; accepted: boolean }

// --- Server -> Client: Session Stream Events ---

export interface TitleChanged extends SessionEvent { type: "title"; title: string; cwd: string; cmdline?: string; baseShell: boolean; oscTitle?: string; displayTitle?: string; busy?: boolean }
export interface SemanticBoundary extends SessionEvent { type: "osc133"; action: string; exitCode?: number }
export interface Bell extends SessionEvent { type: "bell" }
export type SizePolicy = "active" | "latest" | "smallest";
/** The session's PTY size as its server's size policy set it (`pty-size-v1`). */
export interface SessionSize extends SessionEvent { type: "size"; cols: number; rows: number; policy: SizePolicy }

// --- Server -> Client: Process Events ---

export type NotificationSeverity = "info" | "success" | "warning" | "error";
/** A `notifications.*` switch besides `enabled`. */
export type NotificationSwitch = "processFinished" | "processExited" | "processCrashed" | "programStatus";

/**
 * What a window shows for a notification event, as the server classified it
 * (`notification-notices-v1`). Untrusted host data: the title is shown as
 * plain text, unknown values are ignored.
 */
export interface Notice {
  kind: string;
  severity: NotificationSeverity;
  title: string;
  setting?: NotificationSwitch;
  /** Why no window notifies, for example `user-initiated`. */
  suppressed?: string;
}

export interface ProcessFinished extends SessionEvent { type: "process.finished"; process: string; cmdline?: string; notice?: Notice }
export interface ProcessIdle extends SessionEvent { type: "process.idle"; process: string; exitCode?: number; notice?: Notice }
export interface SessionExited extends SessionEvent { type: "session.exited"; exitCode: number; process?: string; command?: string; userInitiated: boolean; notice?: Notice }

/** A session's root program status record changed; the web client follows only its notice. */
export interface ProgramStatusChanged extends ServerEvent {
  type: "program_status.changed";
  sessionId: string;
  paneId?: string;
  screenId?: string;
  status: Record<string, unknown> | null;
  notify?: boolean;
  notice?: Notice;
}

// --- Server -> Client: Notification (generic fallback) ---

export interface NotificationMessage extends ServerEvent {
  type: "notification";
  severity: NotificationSeverity;
  title: string;
  body?: string;
  sessionId?: string;
  notice?: Notice;
}

// --- Server -> Client: Mux ---

export interface MuxWelcome extends ServerEvent { type: "mux.welcome"; clientId: string; screens: ScreenInfo[]; serverMode?: "embedded" | "daemon"; capabilities?: string[] }
export interface MuxScreenSnapshot extends ServerEvent { type: "mux.screen_snapshot"; screen: ScreenState }
export interface MuxWorkspaceSnapshot extends ServerEvent { type: "mux.workspace_snapshot"; screens: ScreenInfo[]; activeScreenId: string | null }
export interface MuxNudge extends ServerEvent { type: "mux.nudge"; reason: string; screenId?: string }
export interface MuxPaneSpawned extends ServerEvent { type: "mux.pane_spawned"; paneId: string; sessionId: string; shellProfileId?: string; shellProfileFallbackReason?: "missing" | "disabled" | "unavailable" }
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
  | { type: "hub.changed" }
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
  // Viewer documents
  | ViewerChunk
  | ViewerUnchanged
  | ViewerFailed
  | ViewerReviewAnswered
  // Session stream
  | TitleChanged
  | SemanticBoundary
  | Bell
  | SessionSize
  // Process events
  | ProcessFinished
  | ProcessIdle
  | SessionExited
  | ProgramStatusChanged
  // Notification
  | NotificationMessage
  // Mux
  | { type: "mux.template_extracted"; requestId: string; template?: Template; error?: string }
  | { type: "mux.close_assessed"; requestId: string; assessment?: CloseAssessment; error?: string }
  | MuxWelcome
  | MuxScreenSnapshot
  | MuxWorkspaceSnapshot
  | MuxNudge
  | MuxPaneSpawned
  | MuxOpFailed
  // Listening ports
  | PortsChanged
  // Commands
  | ServerCommand
  | SettingsChanged;

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
  /** A viewer pane's record (`viewer-panes`); absent for other panes and for a malformed record. */
  viewer?: ViewerRecord;
}

/** What a viewer pane shows: a file on its server's machine, or a text its server keeps (never in a snapshot). */
/** A text's `format` `markdown` renders it as a Markdown document; plain text without one. */
export type ViewerSource = { kind: "file"; path: string } | { kind: "text"; title: string; format?: "markdown" };

export interface ViewerRecord {
  source: ViewerSource;
  /** The terminal pane the viewer was opened from. */
  originPaneId?: string;
  /** Starts at 1; a reopen from the CLI raises it, and a window that shows the pane reads it again. */
  revision: number;
  /**
   * The pane's annotations in creation order, each with its thread
   * (`viewer-annotations`); absent while there are none. Kept by the
   * connection's annotation store, never in a panel's params.
   */
  annotations?: ViewerAnnotation[];
  /** The highest number the pane has given an annotation; absent while 0. Never in a panel's params. */
  lastAnnotationNumber?: number;
  /** A process waits for the user's review of the pane's document; the server never persists it. */
  reviewWait?: ReviewWait;
}

/** A process that waits for a review of a viewer pane (`viewer-annotations`, Waiting For A Review). */
export interface ReviewWait {
  /** `wait-` and 16 lowercase hex characters, chosen by the server. */
  id: string;
  /** The waiting process's pane. */
  paneId: string;
  /** That pane's title when the wait started; untrusted text. */
  title: string;
}

/** `comment`, or `change`: a suggested replacement of the passage, where an empty one suggests deleting it. */
export type AnnotationKind = "comment" | "change";

/** A remark on a viewer pane's document, made in a window (`viewer-annotations`); it opens a thread. */
export interface ViewerAnnotation {
  /** `ann-` and 16 lowercase hex characters, chosen by the window. */
  id: string;
  /** Given by the server when it adds the annotation; never in `put_annotations`. */
  number?: number;
  kind: AnnotationKind;
  /** The remark; a comment needs one. */
  body?: string;
  /** The suggested text of a `change`; empty suggests deleting the passage. */
  replacement?: string;
  /** The passage; absent for a general comment, which is a comment. */
  target?: AnnotationTarget;
  /** Absent while the annotation is open. */
  status?: "resolved";
  /** The user handed the annotation itself over to an agent. */
  sent?: true;
  /** The thread, in the order the replies were added; kept by the server, never in `put_annotations`. */
  replies?: AnnotationReply[];
}

/** A reply in an annotation's thread: the user's, from a window, or an agent's, through the control API. */
export interface AnnotationReply {
  /** `rep-` and 16 lowercase hex characters; a window chooses the user's, the server an agent's. */
  id: string;
  author: "user" | "agent";
  body: string;
  /** The user handed the reply over; user replies only. */
  sent?: true;
  /** The pane an agent's reply came from, and that pane's title then; agent replies only, untrusted text. */
  from?: { paneId: string; title: string };
}

/** A reply of the user as `put_replies` carries it. */
export interface UserReply {
  id: string;
  author: "user";
  body: string;
  sent?: true;
}

/** A passage of a document, found again by its quote; the lines only order and label it. */
/**
 * An entry of the user as a review held it, in `mark_sent`: an annotation with its kind,
 * remark, replacement and passage, or a reply of the user with its text in `body`.
 */
export interface SentEntry {
  id: string;
  kind?: AnnotationKind;
  body?: string;
  replacement?: string;
  target?: AnnotationTarget;
}

export interface AnnotationTarget {
  /** The text the quote was taken from: the source, or the text of the rendered view. */
  view: "source" | "rendered";
  quote: string;
  /** Up to 32 characters before and after the quote in that text. */
  prefix: string;
  suffix: string;
  /** 1-based source lines of the passage when it was annotated. */
  startLine: number;
  endLine: number;
}

export type ViewerPlacement = "right" | "left" | "above" | "below" | "tab";

export interface SessionState {
  displayTitle?: string;
  busy?: boolean;
  restoreMode?: RestoreMode;
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
  /** Raised by every accepted save of the scratchpad; absent while 0 (`scratchpad-versions-v1`). */
  scratchpadVersion?: number;
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

export interface ApplyTemplateOp { type: "apply_template"; template: Template; paneIds: Record<string, string>; root?: string; createScreen?: boolean; replaceConfirmed?: true }

/** What a close would end (`mux-protocol`, Close Assessment; docs/protocol.ts). */
export type CloseScope =
  | { kind: "panes"; screenId: string; paneIds: string[] }
  | { kind: "screens"; screenIds: string[] }
  | { kind: "workspace" };

export interface CloseLoss {
  screenId: string;
  paneId?: string;
  sessionId?: string;
  title?: string;
  running: boolean;
  starting: boolean;
  scratchpad: boolean;
  annotations: number;
}

export interface CloseAssessment {
  confirm: boolean;
  running: number;
  starting: number;
  scratchpads: number;
  annotations: number;
  entries: CloseLoss[];
}

export type MuxOperation =
  | ApplyTemplateOp
  | ({ type: "create_job" } & JobLaunch)
  | { type: "stop_job" | "forget_job"; jobId: string }
  | { type: "split_pane"; paneId: string; axis: "horizontal" | "vertical"; newPaneId?: string; resultingSizes?: number[]; newPaneIndex?: number; cwd?: string; shellProfileId?: string }
  | { type: "new_tab"; paneId: string; newPaneId?: string; cwd?: string; shellProfileId?: string }
  | { type: "close_pane"; paneId: string; resultingSizes?: number[] }
  | { type: "resize_split"; nodeId: string; sizes: number[] }
  | { type: "move_pane"; paneId: string; targetSlotId?: string; targetSplitId?: string; direction?: "right" | "below" | "left" | "above"; resultingSizes?: number[]; sourceResultingSizes?: number[] }
  | { type: "move_tab"; paneId: string; sourceSlotId: string; targetSlotId: string; beforePaneId?: string }
  // The window's grid after a drag; only the panes the root layout holds (`arrange-panes`).
  | { type: "arrange_panes"; root: Arrangement }
  | { type: "select_tab"; paneId: string }
  | { type: "create_screen"; screenId?: string; name?: string; newPaneId?: string; shellProfileId?: string; onlyIfEmpty?: boolean }
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
  | { type: "set_scratchpad"; sessionId: string; text: string; ifVersion?: number }
  | OpenViewerOp
  // A viewer pane's annotations, added or replaced by id (without number and
  // replies), and removed with their replies (`viewer-annotations`); a window's alone.
  | { type: "put_annotations"; paneId: string; annotations: ViewerAnnotation[] }
  | { type: "remove_annotations"; paneId: string; ids: string[] }
  // The user's replies in one thread, added or replaced by id, and replies of either author removed.
  | { type: "put_replies"; paneId: string; annotationId: string; replies: UserReply[] }
  | { type: "remove_replies"; paneId: string; annotationId: string; ids: string[] }
  // Annotations and user replies handed over to an agent, each with the content the review
  // held; an entry that reads otherwise by now stays pending, agents' reply ids are ignored.
  | { type: "mark_sent"; paneId: string; entries: SentEntry[] };

/**
 * A viewer pane next to `paneId` (`viewer-panes`, Opening A Viewer); no session, no
 * effect. A window always names `newPaneId` and adds the panel first.
 */
export interface OpenViewerOp {
  type: "open_viewer";
  paneId?: string;
  placement?: ViewerPlacement;
  newPaneId?: string;
  source: { kind: "file"; path: string } | { kind: "text"; title: string; text: string; format?: "markdown" };
  /** A text's directory. */
  cwd?: string;
  /**
   * With a `text` source and no `newPaneId`: replace the text of the viewer of
   * the same title and origin (`viewer-panes`, Reopening A File). The CLI's
   * `splitty review --text` sets it; a window never does.
   */
  replace?: true;
}
