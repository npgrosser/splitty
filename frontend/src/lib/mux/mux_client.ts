/**
 * MuxClient — one server connection's side of the mux protocol.
 *
 * Shadow Mode: The frontend works as before (dockview is source of truth).
 * Ops go out fire-and-forget to keep the server's model in sync.
 *
 * On connect, the server sends a `mux.welcome` with the screen list and client ID.
 * No explicit attach/detach — the client requests screen state via `mux.resync`
 * when it needs the full layout.
 *
 * Every connection has its own client, and a client only ever sees its own
 * socket's messages: a `mux.pane_spawned` resolves only this connection's
 * waiters, a welcome or snapshot reaches only this connection's handlers. Screen
 * ids here are the server's (wire ids); which screen of the window they mean is
 * decided by the screen sync (`lib/connection/screen_sync.ts`).
 */

import type { ConnSocket } from "../protocol/socket.ts";
import type { CloseAssessment, CloseScope, MuxOperation, OpenViewerOp, ScreenInfo, ScreenState, OutgoingMessage, ViewerPlacement, Template } from "../protocol/types.ts";
import { plainText, MAX_TEXT } from "../protocol/untrusted.ts";
import { ANNOTATIONS_CAPABILITY, ANNOTATION_OPS } from "../viewer/annotations.ts";
import { REVIEWS_CAPABILITY } from "../viewer/server_reviews.ts";
import { VIEWER_CAPABILITY } from "../viewer/limits.ts";
import { closeAssessment, screenList, screenState } from "./incoming.ts";

export interface MuxHandlers {
  onWelcome?: (clientId: string, screens: ScreenInfo[]) => void;
  onSnapshot?: (screen: ScreenState) => void;
  onNudge?: (reason: string, screenId?: string) => void;
}

/** An op of this client the server rejected (`mux.op_failed`), with its fields checked. */
export interface OpFailure {
  readonly op: string;
  readonly screenId?: string;
  readonly paneId?: string;
  /** The server's reason, as plain text. */
  readonly error: string;
}

interface PaneSpawnedWaiter {
  resolve: (sessionId: string) => void;
  reject: (err: Error) => void;
}

/** Rejection reason when no `mux.pane_spawned` arrives within the timeout. */
export class PaneSpawnedTimeoutError extends Error {
  constructor(paneId: string, timeoutMs: number) {
    super(`Timeout waiting for pane_spawned (pane=${paneId}, after ${timeoutMs}ms)`);
    this.name = "PaneSpawnedTimeoutError";
  }
}

/** Rejection reason when the server rejected the op that would spawn the pane. */
export class PaneSpawnRejectedError extends Error {
  constructor(paneId: string, op: string, error: string) {
    super(`Server rejected ${op} for pane ${paneId}: ${error}`);
    this.name = "PaneSpawnRejectedError";
  }
}

export const CLOSE_ASSESSMENT_CAPABILITY = "close-assessment-v1";
/** The server names new screens and takes `onlyIfEmpty` (`mux-protocol`, Server-Chosen Screen Names). */
export const SCREEN_NAMES_CAPABILITY = "screen-names-v1";

export class MuxClient {
  /** From the last `mux.welcome`. */
  clientId: string | undefined;
  screens: ScreenInfo[] = [];
  serverMode: "embedded" | "daemon" = "embedded";
  /** Whether the server keeps scratchpads (`session-scratchpad`, Scratchpad Capability). */
  hubHostDecisions = false;
  scratchpads = false;
  /** Whether the server versions scratchpads and takes `ifVersion` (`session-scratchpad`, Concurrent Edits). */
  scratchpadVersions = false;
  /** Whether the server takes a window's grid as `arrange_panes` (mux-protocol, Window Layout Changes Reach the Server). */
  arrangements = false;
  /** Whether the server takes a selected tab as `select_tab` (mux-protocol, Selected Tabs Reach the Server). */
  tabSelections = false;
  /** Whether the server has viewer panes (`viewer-panes`, Viewer Capability); without it the window offers none. */
  viewers = false;
  /** Whether the server keeps annotations on viewer panes (`viewer-annotations`, Annotations Capability); without it the window offers none. */
  annotations = false;
  /** Whether the server anchors annotations and writes their review (`viewer-reviews-v1`); without it the window offers no annotations either. */
  viewerReviews = false;
  templates = false;
  /** Whether the server says what a close would end (`mux-protocol`, Close Assessment). */
  closeAssessments = false;
  /** Whether the server names screens created without a name (`mux-protocol`, Server-Chosen Screen Names). */
  screenNames = false;
  private readonly earlySpawns = new Map<string, string | Error>();
  private readonly templatePanes = new Map<string, readonly string[]>();
  private readonly extractions = new Map<string, { resolve: (template: Template) => void; reject: (error: Error) => void }>();
  private extractionCounter = 0;
  private readonly assessments = new Map<string, { resolve: (assessment: CloseAssessment) => void; reject: (error: Error) => void }>();
  private assessmentCounter = 0;

