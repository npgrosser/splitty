// @vitest-environment jsdom
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { MuxOperation, ScreenState, ViewerAnnotation, ViewerRecord } from "../../lib/protocol/types.ts";
import { CONTEXT_CHARS } from "../../lib/viewer/anchoring.ts";
import { ViewerAnnotations } from "../../lib/viewer/annotations.ts";
import type { ReviewTarget } from "../../lib/viewer/handover.ts";
import type { BoundaryRange } from "../../lib/viewer/render.ts";
import type { ContextMenuItem } from "../../stores/ui.svelte.ts";
import { ViewerView } from "../viewer_view.ts";
import type { ViewerViewState } from "../viewer_view_state.ts";
import type { AnnotationLayerOptions } from "./annotation_layer.ts";
import { LONG_SELECTION, SHORT_SELECTION } from "./selection_toolbar.ts";

// The settings the source view's shortcuts read query media features when
// they load; jsdom has no matchMedia, and lays out no ranges.
vi.hoisted(() => {
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    value: (media: string) => ({
      matches: false, media, onchange: null,
      addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, dispatchEvent: () => false,
    }),
  });
  const empty = { x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0, toJSON: () => ({}) };
  Range.prototype.getClientRects ??= () => [] as unknown as DOMRectList;
  Range.prototype.getBoundingClientRect ??= () => empty as DOMRect;
});

const encoder = new TextEncoder();
const PLAN_RECORD: ViewerRecord = { source: { kind: "file", path: "/home/me/app/PLAN.md" }, revision: 1, originPaneId: "t1" };
const filler = (from: number, to: number) => Array.from({ length: to - from + 1 }, (_, i) => `Step ${from + i} of the plan.`).join("\n");
/** A plan with `retry three times` on line 30. */
const PLAN = `${filler(1, 29)}\n2. On failure, retry three times before giving up.\n${filler(31, 40)}\n`;
const RENDERED_PLAN = "# Plan\n\nThe first step.\n\n- retry three times\n- ship it\n";

let views: ViewerView[] = [];

afterEach(() => {
  for (const view of views) view.destroy();
  views = [];
  vi.restoreAllMocks();
  document.body.replaceChildren();
});

interface Terminal {
  readonly target: ReviewTarget;
  readonly pasted: string[];
}

function terminal(paneId: string, title: string, options: { bracketedPaste?: boolean; upload?: () => Promise<string> } = {}): Terminal {
  const pasted: string[] = [];
  return {
    pasted,
    target: {
      paneId,
      title,
      bracketedPaste: () => options.bracketedPaste ?? true,
      paste: (text) => pasted.push(text),
      reveal: vi.fn(),
      upload: options.upload ?? (async () => "/home/me/.cache/splitty/uploads/review-PLAN.md"),
    },
  };
}

interface Setup {
  record?: ViewerRecord;
  text?: string;
  mode?: "rendered" | "source";
  available?: boolean;
  origin?: ReviewTarget;
  terminals?: ReviewTarget[];
  state?: ViewerViewState;
  store?: ViewerAnnotations;
}

function setup(params: Setup = {}) {
  const sent: MuxOperation[] = [];
  const store = params.store ?? new ViewerAnnotations({ send: (_screenId, op) => sent.push(op), resync: () => {} });
  const notices = { sent: vi.fn(), undoable: vi.fn(), info: vi.fn(), error: vi.fn() };
  const showMenu = vi.fn<(items: ContextMenuItem[], x: number, y: number) => void>();
  const writeClipboard = vi.fn(async (_text: string) => {});
  const download = vi.fn<(name: string, blob: Blob) => void>();
  let origin = params.origin;
  const record = params.record ?? PLAN_RECORD;
  const options: AnnotationLayerOptions = {
    store,
    paneId: "v1",
    screenId: () => "screen-1",
    available: () => params.available ?? true,
    document: record.source.kind === "file" ? { kind: "file", path: record.source.path } : { kind: "text", title: record.source.title },
    handover: { origin: () => origin, terminals: () => params.terminals ?? [] },
    notices,
    showMenu,
    writeClipboard,
    download,
  };
  const shown = new ViewerView(document, {
    record,
    supported: true,
    state: params.state ?? { mode: params.mode ?? "source" },
    openExternal: vi.fn(),
    openFile: vi.fn(),
    refuseLink: vi.fn(),
    reload: vi.fn(),
    annotations: options,
  });
  document.body.append(shown.element);
  views.push(shown);
  if (params.text !== undefined) shown.update(read(params.text));
  return { shown, store, sent, notices, showMenu, writeClipboard, download, setOrigin: (target: ReviewTarget | undefined) => { origin = target; } };
}

