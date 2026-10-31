import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ConnKey } from "../connection/server_connection.ts";
import { arrangeHostGroups, orderedScreens, reorderScreensInGroup } from "./screen_groups.ts";

const screen = (id: string, conn: ConnKey) => Object.freeze({ id, conn, name: id });
const local1 = screen("local1", "local");
const local2 = screen("local2", "local");
const a1 = screen("a1", "host:a");
const a2 = screen("a2", "host:a");
const a3 = screen("a3", "host:a");
const b1 = screen("b1", "host:b");
const b2 = screen("b2", "host:b");
const ids = (screens: readonly { id: string }[]) => screens.map((s) => s.id);

describe("orderedScreens", () => {
  it("puts local screens first and hosts in first-appearance order, not alphabetical order", () => {
    const raw = Object.freeze([b1, a1, local1, b2, local2, a2]);
    const ordered = orderedScreens(raw);
    expect(ids(ordered)).toEqual(["local1", "local2", "b1", "b2", "a1", "a2"]);
    expect(ids(raw)).toEqual(["b1", "a1", "local1", "b2", "local2", "a2"]);
    // Callers can number and navigate this one array without per-group gaps.
    expect(ordered.indexOf(a1)).toBe(4);
    expect(ordered[0]).toBe(local1);
    expect(ordered[4].name).toBe("a1");
  });

  it("groups hosts without requiring a local screen", () => {
    expect(orderedScreens([b1, a1, b2, a2])).toEqual([b1, b2, a1, a2]);
  });

  it("returns a fresh array even when the input is already grouped", () => {
    const raw = Object.freeze([local1, local2, a1, a2]);
    const ordered = orderedScreens(raw);
    expect(ordered).toEqual(raw);
    expect(ordered).not.toBe(raw);
    ordered.pop();
    expect(raw).toHaveLength(4);
  });

  it("accepts an empty list and a local-only list", () => {
    expect(orderedScreens([])).toEqual([]);
    expect(orderedScreens([local2, local1])).toEqual([local2, local1]);
  });

  it("keeps equal screen ids on distinct connections and treats targets as distinct", () => {
    const alias = screen("same", "host:vm");
    const user = screen("same", "host:me@vm");
    const port = screen("same", "host:vm:2222");
    const local = screen("same", "local");
    expect(orderedScreens([alias, user, port, local])).toEqual([local, alias, user, port]);
  });
});

describe("reorderScreensInGroup", () => {
  const raw = Object.freeze([local1, a1, b1, a2, local2, b2, a3]);

  it("moves forward through a host's raw slots without shifting other connections", () => {
    const result = reorderScreensInGroup(raw, "host:a", 0, 2);
    expect(result).toEqual([local1, a2, b1, a3, local2, b2, a1]);
    expect(result?.[2]).toBe(b1);
    expect(result?.[4]).toBe(local2);
    expect(ids(raw)).toEqual(["local1", "a1", "b1", "a2", "local2", "b2", "a3"]);
    expect(orderedScreens(result ?? [])).toEqual([local1, local2, a2, a3, a1, b1, b2]);
  });

  it("moves backward through a host's raw slots", () => {
    expect(reorderScreensInGroup(raw, "host:a", 2, 0)).toEqual([local1, a3, b1, a1, local2, b2, a2]);
  });

  it("moves local screens without shifting any remote positions", () => {
    expect(reorderScreensInGroup(raw, "local", 0, 1)).toEqual([local2, a1, b1, a2, local1, b2, a3]);
  });

  it("does nothing for an unchanged index, an absent group, or an empty list", () => {
    expect(reorderScreensInGroup(raw, "host:a", 1, 1)).toBeUndefined();
    expect(reorderScreensInGroup(raw, "host:missing", 0, 1)).toBeUndefined();
    expect(reorderScreensInGroup([], "local", 0, 1)).toBeUndefined();
    expect(reorderScreensInGroup([local1], "local", 0, 1)).toBeUndefined();
  });

  it.each([
    [-1, 0], [0, -1], [3, 0], [0, 3],
    [0.5, 1], [0, 1.5], [NaN, 1], [0, NaN], [Infinity, 1], [0, Infinity],
  ])("rejects invalid group indices %s → %s", (from, to) => {
    expect(reorderScreensInGroup(raw, "host:a", from, to)).toBeUndefined();
  });
});

