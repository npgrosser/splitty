// @vitest-environment jsdom
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AnnotationReply, MuxOperation, ReviewWait, ScreenState, ViewerAnnotation, ViewerRecord } from "../../lib/protocol/types.ts";
import { CONTEXT_CHARS } from "../../lib/viewer/anchoring.ts";
import { ViewerAnnotations } from "../../lib/viewer/annotations.ts";
import type { ReviewAnswerRequest, ReviewTarget } from "../../lib/viewer/handover.ts";
import type { BoundaryRange } from "../../lib/viewer/render.ts";
import type { ContextMenuItem } from "../../stores/ui.svelte.ts";
import { ViewerView } from "../viewer_view.ts";
import type { ViewerViewState } from "../viewer_view_state.ts";
import { DELETION_NOTE } from "./annotation_form.ts";
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
/** The passage on line 30, as a window annotated it. */
const RETRY = { view: "source" as const, quote: "retry three times", prefix: "2. On failure, ", suffix: " before giving up.", startLine: 30, endLine: 30 };
const WAIT: ReviewWait = { id: "wait-0123456789abcdef", paneId: "t1", title: "claude" };
const WAITING: ViewerRecord = { ...PLAN_RECORD, reviewWait: WAIT };

function screenWith(annotations: ViewerAnnotation[]): ScreenState {
  return {
    id: "screen-1",
    root: { type: "slot", id: "s1", paneIds: ["v1"] },
    panes: { v1: { id: "v1", role: "viewer", viewer: { ...PLAN_RECORD, ...(annotations.length > 0 ? { annotations } : {}) } } },
    sessions: {},
  } as unknown as ScreenState;
}

/**
 * The pane's server as far as annotations go: it applies the operations of
 * its windows by its rules, numbering each new annotation, and answers a
 * window's resync with a snapshot made after the window's operations. It
 * tells a window nothing of the window's own operations; `broadcast` is the
 * resync of every window after a change of another client.
 */
class PaneServer {
  readonly ops: MuxOperation[] = [];
  annotations: ViewerAnnotation[] = [];
  /** The server rejects every operation: it keeps the workspace as it is. */
  refusing = false;
  private last = 0;
  private replyCount = 0xa000;
  private readonly windows: ViewerAnnotations[] = [];

  /** The annotation store of one more window. */
  window(): ViewerAnnotations {
    const store = new ViewerAnnotations({
      send: (_screenId, op) => {
        this.ops.push(op);
        if (!this.refusing) this.apply(op);
      },
      resync: () => queueMicrotask(() => this.snapshotTo(store)),
    });
    this.windows.push(store);
    return store;
  }

  broadcast(): void {
    for (const store of this.windows) this.snapshotTo(store);
  }

  /** The server holds `annotations` now, as after other clients' changes, and every window resyncs. */
  serve(annotations: ViewerAnnotation[]): void {
    this.annotations = annotations;
    this.last = Math.max(this.last, ...annotations.map((annotation) => annotation.number ?? 0));
    this.broadcast();
  }

  sent<T extends MuxOperation["type"]>(type: T): Extract<MuxOperation, { type: T }>[] {
    return this.ops.filter((op): op is Extract<MuxOperation, { type: T }> => op.type === type);
  }

  get(number: number): ViewerAnnotation {
    return this.annotations.find((annotation) => annotation.number === number)!;
  }

  /** An agent in pane `t1` replies in the thread of annotation `number`. */
  agentReplies(number: number, body: string, title = "claude"): AnnotationReply {
    const reply: AnnotationReply = { id: `rep-${(++this.replyCount).toString(16).padStart(16, "0")}`, author: "agent", body, from: { paneId: "t1", title } };
    this.annotations = this.annotations.map((annotation) => (
      annotation.number === number ? { ...annotation, replies: [...(annotation.replies ?? []), reply] } : annotation));
    this.broadcast();
    return reply;
  }

  dispose(): void {
    for (const store of this.windows) store.dispose();
  }

  private snapshotTo(store: ViewerAnnotations): void {
    store.snapshot(screenWith(JSON.parse(JSON.stringify(this.annotations)) as ViewerAnnotation[]));
  }

  private apply(op: MuxOperation): void {
    const edit = (id: string, change: (annotation: ViewerAnnotation) => ViewerAnnotation) => {
      this.annotations = this.annotations.map((annotation) => (annotation.id === id ? change(annotation) : annotation));
    };
    switch (op.type) {
      case "put_annotations":
        for (const put of op.annotations) {
          const held = this.annotations.find((annotation) => annotation.id === put.id);
          if (held) edit(put.id, () => ({ ...put, number: held.number, ...(held.replies ? { replies: held.replies } : {}) }));
          else this.annotations.push({ ...put, number: ++this.last });
        }
        break;
      case "remove_annotations":
        this.annotations = this.annotations.filter((annotation) => !op.ids.includes(annotation.id));
        break;
      case "put_replies":
        edit(op.annotationId, (annotation) => {
          const replies = [...(annotation.replies ?? [])];
          for (const reply of op.replies) {
            const at = replies.findIndex((each) => each.id === reply.id);
            if (at >= 0) replies[at] = reply;
            else replies.push(reply);
          }
          return { ...annotation, replies };
        });
        break;
      case "remove_replies":
        edit(op.annotationId, (annotation) => {
          const replies = (annotation.replies ?? []).filter((reply) => !op.ids.includes(reply.id));
          const { replies: _replies, ...rest } = annotation;
          return replies.length > 0 ? { ...rest, replies } : rest;
        });
        break;
      case "mark_sent": {
        // As the server: only an entry that reads as the review held it.
        const held = new Map(op.entries.map((entry) => [entry.id, entry]));
        const same = (a: unknown, b: unknown) => JSON.stringify(a ?? null) === JSON.stringify(b ?? null);
        const reads = (annotation: ViewerAnnotation) => {
          const entry = held.get(annotation.id);
          return entry !== undefined && entry.kind === annotation.kind && entry.body === annotation.body
            && entry.replacement === annotation.replacement && same(entry.target, annotation.target);
        };
        this.annotations = this.annotations.map((annotation) => ({
          ...annotation,
          ...(reads(annotation) ? { sent: true as const } : {}),
          ...(annotation.replies ? {
            replies: annotation.replies.map((reply) => (
              reply.author === "user" && held.get(reply.id)?.body === reply.body ? { ...reply, sent: true as const } : reply)),
          } : {}),
        }));
        break;
      }
      default:
        break;
    }
  }
}

let views: ViewerView[] = [];
let servers: PaneServer[] = [];

afterEach(() => {
  for (const view of views) view.destroy();
  for (const server of servers) server.dispose();
  views = [];
  servers = [];
  vi.restoreAllMocks();
  document.body.replaceChildren();
});

interface Terminal {
  readonly target: ReviewTarget;
  readonly pasted: string[];
  scratchpad(): string;
}

function terminal(
  paneId: string,
  title: string,
  options: { bracketedPaste?: boolean; upload?: () => Promise<string>; scratchpad?: boolean } = {},
): Terminal {
  const pasted: string[] = [];
  let pad = "";
  return {
    pasted,
    scratchpad: () => pad,
    target: {
      paneId,
      title,
      bracketedPaste: () => options.bracketedPaste ?? true,
      paste: (text) => pasted.push(text),
      reveal: vi.fn(),
      upload: options.upload ?? (async () => "/home/me/.cache/splitty/uploads/review-PLAN.md"),
      ...(options.scratchpad ? { scratchpad: { text: () => pad, set: (text: string) => { pad = text; }, open: vi.fn() } } : {}),
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
  /** The pane's server, shared by the windows of a test. */
  server?: PaneServer;
  /** The window's store, shared by its views. */
  store?: ViewerAnnotations;
  answer?: (request: ReviewAnswerRequest) => Promise<boolean>;
}

function setup(params: Setup = {}) {
  const server = params.server ?? new PaneServer();
  if (!servers.includes(server)) servers.push(server);
  const store = params.store ?? server.window();
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
    ...(params.answer ? { answer: params.answer } : {}),
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
  return { shown, store, server, notices, showMenu, writeClipboard, download, setOrigin: (target: ReviewTarget | undefined) => { origin = target; } };
}

function read(text: string, version = "v1") {
  return { document: { bytes: encoder.encode(text), version }, reading: false };
}

const query = <T extends Element = HTMLElement>(view: ViewerView, selector: string) => view.element.querySelector<T & HTMLElement>(selector);

function editor(view: ViewerView): EditorView {
  return EditorView.findFromDOM(query(view, ".cm-editor")!)!;
}

/** Select `quote` in the source view. */
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

function formTitle(view: ViewerView): string {
  return form(view)!.querySelector(".viewer-form-title")!.textContent!;
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
  return [...view.element.querySelectorAll<HTMLElement>("li.viewer-annotation")];
}

/** The entry of annotation `number`. */
function entry(view: ViewerView, number: number): HTMLElement {
  return view.element.querySelector<HTMLElement>(`li.viewer-annotation[data-number="${number}"]`)!;
}

function whereOf(view: ViewerView): string[] {
  return entries(view).map((each) => each.querySelector(".viewer-annotation-where")!.textContent!);
}

function click(element: Element | null, selector: string): void {
  element!.querySelector<HTMLButtonElement>(selector)!.click();
}

/** The menu the list's ▾ button shows. */
function handoverMenu(view: ViewerView, showMenu: ReturnType<typeof setup>["showMenu"]): ContextMenuItem[] {
  query<HTMLButtonElement>(view, ".viewer-annotations-more")!.click();
  return showMenu.mock.calls.at(-1)![0];
}

/** Comment on `quote` of the source view through the context menu. */
function comment(view: ViewerView, quote: string, remark: string): void {
  selectSource(view, quote);
  view.annotationMenu()!.annotate("comment");
  write(view, "Comment", remark);
  pressKey(view, { key: "Enter", ctrlKey: true });
}

/** Wait until the server has numbered every annotation the window shows. */
async function numbered(store: ViewerAnnotations): Promise<void> {
  await vi.waitFor(() => expect(store.list("v1").every((annotation) => annotation.number !== undefined)).toBe(true));
}

/** Reply in the thread of `item` through its reply box. */
function reply(item: HTMLElement, text: string, init: KeyboardEventInit = { key: "Enter", ctrlKey: true }): void {
  click(item, ".viewer-annotation-reply");
  const field = item.querySelector<HTMLTextAreaElement>(".viewer-reply-text")!;
  field.value = text;
  field.dispatchEvent(new Event("input", { bubbles: true }));
  field.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }));
}