function read(text: string, version = "v1") {
  return { document: { bytes: encoder.encode(text), version }, reading: false };
}

const query = <T extends Element = HTMLElement>(view: ViewerView, selector: string) => view.element.querySelector<T & HTMLElement>(selector);

function editor(view: ViewerView): EditorView {
  return EditorView.findFromDOM(query(view, ".cm-editor")!)!;
}

/** Select the `occurrence`-th `quote` in the source view. */
function selectSource(view: ViewerView, quote: string): void {
  const text = editor(view).state.doc.toString();
  const start = text.indexOf(quote);
  expect(start).toBeGreaterThanOrEqual(0);
  editor(view).dispatch({ selection: { anchor: start, head: start + quote.length } });
}

/** Select `quote` in the rendered view, as an engine with `getComposedRanges` reports it. */
function selectRendered(view: ViewerView, quote: string): void {
  const root = query(view, ".viewer-rendered")!.shadowRoot!;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode() as Text | null; node; node = walker.nextNode() as Text | null) {
    const at = node.data.indexOf(quote);
    if (at === -1) continue;
    const range: BoundaryRange = { startContainer: node, startOffset: at, endContainer: node, endOffset: at + quote.length, collapsed: false };
    vi.spyOn(document, "getSelection").mockReturnValue({ rangeCount: 0, getComposedRanges: () => [range] } as unknown as Selection);
    return;
  }
  throw new Error(`no text node with ${quote}`);
}

function form(view: ViewerView): HTMLElement | null {
  return query(view, ".viewer-annotation-form");
}

function write(view: ViewerView, label: string, text: string): void {
  const field = form(view)!.querySelector<HTMLTextAreaElement>(`textarea[aria-label="${label}"]`)!;
  field.value = text;
  field.dispatchEvent(new Event("input", { bubbles: true }));
}

function pressKey(view: ViewerView, init: KeyboardEventInit): void {
  const target = form(view)!.querySelector("textarea")!;
  target.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }));
}

function entries(view: ViewerView): HTMLElement[] {
  return [...view.element.querySelectorAll<HTMLElement>(".viewer-annotation")];
}

function whereOf(view: ViewerView): string[] {
  return entries(view).map((entry) => entry.querySelector(".viewer-annotation-where")!.textContent!);
}

/** The menu the list's ▾ button shows. */
function handoverMenu(view: ViewerView, showMenu: ReturnType<typeof setup>["showMenu"]): ContextMenuItem[] {
  query<HTMLButtonElement>(view, ".viewer-annotations-more")!.click();
  return showMenu.mock.calls.at(-1)![0];
}

function screenWith(annotations: ViewerAnnotation[]): ScreenState {
  return {
    id: "screen-1",
    root: { type: "slot", id: "s1", paneIds: ["v1"] },
    panes: { v1: { id: "v1", role: "viewer", viewer: { ...PLAN_RECORD, annotations } } },
    sessions: {},
  } as unknown as ScreenState;
}

/** Ask a question on `quote` of the source view through the context menu. */
function ask(view: ViewerView, quote: string, remark: string): void {
  selectSource(view, quote);
  view.annotationMenu()!.annotate("question");
  write(view, "Question", remark);
  pressKey(view, { key: "Enter", ctrlKey: true });
}

