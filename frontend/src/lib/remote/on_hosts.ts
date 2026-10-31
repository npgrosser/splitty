// Host groups outlive their sockets. The catalog is an observation, never an
// instruction to create or attach a screen on a host.
import { hostKey, hostOf, type ConnKey } from "../connection/server_connection.ts";
import type { ScreenRef } from "../connection/routing.ts";
import type { HostScreen, RemoteState } from "./remote_socket.ts";
import type { HostCatalog } from "./host_directory.ts";
import type { HostPreference } from "./host_preferences.ts";
import type { KnownHost } from "./known_hosts.ts";
import { byHostOrder } from "./screen_groups.ts";

export interface HostConnectionView {
  key: ConnKey;
  state: RemoteState;
  available: readonly HostScreen[];
}

export interface HostGroup<T extends ScreenRef = ScreenRef> {
  key: ConnKey;
  target: string;
  alias?: string;
  hidden?: boolean;
  open: T[];
  /** Screens not open in this window; from the live connection or last observation. */
  screens: readonly HostScreen[];
  state?: RemoteState;
  stale: boolean;
  lastSeen?: number;
}

/**
 * The hosts the user arranged first, in `hostOrder`; then open hosts (in
 * screen order), then recent/cached and connected hosts.
 */
export function hostGroups<T extends ScreenRef>(
  screens: readonly T[], hosts: readonly HostConnectionView[],
  recent: readonly KnownHost[], catalog: readonly HostCatalog[], portHosts: readonly { target: string }[] = [],
  preferences: readonly HostPreference[] = [], hostOrder: readonly string[] = [],
): HostGroup<T>[] {
  const order = new Set<ConnKey>();
  for (const screen of screens) if (screen.conn !== "local") order.add(screen.conn);
  for (const host of recent) if (host.source === "recent") order.add(hostKey(host.target));
  for (const host of catalog) order.add(hostKey(host.target));
  for (const host of hosts) order.add(host.key);
  for (const host of portHosts) order.add(hostKey(host.target));
  for (const host of preferences) if (host.alias && !host.hidden) order.add(hostKey(host.target));
  const metadata = new Map(preferences.map((host) => [host.target, host]));
  const aliases = new Map(recent.map((host) => [host.target, host.alias]));
  const live = new Map(hosts.map((host) => [host.key, host]));
  const saved = new Map(catalog.map((host) => [host.target, host]));
  return byHostOrder([...order], hostOrder).flatMap((key) => {
    const target = hostOf(key);
    if (!target) return [];
    const host = live.get(key);
    const observation = saved.get(target);
    const open = screens.filter((screen) => screen.conn === key);
    const shown = new Set(open.map((screen) => screen.wireId));
    const stale = host?.state.phase !== "ready";
    const available = stale ? observation?.screens ?? [] : host.available;
    const preference = metadata.get(target);
    return [{ key, target, alias: preference ? preference.alias : aliases.get(target), hidden: preference?.hidden, open, screens: available.filter((s) => !shown.has(s.id)),
      state: host?.state, stale, lastSeen: observation?.lastSeen }];
  });
}

/** An explicit browse keeps a connection even with no attached screens. */
export function hostNeeded(key: ConnKey, screens: readonly ScreenRef[], browsing: ReadonlySet<ConnKey>): boolean {
  return browsing.has(key) || screens.some((screen) => screen.conn === key);
}