/** The source view's marked passages. */
function marked(view: ViewerView, selector = ".cm-viewer-marked"): string[] {
  return [...view.element.querySelectorAll(selector)].map((element) => element.textContent!);
}

function bars(view: ViewerView): Element[] {
  return [...view.element.querySelectorAll(".cm-viewer-marks .cm-gutterElement")].filter((element) => element.querySelector(".cm-viewer-mark"));
}

/** The innermost marked span of the source view with `text`, which a pointer over that text is on. */
function markOf(view: ViewerView, text: string): HTMLElement {
  const found = [...view.element.querySelectorAll<HTMLElement>(".cm-viewer-marked")]
    .find((element) => element.textContent === text && !element.querySelector(".cm-viewer-marked"));
  expect(found).toBeDefined();
  return found!;
}

/** Press and release the primary button on `target` without moving, or moved by `moved` pixels. */
function clickOn(target: Element, init: MouseEventInit = {}, moved = 0): void {
  const at = { clientX: 20, clientY: 10 };
  target.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, composed: true, button: 0, ...at }));
  target.dispatchEvent(new MouseEvent("click", { bubbles: true, composed: true, button: 0, detail: 1, ...at, clientX: at.clientX + moved, ...init }));
}

/** Leave the source view with an empty selection, as after a click into it. */
function collapse(view: ViewerView): void {
  editor(view).dispatch({ selection: { anchor: 0 } });
}

function listHidden(view: ViewerView): boolean {
  return query(view, ".viewer-annotations")!.hidden;
}

