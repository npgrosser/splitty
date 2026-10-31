import { getSettings } from "./settings.ts";
import type { ConnKey, ServerConnection } from "./connection/server_connection.ts";
import { MAX_TEXT, MAX_TITLE, plainText } from "./protocol/untrusted.ts";
import type { OutgoingMessage, NotificationSeverity, ProcessFinished, ProcessIdle, SessionExited, NotificationMessage } from "./protocol/types.ts";

export interface SplittyNotification {
  /** `<connection>/<event id>`: two servers may use the same event ids. */
  id: string;
  /** The connection the notification arrived on. */
  conn: ConnKey;
  kind: string;
  severity: NotificationSeverity;
  /** For a server other than the local one, prefixed with its name. */
  title: string;
  body?: string;
  sessionId: string;
  /** The key of the pane's screen in this window, or "". */
  screenId: string;
  paneId: string;
  timestamp: number;
  payload: Record<string, unknown>;
  read: boolean;
  dismissed: boolean;
}

const notifications: SplittyNotification[] = [];
const listeners = new Set<() => void>();
const toastListeners = new Set<(n: SplittyNotification) => void>();
let navigateCallback: ((screenId: string, paneId: string) => void) | null = null;
/** Whether the user looks at a pane of a connection right now; set by the workspace. */
let watching: ((conn: ConnKey, paneId: string) => boolean) | null = null;
let revision = 0;

function notify() {
  revision++;
  for (const cb of listeners) cb();
}

/** Snapshot counter -- changes on every notification mutation. */
export function getRevision(): number {
  return revision;
}

const SEVERITIES: readonly NotificationSeverity[] = ["info", "success", "warning", "error"];

type Common = Omit<SplittyNotification, "kind" | "severity" | "body" | "payload">;

/**
 * The fields every notification of `conn` shares. Panes and screens are looked
 * up in `conn`'s own tables only, and the server's strings are plain text; a
 * server other than the local one is named in the title, where it cannot
 * remove it.
 */
function common(conn: ServerConnection, msg: { id: string; timestamp: number }, sessionId: unknown, title: string): Common {
  const sid = typeof sessionId === "string" ? sessionId : "";
  const paneId = sid ? conn.sessions.paneForSession(sid) ?? "" : "";
  const screenId = paneId ? conn.sessions.screenForPane(paneId) ?? "" : "";
  const text = plainText(title, MAX_TITLE);
  return {
    id: `${conn.key}/${String(msg.id)}`,
    conn: conn.key,
    title: conn.kind === "local" ? text : `${plainText(conn.label, MAX_TITLE)}: ${text}`,
    sessionId: sid,
    screenId,
    paneId,
    timestamp: typeof msg.timestamp === "number" ? msg.timestamp : Date.now(),
    read: false,
    dismissed: false,
  };
}

function fromProcessFinished(conn: ServerConnection, msg: ProcessFinished): SplittyNotification {
  const process = plainText(msg.process, MAX_TITLE);
  return {
    ...common(conn, msg, msg.sessionId, `${process} finished`),
    kind: "process.finished",
    severity: "info",
    payload: { process, cmdline: plainText(msg.cmdline, MAX_TEXT) },
  };
}

function fromProcessIdle(conn: ServerConnection, msg: ProcessIdle): SplittyNotification {
  const process = plainText(msg.process, MAX_TITLE);
  return {
    ...common(conn, msg, msg.sessionId, process ? `${process} is waiting` : "Command finished"),
    kind: "process.idle",
    severity: "info",
    payload: { process, exitCode: msg.exitCode },
  };
}

function fromSessionExited(conn: ServerConnection, msg: SessionExited): SplittyNotification {
  const crashed = msg.exitCode !== 0;
  const process = plainText(msg.process, MAX_TITLE);
  return {
    ...common(conn, msg, msg.sessionId, crashed ? `${process || "session"} failed` : `${process || "session"} exited`),
    kind: crashed ? "session.crashed" : "session.exited",
    severity: crashed ? "error" : "info",
    payload: {
      exitCode: msg.exitCode,
      process,
      command: plainText(msg.command, MAX_TEXT),
      userInitiated: msg.userInitiated === true,
    },
  };
}

function fromGenericNotification(conn: ServerConnection, msg: NotificationMessage): SplittyNotification {
  return {
    ...common(conn, msg, msg.sessionId, msg.title),
    kind: "notification",
    severity: SEVERITIES.includes(msg.severity) ? msg.severity : "info",
    body: typeof msg.body === "string" ? plainText(msg.body, MAX_TEXT) : undefined,
    payload: {},
  };
}

function shouldDesktopNotify(n: SplittyNotification): boolean {
  if (typeof document === "undefined" || !document.hidden) return false;
  if (typeof Notification === "undefined") return false;
  if (Notification.permission !== "granted") return false;

  const ns = getSettings().notifications;
  if (!ns.enabled || !ns.desktop) return false;

  // Check per-type settings
  if (n.kind === "process.finished" && !ns.processFinished) return false;
  if (n.kind === "session.exited" && !ns.processExited) return false;
  if (n.kind === "session.crashed" && !ns.processCrashed) return false;
  // Skip user-initiated session exits
  if ((n.kind === "session.exited" || n.kind === "session.crashed") && n.payload?.userInitiated) {
    return false;
  }

  return true;
}

function showDesktopNotification(n: SplittyNotification) {
  try {
    const sysNotif = new Notification(n.title, {
      body: n.body,
      tag: n.id, // prevents duplicates
    });
    sysNotif.onclick = () => {
      window.focus();
      sysNotif.close();
      if (n.screenId && n.paneId && navigateCallback) {
        navigateCallback(n.screenId, n.paneId);
      }
    };
  } catch {
    // System notifications not available
  }
}