describe("making annotations", () => {
  // viewer-annotations, Annotations On A Viewer Pane: A question on a passage.
  it("asks a question on a passage of the source from the selection's toolbar", async () => {
    const { shown, store, sent } = setup({ text: PLAN });
    selectSource(shown, "retry three times");
    query(shown, ".viewer-document")!.dispatchEvent(new Event("pointerup", { bubbles: true }));
    const toolbar = query(shown, ".viewer-selection-toolbar")!;
    await vi.waitFor(() => expect(toolbar.hidden).toBe(false));
    toolbar.querySelector<HTMLButtonElement>('[data-kind="question"]')!.click();
    expect(toolbar.hidden).toBe(true);

    expect(form(shown)!.querySelector(".viewer-form-title")!.textContent).toBe("New Question");
    expect(form(shown)!.querySelector(".viewer-form-quote")!.textContent).toBe("retry three times");
    expect(form(shown)!.querySelector(".viewer-form-label")!.textContent).toBe("Passage, Line 30");
    write(shown, "Question", "Why three?");
    pressKey(shown, { key: "Enter", ctrlKey: true });

    expect(form(shown)).toBeNull();
    const [annotation] = store.list("v1");
    const at = PLAN.indexOf("retry three times");
    expect(annotation).toMatchObject({
      kind: "question",
      body: "Why three?",
      target: {
        view: "source",
        quote: "retry three times",
        prefix: PLAN.slice(at - CONTEXT_CHARS, at),
        suffix: PLAN.slice(at + 17, at + 17 + CONTEXT_CHARS),
        startLine: 30,
        endLine: 30,
      },
    });
    expect(annotation.id).toMatch(/^ann-[0-9a-f]{16}$/);
    expect(sent).toEqual([{ type: "put_annotations", paneId: "v1", annotations: [annotation] }]);
    // Listed, and marked in the source.
    expect(whereOf(shown)).toEqual(["Line 30"]);
    expect(query(shown, ".cm-viewer-marked")!.textContent).toBe("retry three times");
  });

  // viewer-annotations, Annotations On A Viewer Pane: A general comment.
  it("adds a general comment without a selection", () => {
    const { shown, store } = setup({ text: PLAN });
    const menu = shown.annotationMenu()!;
    expect(menu.selection).toBe("none");
    menu.addComment();
    const kinds = [...form(shown)!.querySelectorAll(".viewer-form-kind")].map((kind) => kind.textContent);
    expect(kinds).toEqual(["Comment", "Question"]);
    expect(form(shown)!.querySelector(".viewer-form-quote")).toBeNull();
    write(shown, "Comment", "Split the plan into two.");
    form(shown)!.querySelector<HTMLButtonElement>(".viewer-form-save")!.click();
    expect(store.list("v1")).toEqual([{ id: expect.stringMatching(/^ann-/), kind: "comment", body: "Split the plan into two." }]);
    expect(whereOf(shown)).toEqual(["General"]);
  });

  it("keeps the form open with the reason when a remark is missing, and cancels with Escape", () => {
    const { shown, store, sent } = setup({ text: PLAN });
    selectSource(shown, "retry three times");
    shown.annotationMenu()!.annotate("comment");
    write(shown, "Comment", "  \n ");
    pressKey(shown, { key: "Enter", ctrlKey: true });
    const problem = form(shown)!.querySelector<HTMLElement>(".viewer-form-problem")!;
    expect(problem.hidden).toBe(false);
    expect(problem.textContent).toBe("A comment needs a remark.");
    pressKey(shown, { key: "Escape" });
    expect(form(shown)).toBeNull();
    expect(store.count("v1")).toBe(0);
    expect(sent).toEqual([]);
  });

  it("prefills a suggested change with the passage and saves its replacement", () => {
    const { shown, store } = setup({ text: PLAN });
    selectSource(shown, "retry three times");
    shown.annotationMenu()!.annotate("change");
    const replacement = form(shown)!.querySelector<HTMLTextAreaElement>('textarea[aria-label="Replacement"]')!;
    expect(replacement.value).toBe("retry three times");
    expect(document.activeElement).toBe(replacement);
    write(shown, "Replacement", "retry twice");
    pressKey(shown, { key: "Enter", ctrlKey: true });
    expect(store.list("v1")[0]).toMatchObject({ kind: "change", replacement: "retry twice" });
    expect(store.list("v1")[0]).not.toHaveProperty("body");
  });

  it("asks for a longer selection, and says when one is too long", async () => {
    const { shown } = setup({ text: `ab ${"x".repeat(17_000)}\n` });
    const toolbar = query(shown, ".viewer-selection-toolbar")!;
    const note = toolbar.querySelector<HTMLElement>(".viewer-selection-note")!;

    selectSource(shown, "ab");
    expect(shown.annotationMenu()!.selection).toBe("short");
    query(shown, ".viewer-document")!.dispatchEvent(new Event("pointerup", { bubbles: true }));
    await vi.waitFor(() => expect(toolbar.hidden).toBe(false));
    expect(note.textContent).toBe(SHORT_SELECTION);
    expect(toolbar.querySelector<HTMLElement>(".viewer-selection-kinds")!.hidden).toBe(true);
    // The menu's entries stay closed for it.
    shown.annotationMenu()!.annotate("comment");
    expect(form(shown)).toBeNull();

    editor(shown).dispatch({ selection: { anchor: 0, head: editor(shown).state.doc.length } });
    expect(shown.annotationMenu()!.selection).toBe("long");
    query(shown, ".viewer-document")!.dispatchEvent(new Event("keyup", { bubbles: true }));
    await vi.waitFor(() => expect(note.textContent).toBe(LONG_SELECTION));
  });

  it("edits an annotation from the list and deletes one with an undo", () => {
    const { shown, store, notices } = setup({ text: PLAN });
    ask(shown, "retry three times", "Why three?");
    const [first] = store.list("v1");

    entries(shown)[0].querySelector<HTMLButtonElement>('[title="Edit this annotation"]')!.click();
    expect(form(shown)!.querySelector(".viewer-form-title")!.textContent).toBe("Edit Question");
    const remark = form(shown)!.querySelector<HTMLTextAreaElement>('textarea[aria-label="Question"]')!;
    expect(remark.value).toBe("Why three?");
    write(shown, "Question", "Why not twice?");
    pressKey(shown, { key: "Enter", metaKey: false, ctrlKey: true });
    expect(store.list("v1")).toEqual([{ ...first, body: "Why not twice?" }]);
    expect(entries(shown)[0].querySelector(".viewer-annotation-body")!.textContent).toBe("Why not twice?");

    entries(shown)[0].querySelector<HTMLButtonElement>('[title="Delete this annotation"]')!.click();
    expect(store.count("v1")).toBe(0);
    expect(notices.undoable).toHaveBeenCalledWith("Annotation deleted", expect.any(Function));
    notices.undoable.mock.calls[0][1]();
    expect(store.list("v1")).toEqual([{ ...first, body: "Why not twice?" }]);
  });

  // viewer-annotations, Concurrent Annotation Edits: the form keeps what the user writes.
  it("keeps the form's text while another window changes the same annotation, and saving replaces that change", () => {
    const { shown, store } = setup({ text: PLAN });
    ask(shown, "retry three times", "Why three?");
    const [own] = store.list("v1");
    entries(shown)[0].querySelector<HTMLButtonElement>('[title="Edit this annotation"]')!.click();
    write(shown, "Question", "Why not twice?");

    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(Date.now() + 5_000);
    store.snapshot(screenWith([{ ...own, body: "Changed elsewhere." }]));
    vi.useRealTimers();
    expect(entries(shown)[0].querySelector(".viewer-annotation-body")!.textContent).toBe("Changed elsewhere.");
    expect(form(shown)!.querySelector<HTMLTextAreaElement>('textarea[aria-label="Question"]')!.value).toBe("Why not twice?");
    pressKey(shown, { key: "Enter", ctrlKey: true });
    expect(store.list("v1")).toEqual([{ ...own, body: "Why not twice?" }]);
  });

  // viewer-annotations, Annotations Capability: Host without annotations.
  it("offers no annotations where the server keeps none", async () => {
    const { shown } = setup({ text: PLAN, available: false });
    expect(shown.annotationMenu()).toBeUndefined();
    expect(query(shown, ".viewer-bar-annotations")!.hidden).toBe(true);
    expect(query(shown, ".viewer-annotations")!.hidden).toBe(true);
    selectSource(shown, "retry three times");
    query(shown, ".viewer-document")!.dispatchEvent(new Event("pointerup", { bubbles: true }));
    await new Promise((resolve) => setTimeout(resolve, 10));
    expect(query(shown, ".viewer-selection-toolbar")!.hidden).toBe(true);
  });

  it("offers none for an image", () => {
    const { shown } = setup({ record: { source: { kind: "file", path: "/home/me/app/logo.png" }, revision: 1 } });
    shown.update({ document: { bytes: new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1]), version: "v1" }, reading: false });
    expect(shown.annotationMenu()).toBeUndefined();
  });

  it("says when the server rejected the window's annotations", () => {
    const { shown, store, notices } = setup({ text: PLAN });
    ask(shown, "retry three times", "Why three?");
    store.failed({ op: "put_annotations", screenId: "screen-1", paneId: "v1", error: "pane v1 is no viewer pane" });
    expect(store.count("v1")).toBe(0);
    expect(notices.error).toHaveBeenCalledWith("The annotations could not be saved", "pane v1 is no viewer pane");
    expect(query(shown, ".viewer-annotations")!.hidden).toBe(true);
    expect(query(shown, ".viewer-bar-annotations")!.textContent).toBe("Annotations");
  });
});