describe("moveScreenInGroup store integration", () => {
  let store: typeof import("../../stores/screens.svelte.ts");

  beforeEach(async () => {
    vi.resetModules();
    store = await import("../../stores/screens.svelte.ts");
  });

  it("keeps raw positions and the active screen, and notifies once with the resulting raw order", () => {
    const local = store.getScreens()[0];
    const a = store.addRemoteScreen("host:a", "a1");
    const b = store.addRemoteScreen("host:b", "b1");
    const secondLocal = store.createScreen();
    const otherA = store.addRemoteScreen("host:a", "a2");
    const before = store.getScreens();
    const notified: string[][] = [];
    const unsubscribe = store.subscribe(() => notified.push(ids(store.getScreens())));
    const onReorder = vi.fn();
    const onSetActive = vi.fn();
    store.setScreenMuxHooks({ onReorder, onSetActive });

    store.moveScreenInGroup("host:a", 0, 1);

    const expected = [local, otherA, b, secondLocal, a];
    expect(store.getScreens()).toEqual(expected);
    expect(ids(before)).toEqual(ids([local, a, b, secondLocal, otherA]));
    expect(store.getActiveScreenId()).toBe(otherA.id);
    expect(notified).toEqual([ids(expected)]);
    expect(onReorder).toHaveBeenCalledExactlyOnceWith(store.getScreens());
    expect(onSetActive).not.toHaveBeenCalled();
    expect(orderedScreens(store.getScreens())).toEqual([local, secondLocal, otherA, a, b]);
    unsubscribe();
  });

  it("leaves the store and its hooks untouched for rejected or unchanged moves", () => {
    const before = store.getScreens();
    const changed = vi.fn();
    const unsubscribe = store.subscribe(changed);
    const onReorder = vi.fn();
    store.setScreenMuxHooks({ onReorder });

    store.moveScreenInGroup("local", 0, 0);
    store.moveScreenInGroup("local", 0, 1);
    store.moveScreenInGroup("host:missing", 0, 1);
    store.moveScreenInGroup("local", NaN, 0);

    expect(store.getScreens()).toBe(before);
    expect(changed).not.toHaveBeenCalled();
    expect(onReorder).not.toHaveBeenCalled();
    unsubscribe();
  });
});

describe("arranged host groups", () => {
  const c1 = screen("c1", "host:c");

  it("puts arranged hosts first in their order, the others after them in first-appearance order, local always first", () => {
    const raw = Object.freeze([a1, b1, local1, c1, a2, b2]);
    expect(ids(orderedScreens(raw, ["c", "b"]))).toEqual(["local1", "c1", "b1", "b2", "a1", "a2"]);
    expect(ids(orderedScreens(raw, ["b"]))).toEqual(["local1", "b1", "b2", "a1", "a2", "c1"]);
  });

  it("ignores arranged hosts without screens and unknown targets", () => {
    expect(ids(orderedScreens([a1, b1], ["gone", "b", "nope"]))).toEqual(["b1", "a1"]);
    expect(ids(orderedScreens([local1, a1], ["local"]))).toEqual(["local1", "a1"]);
  });

  it("stores the shown hosts in their new order, then the arranged hosts without screens", () => {
    expect(arrangeHostGroups(["a", "b", "c"], 2, 0, [])).toEqual(["c", "a", "b"]);
    expect(arrangeHostGroups(["a", "b"], 1, 0, ["x", "b", "y", "a"])).toEqual(["b", "a", "x", "y"]);
  });

  it("refuses an unchanged or invalid move", () => {
    expect(arrangeHostGroups(["a", "b"], 1, 1, [])).toBeUndefined();
    expect(arrangeHostGroups(["a", "b"], 0, 2, [])).toBeUndefined();
    expect(arrangeHostGroups(["a", "b"], -1, 0, [])).toBeUndefined();
    expect(arrangeHostGroups(["a", "b"], 0.5, 1, [])).toBeUndefined();
  });
});
