import {
  subscribe,
  getNotifications,
  getUnreadCountForScreen,
  getUnreadTitlesForScreen,
  getUnreadForPane,
  getUnreadForSession,
  type SplittyNotification,
} from "../lib/notifications.ts";
import type { ConnKey } from "../lib/connection/server_connection.ts";

let rev = $state(0);

subscribe(() => {
  rev++;
});

/** Reactive list of non-dismissed notifications. */
export function notifications(): SplittyNotification[] {
  // Reading `rev` makes this reactive -- Svelte re-evaluates when rev changes.
  void rev;
  return getNotifications();
}

/** Reactive unread count for a screen of `conn`. */
export function unreadCountForScreen(conn: ConnKey, screenId: string): number {
  void rev;
  return getUnreadCountForScreen(conn, screenId);
}

/** Reactive titles of a screen's unread notifications, newest first. */
export function unreadTitlesForScreen(conn: ConnKey, screenId: string): string[] {
  void rev;
  return getUnreadTitlesForScreen(conn, screenId);
}

/** Reactive highest-severity unread notification for a pane of `conn`. */
export function unreadForPane(conn: ConnKey, paneId: string): SplittyNotification | undefined {
  void rev;
  return getUnreadForPane(conn, paneId);
}

/** Reactive highest-severity unread notification for a session of `conn`. */
export function unreadForSession(conn: ConnKey, sessionId: string): SplittyNotification | undefined {
  void rev;
  return getUnreadForSession(conn, sessionId);
}