describe("annotations in the pane", () => {
  it("lists anchored passages in document order, then those not found, then general comments", () => {
    const { shown, store } = setup({ text: PLAN });
    shown.annotationMenu()!.addComment();
    write(shown, "Comment", "General remark.");
    pressKey(shown, { key: "Enter", ctrlKey: true });
    ask(shown, "Step 35 of the plan.", "Later step.");
    ask(shown, "Step 3 of the plan.", "Early step.");
    store.put("screen-1", "v1", [{ id: "ann-00000000000000ff", kind: "comment", body: "Gone.", target: { view: "source", quote: "no such passage", prefix: "", suffix: "", startLine: 2, endLine: 2 } }]);
    expect(whereOf(shown)).toEqual(["Line 3", "Line 35", "Line 2, not found", "General"]);
    expect(entries(shown)[2].classList.contains("stale")).toBe(true);
    expect(query(shown, ".viewer-annotations-count")!.textContent).toBe("4 annotations");
    expect(query(shown, ".viewer-bar-annotations")!.textContent).toBe("4 Annotations");
  });

  // viewer-annotations, Anchoring By Quote: Lines inserted above the passage, The passage removed.
  it("follows the passage through new versions and keeps one that is gone", () => {
    const { shown, showMenu } = setup({ text: PLAN, origin: terminal("t1", "claude").target });
    ask(shown, "retry three times", "Why three?");

    shown.update(read(`${filler(101, 110)}\n${PLAN}`, "v2"));
    expect(whereOf(shown)).toEqual(["Line 40"]);
    expect(query(shown, ".cm-viewer-marked")!.textContent).toBe("retry three times");
    handoverMenu(shown, showMenu).find((item) => item.label === "Copy as Markdown")!.action();

    shown.update(read(PLAN.replace("retry three times", "give up"), "v3"));
    expect(whereOf(shown)).toEqual(["Line 30, not found"]);
    expect(entries(shown)[0].querySelector(".viewer-annotation-quote")!.textContent).toBe("retry three times");
    expect(query(shown, ".cm-viewer-marked")).toBeNull();

    shown.update(read(PLAN, "v4"));
    expect(whereOf(shown)).toEqual(["Line 30"]);
  });

  it("names the passage's current lines in the review, and a passage not found as such", async () => {
    const { shown, showMenu, writeClipboard } = setup({ text: PLAN });
    ask(shown, "retry three times", "Why three?");
    shown.update(read(`${filler(101, 110)}\n${PLAN}`, "v2"));
    handoverMenu(shown, showMenu).find((item) => item.label === "Copy as Markdown")!.action();
    await vi.waitFor(() => expect(writeClipboard).toHaveBeenCalledTimes(1));
    expect(writeClipboard.mock.calls[0][0]).toContain("## 1. Line 40, question\n> retry three times\n\nWhy three?");

    shown.update(read(PLAN.replace("retry three times", "give up"), "v3"));
    handoverMenu(shown, showMenu).find((item) => item.label === "Copy as Markdown")!.action();
    await vi.waitFor(() => expect(writeClipboard).toHaveBeenCalledTimes(2));
    expect(writeClipboard.mock.calls[1][0]).toContain("## 1. Line 30, question (passage not found in the current version)");
  });

  // viewer-annotations, Annotations In The Pane: From the rendered view to the source.
  it("marks the lines of a rendered passage in the source", () => {
    const { shown, store } = setup({ text: RENDERED_PLAN, mode: "rendered" });
    selectRendered(shown, "retry three times");
    shown.annotationMenu()!.annotate("comment");
    write(shown, "Comment", "Make it configurable.");
    pressKey(shown, { key: "Enter", ctrlKey: true });
    expect(store.list("v1")[0].target).toMatchObject({ view: "rendered", quote: "retry three times", startLine: 5, endLine: 5 });
    const root = query(shown, ".viewer-rendered")!.shadowRoot!;
    // Without the highlight API, the passage's block is marked.
    expect(root.querySelector("[data-splitty-mark]")!.textContent).toBe("retry three times");

    shown.toggleMode();
    const bars = [...shown.element.querySelectorAll(".cm-viewer-marks .cm-gutterElement")].filter((element) => element.querySelector(".cm-viewer-mark"));
    expect(bars).toHaveLength(1);
    expect(query(shown, ".cm-viewer-marked")).toBeNull();
    expect(whereOf(shown)).toEqual(["Line 5"]);
  });

  it("marks the blocks of a source passage in the rendered view", () => {
    const { shown } = setup({ text: RENDERED_PLAN, mode: "source" });
    selectSource(shown, "The first step.");
    shown.annotationMenu()!.annotate("comment");
    write(shown, "Comment", "Which step?");
    pressKey(shown, { key: "Enter", ctrlKey: true });
    shown.toggleMode();
    const root = query(shown, ".viewer-rendered")!.shadowRoot!;
    expect([...root.querySelectorAll("[data-splitty-mark]")].map((block) => block.textContent)).toEqual(["The first step."]);
  });

  it("names no lines for a passage of a rendered HTML page", async () => {
    const { shown, showMenu, writeClipboard } = setup({
      record: { source: { kind: "file", path: "/home/me/app/report.html" }, revision: 1 },
      text: "<!doctype html><html><body><h1>Report</h1><p>All checks passed.</p></body></html>\n",
      mode: "rendered",
    });
    selectRendered(shown, "All checks passed");
    shown.annotationMenu()!.annotate("question");
    write(shown, "Question", "Which checks?");
    pressKey(shown, { key: "Enter", ctrlKey: true });
    expect(whereOf(shown)).toEqual(["Passage"]);
    handoverMenu(shown, showMenu).find((item) => item.label === "Copy as Markdown")!.action();
    await vi.waitFor(() => expect(writeClipboard).toHaveBeenCalledTimes(1));
    expect(writeClipboard.mock.calls[0][0]).toContain("## 1. Passage, question\n> All checks passed");
  });

  it("keeps the list closed as the user left it for the pane's next view", () => {
    const { shown, store } = setup({ text: PLAN });
    ask(shown, "retry three times", "Why three?");
    expect(query(shown, ".viewer-annotations")!.hidden).toBe(false);
    query<HTMLButtonElement>(shown, ".viewer-annotations-icon")!.click();
    expect(query(shown, ".viewer-annotations")!.hidden).toBe(true);
    const state = shown.viewState();
    expect(state.annotations).toBe("closed");

    const next = setup({ text: PLAN, store, state }).shown;
    expect(query(next, ".viewer-annotations")!.hidden).toBe(true);
    expect(next.annotationMenu()!.listShown).toBe(false);
    query<HTMLButtonElement>(next, ".viewer-bar-annotations")!.click();
    expect(query(next, ".viewer-annotations")!.hidden).toBe(false);
  });
});

