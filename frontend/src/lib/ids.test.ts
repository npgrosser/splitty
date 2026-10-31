import { describe, expect, it } from "vitest";
import { newPaneId, newRemoteScreenKey, newScreenId } from "./ids.ts";

describe("ids", () => {
  it("keeps the prefixes and carries 64 random bits", () => {
    expect(newPaneId()).toMatch(/^terminal-[0-9a-f]{16}$/);
    expect(newScreenId()).toMatch(/^screen-[0-9a-f]{16}$/);
    expect(newRemoteScreenKey()).toMatch(/^rs-[0-9a-f]{16}$/);
  });

  // workspace-model, Identifier Assignment: ids no longer restart on a reload.
  it("does not repeat", () => {
    const ids = new Set<string>();
    for (let i = 0; i < 10_000; i++) {
      ids.add(newPaneId());
      ids.add(newScreenId());
    }
    expect(ids.size).toBe(20_000);
  });
});
