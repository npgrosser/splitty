import { hostOf, type ConnKey } from "../connection/server_connection.ts";

/**
 * The shared display and keyboard order: local screens first, then the host
 * groups the user arranged, in `hostOrder` (SSH targets), then the other hosts
 * in order of first appearance. Each group's screens keep their relative order.
 * The returned array is new; the raw order used by persistence is unchanged.
 */
export function orderedScreens<T extends { conn: ConnKey }>(screens: readonly T[], hostOrder: readonly string[] = []): T[] {
  const groups = new Map<ConnKey, T[]>();
  for (const screen of screens) {
    const group = groups.get(screen.conn);
    if (group) group.push(screen);
    else groups.set(screen.conn, [screen]);
  }
  const hosts = byHostOrder([...groups.keys()].filter((key) => key !== "local"), hostOrder);
  return [...(groups.get("local") ?? []), ...hosts.flatMap((key) => groups.get(key) ?? [])];
}

/**
 * Host keys with the arranged ones first, in `hostOrder`, and the others
 * after them in their given order.
 */
export function byHostOrder(keys: readonly ConnKey[], hostOrder: readonly string[]): ConnKey[] {
  const rank = new Map(hostOrder.map((target, index) => [target, index]));
  const rankOf = (key: ConnKey) => rank.get(hostOf(key) ?? "") ?? Number.MAX_SAFE_INTEGER;
  return keys
    .map((key, index) => ({ key, index }))
    .sort((a, b) => rankOf(a.key) - rankOf(b.key) || a.index - b.index)
    .map(({ key }) => key);
}

/**
 * The stored order after the user moved a host group from `fromIndex` to
 * `toIndex` among the `shown` host targets: the shown hosts in their new
 * order, then the previously arranged hosts that are not shown, in their
 * previous order. Undefined for an invalid or unchanged move.
 */
export function arrangeHostGroups(
  shown: readonly string[],
  fromIndex: number,
  toIndex: number,
  arranged: readonly string[],
): string[] | undefined {
  if (!Number.isInteger(fromIndex) || !Number.isInteger(toIndex)) return;
  if (fromIndex === toIndex || fromIndex < 0 || toIndex < 0) return;
  if (fromIndex >= shown.length || toIndex >= shown.length) return;
  const next = [...shown];
  const [moved] = next.splice(fromIndex, 1);
  next.splice(toIndex, 0, moved);
  return [...next, ...arranged.filter((target) => !next.includes(target))];
}

/**
 * Move between zero-based indices within one connection's group, replacing
 * only that group's slots in the raw array. Other connections keep their
 * exact positions. Returns undefined for an invalid move or an unchanged
 * index, so callers can avoid notifications and persistence writes.
 */
export function reorderScreensInGroup<T extends { conn: ConnKey }>(
  screens: readonly T[],
  conn: ConnKey,
  fromIndex: number,
  toIndex: number,
): T[] | undefined {
  if (!Number.isInteger(fromIndex) || !Number.isInteger(toIndex)) return;
  if (fromIndex === toIndex || fromIndex < 0 || toIndex < 0) return;
  const group = screens.filter((screen) => screen.conn === conn);
  if (fromIndex >= group.length || toIndex >= group.length) return;

  const [moved] = group.splice(fromIndex, 1);
  group.splice(toIndex, 0, moved);
  let index = 0;
  return screens.map((screen) => screen.conn === conn ? group[index++] : screen);
}