  // mux-protocol, Client Resynchronization: no `create_screen` before the
  // current connection's `mux.welcome`. Sent earlier, it reaches the server ahead
  // of everything the client could know about the server's screens, and used to
  // replace a screen the server already had. Held ops go out after the welcome.
  private welcomed = false;
  private disconnectedOnce = false;
  private readonly heldCreateScreens: { screenId: string; op: MuxOperation }[] = [];
  private handlers: MuxHandlers = {};
  private readonly waiters = new Map<string, PaneSpawnedWaiter>();
  private readonly failureListeners = new Set<(failure: OpFailure) => void>();
  private readonly workspaceListeners = new Set<(screens: ScreenInfo[]) => void>();
  private unsubscribes: (() => void)[];

  constructor(private readonly socket: ConnSocket) {
    this.unsubscribes = [
      socket.onMessage((msg) => this.handle(msg)),
      socket.onDisconnect(() => {
        this.welcomed = false;
        this.templates = false;
        this.earlySpawns.clear();
        this.templatePanes.clear();
        for (const request of this.extractions.values()) request.reject(new Error("Connection ended during template extraction"));
        this.closeAssessments = false;
        this.screenNames = false;
        for (const request of this.assessments.values()) request.reject(new Error("Connection ended during the close assessment"));
        this.disconnectedOnce = true;
        this.heldCreateScreens.length = 0;
        for (const waiter of this.waiters.values()) waiter.reject(new Error("Connection ended before pane creation was confirmed; its outcome is unknown."));
      }),
    ];
  }

  /** Register the handlers for mux events, replacing earlier ones. Returns a reset. */
  setHandlers(handlers: MuxHandlers): () => void {
    this.handlers = handlers;
    return () => {
      if (this.handlers === handlers) this.handlers = {};
    };
  }

  /** Send a layout operation for a screen (fire-and-forget). */
  sendOp(screenId: string, op: MuxOperation): void {
    if (op.type === "create_screen" && !this.welcomed) {
      if (!this.disconnectedOnce && this.heldCreateScreens.length < 64) this.heldCreateScreens.push({ screenId, op });
      return;
    }
    if (op.type === "apply_template") {
      if (!this.welcomed || !this.templates) throw Error("Update this server to apply templates");
      this.templatePanes.set(screenId, Object.values(op.paneIds));
    }
    this.socket.send({ type: "mux.op", screenId, op });
  }

