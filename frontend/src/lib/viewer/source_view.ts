/**
 * The source view of a viewer pane (`viewer-panes`, Showing A Document): a
 * read-only CodeMirror 6 editor with line numbers, search (Mod+F), optional
 * line wrapping and highlighting by the file's name.
 *
 * Its keymap holds navigation, selection and search only. Every key Splitty
 * uses itself (`isSplittyShortcut`) passes CodeMirror by, so splitting,
 * closing and moving between panes work from a viewer as from a terminal;
 * Mod+F is the exception and searches the document.
 *
 * Positions are UTF-16 offsets into the text and lines are 1-based, as in
 * `anchoring.ts`. Annotations show as marks (`viewer-annotations`, D5): a
 * decoration over the passage of the source view's own annotations, and a
 * bar in the gutter beside the lines of every annotated passage, those of
 * the rendered view included.
 */

import {
  cursorCharLeft, cursorCharRight, cursorDocEnd, cursorDocStart, cursorGroupLeft, cursorGroupRight,
  cursorLineBoundaryBackward, cursorLineBoundaryForward, cursorLineDown, cursorLineUp, cursorPageDown, cursorPageUp,
  selectAll, selectCharLeft, selectCharRight, selectDocEnd, selectDocStart, selectGroupLeft, selectGroupRight,
  selectLineBoundaryBackward, selectLineBoundaryForward, selectLineDown, selectLineUp, selectPageDown, selectPageUp,
} from "@codemirror/commands";
import { syntaxHighlighting } from "@codemirror/language";
import { closeSearchPanel, findNext, findPrevious, openSearchPanel, search } from "@codemirror/search";
import { Compartment, EditorSelection, EditorState, Prec, RangeSet, StateEffect, StateField, type Extension } from "@codemirror/state";
import {
  Decoration, EditorView, GutterMarker, drawSelection, gutter, highlightActiveLineGutter, keymap, lineNumbers,
  type DecorationSet, type KeyBinding,
} from "@codemirror/view";
import { isSplittyShortcut } from "../terminal/terminal_shared.ts";
import { highlighter, languageForFile, loadLanguage } from "./syntax.ts";

export interface SourceSelection {
  readonly text: string;
  /** UTF-16 offsets into the text, `to` exclusive. */
  readonly from: number;
  readonly to: number;
  readonly startLine: number;
  readonly endLine: number;
}

/** An annotated passage: its text, where it has one in this view, and its lines. */
export interface SourceMark {
  /** UTF-16 offsets of the passage, `to` exclusive; absent for a passage of the other view. */
  readonly from?: number;
  readonly to?: number;
  readonly startLine: number;
  readonly endLine: number;
  /** The passage the user chose. */
  readonly active?: boolean;
}

/** The most lines one mark puts a bar beside. */
const MAX_MARKED_LINES = 10_000;

const isMac = typeof navigator !== "undefined" && /mac/i.test(navigator.platform);

/** The search keys, which the source view keeps although Splitty knows Mod+F. */
function isSearchKey(event: KeyboardEvent): boolean {
  const mod = isMac ? event.metaKey : event.ctrlKey;
  return mod && !event.altKey && event.key.toLowerCase() === "f";
}

/** Whether a key goes past the editor to the window's shortcuts. */
export function leavesToWindow(event: KeyboardEvent, splittyShortcut: (event: KeyboardEvent) => boolean = isSplittyShortcut): boolean {
  return splittyShortcut(event) && !isSearchKey(event);
}