function ingest(n: SplittyNotification | undefined) {
  if (!n) return;

  const ns = getSettings().notifications;

  // Master toggle off → discard completely (no badges, no toasts, no desktop)
  if (!ns.enabled) return;

  // Per-type settings → discard if the specific notification type is disabled
  if (n.kind === "process.finished" && !ns.processFinished) return;
  if (n.kind === "session.exited" && !ns.processExited) return;
  if (n.kind === "session.crashed" && !ns.processCrashed) return;

  // Dedup by id
  if (notifications.some((existing) => existing.id === n.id)) return;

  // What happens in the pane the user looks at is seen as it happens.
  if (n.paneId && watching?.(n.conn, n.paneId)) n.read = true;

  notifications.push(n);

  // Keep max 100 notifications
  if (notifications.length > 100) {
    notifications.splice(0, notifications.length - 100);
  }

  notify();

  // Fire toast listeners for new notifications
  for (const cb of toastListeners) cb(n);

  // Desktop/system notification when window not focused
  if (shouldDesktopNotify(n)) {
    showDesktopNotification(n);
  }
}

function handleMessage(conn: ServerConnection, msg: OutgoingMessage) {
  switch (msg.type) {
    case "process.finished":
      return ingest(fromProcessFinished(conn, msg));
    case "process.idle":
      return ingest(fromProcessIdle(conn, msg));
    case "session.exited":
      return ingest(fromSessionExited(conn, msg));
    case "notification":
      return ingest(fromGenericNotification(conn, msg));
    default:
      return;
  }
}

/** Collect the notifications of a connection. Returns unsubscribe. */
export function attachNotifications(conn: ServerConnection): () => void {
  return conn.socket.onMessage((msg) => handleMessage(conn, msg));
}

/** Subscribe to notification store changes. */
export function subscribe(cb: () => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

/** Subscribe to new notifications (for toasts). */
export function onNewNotification(cb: (n: SplittyNotification) => void): () => void {
  toastListeners.add(cb);
  return () => toastListeners.delete(cb);
}

/** Get all non-dismissed notifications. */
export function getNotifications(): SplittyNotification[] {
  return notifications.filter((n) => !n.dismissed);
}

/** Get unread notification count for a screen of `conn`. */
export function getUnreadCountForScreen(conn: ConnKey, screenId: string): number {
  return notifications.filter((n) => n.conn === conn && n.screenId === screenId && !n.read && !n.dismissed).length;
}

/** The titles of a screen's unread notifications, newest first. */
export function getUnreadTitlesForScreen(conn: ConnKey, screenId: string): string[] {
  return notifications
    .filter((n) => n.conn === conn && n.screenId === screenId && !n.read && !n.dismissed)
    .reverse()
    .map((n) => n.title);
}

/** Get the highest-severity unread notification for a pane of `conn`. */
export function getUnreadForPane(conn: ConnKey, paneId: string): SplittyNotification | undefined {
  const unread = notifications.filter((n) => n.conn === conn && n.paneId === paneId && !n.read && !n.dismissed);
  if (unread.length === 0) return undefined;
  // Priority: error > warning > success > info
  const severityOrder: Record<string, number> = { error: 3, warning: 2, success: 1, info: 0 };
  unread.sort((a, b) => (severityOrder[b.severity] ?? 0) - (severityOrder[a.severity] ?? 0));
  return unread[0];
}

/** Get the highest-severity unread notification for a session of `conn`. */
export function getUnreadForSession(conn: ConnKey, sessionId: string): SplittyNotification | undefined {
  const unread = notifications.filter((n) => n.conn === conn && n.sessionId === sessionId && !n.read && !n.dismissed);
  if (unread.length === 0) return undefined;
  const severityOrder: Record<string, number> = { error: 3, warning: 2, success: 1, info: 0 };
  unread.sort((a, b) => (severityOrder[b.severity] ?? 0) - (severityOrder[a.severity] ?? 0));
  return unread[0];
}

/** Mark all notifications for a pane of `conn` as read. */
export function markPaneRead(conn: ConnKey, paneId: string): void {
  let changed = false;
  for (const n of notifications) {
    if (n.conn === conn && n.paneId === paneId && !n.read) {
      n.read = true;
      changed = true;
    }
  }
  if (changed) notify();
}

/** Mark all notifications for a session of `conn` as read. */
export function markSessionRead(conn: ConnKey, sessionId: string): void {
  let changed = false;
  for (const n of notifications) {
    if (n.conn === conn && n.sessionId === sessionId && !n.read) {
      n.read = true;
      changed = true;
    }
  }
  if (changed) notify();
}

/** Dismiss a notification. */
export function dismissNotification(id: string): void {
  const n = notifications.find((n) => n.id === id);
  if (n && !n.dismissed) {
    n.dismissed = true;
    notify();
  }
}

/** Request desktop notification permission. */
export async function requestDesktopPermission(): Promise<boolean> {
  if (typeof Notification === "undefined") return false;
  if (Notification.permission === "granted") return true;
  if (Notification.permission === "denied") return false;
  const result = await Notification.requestPermission();
  return result === "granted";
}

/** Get current desktop notification permission state. */
export function getDesktopPermission(): string {
  if (typeof Notification === "undefined") return "unavailable";
  return Notification.permission;
}

/**
 * Tell which pane the user looks at (the active pane of the active screen, in a
 * focused window): a notification for it arrives already read.
 */
export function setWatchedPane(cb: ((conn: ConnKey, paneId: string) => boolean) | null): void {
  watching = cb;
}

/** Set the callback used to navigate to a terminal when a desktop notification is clicked. */
export function setNavigateCallback(cb: ((screenId: string, paneId: string) => void) | null): void {
  navigateCallback = cb;
}