describe("making annotations", () => {
  // viewer-annotations, Annotations On A Viewer Pane: A question on a passage.
  it("comments on a passage of the source from the selection's toolbar, and shows the server's number", async () => {
    const { shown, store, server } = setup({ text: PLAN });
    selectSource(shown, "retry three times");
    query(shown, ".viewer-document")!.dispatchEvent(new Event("pointerup", { bubbles: true }));
    const toolbar = query(shown, ".viewer-selection-toolbar")!;
    await vi.waitFor(() => expect(toolbar.hidden).toBe(false));
    // Two kinds: a comment, which also asks, and a suggested change.
    expect([...toolbar.querySelectorAll(".viewer-selection-kind")].map((kind) => kind.textContent)).toEqual(["Comment", "Suggest Change"]);
    toolbar.querySelector<HTMLButtonElement>('[data-kind="comment"]')!.click();
    expect(toolbar.hidden).toBe(true);

    expect(formTitle(shown)).toBe("New Comment");
    expect(form(shown)!.querySelector(".viewer-form-quote")!.textContent).toBe("retry three times");
    expect(form(shown)!.querySelector(".viewer-form-label")!.textContent).toBe("Passage, Line 30");
    write(shown, "Comment", "Why three?");
    pressKey(shown, { key: "Enter", ctrlKey: true });

    expect(form(shown)).toBeNull();
    const [annotation] = store.list("v1");
    const at = PLAN.indexOf("retry three times");
    const target = {
      view: "source",
      quote: "retry three times",
      prefix: PLAN.slice(at - CONTEXT_CHARS, at),
      suffix: PLAN.slice(at + 17, at + 17 + CONTEXT_CHARS),
      startLine: 30,
      endLine: 30,
    };
    expect(annotation).toMatchObject({ kind: "comment", body: "Why three?", target });
    expect(annotation.id).toMatch(/^ann-[0-9a-f]{16}$/);
    // Without a number: the server gives it.
    expect(server.ops).toEqual([{ type: "put_annotations", paneId: "v1", annotations: [{ id: annotation.id, kind: "comment", body: "Why three?", target }] }]);
    await numbered(store);
    expect(store.list("v1")[0].number).toBe(1);
    expect(entries(shown)[0].dataset.number).toBe("1");
    expect(entries(shown)[0].querySelector(".viewer-annotation-number")!.textContent).toBe("#1");
    // Listed, and marked in the source.
    expect(whereOf(shown)).toEqual(["Line 30"]);
    expect(marked(shown)).toEqual(["retry three times"]);
  });

  // viewer-annotations, Annotations On A Viewer Pane: A general comment.
  it("adds a general comment without a selection, which is a comment", async () => {
    const { shown, store } = setup({ text: PLAN });
    const menu = shown.annotationMenu()!;
    expect(menu.selection).toBe("none");
    menu.addComment();
    expect(form(shown)!.querySelectorAll(".viewer-form-kind")).toHaveLength(0);
    expect(form(shown)!.querySelector(".viewer-form-quote")).toBeNull();
    write(shown, "Comment", "Split the plan into two.");
    form(shown)!.querySelector<HTMLButtonElement>(".viewer-form-save")!.click();
    await numbered(store);
    expect(store.list("v1")).toEqual([{ id: expect.stringMatching(/^ann-/), number: 1, kind: "comment", body: "Split the plan into two." }]);
    expect(whereOf(shown)).toEqual(["General"]);
    expect(entries(shown)[0].dataset.state).toBe("general");
  });

  it("keeps the form open with the reason when a remark is missing, and cancels with Escape", () => {
    const { shown, store, server } = setup({ text: PLAN });
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
    expect(server.ops).toEqual([]);
  });

  it("starts a suggested change with the passage and saves its replacement", () => {
    const { shown, store } = setup({ text: PLAN });
    selectSource(shown, "retry three times");
    shown.annotationMenu()!.annotate("change");
    expect(formTitle(shown)).toBe("New Suggested Change");
    const replacement = form(shown)!.querySelector<HTMLTextAreaElement>('textarea[aria-label="Replacement"]')!;
    expect(replacement.value).toBe("retry three times");
    expect(document.activeElement).toBe(replacement);
    // Its remark is a reason, and optional.
    expect(form(shown)!.querySelector('textarea[aria-label="Reason (optional)"]')).not.toBeNull();
    write(shown, "Replacement", "retry twice");
    pressKey(shown, { key: "Enter", ctrlKey: true });
    expect(store.list("v1")[0]).toMatchObject({ kind: "change", replacement: "retry twice" });
    expect(store.list("v1")[0]).not.toHaveProperty("body");
    expect(entries(shown)[0].querySelector(".viewer-annotation-kind")!.textContent).toBe("Change");
    expect(entries(shown)[0].querySelector(".viewer-annotation-replacement")!.textContent).toBe("retry twice");
  });

  // viewer-annotations, Annotations On A Viewer Pane: A suggested deletion.
  it("reads an empty replacement as a suggested deletion", () => {
    const { shown, store } = setup({ text: PLAN });
    selectSource(shown, "retry three times");
    shown.annotationMenu()!.annotate("change");
    const note = form(shown)!.querySelector<HTMLElement>(".viewer-form-deletion")!;
    expect(note.hidden).toBe(true);
    write(shown, "Replacement", "");
    expect(formTitle(shown)).toBe("New Suggested Deletion");
    expect(note.hidden).toBe(false);
    expect(note.textContent).toBe(DELETION_NOTE);
    pressKey(shown, { key: "Enter", ctrlKey: true });
    expect(store.list("v1")[0]).toMatchObject({ kind: "change", replacement: "" });
    const [item] = entries(shown);
    expect(item.dataset.deletion).toBe("");
    expect(item.querySelector(".viewer-annotation-kind")!.textContent).toBe("Deletion");
    expect(item.querySelector(".viewer-annotation-deletion")!.textContent).toBe("Delete this passage");
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

  it("edits an annotation from the list, and puts a deleted one back under a new number", async () => {
    const { shown, store, notices } = setup({ text: PLAN });
    comment(shown, "retry three times", "Why three?");
    await numbered(store);
    const [first] = store.list("v1");

    click(entries(shown)[0], '[title="Edit this annotation"]');
    expect(formTitle(shown)).toBe("Edit Comment");
    const remark = form(shown)!.querySelector<HTMLTextAreaElement>('textarea[aria-label="Comment"]')!;
    expect(remark.value).toBe("Why three?");
    write(shown, "Comment", "Why not twice?");
    pressKey(shown, { key: "Enter", ctrlKey: true });
    expect(store.list("v1")).toEqual([{ ...first, body: "Why not twice?" }]);
    expect(entries(shown)[0].querySelector(".viewer-annotation-body")!.textContent).toBe("Why not twice?");

    click(entries(shown)[0], '[title="Delete this annotation"]');
    expect(store.count("v1")).toBe(0);
    expect(notices.undoable).toHaveBeenCalledWith("Annotation deleted", expect.any(Function));
    notices.undoable.mock.calls[0][1]();
    expect(store.list("v1")).toEqual([{ ...first, number: undefined, body: "Why not twice?" }]);
    // The server numbers it anew: a number is never given twice in a pane.
    await vi.waitFor(() => expect(store.list("v1")[0].number).toBe(2));
  });

  // viewer-annotations, Saving Annotations Through The Window: An edit after sending.
  it("puts an edited annotation that was handed over as pending, and one saved unchanged as it was", () => {
    const { shown, store, server } = setup({ text: PLAN });
    server.serve([{ id: "ann-0000000000000001", number: 1, kind: "comment", body: "Why three?", target: RETRY, sent: true }]);
    expect(entries(shown)[0].dataset.sent).toBe("");

    click(entries(shown)[0], '[title="Edit this annotation"]');
    pressKey(shown, { key: "Enter", ctrlKey: true });
    expect(server.sent("put_annotations").at(-1)!.annotations[0]).toMatchObject({ body: "Why three?", sent: true });

    click(entries(shown)[0], '[title="Edit this annotation"]');
    write(shown, "Comment", "Why not twice?");
    pressKey(shown, { key: "Enter", ctrlKey: true });
    expect(server.sent("put_annotations").at(-1)!.annotations[0]).not.toHaveProperty("sent");
    expect(store.list("v1")[0]).not.toHaveProperty("sent");
    expect(entries(shown)[0].dataset.sent).toBeUndefined();
    expect(entries(shown)[0].querySelector(".viewer-annotation-sent")).toBeNull();
  });

  // viewer-annotations, Concurrent Annotation Edits: the form keeps what the user writes.
  it("keeps the form's text while another window changes the same annotation, and saving replaces that change", async () => {
    const { shown, store, server } = setup({ text: PLAN });
    comment(shown, "retry three times", "Why three?");
    await numbered(store);
    const [own] = store.list("v1");
    click(entries(shown)[0], '[title="Edit this annotation"]');
    write(shown, "Comment", "Why not twice?");

    server.serve([{ ...server.get(1), body: "Changed elsewhere." }]);
    expect(entries(shown)[0].querySelector(".viewer-annotation-body")!.textContent).toBe("Changed elsewhere.");
    expect(form(shown)!.querySelector<HTMLTextAreaElement>('textarea[aria-label="Comment"]')!.value).toBe("Why not twice?");
    pressKey(shown, { key: "Enter", ctrlKey: true });
    expect(store.list("v1")).toEqual([{ ...own, body: "Why not twice?" }]);
  });

  // viewer-annotations, Anchoring By Quote: Moved to another passage.
  it("Moved to another passage: gives an annotation not found the selected passage", () => {
    const { shown, store, server } = setup({ text: PLAN });
    server.serve([{
      id: "ann-0000000000000001", number: 1, kind: "comment", body: "Gone?", sent: true,
      target: { view: "source", quote: "no such passage", prefix: "", suffix: "", startLine: 2, endLine: 2 },
    }]);
    const item = entry(shown, 1);
    expect(item.dataset.state).toBe("stale");
    expect(item.querySelector(".viewer-annotation-not-found")!.textContent).toBe("not found");
    const move = item.querySelector<HTMLButtonElement>(".viewer-annotation-move")!;
    expect(move.disabled).toBe(true);

    selectSource(shown, "Step 12 of the plan.");
    expect(move.disabled).toBe(false);
    move.click();
    const [moved] = server.sent("put_annotations");
    expect(moved.annotations[0]).toMatchObject({ id: "ann-0000000000000001", target: { quote: "Step 12 of the plan.", startLine: 12, endLine: 12 } });
    // Its passage changed: it is pending again.
    expect(moved.annotations[0]).not.toHaveProperty("sent");
    expect(store.list("v1")[0].number).toBe(1);
    expect(entry(shown, 1).dataset.state).toBe("anchored");
    expect(entry(shown, 1).querySelector(".viewer-annotation-not-found")).toBeNull();
    expect(whereOf(shown)).toEqual(["Line 12"]);
    expect(marked(shown)).toEqual(["Step 12 of the plan."]);
  });

  // viewer-annotations, Annotations Capability: Host without annotations.
  it("offers no annotations where the server keeps none", async () => {
    const { shown } = setup({ text: PLAN, available: false });
    expect(shown.annotationMenu()).toBeUndefined();
    expect(query(shown, ".viewer-bar-annotations")!.hidden).toBe(true);
    expect(query(shown, ".viewer-bar-marks")!.hidden).toBe(true);
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
    const { shown, store, server, notices } = setup({ text: PLAN });
    server.refusing = true;
    comment(shown, "retry three times", "Why three?");
    store.failed({ op: "put_annotations", screenId: "screen-1", paneId: "v1", error: "pane v1 is no viewer pane" });
    expect(store.count("v1")).toBe(0);
    expect(notices.error).toHaveBeenCalledWith("The annotations could not be saved", "pane v1 is no viewer pane");
    expect(query(shown, ".viewer-annotations")!.hidden).toBe(true);
    expect(query(shown, ".viewer-bar-annotations")!.textContent).toBe("Annotations");
  });
});

describe("threads", () => {
  /** #1 on line 30, handed over, with an agent's answer; #2 on line 3. */
  function answered(params: Setup = {}) {
    const setUp = setup({ text: PLAN, ...params });
    setUp.server.serve([
      {
        id: "ann-0000000000000001", number: 1, kind: "comment", body: "Why three?", target: RETRY, sent: true,
        replies: [{ id: "rep-0000000000000001", author: "agent", body: "Use <b>on-demand</b> **retries**.\u001b[2J", from: { paneId: "t1", title: "claude" } }],
      },
      {
        id: "ann-0000000000000002", number: 2, kind: "comment", body: "Early step.",
        target: { view: "source", quote: "Step 3 of the plan.", prefix: "", suffix: "", startLine: 3, endLine: 3 },
      },
    ]);
    return setUp;
  }

  // viewer-annotations, Annotations In The Pane: an agent's reply as plain text with the title of its pane.
  it("shows an agent's reply as text, with the title of the pane it came from and its controls replaced", () => {
    const { shown } = answered();
    const item = entry(shown, 1);
    expect(item.dataset.sent).toBe("");
    expect(item.querySelector(".viewer-annotation-sent")!.textContent).toBe("sent");
    const replies = [...item.querySelectorAll<HTMLElement>("li.viewer-reply")];
    expect(replies.map((each) => each.dataset.author)).toEqual(["agent"]);
    expect(replies[0].querySelector(".viewer-reply-author")!.textContent).toBe("Agent (claude)");
    const body = replies[0].querySelector(".viewer-reply-body")!;
    // Text, never markup, with what a terminal acts on or what hides text replaced.
    expect(body.textContent).toBe("Use <b>on-demand</b> **retries**.\ufffd[2J");
    expect(body.querySelector("b")).toBeNull();
    expect(body.children).toHaveLength(0);
  });

  // viewer-annotations, Threads: The user answers an agent.
  it("The user answers an agent: the reply follows the agent's, as the user's", () => {
    const { shown, store, server } = answered();
    reply(entry(shown, 1), "Then retry five times.");
    const [put] = server.sent("put_replies");
    expect(put).toEqual({
      type: "put_replies", paneId: "v1", annotationId: "ann-0000000000000001",
      replies: [{ id: expect.stringMatching(/^rep-[0-9a-f]{16}$/), author: "user", body: "Then retry five times." }],
    });
    expect(store.get("v1", "ann-0000000000000001")!.replies!.map((each) => each.author)).toEqual(["agent", "user"]);
    const replies = [...entry(shown, 1).querySelectorAll<HTMLElement>("li.viewer-reply")];
    expect(replies.map((each) => each.querySelector(".viewer-reply-author")!.textContent)).toEqual(["Agent (claude)", "You"]);
    // The thread's first message names its sender as well.
    expect(entry(shown, 1).querySelector(".viewer-annotation-author")!.textContent).toBe("You");
    expect(entry(shown, 1).querySelector<HTMLElement>(".viewer-reply-box")!.hidden).toBe(true);
  });

  it("names the thread in its reply box, refuses an empty reply and cancels with Escape", () => {
    const { shown, server } = answered();
    const item = entry(shown, 1);
    reply(item, "  ");
    const box = item.querySelector<HTMLElement>(".viewer-reply-box")!;
    expect(box.hidden).toBe(false);
    expect(item.querySelector(".viewer-reply-text")!.getAttribute("aria-label")).toBe("Reply to #1");
    expect(item.querySelector<HTMLElement>(".viewer-reply-problem")!.textContent).toBe("A reply needs a text.");
    item.querySelector(".viewer-reply-text")!.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Escape" }));
    expect(box.hidden).toBe(true);
    expect(server.sent("put_replies")).toEqual([]);
  });

  it("keeps a reply being written while an agent answers in the same thread", () => {
    const { shown, server } = answered();
    const item = entry(shown, 1);
    click(item, ".viewer-annotation-reply");
    const field = item.querySelector<HTMLTextAreaElement>(".viewer-reply-text")!;
    field.value = "Half a thought";
    server.agentReplies(1, "One more thing.");
    expect(entry(shown, 1)).toBe(item);
    expect(item.querySelectorAll("li.viewer-reply")).toHaveLength(2);
    expect(item.querySelector<HTMLTextAreaElement>(".viewer-reply-text")!.value).toBe("Half a thought");
    expect(item.querySelector<HTMLElement>(".viewer-reply-box")!.hidden).toBe(false);
  });

  it("edits the user's reply, and removes an agent's without an undo and the user's with one", () => {
    const { shown, store, server, notices } = answered();
    reply(entry(shown, 1), "Then retry five times.");
    const own = store.get("v1", "ann-0000000000000001")!.replies![1];
    const [agent, mine] = [...entry(shown, 1).querySelectorAll<HTMLElement>("li.viewer-reply")];
    // An agent's reply cannot be edited.
    expect(agent.querySelector('[title="Edit this reply"]')).toBeNull();

    click(mine, '[title="Edit this reply"]');
    const field = entry(shown, 1).querySelector<HTMLTextAreaElement>(".viewer-reply-text")!;
    expect(field.value).toBe("Then retry five times.");
    expect(entry(shown, 1).querySelector(".viewer-reply-submit")!.textContent).toBe("Save");
    field.value = "Then retry four times.";
    field.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Enter", ctrlKey: true }));
    expect(server.sent("put_replies").at(-1)!.replies).toEqual([{ id: own.id, author: "user", body: "Then retry four times." }]);

    click(entry(shown, 1).querySelector("li.viewer-reply")!, '[title="Remove this reply"]');
    expect(server.sent("remove_replies")).toEqual([{ type: "remove_replies", paneId: "v1", annotationId: "ann-0000000000000001", ids: ["rep-0000000000000001"] }]);
    expect(notices.undoable).not.toHaveBeenCalled();

    click(entry(shown, 1).querySelector("li.viewer-reply")!, '[title="Remove this reply"]');
    expect(store.get("v1", "ann-0000000000000001")).not.toHaveProperty("replies");
    expect(notices.undoable).toHaveBeenCalledWith("Reply removed", expect.any(Function));
    notices.undoable.mock.calls[0][1]();
    expect(store.get("v1", "ann-0000000000000001")!.replies).toEqual([{ id: own.id, author: "user", body: "Then retry four times." }]);
  });

  // viewer-annotations, Resolving A Thread: Resolved after the agent's answer.
  it("Resolved after the agent's answer: folds it among the resolved ones with its replies, and marks it no longer", () => {
    const { shown, store, server } = answered();
    expect(marked(shown)).toEqual(["Step 3 of the plan.", "retry three times"]);
    const resolve = entry(shown, 1).querySelector<HTMLButtonElement>(".viewer-annotation-resolve")!;
    expect(resolve.textContent).toBe("Resolve");
    resolve.click();

    // The status goes with the annotation; its number, thread and hand-over stay.
    expect(server.sent("put_annotations")).toEqual([{
      type: "put_annotations", paneId: "v1",
      annotations: [{ id: "ann-0000000000000001", kind: "comment", body: "Why three?", target: RETRY, status: "resolved", sent: true }],
    }]);
    expect(store.get("v1", "ann-0000000000000001")).toMatchObject({ number: 1, status: "resolved", replies: [expect.objectContaining({ author: "agent" })] });
    const group = query<HTMLDetailsElement>(shown, "details.viewer-annotations-resolved")!;
    expect(group.hidden).toBe(false);
    expect(group.open).toBe(false);
    expect(group.querySelector(".viewer-annotations-resolved-summary")!.textContent).toBe("Resolved (1)");
    const item = group.querySelector<HTMLElement>("li.viewer-annotation")!;
    expect(item.dataset.number).toBe("1");
    expect(item.dataset.status).toBe("resolved");
    expect(item.querySelectorAll("li.viewer-reply")).toHaveLength(1);
    expect([...query(shown, ".viewer-annotations-list:not(.viewer-annotations-resolved-list)")!.children].map((each) => (each as HTMLElement).dataset.number)).toEqual(["2"]);
    expect(marked(shown)).toEqual(["Step 3 of the plan."]);
    expect(query(shown, ".viewer-annotations-count")!.textContent).toBe("1 open, 1 resolved");
    expect(query(shown, ".viewer-bar-annotations")!.textContent).toBe("1 Annotation");
    // A resolved thread is reopened before it goes on.
    expect(item.querySelector<HTMLElement>(".viewer-annotation-actions")!.hidden).toBe(true);
    // The group stays folded; the keyboard goes to it.
    expect(document.activeElement).toBe(group.querySelector("summary"));
  });

  // viewer-annotations, Resolving A Thread: Reopened.
  it("Reopened: open again with its number and replies, and marked again", () => {
    const { shown, store, server } = answered();
    click(entry(shown, 1), ".viewer-annotation-resolve");
    const reopen = entry(shown, 1).querySelector<HTMLButtonElement>(".viewer-annotation-resolve")!;
    expect(reopen.textContent).toBe("Reopen");
    expect(reopen.title).toBe("Reopen this annotation");
    reopen.click();
    expect(server.sent("put_annotations").at(-1)!.annotations[0]).not.toHaveProperty("status");
    expect(store.get("v1", "ann-0000000000000001")).toMatchObject({ number: 1, sent: true, replies: [expect.objectContaining({ author: "agent" })] });
    expect(store.get("v1", "ann-0000000000000001")).not.toHaveProperty("status");
    expect(entries(shown).map((each) => each.dataset.number)).toEqual(["2", "1"]);
    expect(query(shown, "details.viewer-annotations-resolved")!.hidden).toBe(true);
    expect(marked(shown)).toEqual(["Step 3 of the plan.", "retry three times"]);
    expect(document.activeElement).toBe(entry(shown, 1));
  });
});

describe("marks", () => {
  // viewer-annotations, Annotations In The Pane: Marks hidden.
  it("Marks hidden: no marks in the document, the list keeps the annotations, and other windows keep their marks", async () => {
    const server = new PaneServer();
    const first = setup({ text: PLAN, server });
    const second = setup({ text: PLAN, server });
    comment(first.shown, "retry three times", "Why three?");
    await numbered(first.store);
    server.broadcast();
    expect(marked(second.shown)).toEqual(["retry three times"]);

    // The eye in the pane's bar, beside the list's button.
    const hide = query<HTMLButtonElement>(first.shown, ".viewer-bar .viewer-bar-marks")!;
    expect(hide.nextElementSibling!.classList.contains("viewer-bar-annotations")).toBe(true);
    expect(hide.getAttribute("aria-pressed")).toBe("false");
    expect(hide.title).toBe("Hide the marks in the document");
    expect(query(first.shown, ".viewer-annotations-hide-marks")).toBeNull();
    hide.click();
    expect(hide.getAttribute("aria-pressed")).toBe("true");
    expect(hide.title).toBe("Show the marks in the document");
    expect(marked(first.shown)).toEqual([]);
    expect(bars(first.shown)).toHaveLength(0);
    expect(entries(first.shown)).toHaveLength(1);
    expect(marked(second.shown)).toEqual(["retry three times"]);
    expect(bars(second.shown)).toHaveLength(1);
    expect(first.shown.annotationMenu()!.marksHidden).toBe(true);

    // The choice stays with the pane's next view in this window.
    const state = first.shown.viewState();
    expect(state).toMatchObject({ hideMarks: true });
    const next = setup({ text: PLAN, server, store: first.store, state }).shown;
    expect(marked(next)).toEqual([]);
    next.annotationMenu()!.toggleMarks();
    expect(marked(next)).toEqual(["retry three times"]);
    expect(next.viewState()).not.toHaveProperty("hideMarks");
  });

  // viewer-annotations, Annotations In The Pane: An annotation chosen while the marks are hidden (D2 of viewer-clickable-marks).
  it("An annotation chosen while the marks are hidden: marks only its passage, and every mark once they show again", async () => {
    const { shown, store } = setup({ text: PLAN });
    comment(shown, "retry three times", "Why three?");
    comment(shown, "Step 12 of the plan.", "Which step?");
    await numbered(store);
    const eye = query<HTMLButtonElement>(shown, ".viewer-bar-marks")!;
    eye.click();
    // Hiding the marks hides the passage just annotated as well.
    expect(marked(shown)).toEqual([]);
    entry(shown, 1).click();
    expect(marked(shown, ".cm-viewer-marked-active")).toEqual(["retry three times"]);
    expect(bars(shown)).toHaveLength(1);
    entry(shown, 2).click();
    expect(marked(shown)).toEqual(["Step 12 of the plan."]);
    eye.click();
    expect(marked(shown).sort()).toEqual(["Step 12 of the plan.", "retry three times"]);
    eye.click();
    expect(marked(shown)).toEqual([]);
  });

  it("marks the passages of resolved annotations only while the user shows them", () => {
    const { shown, server } = setup({ text: PLAN });
    server.serve([{ id: "ann-0000000000000001", number: 1, kind: "comment", body: "Why three?", target: RETRY, status: "resolved" }]);
    expect(marked(shown)).toEqual([]);
    const show = query<HTMLButtonElement>(shown, ".viewer-annotations-show-resolved")!;
    expect(show.disabled).toBe(false);
    show.click();
    expect(show.getAttribute("aria-pressed")).toBe("true");
    expect(marked(shown, ".cm-viewer-marked-resolved")).toEqual(["retry three times"]);
    expect(shown.viewState()).toMatchObject({ showResolved: true });
    shown.annotationMenu()!.toggleResolved();
    expect(marked(shown)).toEqual([]);
  });

  // viewer-annotations, Anchoring By Quote: The passage reworded.
  it("The passage reworded: marks it as changed and names its recorded lines in the review", async () => {
    const { shown, store, showMenu, writeClipboard } = setup({ text: PLAN });
    comment(shown, "retry three times", "Why three?");
    await numbered(store);
    shown.update(read(`Step 0 of the plan.\n${PLAN.replace("retry three times", "retry it three times")}`, "v2"));
    const [item] = entries(shown);
    expect(item.dataset.state).toBe("changed");
    expect(item.classList.contains("changed")).toBe(true);
    expect(item.querySelector(".viewer-annotation-changed")!.textContent).toBe("changed");
    expect(item.querySelector(".viewer-annotation-quote")!.textContent).toBe("retry three times");
    expect(whereOf(shown)).toEqual(["Line 31"]);
    expect(marked(shown, ".cm-viewer-marked-changed").join("")).toContain("three times");
    expect(query(shown, ".cm-viewer-mark-changed")).not.toBeNull();

    handoverMenu(shown, showMenu).find((each) => each.label === "Copy as Markdown")!.action();
    await vi.waitFor(() => expect(writeClipboard).toHaveBeenCalledTimes(1));
    expect(writeClipboard.mock.calls[0][0]).toContain("## #1 Line 30, comment (passage changed since)\n> retry three times\n\nWhy three?");
  });

  // viewer-annotations, Annotations In The Pane: From the rendered view to the source.
  it("From the rendered view to the source: marks the lines of a rendered passage in the source", () => {
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
    expect(bars(shown)).toHaveLength(1);
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
});

// viewer-annotations, Annotations In The Pane: A mark chosen (D1 of viewer-clickable-marks).
describe("from a mark to its thread", () => {
  it("A mark chosen: a click on a marked passage opens the closed list on its thread and gives it the keyboard", async () => {
    const { shown, store } = setup({ text: PLAN });
    comment(shown, "retry three times", "Why three?");
    comment(shown, "Step 12 of the plan.", "Which step?");
    await numbered(store);
    query<HTMLButtonElement>(shown, ".viewer-bar-annotations")!.click();
    expect(listHidden(shown)).toBe(true);
    collapse(shown);
    const scrolled = vi.spyOn(EditorView, "scrollIntoView");

    clickOn(markOf(shown, "retry three times"));
    expect(listHidden(shown)).toBe(false);
    expect(entry(shown, 1).classList.contains("active")).toBe(true);
    expect(document.activeElement).toBe(entry(shown, 1));
    expect(marked(shown, ".cm-viewer-marked-active")).toEqual(["retry three times"]);
    // The passage is where the user pointed: the document stays where it is.
    expect(scrolled).not.toHaveBeenCalled();
    expect(shown.viewState()).toMatchObject({ annotations: "open" });
  });

  // viewer-annotations, Annotations In The Pane: Overlapping passages.
  it("Overlapping passages: chooses the shortest first and the others on further clicks", async () => {
    const { shown, store, server } = setup({ text: PLAN });
    comment(shown, "retry three times before giving up", "Why retry?");
    comment(shown, "three", "Why three?");
    await numbered(store);
    collapse(shown);
    clickOn(markOf(shown, "three"));
    expect(entry(shown, 2).classList.contains("active")).toBe(true);
    clickOn(markOf(shown, "three"));
    expect(entry(shown, 1).classList.contains("active")).toBe(true);
    clickOn(markOf(shown, "three"));
    expect(entry(shown, 2).classList.contains("active")).toBe(true);
    // Elsewhere on the longer passage: only it.
    const longer = shown.element.querySelector(`[data-annotation-id="${server.get(1).id}"]`)!;
    clickOn(longer.lastChild as Element);
    expect(entry(shown, 1).classList.contains("active")).toBe(true);
    clickOn(longer.lastChild as Element);
    expect(entry(shown, 1).classList.contains("active")).toBe(true);
  });

  it("leaves a click that ends a selection, a drag, a double click and a click with a modifier to the document", async () => {
    const { shown, store } = setup({ text: PLAN });
    comment(shown, "retry three times", "Why three?");
    comment(shown, "Step 12 of the plan.", "Which step?");
    await numbered(store);
    query<HTMLButtonElement>(shown, ".viewer-bar-annotations")!.click();

    selectSource(shown, "retry three");
    clickOn(markOf(shown, "retry three times"));
    expect(listHidden(shown)).toBe(true);
    collapse(shown);
    clickOn(markOf(shown, "retry three times"), {}, 12);
    clickOn(markOf(shown, "retry three times"), { detail: 2 });
    clickOn(markOf(shown, "retry three times"), { ctrlKey: true });
    expect(listHidden(shown)).toBe(true);
    // Text without a mark.
    clickOn([...shown.element.querySelectorAll(".cm-line")][2]);
    expect(listHidden(shown)).toBe(true);
    clickOn(markOf(shown, "retry three times"));
    expect(listHidden(shown)).toBe(false);
  });

  it("opens the thread from the bar beside a marked line", async () => {
    const { shown, store } = setup({ text: PLAN });
    comment(shown, "retry three times", "Why three?");
    comment(shown, "Step 12 of the plan.", "Which step?");
    await numbered(store);
    collapse(shown);
    const view = editor(shown);
    const gutter = query(shown, ".cm-viewer-marks")!;
    const clientY = view.lineBlockAt(view.state.doc.line(30).from).top + view.documentTop + 1;
    gutter.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, button: 0, clientX: 2, clientY }));
    gutter.dispatchEvent(new MouseEvent("click", { bubbles: true, button: 0, detail: 1, clientX: 2, clientY }));
    expect(entry(shown, 1).classList.contains("active")).toBe(true);
  });

  it("opens the thread of a marked block of the rendered view", async () => {
    const { shown, store } = setup({ text: RENDERED_PLAN, mode: "rendered" });
    selectRendered(shown, "retry three times");
    shown.annotationMenu()!.annotate("comment");
    write(shown, "Comment", "Make it configurable.");
    pressKey(shown, { key: "Enter", ctrlKey: true });
    await numbered(store);
    vi.restoreAllMocks();
    query<HTMLButtonElement>(shown, ".viewer-bar-annotations")!.click();
    const root = query(shown, ".viewer-rendered")!.shadowRoot!;
    // Without the highlight API, the passage's block is marked.
    const block = root.querySelector("[data-splitty-mark]")!;
    clickOn(block.firstChild!.parentElement!);
    expect(listHidden(shown)).toBe(false);
    expect(document.activeElement).toBe(entry(shown, 1));
    expect(root.querySelector("[data-splitty-mark]")!.getAttribute("data-splitty-mark")).toBe("active");
  });

  it("shows which annotation the mark under the pointer is, and what a click there chooses", async () => {
    const { shown, store } = setup({ text: PLAN });
    comment(shown, "retry three times before giving up", "Why retry?");
    selectSource(shown, "three");
    shown.annotationMenu()!.annotate("change");
    write(shown, "Replacement", "five");
    pressKey(shown, { key: "Enter", ctrlKey: true });
    await numbered(store);
    collapse(shown);
    const box = query(shown, ".viewer-document")!;
    const move = (target: Element) => target.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, buttons: 0, clientX: 20, clientY: 10 }));

    move(markOf(shown, "three"));
    await vi.waitFor(() => expect(box.title).toBe("#2 · Change, #1 · Comment"));
    expect(box.classList.contains("viewer-mark-hover")).toBe(true);
    expect(marked(shown, ".cm-viewer-marked-hover")).toEqual(["three"]);

    // After a click there, the pointer shows the one the next click chooses.
    clickOn(markOf(shown, "three"));
    move(markOf(shown, "three"));
    await vi.waitFor(() => expect(marked(shown, ".cm-viewer-marked-hover").join("")).toContain("retry"));
    expect(box.title).toBe("#2 · Change, #1 · Comment");

    box.dispatchEvent(new MouseEvent("mouseleave"));
    expect(box.hasAttribute("title")).toBe(false);
    expect(box.classList.contains("viewer-mark-hover")).toBe(false);
    expect(marked(shown, ".cm-viewer-marked-hover")).toEqual([]);
  });
});

