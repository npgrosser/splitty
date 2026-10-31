import { describe, expect, it, vi } from "vitest";

vi.mock("../platform.ts", () => ({ shellKind: "web" }));

import { isSplittyShortcut } from "./terminal_shared.ts";
import { isMac } from "../shortcuts.ts";

const key = (fields: Partial<KeyboardEvent>) => ({ key: "", code: "", metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...fields }) as KeyboardEvent;

describe("keys Splitty takes from the terminal", () => {
  // session-scratchpad, Scratchpad In The Pane: Mod+J opens and closes it.
  it("takes Mod+J for the scratchpad and leaves Ctrl+J, a line feed, to the terminal", () => {
    expect(isSplittyShortcut(key({ key: "j", [isMac ? "metaKey" : "altKey"]: true }))).toBe(true);
    expect(isSplittyShortcut(key({ key: "j", ctrlKey: true }))).toBe(false);
    expect(isSplittyShortcut(key({ key: "j" }))).toBe(false);
  });
});
