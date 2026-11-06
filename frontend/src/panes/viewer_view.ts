/**
 * What a viewer pane shows (`viewer-panes`, Showing A Document and Keeping A
 * Shown Document Current): its document by presentation, a notice in place of
 * it while it cannot be shown, and a bar with the document's path and the
 * switch between a rendered document and its source.
 *
 * A new version of the document replaces the old one in place: the source
 * view keeps its first visible line, a rendered document its scroll fraction.
 * A view built again for the same pane starts from the state the last one was
 * left in (`viewer_view_state.ts`); its reading position is restored once the
 * document is shown and laid out.
 *
 * While the pointer is over a link of a rendered document, or a link has the
 * keyboard focus, the bar shows where following it leads in place of the
 * path: the file a relative link opens, the URL of a web link, or that the
 * pane opens nothing for it. The bar and the notices are outside the rendered
 * document's shadow host.
 */

import type { ViewerRecord } from "../lib/protocol/types.ts";
import type { DocumentState, ReadFailure, ViewerDocument } from "../lib/viewer/documents.ts";
import type { DocumentLink } from "../lib/viewer/links.ts";
import { decodeText, fileName, imageDataUrl, presentationOf, type Presentation } from "../lib/viewer/presentation.ts";
import { RenderedDocument } from "../lib/viewer/render.ts";
import { SourceView } from "../lib/viewer/source_view.ts";
import { formatBytes } from "../lib/terminal/image_paste.ts";
import type { ViewerMode, ViewerPosition, ViewerViewState } from "./viewer_view_state.ts";

export type { ViewerMode } from "./viewer_view_state.ts";

export interface ViewerViewOptions {
  /** Missing or malformed: the pane says it cannot be shown. */
  readonly record?: ViewerRecord;
  /** Whether the pane's server announced viewer panes. */
  readonly supported: boolean;
  /**
   * Where the pane's last view was left. Without it a document that can be
   * rendered shows rendered, lines wrap by the document's kind, and reading
   * starts at the top.
   */
  readonly state?: ViewerViewState;
  openExternal(url: string): void;
  openFile(path: string): void;
  /** A relative link of a rendered document names no path a viewer pane can show. */
  refuseLink(): void;
  reload(): void;
}

interface Shown {
  readonly version: string;
  readonly presentation: Presentation;
  readonly bytes: Uint8Array;
  text?: string;
}

/** What the bar says for a link that opens nothing. */
export const UNFOLLOWED_LINK = "This link cannot be opened";

// Control and invisible formatting characters, which could make a target
// read as another one: those a viewer path may not hold (`lib/viewer/limits.ts`),
// so only a fragment can bring them.
// eslint-disable-next-line no-control-regex
const UNPRINTABLE = /[\u0000-\u001f\u007f-\u009f\u200b-\u200f\u202a-\u202e\u2060-\u206f\ufeff]/g;

/** Where following `link` of the document labelled `label` leads, as the bar says it. */
export function linkDescription(link: DocumentLink, label: string): string {
  switch (link.kind) {
    case "file":
      return link.path;
    case "external":
      return link.url;
    case "fragment":
      return `${label}#${link.id.replace(UNPRINTABLE, "\ufffd")}`;
    default:
      return UNFOLLOWED_LINK;
  }
}