describe("annotations in the pane", () => {
  // viewer-annotations, Annotations In The Pane: General comments first.
  it("lists the open ones, general comments first, then anchored in document order, then those not found, and the resolved ones folded after them", async () => {
    const { shown, store } = setup({ text: PLAN });
    shown.annotationMenu()!.addComment();
    write(shown, "Comment", "General remark.");
    pressKey(shown, { key: "Enter", ctrlKey: true });
    comment(shown, "Step 35 of the plan.", "Later step.");
    comment(shown, "Step 3 of the plan.", "Early step.");
    store.put("screen-1", "v1", [{
      id: "ann-00000000000000ff", kind: "comment", body: "Gone.",
      target: { view: "source", quote: "no such passage", prefix: "", suffix: "", startLine: 2, endLine: 2 },
    }]);
    store.put("screen-1", "v1", [{
      id: "ann-00000000000000fe", kind: "comment", body: "Done.", status: "resolved",
      target: { view: "source", quote: "Step 20 of the plan.", prefix: "", suffix: "", startLine: 20, endLine: 20 },
    }]);
    await numbered(store);
    expect(whereOf(shown)).toEqual(["General", "Line 3", "Line 35", "Line 2", "Line 20"]);
    expect(entries(shown).map((each) => each.dataset.number)).toEqual(["1", "3", "2", "4", "5"]);
    expect(entries(shown).map((each) => each.dataset.state)).toEqual(["general", "anchored", "anchored", "stale", "anchored"]);
    expect(entries(shown)[3].classList.contains("stale")).toBe(true);
    expect(entries(shown)[3].querySelector(".viewer-annotation-not-found")).not.toBeNull();
    expect(query(shown, ".viewer-annotations-resolved-list")!.contains(entries(shown)[4])).toBe(true);
    expect(query<HTMLDetailsElement>(shown, "details.viewer-annotations-resolved")!.open).toBe(false);
    expect(query(shown, ".viewer-annotations-count")!.textContent).toBe("4 open, 1 resolved");
    expect(query(shown, ".viewer-bar-annotations")!.textContent).toBe("4 Annotations");
  });

  // viewer-annotations, Anchoring By Quote: Lines inserted above the passage, The passage removed.
  it("follows the passage through new versions and keeps one that is gone", () => {
    const { shown } = setup({ text: PLAN });
    comment(shown, "retry three times", "Why three?");

    shown.update(read(`${filler(101, 110)}\n${PLAN}`, "v2"));
    expect(whereOf(shown)).toEqual(["Line 40"]);
    expect(marked(shown)).toEqual(["retry three times"]);

    shown.update(read(PLAN.replace("retry three times", "give up"), "v3"));
    expect(whereOf(shown)).toEqual(["Line 30"]);
    expect(entries(shown)[0].dataset.state).toBe("stale");
    expect(entries(shown)[0].querySelector(".viewer-annotation-not-found")!.textContent).toBe("not found");
    expect(entries(shown)[0].querySelector(".viewer-annotation-quote")!.textContent).toBe("retry three times");
    expect(query(shown, ".cm-viewer-marked")).toBeNull();

    shown.update(read(PLAN, "v4"));
    expect(whereOf(shown)).toEqual(["Line 30"]);
    expect(entries(shown)[0].dataset.state).toBe("anchored");
  });

  it("names the passage's current lines in the review, and a passage not found as such", async () => {
    const { shown, showMenu, writeClipboard } = setup({ text: PLAN });
    comment(shown, "retry three times", "Why three?");
    shown.update(read(`${filler(101, 110)}\n${PLAN}`, "v2"));
    handoverMenu(shown, showMenu).find((item) => item.label === "Copy as Markdown")!.action();
    await vi.waitFor(() => expect(writeClipboard).toHaveBeenCalledTimes(1));
    expect(writeClipboard.mock.calls[0][0]).toContain("## #1 Line 40, comment\n> retry three times\n\nWhy three?");

    shown.update(read(PLAN.replace("retry three times", "give up"), "v3"));
    handoverMenu(shown, showMenu).find((item) => item.label === "Copy as Markdown")!.action();
    await vi.waitFor(() => expect(writeClipboard).toHaveBeenCalledTimes(2));
    expect(writeClipboard.mock.calls[1][0]).toContain("## #1 Line 30, comment (passage not found in the current version)");
  });

  it("names no lines for a passage of a rendered HTML page", async () => {
    const { shown, showMenu, writeClipboard } = setup({
      record: { source: { kind: "file", path: "/home/me/app/report.html" }, revision: 1 },
      text: "<!doctype html><html><body><h1>Report</h1><p>All checks passed.</p></body></html>\n",
      mode: "rendered",
    });
    selectRendered(shown, "All checks passed");
    shown.annotationMenu()!.annotate("comment");
    write(shown, "Comment", "Which checks?");
    pressKey(shown, { key: "Enter", ctrlKey: true });
    expect(whereOf(shown)).toEqual(["Passage"]);
    handoverMenu(shown, showMenu).find((item) => item.label === "Copy as Markdown")!.action();
    await vi.waitFor(() => expect(writeClipboard).toHaveBeenCalledTimes(1));
    expect(writeClipboard.mock.calls[0][0]).toContain("## #1 Passage, comment\n> All checks passed");
  });

  it("keeps the list closed as the user left it for the pane's next view", () => {
    const { shown, store, server } = setup({ text: PLAN });
    comment(shown, "retry three times", "Why three?");
    expect(query(shown, ".viewer-annotations")!.hidden).toBe(false);
    query<HTMLButtonElement>(shown, ".viewer-annotations-icon")!.click();
    expect(query(shown, ".viewer-annotations")!.hidden).toBe(true);
    const state = shown.viewState();
    expect(state.annotations).toBe("closed");

    const next = setup({ text: PLAN, server, store, state }).shown;
    expect(query(next, ".viewer-annotations")!.hidden).toBe(true);
    expect(next.annotationMenu()!.listShown).toBe(false);
    query<HTMLButtonElement>(next, ".viewer-bar-annotations")!.click();
    expect(query(next, ".viewer-annotations")!.hidden).toBe(false);
  });
});