const VIEWER_KEYS: readonly KeyBinding[] = [
  { key: "ArrowLeft", run: cursorCharLeft, shift: selectCharLeft },
  { key: "ArrowRight", run: cursorCharRight, shift: selectCharRight },
  { key: "ArrowUp", run: cursorLineUp, shift: selectLineUp },
  { key: "ArrowDown", run: cursorLineDown, shift: selectLineDown },
  { key: "Mod-ArrowLeft", mac: "Alt-ArrowLeft", run: cursorGroupLeft, shift: selectGroupLeft },
  { key: "Mod-ArrowRight", mac: "Alt-ArrowRight", run: cursorGroupRight, shift: selectGroupRight },
  { mac: "Cmd-ArrowLeft", run: cursorLineBoundaryBackward, shift: selectLineBoundaryBackward },
  { mac: "Cmd-ArrowRight", run: cursorLineBoundaryForward, shift: selectLineBoundaryForward },
  { mac: "Cmd-ArrowUp", run: cursorDocStart, shift: selectDocStart },
  { mac: "Cmd-ArrowDown", run: cursorDocEnd, shift: selectDocEnd },
  { key: "Home", run: cursorLineBoundaryBackward, shift: selectLineBoundaryBackward },
  { key: "End", run: cursorLineBoundaryForward, shift: selectLineBoundaryForward },
  { key: "PageUp", run: cursorPageUp, shift: selectPageUp },
  { key: "PageDown", run: cursorPageDown, shift: selectPageDown },
  { key: "Mod-Home", run: cursorDocStart, shift: selectDocStart },
  { key: "Mod-End", run: cursorDocEnd, shift: selectDocEnd },
  { key: "Mod-a", run: selectAll },
  { key: "Mod-f", run: openSearchPanel, scope: "editor search-panel", preventDefault: true },
  { key: "F3", run: findNext, shift: findPrevious, scope: "editor search-panel", preventDefault: true },
  { key: "Mod-g", run: findNext, shift: findPrevious, scope: "editor search-panel", preventDefault: true },
  { key: "Escape", run: closeSearchPanel, scope: "editor search-panel" },
];

const MONO = `"SplittyMono", "JetBrains Mono", ui-monospace, SFMono-Regular, Menlo, Consolas, monospace`;

/** Colors from Splitty's tokens, which the pane sets on its element. */
const theme = EditorView.theme({
  "&": { height: "100%", color: "var(--splitty-text-primary)", backgroundColor: "var(--splitty-surface-base)", fontSize: "13px" },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": { fontFamily: MONO, lineHeight: "1.55", overflow: "auto" },
  ".cm-content": { caretColor: "var(--splitty-text-primary)", padding: "6px 0" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--splitty-text-primary)" },
  ".cm-gutters": { backgroundColor: "var(--splitty-surface-base)", color: "var(--splitty-text-muted)", border: "none" },
  ".cm-lineNumbers .cm-gutterElement": { padding: "0 10px 0 14px" },
  ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--splitty-text-secondary)" },
  ".cm-selectionBackground, &.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground": {
    backgroundColor: "var(--sv-selection, rgba(99, 160, 255, 0.28))",
  },
  ".cm-searchMatch": { backgroundColor: "var(--splitty-accent-muted)", outline: "1px solid var(--splitty-accent-muted)" },
  ".cm-searchMatch.cm-searchMatch-selected": { backgroundColor: "var(--splitty-accent)", color: "var(--splitty-surface-base)" },
  ".cm-panels": { backgroundColor: "var(--splitty-surface-raised)", color: "var(--splitty-text-primary)" },
  ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--splitty-border-default)" },
  ".cm-panel.cm-search": { padding: "6px 8px", fontFamily: "inherit", fontSize: "12px" },
  ".cm-panel.cm-search input, .cm-panel.cm-search button": {
    font: "inherit", color: "var(--splitty-text-primary)", backgroundColor: "var(--splitty-surface-overlay)",
    border: "1px solid var(--splitty-border-default)", borderRadius: "4px", padding: "2px 6px", margin: "0 4px 0 0",
  },
  ".cm-panel.cm-search input[type=checkbox]": { padding: "0", margin: "0 3px 0 6px" },
  ".cm-panel.cm-search label": { color: "var(--splitty-text-secondary)", marginRight: "6px" },
  ".cm-panel.cm-search [name=close]": { color: "var(--splitty-text-secondary)", backgroundColor: "transparent", border: "none" },
  ".cm-viewer-marks .cm-gutterElement": { padding: "0 2px" },
  ".cm-viewer-mark": { width: "3px", height: "100%", borderRadius: "1px", backgroundColor: "var(--splitty-accent)" },
  ".cm-viewer-marked": { backgroundColor: "var(--splitty-annotation-bg, var(--splitty-accent-muted))" },
  ".cm-viewer-marked-active": { backgroundColor: "var(--splitty-annotation-active-bg, var(--splitty-accent-muted))" },
  ".sv-keyword": { color: "var(--sv-keyword)" },
  ".sv-string": { color: "var(--sv-string)" },
  ".sv-number": { color: "var(--sv-number)" },
  ".sv-comment": { color: "var(--sv-comment)", fontStyle: "italic" },
  ".sv-function": { color: "var(--sv-function)" },
  ".sv-type": { color: "var(--sv-type)" },
  ".sv-property": { color: "var(--sv-property)" },
  ".sv-operator": { color: "var(--sv-operator)" },
  ".sv-meta": { color: "var(--sv-comment)" },
  ".sv-heading": { color: "var(--sv-function)", fontWeight: "600" },
  ".sv-link": { color: "var(--sv-string)", textDecoration: "underline" },
  ".sv-emphasis": { fontStyle: "italic" },
  ".sv-strong": { fontWeight: "600" },
  ".sv-strike": { textDecoration: "line-through" },
  ".sv-inserted": { color: "var(--sv-string)" },
  ".sv-deleted": { color: "var(--sv-deleted)" },
});

