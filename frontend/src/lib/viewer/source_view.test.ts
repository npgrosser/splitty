// @vitest-environment jsdom
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SourceView, leavesToWindow } from "./source_view.ts";

// The settings the shortcuts read query media features when they load;
// jsdom has no matchMedia, and lays out no ranges, which the editor measures.
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

function key(init: KeyboardEventInit): KeyboardEvent {
  return new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
}

let views: SourceView[] = [];

function mount(text: string, options: { fileName?: string; wrap?: boolean } = {}): SourceView {
  const parent = document.createElement("div");
  document.body.append(parent);
  const view = new SourceView(parent, { text, fileName: options.fileName, wrap: options.wrap ?? false });
  views.push(view);
  return view;
}

function editorOf(view: SourceView): EditorView {
  return EditorView.findFromDOM(view.dom)!;
}

afterEach(() => {
  for (const view of views) view.destroy();
  views = [];
  document.body.replaceChildren();
});

// viewer-panes, Showing A Document: the source view leaves Splitty's keys to
// the window, so pane shortcuts work from a viewer as from a terminal.
describe("leavesToWindow", () => {
  it("passes Splitty's keys by and keeps the search key", () => {
    const splitty = (event: KeyboardEvent) => (event.altKey && ["d", "w"].includes(event.key)) || (event.ctrlKey && event.key === "f");
    expect(leavesToWindow(key({ key: "d", altKey: true }), splitty)).toBe(true);
    expect(leavesToWindow(key({ key: "w", altKey: true }), splitty)).toBe(true);
    expect(leavesToWindow(key({ key: "f", ctrlKey: true }), splitty)).toBe(false);
    expect(leavesToWindow(key({ key: "ArrowDown" }), splitty)).toBe(false);
  });

  it("asks Splitty's own shortcuts by default", () => {
    expect(leavesToWindow(key({ key: "w", altKey: true }))).toBe(true);
    expect(leavesToWindow(key({ key: "f", ctrlKey: true }))).toBe(false);
    expect(leavesToWindow(key({ key: "a", ctrlKey: true }))).toBe(false);
  });
});