describe("handing the review over", () => {
  // viewer-annotations, Handing A Review To A Terminal: Send to the agent, Undo after sending.
  it("Send to the agent: pastes the review, keeps the annotations marked as handed over, and offers no undo", async () => {
    const agent = terminal("t1", "claude");
    const { shown, store, server, notices } = setup({ text: PLAN, origin: agent.target });
    comment(shown, "retry three times", "Why three?");
    const send = query<HTMLButtonElement>(shown, ".viewer-annotations-primary")!;
    expect(send.textContent).toBe("Send to claude");
    send.click();
    await vi.waitFor(() => expect(server.sent("mark_sent")).toHaveLength(1));
    expect(agent.pasted).toHaveLength(1);
    expect(agent.pasted[0]).toMatch(/^# Review of PLAN\.md\n\nDocument: \/home\/me\/app\/PLAN\.md\n\n## #1 Line 30, comment\n> retry three times\n\nWhy three\?\n\n/);
    expect(agent.pasted[0].endsWith("Reply in a thread: splitty comments reply v1 <number> '<text>'")).toBe(true);
    expect(agent.target.reveal).toHaveBeenCalledTimes(1);

    const [annotation] = store.list("v1");
    expect(server.sent("mark_sent")).toEqual([{ type: "mark_sent", paneId: "v1", entries: [
      { id: annotation.id, kind: "comment", body: "Why three?", target: annotation.target },
    ] }]);
    expect(store.count("v1")).toBe(1);
    expect(annotation.sent).toBe(true);
    expect(entries(shown)[0].dataset.sent).toBe("");
    expect(entries(shown)[0].querySelector(".viewer-annotation-sent")).not.toBeNull();
    expect(notices.sent).toHaveBeenCalledWith("The review is at the prompt of claude");
    expect(notices.sent.mock.calls[0]).toHaveLength(1);
    expect(notices.undoable).not.toHaveBeenCalled();
    expect(send.textContent).toBe("Send Again to claude");
  });

  // viewer-annotations, The Review: A thread with an answer; Handing A Review To A Terminal: Sent again.
  it("sends what is new in a thread after an agent's answer, and everything open again when nothing is", async () => {
    const agent = terminal("t1", "claude");
    const { shown, store, server } = setup({ text: PLAN, origin: agent.target });
    comment(shown, "retry three times", "Why three?");
    comment(shown, "Step 3 of the plan.", "Early step.");
    const send = query<HTMLButtonElement>(shown, ".viewer-annotations-primary")!;
    send.click();
    await vi.waitFor(() => expect(server.sent("mark_sent")).toHaveLength(1));
    expect(agent.pasted[0]).toContain("## #2 Line 3, comment");
    expect(agent.pasted[0]).toContain("## #1 Line 30, comment");

    server.agentReplies(1, "Three covers a short outage.");
    // An agent's reply is nothing the user sends.
    expect(send.textContent).toBe("Send Again to claude");
    reply(entry(shown, 1), "Then make it five.");
    expect(send.textContent).toBe("Send to claude");
    const own = store.get("v1", server.get(1).id)!.replies!.find((each) => each.author === "user")!;
    send.click();
    await vi.waitFor(() => expect(server.sent("mark_sent")).toHaveLength(2));
    expect(agent.pasted[1]).toContain(
      "## #1 Line 30, comment\n> retry three times\n\nWhy three?\n\nAgent (claude): Three covers a short outage.\n\nUser: Then make it five.",
    );
    expect(agent.pasted[1]).not.toContain("## #2");
    expect(server.sent("mark_sent")[1].entries).toEqual([{ id: own.id, body: "Then make it five." }]);
    expect(entry(shown, 1).querySelector("li.viewer-reply[data-author='user']")!.querySelector(".viewer-reply-sent")).not.toBeNull();

    // Sent again: every open annotation, and nothing more to mark.
    expect(send.textContent).toBe("Send Again to claude");
    expect(send.title).toContain("all open annotations");
    send.click();
    await vi.waitFor(() => expect(agent.pasted).toHaveLength(3));
    expect(agent.pasted[2]).toContain("## #2 Line 3, comment");
    expect(agent.pasted[2]).toContain("## #1 Line 30, comment");
    expect(server.sent("mark_sent")).toHaveLength(2);
  });

  it("holds no resolved annotation in a review", async () => {
    const agent = terminal("t1", "claude");
    const { shown, server } = setup({ text: PLAN, origin: agent.target });
    server.serve([
      { id: "ann-0000000000000001", number: 1, kind: "comment", body: "Why three?", target: RETRY, status: "resolved" },
      { id: "ann-0000000000000002", number: 2, kind: "comment", body: "Split the plan." },
    ]);
    query<HTMLButtonElement>(shown, ".viewer-annotations-primary")!.click();
    await vi.waitFor(() => expect(agent.pasted).toHaveLength(1));
    expect(agent.pasted[0]).toContain("## General comments\n\n- #2 Split the plan.");
    expect(agent.pasted[0]).not.toContain("#1");
  });

  // viewer-annotations, Handing A Review To A Terminal: The origin is gone.
  it("The origin is gone: asks for a terminal of the screen, and sends nothing before", async () => {
    const shell = terminal("t2", "bash");
    const { shown, store, server, showMenu } = setup({ text: PLAN, terminals: [shell.target] });
    comment(shown, "retry three times", "Why three?");
    const send = query<HTMLButtonElement>(shown, ".viewer-annotations-primary")!;
    expect(send.textContent).toBe("Send to Terminal…");
    send.click();
    const items = showMenu.mock.calls.at(-1)![0];
    expect(items.map((item) => item.label)).toEqual(["bash"]);
    expect(shell.pasted).toEqual([]);
    items[0].action();
    await vi.waitFor(() => expect(shell.pasted).toHaveLength(1));
    await vi.waitFor(() => expect(server.sent("mark_sent")).toHaveLength(1));
    expect(store.count("v1")).toBe(1);
    expect(send.textContent).toBe("Send Again to Terminal…");
  });

  // viewer-annotations, Handing A Review To A Terminal: A program without bracketed paste.
  it("A program without bracketed paste: stores the review as a file, inserts its path and marks what it held", async () => {
    const agent = terminal("t1", "claude", { bracketedPaste: false });
    const { shown, server, notices } = setup({ text: PLAN, origin: agent.target });
    comment(shown, "retry three times", "Why three?");
    query<HTMLButtonElement>(shown, ".viewer-annotations-primary")!.click();
    await vi.waitFor(() => expect(server.sent("mark_sent")).toHaveLength(1));
    expect(agent.pasted).toEqual(["/home/me/.cache/splitty/uploads/review-PLAN.md"]);
    expect(notices.sent.mock.calls[0][0]).toContain("takes no bracketed paste");
  });

  // viewer-annotations, Handing A Review To A Terminal: Into the scratchpad.
  it("Into the scratchpad: appends the review, sends the terminal nothing and marks what it held", async () => {
    const agent = terminal("t1", "claude", { scratchpad: true });
    const { shown, server, showMenu } = setup({ text: PLAN, origin: agent.target });
    comment(shown, "retry three times", "Why three?");
    const into = handoverMenu(shown, showMenu).find((item) => item.label === "Append to Scratchpad Of")!;
    expect(into.children!.map((item) => [item.label, !!item.disabled])).toEqual([["claude", false]]);
    into.children![0].action();
    await vi.waitFor(() => expect(server.sent("mark_sent")).toHaveLength(1));
    expect(agent.scratchpad()).toMatch(/^# Review of PLAN\.md\n/);
    expect(agent.scratchpad()).toContain("## #1 Line 30, comment");
    expect(agent.pasted).toEqual([]);
    expect(agent.target.scratchpad!.open).toHaveBeenCalledTimes(1);
  });

  it("marks nothing when the review did not go out", async () => {
    const agent = terminal("t1", "claude", { upload: async () => { throw new Error("The connection changed during the upload."); } });
    const { shown, server, showMenu, notices } = setup({ text: PLAN, origin: agent.target });
    comment(shown, "retry three times", "Why three?");
    handoverMenu(shown, showMenu).find((item) => item.label === "Send as File To")!.children![0].action();
    await vi.waitFor(() => expect(notices.error).toHaveBeenCalledWith("The review was not sent", "The connection changed during the upload."));
    expect(server.sent("mark_sent")).toEqual([]);
  });

  it("offers every form of hand-over, the origin first", () => {
    const agent = terminal("t1", "claude");
    const shell = terminal("t2", "bash");
    const { shown, showMenu } = setup({ text: PLAN, origin: agent.target, terminals: [shell.target, agent.target] });
    comment(shown, "retry three times", "Why three?");
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
    comment(shown, "retry three times", "Why three?");
    const items = handoverMenu(shown, showMenu);
    expect(items[0].children).toEqual([expect.objectContaining({ label: "No terminal pane on this screen", disabled: true })]);
  });

  // viewer-annotations, Handing A Review To A Terminal: Copied.
  it("Copied: copies and saves the review, and marks nothing", async () => {
    const { shown, store, server, showMenu, writeClipboard, download, notices } = setup({ text: PLAN });
    comment(shown, "retry three times", "Why three?");
    handoverMenu(shown, showMenu).find((item) => item.label === "Copy as Markdown")!.action();
    await vi.waitFor(() => expect(notices.info).toHaveBeenCalledWith("The review is in the clipboard"));
    expect(writeClipboard.mock.calls[0][0]).toMatch(/^# Review of PLAN\.md/);
    handoverMenu(shown, showMenu).find((item) => item.label === "Save as File")!.action();
    await vi.waitFor(() => expect(download).toHaveBeenCalledTimes(1));
    expect(download.mock.calls[0][0]).toBe("review-PLAN.md");
    expect(await download.mock.calls[0][1].text()).toBe(writeClipboard.mock.calls[0][0]);
    expect(server.sent("mark_sent")).toEqual([]);
    expect(store.list("v1")[0]).not.toHaveProperty("sent");
  });

  it("clears the annotations with an undo", async () => {
    const { shown, store, showMenu, notices } = setup({ text: PLAN });
    comment(shown, "retry three times", "Why three?");
    comment(shown, "Step 3 of the plan.", "Early step.");
    await numbered(store);
    handoverMenu(shown, showMenu).find((item) => item.label === "Clear Annotations")!.action();
    expect(store.count("v1")).toBe(0);
    expect(notices.undoable).toHaveBeenCalledWith("2 annotations cleared", expect.any(Function));
    notices.undoable.mock.calls[0][1]();
    expect(store.count("v1")).toBe(2);
    await vi.waitFor(() => expect(store.list("v1").map((annotation) => annotation.number)).toEqual([3, 4]));
  });

  // viewer-annotations, Handing A Review To A Terminal: Added while sending.
  it("Added while sending: an annotation another window added stays pending, as does one changed meanwhile", async () => {
    let stored: ((path: string) => void) | undefined;
    const agent = terminal("t1", "claude", { upload: () => new Promise<string>((resolve) => { stored = resolve; }) });
    const server = new PaneServer();
    const first = setup({ text: PLAN, server, origin: agent.target });
    const second = setup({ text: PLAN, server });
    comment(first.shown, "retry three times", "Why three?");
    comment(first.shown, "Step 3 of the plan.", "Early step.");
    await numbered(first.store);
    handoverMenu(first.shown, first.showMenu).find((item) => item.label === "Send as File To")!.children![0].action();
    await vi.waitFor(() => expect(stored).toBeDefined());

    // While the review is on its way, the other window adds one, and this window edits #2.
    server.broadcast();
    comment(second.shown, "Step 12 of the plan.", "From the other window.");
    await numbered(second.store);
    server.broadcast();
    click(entry(first.shown, 2), '[title="Edit this annotation"]');
    write(first.shown, "Comment", "Earlier step.");
    pressKey(first.shown, { key: "Enter", ctrlKey: true });

    stored!("/home/me/.cache/splitty/uploads/review-PLAN.md");
    await vi.waitFor(() => expect(server.sent("mark_sent")).toHaveLength(1));
    expect(server.sent("mark_sent")[0].entries.map((entry) => entry.id)).toEqual([server.get(1).id]);
    expect(server.get(1).sent).toBe(true);
    expect(server.get(2)).not.toHaveProperty("sent");
    expect(server.get(3)).not.toHaveProperty("sent");
    expect(query(first.shown, ".viewer-annotations-primary")!.textContent).toBe("Send to claude");
  });
});

describe("waiting for a review", () => {
  function banner(view: ViewerView): HTMLElement {
    return query(view, ".viewer-review-wait")!;
  }
  const approveButton = (view: ViewerView) => banner(view).querySelector<HTMLButtonElement>(".viewer-review-wait-approve")!;
  const changesButton = (view: ViewerView) => banner(view).querySelector<HTMLButtonElement>(".viewer-review-wait-changes")!;
  const overallField = (view: ViewerView) => banner(view).querySelector<HTMLTextAreaElement>(".viewer-review-wait-comment")!;
  function writeOverall(view: ViewerView, text: string): void {
    const field = overallField(view);
    field.value = text;
    field.dispatchEvent(new Event("input", { bubbles: true }));
  }

  // viewer-annotations, Waiting For A Review: Feedback on a plan.
  it("Feedback on a plan: requests changes with the review, and marks it once the server took it", async () => {
    let take: ((accepted: boolean) => void) | undefined;
    const answer = vi.fn((_request: ReviewAnswerRequest) => new Promise<boolean>((resolve) => { take = resolve; }));
    const { shown, store, server, notices } = setup({ text: PLAN, record: WAITING, answer });
    const shownBanner = banner(shown);
    expect(shown.element.firstElementChild).toBe(shownBanner);
    expect(shownBanner.hidden).toBe(false);
    expect(shownBanner.dataset.waitId).toBe(WAIT.id);
    expect(shownBanner.querySelector(".viewer-review-wait-text")!.textContent).toBe("claude waits for a review");
    expect(changesButton(shown).textContent).toBe("Request Changes");
    expect(approveButton(shown).textContent).toBe("Approve");
    expect(changesButton(shown).disabled).toBe(false);
    expect(shownBanner.querySelector<HTMLElement>(".viewer-review-wait-note")!.hidden).toBe(true);

    comment(shown, "retry three times", "Why three?");
    comment(shown, "Step 3 of the plan.", "Early step.");
    changesButton(shown).click();
    await vi.waitFor(() => expect(answer).toHaveBeenCalledTimes(1));
    const [request] = answer.mock.calls[0];
    expect(request).toMatchObject({ screenId: "screen-1", paneId: "v1", waitId: WAIT.id, outcome: "changes_requested" });
    expect(request.review).toMatch(/^# Review of PLAN\.md\n\nDocument: \/home\/me\/app\/PLAN\.md\n\nVerdict: changes requested\n\n/);
    expect(request.review).toContain("## #2 Line 3, comment");
    expect(request.review).toContain("## #1 Line 30, comment");
    // Nothing is marked before the server took the answer; meanwhile the banner waits.
    expect(server.sent("mark_sent")).toEqual([]);
    expect(changesButton(shown).disabled).toBe(true);
    expect(approveButton(shown).disabled).toBe(true);
    expect(overallField(shown).disabled).toBe(true);
    expect(shownBanner.getAttribute("aria-busy")).toBe("true");

    take!(true);
    await vi.waitFor(() => expect(server.sent("mark_sent")).toHaveLength(1));
    expect(server.sent("mark_sent")[0].entries.map((entry) => entry.id).sort()).toEqual(store.list("v1").map((annotation) => annotation.id).sort());
    expect(store.count("v1")).toBe(2);
    expect(shownBanner.hidden).toBe(true);
    expect(notices.sent).toHaveBeenCalledWith("Changes requested from claude");
  });

  // viewer-annotations, Waiting For A Review: A plain rejection.
  it("A plain rejection: requests changes with a review that holds no annotation", async () => {
    const answer = vi.fn(async (_request: ReviewAnswerRequest) => true);
    const { shown, server } = setup({ text: PLAN, record: WAITING, answer });
    changesButton(shown).click();
    await vi.waitFor(() => expect(banner(shown).hidden).toBe(true));
    expect(answer.mock.calls[0][0]).toEqual({
      screenId: "screen-1", paneId: "v1", waitId: WAIT.id, outcome: "changes_requested",
      review: "# Review of PLAN.md\n\nDocument: /home/me/app/PLAN.md\n\nVerdict: changes requested\n",
    });
    expect(server.sent("mark_sent")).toEqual([]);
  });

  // viewer-annotations, Waiting For A Review: An overall comment.
  it("An overall comment: becomes a numbered general comment before the answer, and is handed over with it", async () => {
    const answer = vi.fn(async (_request: ReviewAnswerRequest) => true);
    const { shown, store, server } = setup({ text: PLAN, record: WAITING, answer });
    comment(shown, "retry three times", "Why three?");
    writeOverall(shown, "Split the migration into its own step.");
    changesButton(shown).click();
    await vi.waitFor(() => expect(answer).toHaveBeenCalledTimes(1));
    const general = store.list("v1").find((annotation) => !annotation.target)!;
    expect(general).toMatchObject({ kind: "comment", body: "Split the migration into its own step.", number: 2 });
    const review = answer.mock.calls[0][0].review!;
    expect(review).toContain("Verdict: changes requested\n\n## General comments\n\n- #2 Split the migration into its own step.\n\n## #1 Line 30, comment");
    await vi.waitFor(() => expect(server.sent("mark_sent")).toHaveLength(1));
    expect(server.sent("mark_sent")[0].entries.map((entry) => entry.id)).toContain(general.id);
    expect(overallField(shown).value).toBe("");
  });

  it("keeps an overall comment that no answer took, for the next wait and a rebuilt pane", () => {
    const store = setup({ text: PLAN, record: WAITING, answer: async () => true });
    writeOverall(store.shown, "Not yet.");
    store.shown.setRecord(PLAN_RECORD);
    expect(banner(store.shown).hidden).toBe(true);
    store.shown.setRecord({ ...PLAN_RECORD, reviewWait: { ...WAIT, id: "wait-00000000000000ee" } });
    expect(overallField(store.shown).value).toBe("Not yet.");
    const rebuilt = setup({ text: PLAN, record: WAITING, answer: async () => true, server: store.server, store: store.store });
    expect(overallField(rebuilt.shown).value).toBe("Not yet.");
    expect(store.store.count("v1")).toBe(0);
    // An emptied field is forgotten.
    writeOverall(rebuilt.shown, "  ");
    const later = setup({ text: PLAN, record: WAITING, answer: async () => true, server: store.server, store: store.store });
    expect(overallField(later.shown).value).toBe("");
  });

  // viewer-annotations, Waiting For A Review: Approved.
  it("Approved: answers without a review and marks nothing while nothing is pending", async () => {
    const answer = vi.fn(async (_request: ReviewAnswerRequest) => true);
    const { shown, server, notices } = setup({ text: PLAN, record: WAITING, answer });
    approveButton(shown).click();
    await vi.waitFor(() => expect(banner(shown).hidden).toBe(true));
    expect(answer.mock.calls[0][0]).toEqual({ screenId: "screen-1", paneId: "v1", waitId: WAIT.id, outcome: "approved" });
    expect(server.sent("mark_sent")).toEqual([]);
    expect(notices.info).toHaveBeenCalledWith("Approved for claude");
  });

  // viewer-annotations, Waiting For A Review: Approved with notes.
  it("Approved with notes: carries the pending comments, and marks them once the server took the answer", async () => {
    const answer = vi.fn(async (_request: ReviewAnswerRequest) => true);
    const { shown, store, server, notices } = setup({ text: PLAN, record: WAITING, answer });
    comment(shown, "retry three times", "Why three?");
    approveButton(shown).click();
    await vi.waitFor(() => expect(server.sent("mark_sent")).toHaveLength(1));
    const [request] = answer.mock.calls[0];
    expect(request.outcome).toBe("approved");
    expect(request.review).toMatch(/\n\nVerdict: approved\n\n## #1 Line 30, comment\n/);
    expect(server.sent("mark_sent")[0].entries.map((entry) => entry.id)).toEqual([store.list("v1")[0].id]);
    expect(notices.sent).toHaveBeenCalledWith("Approved for claude, with the new comments as notes");
  });

  it("repeats no handed-over annotation in an approval", async () => {
    const answer = vi.fn(async (_request: ReviewAnswerRequest) => true);
    const { shown, store, server } = setup({ text: PLAN, record: WAITING, answer });
    comment(shown, "retry three times", "Why three?");
    await numbered(store);
    store.markSent("screen-1", "v1", [{ id: store.list("v1")[0].id, kind: "comment", body: "Why three?", target: store.list("v1")[0].target }]);
    await vi.waitFor(() => expect(store.list("v1")[0].sent).toBe(true));
    approveButton(shown).click();
    await vi.waitFor(() => expect(answer).toHaveBeenCalledTimes(1));
    expect(answer.mock.calls[0][0]).not.toHaveProperty("review");
    expect(server.sent("mark_sent")).toHaveLength(1);
  });

  // viewer-annotations, Waiting For A Review: The file changed during the review.
  it("The file changed during the review: says so, and notes it in the review", async () => {
    const answer = vi.fn(async (_request: ReviewAnswerRequest) => true);
    const { shown } = setup({ text: PLAN, record: WAITING, answer });
    const note = () => banner(shown).querySelector<HTMLElement>(".viewer-review-wait-note")!;
    // The read that was on its way when the wait appeared, and the first one after it, change nothing.
    shown.update(read(`${PLAN}Step 41 of the plan.\n`, "v2"));
    shown.update({ ...read(PLAN, "v2"), reading: true });
    shown.update(read(`${PLAN}Step 41 of the plan.\n`, "v2"));
    expect(note().hidden).toBe(true);
    // A later read with another version is a change.
    shown.update({ ...read(PLAN, "v2"), reading: true });
    shown.update(read(`${PLAN}Step 42 of the plan.\n`, "v3"));
    expect(note().hidden).toBe(false);
    expect(note().textContent).toBe("The document changed since the review began");
    comment(shown, "retry three times", "Why three?");
    changesButton(shown).click();
    await vi.waitFor(() => expect(answer).toHaveBeenCalledTimes(1));
    expect(answer.mock.calls[0][0].review).toContain(
      "Verdict: changes requested\n\nNote: the document changed during the review; lines and quotes refer to the version shown when the review was answered.\n\n## #1",
    );
  });

  it("notes no change for a text document, nor for a new wait", () => {
    const text: ViewerRecord = { source: { kind: "text", title: "Plan" }, revision: 1, originPaneId: "t1", reviewWait: WAIT };
    const { shown } = setup({ text: PLAN, record: text, answer: async () => true });
    shown.update({ ...read(PLAN), reading: true });
    shown.update(read(PLAN, "v1"));
    shown.update({ ...read(PLAN), reading: true });
    shown.update(read(`${PLAN}More.\n`, "v2"));
    expect(banner(shown).querySelector<HTMLElement>(".viewer-review-wait-note")!.hidden).toBe(true);

    const file = setup({ text: PLAN, record: WAITING, answer: async () => true });
    file.shown.update({ ...read(PLAN), reading: true });
    file.shown.update(read(PLAN, "v1"));
    file.shown.update({ ...read(PLAN), reading: true });
    file.shown.update(read(`${PLAN}More.\n`, "v2"));
    expect(banner(file.shown).querySelector<HTMLElement>(".viewer-review-wait-note")!.hidden).toBe(false);
    file.shown.setRecord({ ...PLAN_RECORD, reviewWait: { ...WAIT, id: "wait-00000000000000dd" } });
    expect(banner(file.shown).querySelector<HTMLElement>(".viewer-review-wait-note")!.hidden).toBe(true);
  });

  // viewer-annotations, Waiting For A Review: Two windows answer.
  it("Two windows answer: the window whose answer came second marks nothing", async () => {
    const server = new PaneServer();
    let settled = false;
    const answer = vi.fn(async (_request: ReviewAnswerRequest) => {
      const first = !settled;
      settled = true;
      return first;
    });
    const first = setup({ text: PLAN, server, record: WAITING, answer });
    const second = setup({ text: PLAN, server, record: WAITING, answer });
    comment(first.shown, "retry three times", "Why three?");
    await numbered(first.store);
    server.broadcast();

    approveButton(second.shown).click();
    await vi.waitFor(() => expect(answer).toHaveBeenCalledTimes(1));
    changesButton(first.shown).click();
    await vi.waitFor(() => expect(first.notices.info).toHaveBeenCalledWith("The request for changes was not taken: the command no longer waits for it"));
    expect(answer).toHaveBeenCalledTimes(2);
    // Only the approval that settled the wait, with the comment as its note, marks it.
    expect(answer.mock.calls[0][0]).toMatchObject({ outcome: "approved" });
    expect(answer.mock.calls[0][0].review).toContain("## #1 Line 30, comment");
    expect(server.sent("mark_sent")).toHaveLength(1);
    expect(banner(first.shown).hidden).toBe(true);
  });

  // viewer-annotations, Waiting For A Review: The agent stops waiting.
  it("The agent stops waiting: offers no answer any more, and the annotations stay", () => {
    const { shown, store } = setup({ text: PLAN, record: WAITING, answer: async () => true });
    comment(shown, "retry three times", "Why three?");
    shown.setRecord(PLAN_RECORD);
    expect(banner(shown).hidden).toBe(true);
    expect(store.count("v1")).toBe(1);
    // A new wait is offered again.
    shown.setRecord({ ...PLAN_RECORD, reviewWait: { ...WAIT, id: "wait-00000000000000ff", title: "" } });
    expect(banner(shown).hidden).toBe(false);
    expect(banner(shown).querySelector(".viewer-review-wait-text")!.textContent).toBe("A command waits for a review");
  });

  it("says when the answer did not arrive, and offers it again", async () => {
    const answer = vi.fn(async (_request: ReviewAnswerRequest): Promise<boolean> => {
      throw new Error("The connection to the server ended before it took the answer.");
    });
    const { shown, server, notices } = setup({ text: PLAN, record: WAITING, answer });
    comment(shown, "retry three times", "Why three?");
    changesButton(shown).click();
    await vi.waitFor(() => expect(notices.error).toHaveBeenCalledWith(
      "The answer did not reach claude", "The connection to the server ended before it took the answer.",
    ));
    expect(server.sent("mark_sent")).toEqual([]);
    expect(banner(shown).hidden).toBe(false);
    expect(changesButton(shown).disabled).toBe(false);
  });

  it("shows no banner where the server keeps no annotations", () => {
    const { shown } = setup({ text: PLAN, record: WAITING, answer: async () => true, available: false });
    expect(banner(shown).hidden).toBe(true);
  });
});