class MarkMarker extends GutterMarker {
  override toDOM(view: EditorView): Node {
    const dot = view.dom.ownerDocument.createElement("div");
    dot.className = "cm-viewer-mark";
    return dot;
  }
}

const markMarker = new MarkMarker();

interface Marks {
  readonly decorations: DecorationSet;
  readonly markers: RangeSet<GutterMarker>;
}

const setMarks = StateEffect.define<readonly SourceMark[]>();

const markedPassage = Decoration.mark({ class: "cm-viewer-marked" });
const activePassage = Decoration.mark({ class: "cm-viewer-marked cm-viewer-marked-active" });

function buildMarks(marks: readonly SourceMark[], state: EditorState): Marks {
  const doc = state.doc;
  const decorations = [];
  const lines = new Set<number>();
  for (const mark of marks) {
    if (mark.from !== undefined && mark.to !== undefined) {
      const from = Math.max(0, Math.min(doc.length, mark.from));
      const to = Math.max(from, Math.min(doc.length, mark.to));
      if (to > from) decorations.push((mark.active ? activePassage : markedPassage).range(from, to));
    }
    const first = Math.max(1, Math.min(doc.lines, mark.startLine));
    const last = Math.min(doc.lines, Math.max(first, mark.endLine), first + MAX_MARKED_LINES - 1);
    for (let line = first; line <= last; line++) lines.add(line);
  }
  const markers = [...lines].sort((a, b) => a - b).map((line) => markMarker.range(doc.line(line).from));
  return { decorations: Decoration.set(decorations, true), markers: RangeSet.of(markers, true) };
}

const marksField = StateField.define<Marks>({
  create: () => ({ decorations: Decoration.none, markers: RangeSet.empty }),
  update(value, tr) {
    let next: Marks = tr.docChanged
      ? { decorations: value.decorations.map(tr.changes), markers: value.markers.map(tr.changes) }
      : value;
    for (const effect of tr.effects) if (effect.is(setMarks)) next = buildMarks(effect.value, tr.state);
    return next;
  },
  provide: (field) => EditorView.decorations.from(field, (value) => value.decorations),
});

export interface SourceViewOptions {
  readonly text: string;
  /** Highlighting by this name; none for plain text. */
  readonly fileName?: string;
  readonly wrap: boolean;
  /** Called when the selection changes. */
  onSelectionChange?(): void;
}

export class SourceView {
  private readonly view: EditorView;
  private readonly language = new Compartment();
  private readonly wrapping = new Compartment();
  private wrapped: boolean;
  private languageName: string | undefined;
  private destroyed = false;