describe("handing the review over", () => {
  // viewer-annotations, Handing A Review To A Terminal: Send to the agent.
  it("sends the review to the viewer's origin and ends the round", async () => {
    const agent = terminal("t1", "claude");
    const { shown, store, notices } = setup({ text: PLAN, origin: agent.target });
    ask(shown, "retry three times", "Why three?");
    const send = query<HTMLButtonElement>(shown, ".viewer-annotations-primary")!;
    expect(send.textContent).toBe("Send to claude");
    send.click();
    await vi.waitFor(() => expect(store.count("v1")).toBe(0));
    expect(agent.pasted).toHaveLength(1);
    expect(agent.pasted[0]).toMatch(/^# Review of PLAN\.md\n\nDocument: \/home\/me\/app\/PLAN\.md\n\n## 1\. Line 30, question/);
    expect(agent.target.reveal).toHaveBeenCalledTimes(1);

    // viewer-annotations, Handing A Review To A Terminal: Undo after sending.
    const undo = notices.sent.mock.calls[0][1] as () => void;
    undo();
    expect(store.count("v1")).toBe(1);
    expect(agent.pasted).toHaveLength(1);
  });

  // viewer-annotations, Handing A Review To A Terminal: The origin is gone.
  it("asks for a terminal of the screen when the origin is gone, and sends nothing before", async () => {
    const shell = terminal("t2", "bash");
    const { shown, store, showMenu } = setup({ text: PLAN, terminals: [shell.target] });
    ask(shown, "retry three times", "Why three?");
    const send = query<HTMLButtonElement>(shown, ".viewer-annotations-primary")!;
    expect(send.textContent).toBe("Send to Terminal…");
    send.click();
    const items = showMenu.mock.calls.at(-1)![0];
    expect(items.map((item) => item.label)).toEqual(["bash"]);
    expect(shell.pasted).toEqual([]);
    expect(store.count("v1")).toBe(1);
    items[0].action();
    await vi.waitFor(() => expect(store.count("v1")).toBe(0));
    expect(shell.pasted).toHaveLength(1);
  });

  it("offers every form of hand-over, the origin first", () => {
    const agent = terminal("t1", "claude");
    const shell = terminal("t2", "bash");
    const { shown, showMenu } = setup({ text: PLAN, origin: agent.target, terminals: [shell.target, agent.target] });
    ask(shown, "retry three times", "Why three?");
    const items = handoverMenu(shown, showMenu);
    expect(items.filter((item) => !item.separator).map((item) => item.label)).toEqual([
      "Send To", "Send as File To", "Append to Scratchpad Of", "Copy as Markdown", "Save as File", "Clear Annotations",
    ]);
    expect(items[0].children!.map((item) => item.label)).toEqual(["claude", "bash"]);
    // Neither of these terminals' servers keeps scratchpads.
    expect(items[2].children!.every((item) => item.disabled)).toBe(true);
  });

  it("names no terminal where the screen has none", () => {
    const { shown, showMenu } = setup({ text: PLAN });
    ask(shown, "retry three times", "Why three?");
    const items = handoverMenu(shown, showMenu);
    expect(items[0].children).toEqual([expect.objectContaining({ label: "No terminal pane on this screen", disabled: true })]);
  });

  // viewer-annotations, Handing A Review To A Terminal: Copied.
  it("copies and saves the review and keeps the annotations", async () => {
    const { shown, store, showMenu, writeClipboard, download, notices } = setup({ text: PLAN });
    ask(shown, "retry three times", "Why three?");
    handoverMenu(shown, showMenu).find((item) => item.label === "Copy as Markdown")!.action();
    await vi.waitFor(() => expect(notices.info).toHaveBeenCalledWith("The review is in the clipboard"));
    expect(writeClipboard.mock.calls[0][0]).toMatch(/^# Review of PLAN\.md/);
    handoverMenu(shown, showMenu).find((item) => item.label === "Save as File")!.action();
    expect(download.mock.calls[0][0]).toBe("review-PLAN.md");
    expect(await download.mock.calls[0][1].text()).toBe(writeClipboard.mock.calls[0][0]);
    expect(store.count("v1")).toBe(1);
  });

  it("clears the annotations with an undo", () => {
    const { shown, store, showMenu, notices } = setup({ text: PLAN });
    ask(shown, "retry three times", "Why three?");
    ask(shown, "Step 3 of the plan.", "Early step.");
    handoverMenu(shown, showMenu).find((item) => item.label === "Clear Annotations")!.action();
    expect(store.count("v1")).toBe(0);
    expect(notices.undoable).toHaveBeenCalledWith("2 annotations cleared", expect.any(Function));
    notices.undoable.mock.calls[0][1]();
    expect(store.count("v1")).toBe(2);
  });

  // viewer-annotations, Handing A Review To A Terminal: Added while sending.
  it("keeps an annotation another window added while the review was on its way", async () => {
    let stored: (path: string) => void = () => {};
    const agent = terminal("t1", "claude", { upload: () => new Promise<string>((resolve) => { stored = resolve; }) });
    const { shown, store, showMenu } = setup({ text: PLAN, origin: agent.target });
    ask(shown, "retry three times", "Why three?");
    const [own] = store.list("v1");
    handoverMenu(shown, showMenu).find((item) => item.label === "Send as File To")!.children![0].action();

    const theirs: ViewerAnnotation = { id: "ann-00000000000000bb", kind: "comment", body: "From the other window." };
    store.snapshot(screenWith([own, theirs]));
    expect(store.count("v1")).toBe(2);
    stored("/home/me/.cache/splitty/uploads/review-PLAN.md");
    await vi.waitFor(() => expect(agent.pasted).toEqual(["/home/me/.cache/splitty/uploads/review-PLAN.md"]));
    expect(store.list("v1")).toEqual([theirs]);
  });
});
