import { describe, expect, it } from "vitest";
import { hitOrder, pickMark, type MarkHit } from "./mark_hits.ts";

const passage = (id: string, start: number, length: number): MarkHit => ({ id, lines: false, length, start });
const lines = (id: string, start: number, length: number): MarkHit => ({ id, lines: true, length, start });

// viewer-annotations, Annotations In The Pane (D1 of viewer-clickable-marks).
describe("pickMark", () => {
  it("chooses nothing without a mark", () => {
    expect(pickMark([])).toBeUndefined();
    expect(pickMark([], { ids: ["a"], chosen: "a" })).toBeUndefined();
  });

  it("chooses the shortest passage, a passage before lines, and an earlier one among equally long ones", () => {
    expect(pickMark([passage("long", 0, 40), passage("short", 10, 5)])!.chosen).toBe("short");
    expect(pickMark([lines("lines", 3, 1), passage("passage", 10, 400)])!.chosen).toBe("passage");
    expect(hitOrder([passage("b", 12, 5), passage("a", 10, 5), lines("c", 1, 2), lines("d", 1, 1)])).toEqual(["a", "b", "d", "c"]);
  });

  it("goes through overlapping marks on further clicks on them, and starts over elsewhere", () => {
    const hits = [passage("outer", 0, 40), passage("inner", 10, 5), lines("other", 1, 3)];
    const first = pickMark(hits)!;
    expect(first).toEqual({ ids: ["inner", "outer", "other"], chosen: "inner" });
    const second = pickMark([...hits].reverse(), first)!;
    expect(second.chosen).toBe("outer");
    const third = pickMark(hits, second)!;
    expect(third.chosen).toBe("other");
    expect(pickMark(hits, third)!.chosen).toBe("inner");

    // Other marks under the pointer: the shortest of them again.
    expect(pickMark([passage("outer", 0, 40)], second)!.chosen).toBe("outer");
    expect(pickMark([passage("outer", 0, 40), passage("third", 2, 3)], first)!.chosen).toBe("third");
  });

  it("names a mark once, however often it is under the pointer", () => {
    expect(hitOrder([passage("a", 0, 3), passage("a", 0, 3), passage("b", 0, 9)])).toEqual(["a", "b"]);
  });
});