  constructor(parent: HTMLElement, options: SourceViewOptions) {
    this.wrapped = options.wrap;
    const extensions: Extension[] = [
      EditorState.readOnly.of(true),
      // Splitty's keys first, before any of CodeMirror's handlers sees them.
      Prec.highest(EditorView.domEventHandlers({ keydown: (event) => leavesToWindow(event) })),
      keymap.of(VIEWER_KEYS),
      lineNumbers(),
      highlightActiveLineGutter(),
      gutter({ class: "cm-viewer-marks", markers: (view) => view.state.field(marksField).markers }),
      drawSelection(),
      search({ top: true }),
      syntaxHighlighting(highlighter),
      this.language.of([]),
      this.wrapping.of(options.wrap ? EditorView.lineWrapping : []),
      marksField,
      theme,
      EditorView.updateListener.of((update) => {
        if (update.selectionSet) options.onSelectionChange?.();
      }),
    ];
    this.view = new EditorView({
      parent,
      root: parent.ownerDocument,
      state: EditorState.create({ doc: options.text, extensions }),
    });
    this.setLanguage(options.fileName);
  }

  get dom(): HTMLElement {
    return this.view.dom;
  }

  /** Replace the text; the first visible line stays in view as far as the new text has it. */
  setText(text: string, fileName?: string): void {
    const line = this.firstVisibleLine();
    this.view.dispatch({
      changes: { from: 0, to: this.view.state.doc.length, insert: text },
      effects: setMarks.of([]),
    });
    this.setLanguage(fileName);
    this.scrollToLine(line);
  }

  get wrap(): boolean {
    return this.wrapped;
  }

  setWrap(wrap: boolean): void {
    if (wrap === this.wrapped) return;
    this.wrapped = wrap;
    const line = this.firstVisibleLine();
    this.view.dispatch({ effects: this.wrapping.reconfigure(wrap ? EditorView.lineWrapping : []) });
    this.scrollToLine(line);
  }

  selection(): SourceSelection | undefined {
    const { from, to } = this.view.state.selection.main;
    if (from === to) return undefined;
    const doc = this.view.state.doc;
    return {
      text: doc.sliceString(from, to),
      from,
      to,
      startLine: doc.lineAt(from).number,
      endLine: doc.lineAt(Math.max(from, to - 1)).number,
    };
  }

  /** Mark annotated passages and their lines; replaces earlier marks, and a later `setText` clears them. */
  setMarks(marks: readonly SourceMark[]): void {
    this.view.dispatch({ effects: setMarks.of(marks) });
  }

  /** Where the selection's head is on the screen, or undefined without a selection or layout. */
  selectionHead(): { left: number; top: number; bottom: number } | undefined {
    const { from, to, head } = this.view.state.selection.main;
    if (from === to) return undefined;
    return this.view.coordsAtPos(head, head === from ? 1 : -1) ?? undefined;
  }

  /** Bring a passage into view, in the middle where it fits. */
  scrollToPassage(from: number, to: number): void {
    const length = this.view.state.doc.length;
    const start = Math.max(0, Math.min(length, from));
    const range = EditorSelection.range(start, Math.max(start, Math.min(length, to)));
    this.view.dispatch({ effects: EditorView.scrollIntoView(range, { y: "center" }) });
  }

  firstVisibleLine(): number {
    const top = this.view.scrollDOM.scrollTop;
    const block = this.view.lineBlockAtHeight(Math.max(0, top - this.view.documentPadding.top));
    return this.view.state.doc.lineAt(Math.min(block.from, this.view.state.doc.length)).number;
  }

  scrollToLine(line: number): void {
    const doc = this.view.state.doc;
    const target = doc.line(Math.max(1, Math.min(line, doc.lines)));
    this.view.dispatch({ effects: EditorView.scrollIntoView(target.from, { y: "start" }) });
  }

  focus(): void {
    this.view.focus();
  }

  destroy(): void {
    this.destroyed = true;
    this.view.destroy();
  }

  /** Highlighting by `fileName`, loaded on demand; plain text without one. */
  private setLanguage(fileName: string | undefined): void {
    if (fileName === this.languageName) return;
    this.languageName = fileName;
    const language = fileName ? languageForFile(fileName) : null;
    if (!language) {
      this.view.dispatch({ effects: this.language.reconfigure([]) });
      return;
    }
    void loadLanguage(language).then((support) => {
      if (this.destroyed || this.languageName !== fileName) return;
      this.view.dispatch({ effects: this.language.reconfigure(support ?? []) });
    });
  }
}