  extractTemplate(screenId: string, name: string, root = ""): Promise<Template> {
    if (!this.welcomed || !this.templates) return Promise.reject(Error("Update this server to extract templates"));
    if (this.extractions.size >= 8) return Promise.reject(Error("Too many template extraction requests"));
    const requestId = `template-${++this.extractionCounter}`;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => settle(() => reject(Error("Template extraction timed out"))), 10000);
      const settle = (done: () => void) => { clearTimeout(timer); this.extractions.delete(requestId); done(); };
      this.extractions.set(requestId, {
        resolve: template => settle(() => resolve(template)),
        reject: error => settle(() => reject(error)),
      });
      this.socket.send({ type: "mux.extract_template", requestId, screenId, name, root });
    });
  }

  /**
   * What closing `scope` would end, from the server (`mux-protocol`, Close
   * Assessment). Rejects without the capability, on disconnect, on an error
   * answer and after `timeoutMs`; a caller then asks generically.
   */
  assessClose(scope: CloseScope, timeoutMs = 5000): Promise<CloseAssessment> {
    if (!this.welcomed || !this.closeAssessments) return Promise.reject(Error("This server does not assess closes"));
    if (this.assessments.size >= 16) return Promise.reject(Error("Too many close assessments"));
    const requestId = `close-${++this.assessmentCounter}`;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => settle(() => reject(Error("Close assessment timed out"))), timeoutMs);
      const settle = (done: () => void) => { clearTimeout(timer); this.assessments.delete(requestId); done(); };
      this.assessments.set(requestId, {
        resolve: assessment => settle(() => resolve(assessment)),
        reject: error => settle(() => reject(error)),
      });
      this.socket.send({ type: "mux.assess_close", requestId, scope });
    });
  }

  /** Request a full state resync for a screen. */
  resync(screenId: string): void {
    this.socket.send({ type: "mux.resync", screenId });
  }

  /**
   * Ask for the ordered screen list with the names the server gave
   * (`mux.workspace_resync`), after the ops this client sent before.
   * Servers with `screen-names-v1` answer it.
   */
  requestWorkspace(): void {
    this.socket.send({ type: "mux.workspace_resync" });
  }

  /** Called with every `mux.workspace_snapshot`'s screens. Returns the removal. */
  onWorkspace(listener: (screens: ScreenInfo[]) => void): () => void {
    this.workspaceListeners.add(listener);
    return () => this.workspaceListeners.delete(listener);
  }

  /** Called for every op of this client the server rejects. Returns the removal. */
  onOpFailed(listener: (failure: OpFailure) => void): () => void {
    this.failureListeners.add(listener);
    return () => this.failureListeners.delete(listener);
  }

  /**
   * Wait for a mux.pane_spawned event for a specific pane.
   * Resolves with the sessionId when the server spawns the PTY.
   * Rejects with a `PaneSpawnRejectedError` as soon as the server reports the op
   * failed, or with a `PaneSpawnedTimeoutError` after timeoutMs (default 8000ms).
   * Both are recoverable — the caller spawns the session itself instead.
   */
  waitForPaneSpawned(paneId: string, timeoutMs = 8000): Promise<string> {
    const early = this.earlySpawns.get(paneId);
    this.earlySpawns.delete(paneId);
    if (early !== undefined) return early instanceof Error ? Promise.reject(early) : Promise.resolve(early);
    return new Promise<string>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.waiters.delete(paneId);
        reject(new PaneSpawnedTimeoutError(paneId, timeoutMs));
      }, timeoutMs);

      const settle = (fn: () => void) => {
        clearTimeout(timer);
        this.waiters.delete(paneId);
        fn();
      };

      this.waiters.set(paneId, {
        resolve: (sessionId) => settle(() => resolve(sessionId)),
        reject: (err) => settle(() => reject(err)),
      });
    });
  }

  /** Stop listening and revoke waits owned by this connection. */
  dispose(): void {
    for (const unsubscribe of this.unsubscribes) unsubscribe();
    this.unsubscribes = [];
    this.handlers = {};
    this.failureListeners.clear();
    this.workspaceListeners.clear();
    this.earlySpawns.clear();
    this.templatePanes.clear();
    for (const request of this.extractions.values()) request.reject(Error("Connection disposed"));
    for (const request of this.assessments.values()) request.reject(Error("Connection disposed"));
    for (const waiter of this.waiters.values()) waiter.reject(new Error("Connection disposed before pane creation was confirmed."));
    this.heldCreateScreens.length = 0;
  }

  private handle(msg: OutgoingMessage): void {
    switch (msg.type) {
      case "mux.welcome": {
        const screens = screenList(msg.screens);
        this.clientId = typeof msg.clientId === "string" ? msg.clientId : undefined;
        this.screens = screens;
        this.serverMode = msg.serverMode === "daemon" ? "daemon" : "embedded";
        this.hubHostDecisions = Array.isArray(msg.capabilities) && msg.capabilities.includes("hub-host-decisions-v1");
        this.scratchpads = Array.isArray(msg.capabilities) && msg.capabilities.includes("scratchpad-v1");
        this.scratchpadVersions = Array.isArray(msg.capabilities) && msg.capabilities.includes("scratchpad-versions-v1");
        this.arrangements = Array.isArray(msg.capabilities) && msg.capabilities.includes("layout-arrange-v1");
        this.tabSelections = Array.isArray(msg.capabilities) && msg.capabilities.includes("tab-select-v1");
        this.viewers = Array.isArray(msg.capabilities) && msg.capabilities.includes(VIEWER_CAPABILITY);
        this.templates = Array.isArray(msg.capabilities) && msg.capabilities.includes("server-templates-v1");
        this.annotations = Array.isArray(msg.capabilities) && msg.capabilities.includes(ANNOTATIONS_CAPABILITY);
        this.viewerReviews = Array.isArray(msg.capabilities) && msg.capabilities.includes(REVIEWS_CAPABILITY);
        this.closeAssessments = Array.isArray(msg.capabilities) && msg.capabilities.includes(CLOSE_ASSESSMENT_CAPABILITY);
        this.screenNames = Array.isArray(msg.capabilities) && msg.capabilities.includes(SCREEN_NAMES_CAPABILITY);
        this.welcomed = true;
        this.handlers.onWelcome?.(this.clientId ?? "", screens);
        for (const { screenId, op } of this.heldCreateScreens.splice(0)) {
          this.socket.send({ type: "mux.op", screenId, op });
        }
        break;
      }

      case "mux.screen_snapshot": {
        const screen = screenState(msg.screen);
        if (screen) this.handlers.onSnapshot?.(screen);
        break;
      }

      case "mux.workspace_snapshot": {
        const screens = screenList(msg.screens);
        for (const listener of [...this.workspaceListeners]) listener(screens);
        break;
      }

      case "mux.nudge":
        this.handlers.onNudge?.(
          typeof msg.reason === "string" ? msg.reason : "",
          typeof msg.screenId === "string" ? msg.screenId : undefined,
        );
        break;

      case "mux.template_extracted": {
        const request = this.extractions.get(msg.requestId);
        if (typeof msg.error === "string") request?.reject(Error(plainText(msg.error, MAX_TEXT)));
        else if (msg.template && typeof msg.template === "object") request?.resolve(msg.template);
        else request?.reject(Error("Invalid extracted template"));
        break;
      }

      case "mux.close_assessed": {
        const request = this.assessments.get(msg.requestId);
        const assessment = closeAssessment(msg.assessment);
        if (typeof msg.error === "string") request?.reject(Error(plainText(msg.error, MAX_TEXT)));
        else if (assessment) request?.resolve(assessment);
        else request?.reject(Error("Invalid close assessment"));
        break;
      }

      case "mux.pane_spawned":
        if (typeof msg.paneId === "string" && typeof msg.sessionId === "string") {
          const waiter = this.waiters.get(msg.paneId);
          if (waiter) waiter.resolve(msg.sessionId);
          else if ([...this.templatePanes.values()].some(ids => ids.includes(msg.paneId))) {
            if (this.earlySpawns.size >= 256) this.earlySpawns.delete(this.earlySpawns.keys().next().value!);
            this.earlySpawns.set(msg.paneId, msg.sessionId);
          }
        }
        break;

      case "mux.op_failed": {
        const op = typeof msg.op === "string" ? msg.op : "";
        const screenId = typeof msg.screenId === "string" && msg.screenId.length > 0 ? msg.screenId : undefined;
        const paneId = typeof msg.paneId === "string" ? msg.paneId : undefined;
        // An annotation op names the viewer pane it was for, which nothing waits for.
        if (paneId !== undefined && !ANNOTATION_OPS.has(op)) {
          this.waiters
            .get(paneId)
            ?.reject(new PaneSpawnRejectedError(paneId, op, String(msg.error)));
        }
        if (op === "apply_template" && screenId !== undefined) {
          const ids = paneId ? [paneId] : this.templatePanes.get(screenId) ?? [];
          for (const id of ids) {
            const error = new PaneSpawnRejectedError(id, op, String(msg.error));
            const waiter = this.waiters.get(id);
            if (waiter) waiter.reject(error);
            else this.earlySpawns.set(id, error);
          }
        }
        const failure: OpFailure = {
          op,
          ...(screenId !== undefined ? { screenId } : {}),
          ...(paneId !== undefined ? { paneId } : {}),
          error: plainText(msg.error, MAX_TEXT),
        };
        for (const listener of [...this.failureListeners]) listener(failure);
        // Any rejected optimistic edit can have left the local screen stale,
        // including operations that create no pane and have no spawn waiter.
        if (screenId !== undefined) this.resync(screenId);
        break;
      }
    }
  }
}