describe("SourceView", () => {
  it("shows the text read-only with line numbers", () => {
    const view = mount("fn main() {\n    println!(\"hi\");\n}\n", { fileName: "main.rs" });
    const editor = editorOf(view);
    expect(editor.state.readOnly).toBe(true);
    expect(view.dom.querySelector(".cm-lineNumbers")).not.toBeNull();
    expect(editor.state.doc.lines).toBe(4);
  });

  it("names the selection's text, offsets and lines", () => {
    const view = mount("one\ntwo\nthree\n");
    expect(view.selection()).toBeUndefined();
    editorOf(view).dispatch({ selection: { anchor: 9, head: 2 } });
    expect(view.selection()).toEqual({ text: "e\ntwo\nt", from: 2, to: 9, startLine: 1, endLine: 3 });
    // A selection that ends at a line's start does not take that line.
    editorOf(view).dispatch({ selection: { anchor: 4, head: 8 } });
    expect(view.selection()).toEqual({ text: "two\n", from: 4, to: 8, startLine: 2, endLine: 2 });
  });

  it("replaces its text in place and clears its marks", () => {
    const view = mount("old\n");
    view.setMarks([{ from: 0, to: 3, startLine: 1, endLine: 1 }]);
    expect(view.dom.querySelectorAll(".cm-viewer-marked")).toHaveLength(1);
    expect(view.dom.querySelectorAll(".cm-viewer-mark")).toHaveLength(1);
    view.setText("new\ntext\n");
    expect(editorOf(view).state.doc.toString()).toBe("new\ntext\n");
    expect(view.dom.querySelectorAll(".cm-viewer-marked")).toHaveLength(0);
  });

  // viewer-annotations, Annotations In The Pane: a passage of the source view
  // is marked, and every line of an annotation has a bar beside it.
  it("marks annotated passages and puts one bar beside each of their lines", () => {
    const view = mount("one\ntwo\nthree\nfour\n");
    view.setMarks([
      { from: 4, to: 13, startLine: 2, endLine: 3 },
      { from: 4, to: 7, startLine: 2, endLine: 2, active: true },
      // A passage of the rendered view: its lines only.
      { startLine: 3, endLine: 4 },
      // Lines past the text end at its last line.
      { startLine: 4, endLine: 90 },
    ]);
    const bars = [...view.dom.querySelectorAll(".cm-viewer-marks .cm-gutterElement")]
      .filter((element) => element.querySelector(".cm-viewer-mark"));
    expect(bars).toHaveLength(4);
    expect(view.dom.querySelector(".cm-viewer-marked-active")!.textContent).toBe("two");
    const marked = [...view.dom.querySelectorAll(".cm-viewer-marked")].map((element) => element.textContent).join("|");
    expect(marked).toContain("three");
    expect(marked).not.toContain("four");

    view.setMarks([]);
    expect(view.dom.querySelectorAll(".cm-viewer-mark")).toHaveLength(0);
    expect(view.dom.querySelectorAll(".cm-viewer-marked")).toHaveLength(0);
  });

  // viewer-annotations (D7 of review-threads): a changed passage and a resolved
  // one in their own style; a shared line's bar shows the open one first.
  it("marks a changed passage and a resolved one in their own styles", () => {
    const view = mount("one\ntwo\nthree\nfour\n");
    view.setMarks([
      { from: 0, to: 3, startLine: 1, endLine: 1, style: "changed" },
      { from: 4, to: 7, startLine: 2, endLine: 2, style: "resolved" },
      { from: 8, to: 13, startLine: 3, endLine: 3, style: "resolved" },
      { startLine: 3, endLine: 3, style: "changed" },
      { startLine: 4, endLine: 4, style: "resolved" },
      { startLine: 4, endLine: 4 },
    ]);
    expect(view.dom.querySelector(".cm-viewer-marked-changed")!.textContent).toBe("one");
    expect([...view.dom.querySelectorAll(".cm-viewer-marked-resolved")].map((element) => element.textContent)).toEqual(["two", "three"]);
    const bars = [...view.dom.querySelectorAll(".cm-viewer-marks .cm-gutterElement")]
      .map((element) => element.querySelector(".cm-viewer-mark"))
      .filter((bar): bar is Element => bar !== null)
      .map((bar) => bar.className);
    expect(bars).toEqual([
      "cm-viewer-mark cm-viewer-mark-changed",
      "cm-viewer-mark cm-viewer-mark-resolved",
      "cm-viewer-mark cm-viewer-mark-changed",
      "cm-viewer-mark",
    ]);
  });

  // viewer-annotations, Annotations In The Pane (D4 of viewer-clickable-marks):
  // the marks under the pointer are the decorations around its target, and on
  // the bars, every mark of the line beside them.
  it("names the marks under the pointer, in the passages and on the bars", () => {
    const view = mount("one two three\nfour\nfive\n");
    view.setMarks([
      { id: "outer", from: 0, to: 13, startLine: 1, endLine: 1 },
      { id: "inner", from: 4, to: 7, startLine: 1, endLine: 1, hover: true },
      { id: "lines", startLine: 2, endLine: 3 },
    ]);
    const innermost = (text: string) => [...view.dom.querySelectorAll(".cm-viewer-marked")]
      .filter((element) => element.textContent === text && !element.querySelector(".cm-viewer-marked"))[0];
    expect(view.marksAt(innermost("two"), 0).sort()).toEqual(["inner", "outer"]);
    expect(view.dom.querySelector(".cm-viewer-marked-hover")!.textContent).toBe("two");
    const outerText = [...view.dom.querySelectorAll(".cm-viewer-marked")].find((element) => element.getAttribute("data-annotation-id") === "outer")!;
    expect(view.marksAt(outerText.firstChild, 0)).toEqual(["outer"]);
    // Text without a mark, and an element outside the view.
    const line = [...view.dom.querySelectorAll(".cm-line")][1];
    expect(view.marksAt(line, 0)).toEqual([]);
    expect(view.marksAt(document.body, 0)).toEqual([]);

    const editor = editorOf(view);
    const gutter = view.dom.querySelector(".cm-viewer-marks")!;
    const heightOf = (number: number) => editor.lineBlockAt(editor.state.doc.line(number).from).top + editor.documentTop + 1;
    expect(view.marksAt(gutter, heightOf(1)).sort()).toEqual(["inner", "outer"]);
    expect(view.marksAt(gutter, heightOf(3))).toEqual(["lines"]);
    expect(view.marksAt(gutter, heightOf(4))).toEqual([]);
    // The line numbers are no bars.
    expect(view.marksAt(view.dom.querySelector(".cm-lineNumbers")!, heightOf(1))).toEqual([]);

    view.setText("other\n");
    expect(view.marksAt(view.dom.querySelector(".cm-viewer-marks")!, heightOf(1))).toEqual([]);
  });

  it("tells when the selection changes", () => {
    const parent = document.createElement("div");
    document.body.append(parent);
    const changed = vi.fn();
    const view = new SourceView(parent, { text: "alpha beta\n", wrap: false, onSelectionChange: changed });
    views.push(view);
    editorOf(view).dispatch({ selection: { anchor: 0, head: 5 } });
    expect(changed).toHaveBeenCalledTimes(1);
    expect(view.selection()?.text).toBe("alpha");
  });

  it("wraps lines on request", () => {
    const view = mount("a long line\n", { wrap: true });
    expect(view.wrap).toBe(true);
    expect(view.dom.querySelector(".cm-lineWrapping")).not.toBeNull();
    view.setWrap(false);
    expect(view.wrap).toBe(false);
    expect(view.dom.querySelector(".cm-lineWrapping")).toBeNull();
  });

  it("does not take Splitty's keys from the window", () => {
    const view = mount("text\n");
    const content = view.dom.querySelector(".cm-content")!;
    const split = key({ key: "d", altKey: true });
    let reached = false;
    window.addEventListener("keydown", () => { reached = true; }, { once: true });
    content.dispatchEvent(split);
    expect(reached).toBe(true);
  });
});
