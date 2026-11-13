import { getSettings, subscribe as subscribeSettings, type NotificationSettings } from "./settings.ts";
import type { ConnKey, ServerConnection } from "./connection/server_connection.ts";
import { MAX_TITLE, plainText } from "./protocol/untrusted.ts";
import type { NotificationSeverity, NotificationSwitch, OutgoingMessage } from "./protocol/types.ts";

/**
 * A notification of one window. The server classifies each event (`notice`,
 * `notification-notices-v1`); the window keeps only unread state, which pane
 * it shows, its own switches and the host's name in the title.
 */
export interface SplittyNotification {
  /** `<connection>/<event id>`: two servers may use the same event ids. */
  id: string;
  /** The connection the notification arrived on. */
  conn: ConnKey;
  /** The server's kind, for example `process.finished`; informational. */
  kind: string;
  severity: NotificationSeverity;
  /** For a server other than the local one, prefixed with its name. */
  title: string;
  /** The `notifications.*` switch that governs it. */
  setting?: NotificationSwitch;
  sessionId: string;
  /** The key of the pane's screen in this window, or "". */
  screenId: string;
  paneId: string;
  timestamp: number;
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
const SWITCHES: readonly NotificationSwitch[] = ["processFinished", "processExited", "processCrashed", "programStatus"];
/** The events an older server sends that a window notifies about. */
const NOTIFYING = new Set(["process.finished", "process.idle", "session.exited"]);
/** The events a server classifies; no server builds the generic `notification`. */
const CLASSIFIED = new Set([...NOTIFYING, "program_status.changed"]);

interface Notice { kind: string; severity: NotificationSeverity; title: string; setting?: NotificationSwitch }

/**
 * The event's notice, unless it is suppressed or malformed. Without one (a
 * server without `notification-notices-v1`) a notification event gets a
 * generic notice and `program_status.changed` none.
 */
function noticeOf(msg: OutgoingMessage): Notice | null {
  if (!CLASSIFIED.has(msg.type)) return null;
  const raw = (msg as { notice?: unknown }).notice;
  if (raw === undefined) {
    if (!NOTIFYING.has(msg.type)) return null;
    const process = plainText((msg as { process?: unknown }).process, MAX_TITLE);
    return { kind: msg.type, severity: "info", title: process ? `Activity in ${process}` : "Session activity" };
  }
  if (!raw || typeof raw !== "object") return null;
  const notice = raw as Record<string, unknown>;
  if (notice.suppressed !== undefined && notice.suppressed !== null) return null;
  if (typeof notice.title !== "string") return null;
  return {
    kind: typeof notice.kind === "string" ? plainText(notice.kind, MAX_TITLE) : msg.type,
    severity: SEVERITIES.includes(notice.severity as NotificationSeverity) ? notice.severity as NotificationSeverity : "info",
    title: notice.title,
    // A switch this window does not know governs nothing here.
    setting: SWITCHES.includes(notice.setting as NotificationSwitch) ? notice.setting as NotificationSwitch : undefined,
  };
}

/** Whether the window's switches let a notification governed by `setting` through. */
function switchedOn(ns: NotificationSettings, setting: NotificationSwitch | undefined): boolean {
  // An older server's settings lack `programStatus`, which is on there.
  return ns.enabled && (!setting || ns[setting] !== false);
}

/**
 * The notification for `msg` of `conn`. Panes and screens are looked up in
 * `conn`'s own tables only, and the server's strings are plain text; a server
 * other than the local one is named in the title, where it cannot remove it.
 */
function fromMessage(conn: ServerConnection, msg: OutgoingMessage, notice: Notice): SplittyNotification {
  const raw = msg as { id?: unknown; timestamp?: unknown; sessionId?: unknown };
  const sid = typeof raw.sessionId === "string" ? raw.sessionId : "";
  const paneId = sid ? conn.sessions.paneForSession(sid) ?? "" : "";
  const screenId = paneId ? conn.sessions.screenForPane(paneId) ?? "" : "";
  const text = plainText(notice.title, MAX_TITLE);
  return {
    id: `${conn.key}/${String(raw.id)}`,
    conn: conn.key,
    kind: notice.kind,
    severity: notice.severity,
    title: conn.kind === "local" ? text : `${plainText(conn.label, MAX_TITLE)}: ${text}`,
    setting: notice.setting,
    sessionId: sid,
    screenId,
    paneId,
    timestamp: typeof raw.timestamp === "number" ? raw.timestamp : Date.now(),
    read: false,
    dismissed: false,
  };
}

function shouldDesktopNotify(ns: NotificationSettings): boolean {
  if (typeof document === "undefined" || !document.hidden) return false;
  if (typeof Notification === "undefined") return false;
  if (Notification.permission !== "granted") return false;
  return ns.desktop;
}

function showDesktopNotification(n: SplittyNotification) {
  try {
    const sysNotif = new Notification(n.title, {
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

function handleMessage(conn: ServerConnection, msg: OutgoingMessage) {
  const notice = noticeOf(msg);
  if (!notice) return;
  const ns = getSettings().notifications;
  if (!ns || !switchedOn(ns, notice.setting)) return;
  const n = fromMessage(conn, msg, notice);

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
  if (n.read) return;

  // A notice in the window, and from the system while the window is hidden.
  if (ns.toasts) for (const cb of toastListeners) cb(n);
  if (shouldDesktopNotify(ns)) showDesktopNotification(n);
}

/** A switch turned off also removes the notifications it governs. */
function applySwitches(): void {
  const ns = getSettings().notifications;
  if (!ns) return;
  const kept = notifications.filter((n) => switchedOn(ns, n.setting));
  if (kept.length === notifications.length) return;
  notifications.splice(0, notifications.length, ...kept);
  notify();
}
subscribeSettings(applySwitches);

/** Collect the notifications of a connection. Returns unsubscribe. */
export function attachNotifications(conn: ServerConnection): () => void {
  return conn.socket.onMessage((msg) => handleMessage(conn, msg));
}

/** Subscribe to notification store changes. */
export function subscribe(cb: () => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

/** Subscribe to new notifications that show a notice in the window (toasts). */
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