function element<K extends keyof HTMLElementTagNameMap>(doc: Document, tag: K, className: string, text?: string): HTMLElementTagNameMap[K] {
  const node = doc.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

export class ViewerView {
  readonly element: HTMLElement;
  private readonly doc: Document;
  private record: ViewerRecord | undefined;
  /** Holds the document's view; hidden while a notice stands in its place. */
  private readonly documentBox: HTMLElement;
  private readonly notice: HTMLElement;
  private readonly barLabel: HTMLElement;
  private readonly toggle: HTMLButtonElement;
  private mode: ViewerMode;
  private wrapChoice: boolean | undefined;
  private shown: Shown | undefined;
  private source: SourceView | null = null;
  private rendered: RenderedDocument | null = null;
  /** An image or the notice of a binary file. */
  private still: HTMLElement | null = null;
  /** Where a link under the pointer or with the focus leads; the bar shows it in place of the path. */
  private linkTarget: string | undefined;
  /** The position to restore, until the document is shown and laid out. */
  private pending: ViewerPosition | undefined;
  /**
   * Where reading stands: as the last scroll left it or as it was restored,
   * also while the document is hidden. Undefined at the top.
   */
  private position: ViewerPosition | undefined;
  private layoutWatch: ResizeObserver | null = null;

  constructor(doc: Document, private readonly options: ViewerViewOptions) {
    this.doc = doc;
    this.record = options.record;
    this.mode = options.state?.mode ?? "rendered";
    this.wrapChoice = options.state?.wrap;
    this.pending = options.state?.position;
    this.element = element(doc, "div", "viewer-pane");
    const body = element(doc, "div", "viewer-body");
    this.documentBox = element(doc, "div", "viewer-document");
    // Scroll events do not bubble; the capture phase sees those of the source
    // view's scroller and of the rendered document's host.
    this.documentBox.addEventListener("scroll", this.notePosition, { capture: true, passive: true });
    this.notice = element(doc, "div", "viewer-notice");
    this.notice.hidden = true;
    body.append(this.documentBox, this.notice);

    const bar = element(doc, "div", "viewer-bar");
    this.barLabel = element(doc, "span", "viewer-bar-label");
    this.showLabel();
    this.toggle = element(doc, "button", "viewer-bar-button");
    this.toggle.type = "button";
    this.toggle.hidden = true;
    this.toggle.addEventListener("click", () => this.toggleMode());
    bar.append(this.barLabel, this.toggle);
    this.element.append(body, bar);

    if (!this.record || !options.supported) {
      this.showNotice(options.supported
        ? "This pane cannot be shown: its document is missing or malformed."
        : "This pane cannot be shown: the Splitty of its server has no viewer panes. Update Splitty there to show it.");
    } else {
      this.showNotice("Reading…", { muted: true });
    }
  }

  /** The document's path, or a text's title. */
  private label(): string {
    const source = this.record?.source;
    if (!source) return "Viewer";
    return source.kind === "file" ? source.path : source.title;
  }

  /**
   * A newer record of the pane: another origin or revision. Its document is
   * the same one (`layout/screen_snapshot.ts` builds the pane anew otherwise).
   */
  setRecord(record: ViewerRecord): void {
    this.record = record;
    this.showLabel();
  }

  update(state: DocumentState): void {
    if (!this.record || !this.options.supported) return;
    if (state.failure) {
      this.showFailure(state.failure);
      return;
    }
    if (state.document) {
      this.showDocument(state.document);
      return;
    }
    this.showNotice(state.reading ? "Reading…" : "", { muted: true });
  }

  /** The current mode, where the document can switch between rendered and source. */
  get switchableMode(): ViewerMode | undefined {
    return this.shown?.presentation.kind === "rendered" ? this.mode : undefined;
  }

  /** Whether the source view wraps lines; undefined while no source view is shown. */
  get wrapping(): boolean | undefined {
    return this.source?.wrap;
  }

  /** How the view is left, for the pane's next view. */
  viewState(): ViewerViewState {
    const position = this.pending ?? this.position;
    return {
      mode: this.mode,
      ...(this.wrapChoice !== undefined ? { wrap: this.wrapChoice } : {}),
      ...(position ? { position } : {}),
    };
  }

  toggleMode(): void {
    if (this.shown?.presentation.kind !== "rendered") return;
    this.mode = this.mode === "rendered" ? "source" : "rendered";
    // A position in the other mode means nothing in this one.
    this.pending = undefined;
    this.position = undefined;
    this.present(this.shown, true);
  }

  toggleWrap(): void {
    if (!this.source) return;
    this.wrapChoice = !this.source.wrap;
    this.source.setWrap(this.wrapChoice);
  }

  /** The selected text of the document. */
  selectionText(): string {
    if (!this.documentBox.hidden) {
      if (this.source) return this.source.selection()?.text ?? "";
      if (this.rendered) return this.rendered.selection()?.text ?? "";
    }
    return this.doc.getSelection()?.toString() ?? "";
  }

  focus(): void {
    if (!this.documentBox.hidden && this.source) this.source.focus();
    else if (!this.documentBox.hidden && this.rendered) this.rendered.focus();
    else this.element.focus({ preventScroll: true });
  }

  destroy(): void {
    this.layoutWatch?.disconnect();
    this.layoutWatch = null;
    this.clearDocument();
    this.element.remove();
  }

  // ── States ──

  private showNotice(text: string, options: { muted?: boolean; action?: { label: string; run: () => void } } = {}): void {
    this.documentBox.hidden = true;
    this.notice.hidden = false;
    this.notice.classList.toggle("muted", !!options.muted);
    const message = element(this.doc, "p", "viewer-notice-text", text);
    const nodes: Node[] = [message];
    if (options.action) {
      const { run } = options.action;
      const button = element(this.doc, "button", "viewer-notice-button", options.action.label);
      button.type = "button";
      button.addEventListener("click", () => run());
      nodes.push(button);
    }
    this.notice.replaceChildren(...nodes);
  }

  private showFailure(failure: ReadFailure): void {
    this.showNotice(failure.message, { action: { label: "Read Again", run: () => this.options.reload() } });
  }

  private showDocument(document: ViewerDocument): void {
    const record = this.record!;
    if (this.shown?.version === document.version && this.shown.bytes === document.bytes) {
      this.reveal();
      return;
    }
    const presentation = presentationOf(record.source, document.bytes);
    const shown: Shown = { version: document.version, presentation, bytes: document.bytes };
    const sameKind = this.shown?.presentation.kind === presentation.kind;
    this.shown = shown;
    this.present(shown, !sameKind);
  }

  private reveal(): void {
    this.notice.hidden = true;
    this.notice.replaceChildren();
    this.documentBox.hidden = false;
  }

  private textOf(shown: Shown): string {
    shown.text ??= decodeText(shown.bytes);
    return shown.text;
  }

  /** Show `shown`; `rebuild` replaces the document's view instead of updating it. */
  private present(shown: Shown, rebuild: boolean): void {
    const { presentation } = shown;
    const source = this.record!.source;
    this.toggle.hidden = presentation.kind !== "rendered";
    if (presentation.kind === "rendered") this.toggle.textContent = this.mode === "rendered" ? "Show Source" : "Show Rendered";
    this.reveal();

    if (presentation.kind === "rendered" && this.mode === "rendered") {
      if (rebuild || !this.rendered) {
        this.clearDocument();
        this.rendered = new RenderedDocument(this.doc, {
          documentPath: source.kind === "file" ? source.path : "",
          openExternal: (url) => this.options.openExternal(url),
          openFile: (path) => this.options.openFile(path),
          refuseLink: () => this.options.refuseLink(),
          pointLink: (link) => this.showLink(link),
        });
        this.documentBox.append(this.rendered.host);
        this.rendered.render(presentation.format, this.textOf(shown));
        this.restorePosition();
      } else {
        const fraction = this.rendered.scrollFraction();
        this.rendered.render(presentation.format, this.textOf(shown));
        this.rendered.scrollToFraction(fraction);
      }
      return;
    }

    if (presentation.kind === "rendered" || presentation.kind === "source") {
      const name = presentation.kind === "source" ? presentation.fileName : source.kind === "file" ? fileName(source.path) : undefined;
      if (rebuild || !this.source) {
        this.clearDocument();
        const wrap = this.wrapChoice ?? (source.kind === "text" || presentation.kind === "rendered");
        this.source = new SourceView(this.documentBox, { text: this.textOf(shown), fileName: name, wrap });
        this.restorePosition();
      } else {
        this.source.setText(this.textOf(shown), name);
      }
      return;
    }

    this.clearDocument();
    // An image or a binary file has no reading position.
    this.pending = undefined;
    if (presentation.kind === "image") {
      const box = element(this.doc, "div", "viewer-image");
      const image = element(this.doc, "img", "");
      image.alt = fileName(source.kind === "file" ? source.path : source.title);
      image.src = imageDataUrl(presentation.type, shown.bytes);
      box.append(image);
      this.still = box;
      this.documentBox.append(box);
      return;
    }
    // A binary file: its size, none of its bytes.
    const box = element(this.doc, "div", "viewer-binary");
    box.append(element(this.doc, "p", "viewer-notice-text", `A binary file of ${formatBytes(presentation.size)}. Its content is not shown.`));
    this.still = box;
    this.documentBox.append(box);
  }

  private clearDocument(): void {
    this.source?.destroy();
    this.source = null;
    this.rendered?.destroy();
    this.rendered = null;
    this.still?.remove();
    this.still = null;
    this.documentBox.replaceChildren();
    this.showLink(undefined);
  }

  // ── The Bar ──

  private showLink(link: DocumentLink | undefined): void {
    this.linkTarget = link ? linkDescription(link, this.label()) : undefined;
    this.showLabel();
  }

  private showLabel(): void {
    const text = this.linkTarget ?? this.label();
    this.barLabel.textContent = text;
    this.barLabel.title = text;
    this.barLabel.classList.toggle("viewer-bar-link", this.linkTarget !== undefined);
  }

  // ── Reading Position ──

  // A scroll of the document shown and laid out moves the reading position.
  private readonly notePosition = (): void => {
    if (this.documentBox.hidden || this.documentBox.clientHeight === 0) return;
    if (this.source) this.position = { line: this.source.firstVisibleLine() };
    else if (this.rendered) this.position = { fraction: this.rendered.scrollFraction() };
  };

  /** Restore the pending position in the view just built, at once or when it is laid out. */
  private restorePosition(): void {
    const pending = this.pending;
    if (!pending) return;
    if ("line" in pending ? !this.source : !this.rendered) {
      this.pending = undefined;
      return;
    }
    if (this.documentBox.clientHeight > 0) {
      this.applyPosition();
      return;
    }
    const Observer = this.doc.defaultView?.ResizeObserver;
    if (!Observer || this.layoutWatch) return;
    this.layoutWatch = new Observer(() => {
      if (this.documentBox.clientHeight > 0) this.applyPosition();
    });
    this.layoutWatch.observe(this.documentBox);
  }

  private applyPosition(): void {
    const pending = this.pending;
    this.pending = undefined;
    this.layoutWatch?.disconnect();
    this.layoutWatch = null;
    if (!pending) return;
    if ("line" in pending) this.source?.scrollToLine(pending.line);
    else this.rendered?.scrollToFraction(pending.fraction);
    this.position = pending;
  }
}
