// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import type { LayoutNode, PaneState, ScreenState } from "../lib/protocol/types.ts";
import type { TestScreen } from "../lib/testing/screen_layout.ts";

// The view's modules query media features when they load; jsdom has no matchMedia.
vi.hoisted(() => {
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    value: (media: string) => ({
      matches: false, media, onchange: null,
      addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, dispatchEvent: () => false,
    }),
  });
});

let screens: TestScreen[] = [];

afterEach(() => {
  for (const screen of screens) screen.dispose();
  screens = [];
  vi.doUnmock("./viewer_view.ts");
  vi.restoreAllMocks();
});

const viewer = (id: string, path: string): PaneState => ({
  id, role: "viewer", title: path.split("/").pop(), cwd: "/home/me/app",
  viewer: { source: { kind: "file", path }, originPaneId: "t1", revision: 1 },
});
const slot = (id: string, paneIds: string[]): LayoutNode => ({ type: "slot", id, paneIds, activePaneId: paneIds[0] });
const row = (...children: LayoutNode[]): LayoutNode =>
  ({ type: "split", id: "row", axis: "horizontal", sizes: children.map(() => 1 / children.length), children });
const state = (root: LayoutNode, panes: PaneState[]): ScreenState => ({
  id: "s1", revision: 1, root, floating: [], panes: Object.fromEntries(panes.map(pane => [pane.id, pane])), sessions: {},
});

/**
 * A screen with the viewer panes of `panes`, in a window that has not loaded
 * the view yet: the modules are loaded anew, after the view's mock.
 */
async function open(root: LayoutNode, panes: PaneState[], capabilities?: string[]) {
  vi.resetModules();
  const { testScreen } = await import("../lib/testing/screen_layout.ts");
  const { applyScreenSnapshot } = await import("../layout/screen_snapshot.ts");
  const { VIEW_LOAD_FAILED } = await import("./viewer_pane.ts");
  const screen = testScreen({ capabilities });
  screens.push(screen);
  // As the connection's snapshot handler does: the annotations first, then the layout.
  const apply = (root: LayoutNode, panes: PaneState[]) => {
    screen.binding.conn.annotations.snapshot(state(root, panes));
    applyScreenSnapshot(screen.api, screen.binding, state(root, panes));
  };
  apply(root, panes);
  return { screen, apply, VIEW_LOAD_FAILED };
}

// Each test loads the pane's modules anew, and with them the view's libraries:
// slow while other test files run.
const LOAD = { timeout: 15_000 };

/** Until the view's load has settled for every pane that waits for it. */
async function settled(): Promise<void> {
  await import("./viewer_view.ts").catch(() => undefined);
  await new Promise(resolve => setTimeout(resolve, 0));
}

describe("ViewerPaneRenderer", { timeout: 30_000 }, () => {
  it("says so when its view cannot be loaded, and a later pane tries again", async () => {
    vi.doMock("./viewer_view.ts", () => {
      throw new Error("The module is gone.");
    });
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const plan = viewer("v1", "/home/me/app/PLAN.md");
    const { screen, apply, VIEW_LOAD_FAILED } = await open(slot("docs", ["v1"]), [plan]);
    const element = screen.content("v1").element;
    await vi.waitFor(() => expect(element.querySelector(".viewer-notice-text")?.textContent).toBe(VIEW_LOAD_FAILED), LOAD);
    expect(element.querySelector(".viewer-bar")).toBeNull();
    expect(warn).toHaveBeenCalledWith("pane v1: the viewer could not be loaded", expect.any(Error));

    vi.doUnmock("./viewer_view.ts");
    apply(row(slot("docs", ["v1"]), slot("more", ["v2"])), [plan, viewer("v2", "/home/me/app/NOTES.md")]);
    const next = screen.content("v2").element;
    await vi.waitFor(() => expect(next.querySelector(".viewer-bar-label")?.textContent).toBe("/home/me/app/NOTES.md"), LOAD);
  });

  // viewer-annotations, Handing A Review To A Terminal: Send to the agent.
  it("sends a pane's review to its origin terminal and gives that terminal the keyboard", async () => {
    const annotation = {
      id: "ann-0123456789abcdef", number: 1, kind: "comment" as const, body: "Why three?",
      target: { view: "source" as const, quote: "retry three times", prefix: "", suffix: "", startLine: 1, endLine: 1 },
    };
    const plan = viewer("v1", "/home/me/app/PLAN.md");
    const annotated: PaneState = { ...plan, viewer: { ...plan.viewer!, annotations: [annotation] } };
    const agent: PaneState = { id: "t1", role: "terminal", title: "claude", sessionId: "sess-1" };
    const { screen } = await open(row(slot("main", ["t1"]), slot("docs", ["v1"])), [agent, annotated], ["viewer-v1", "viewer-annotations-v1"]);
    const { TerminalSession } = await import("../lib/terminal/terminal_session.ts");
    const { fakeEmulator } = await import("../lib/testing/fake_server.ts");
    const emulator = fakeEmulator();
    (emulator as { bracketedPaste: boolean }).bracketedPaste = true;
    const session = new TerminalSession(screen.binding.conn.sessions, "sess-1", emulator);
    session.wire();
    screen.binding.linkPane("t1", "sess-1");
    screen.answerReads({ v1: "retry three times\n" });

    const element = screen.content("v1").element;
    await vi.waitFor(() => expect(element.querySelector(".viewer-annotation")).not.toBeNull(), LOAD);
    const list = element.querySelector<HTMLElement>(".viewer-annotations")!;
    list.dispatchEvent(new Event("pointerenter"));
    const send = list.querySelector<HTMLButtonElement>(".viewer-annotations-primary")!;
    expect(send.textContent).toBe("Send to claude");
    send.click();

    await vi.waitFor(() => expect(screen.socket.sentOfType("input")).toHaveLength(1));
    expect(screen.socket.sentOfType("input")[0]).toMatchObject({ id: "sess-1", data: expect.stringMatching(/^# Review of PLAN\.md\n/) });
    expect(screen.socket.sentOfType("input")[0].data).toContain("## #1 Line 1, comment");
    // The pane keeps the annotation, marked as handed over.
    expect(screen.socket.sentOfType("mux.op").map((message) => message.op)).toEqual([
      { type: "mark_sent", paneId: "v1", entries: [expect.objectContaining({ id: annotation.id, kind: "comment" })] },
    ]);
    expect(screen.binding.conn.annotations.count("v1")).toBe(1);
    expect(screen.binding.conn.annotations.get("v1", annotation.id)?.sent).toBe(true);
    await vi.waitFor(() => expect((emulator as unknown as { focusCount: number }).focusCount).toBe(1));
    session.dispose();
  });

  it("builds no view for a pane closed while the view loads", async () => {
    let release!: () => void;
    const gate = new Promise<void>(resolve => { release = resolve; });
    vi.doMock("./viewer_view.ts", async (importOriginal) => {
      await gate;
      return importOriginal();
    });
    const { screen } = await open(
      row(slot("docs", ["v1"]), slot("more", ["v2"])),
      [viewer("v1", "/home/me/app/PLAN.md"), viewer("v2", "/home/me/app/NOTES.md")],
    );
    const closed = screen.content("v1").element;
    const kept = screen.content("v2").element;
    expect(closed.childElementCount).toBe(0);
    expect(kept.childElementCount).toBe(0);

    screen.api.getPanel("v1")!.api.close();
    release();
    await settled();
    expect(kept.querySelector(".viewer-bar-label")?.textContent).toBe("/home/me/app/NOTES.md");
    expect(closed.childElementCount).toBe(0);
  });
});