// ── Op builders ──

export function splitOp(
  paneId: string,
  newPaneId: string,
  direction: "right" | "below",
  cwd?: string,
  shellProfileId?: string,
): MuxOperation {
  const axis = direction === "right" ? "horizontal" : "vertical";
  return { type: "split_pane", paneId, axis, newPaneId, cwd, shellProfileId };
}

/** A new tab in the same slot as `paneId`. */
export function newTabOp(paneId: string, newPaneId: string, cwd?: string, shellProfileId?: string): MuxOperation {
  return { type: "new_tab", paneId, newPaneId, cwd, shellProfileId };
}

/**
 * `create_screen`; without `name` the server names it. `onlyIfEmpty`: the
 * workspace's first screen, refused once the workspace has one
 * (`workspace-model`, First Screen of an Empty Workspace).
 */
export function createScreenOp(
  screenId: string,
  name?: string,
  initialPaneId?: string,
  shellProfileId?: string,
  options?: { onlyIfEmpty?: boolean },
): MuxOperation {
  return {
    type: "create_screen", screenId, name, newPaneId: initialPaneId, shellProfileId,
    ...(options?.onlyIfEmpty ? { onlyIfEmpty: true } : {}),
  };
}

/** A viewer pane `newPaneId` for `source`, placed next to `paneId` (`viewer-panes`, Opening A Viewer). */
export function openViewerOp(
  newPaneId: string,
  source: OpenViewerOp["source"],
  options: { paneId?: string; placement?: ViewerPlacement; cwd?: string } = {},
): OpenViewerOp {
  return {
    type: "open_viewer",
    ...(options.paneId ? { paneId: options.paneId } : {}),
    ...(options.placement ? { placement: options.placement } : {}),
    newPaneId,
    source,
    ...(options.cwd ? { cwd: options.cwd } : {}),
  };
}
