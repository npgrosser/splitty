// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ViewerRecord } from "../lib/protocol/types.ts";
import type { DocumentState } from "../lib/viewer/documents.ts";
import { UNFOLLOWED_LINK, ViewerView, linkDescription, type ViewerViewOptions } from "./viewer_view.ts";

// The settings the source view's shortcuts read query media features when
// they load; jsdom has no matchMedia.
vi.hoisted(() => {
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    value: (media: string) => ({
      matches: false, media, onchange: null,
      addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, dispatchEvent: () => false,
    }),
  });
});

const encoder = new TextEncoder();
const PLAN: ViewerRecord = { source: { kind: "file", path: "/home/me/app/PLAN.md" }, revision: 1 };

let views: ViewerView[] = [];

function view(record: ViewerRecord | undefined, options: Partial<ViewerViewOptions> = {}) {
  const callbacks = { openExternal: vi.fn(), openFile: vi.fn(), refuseLink: vi.fn(), reload: vi.fn() };
  const shown = new ViewerView(document, { record, supported: true, ...callbacks, ...options });
  document.body.append(shown.element);
  views.push(shown);
  return { shown, ...callbacks };
}

function read(content: string | Uint8Array, version = "v1"): DocumentState {
  return { document: { bytes: typeof content === "string" ? encoder.encode(content) : content, version }, reading: false };
}

const notice = (shown: ViewerView) => shown.element.querySelector<HTMLElement>(".viewer-notice")!;
const toggle = (shown: ViewerView) => shown.element.querySelector<HTMLButtonElement>(".viewer-bar-button")!;

afterEach(() => {
  for (const shown of views) shown.destroy();
  views = [];
  document.body.replaceChildren();
});

describe("ViewerView", () => {
  // viewer-panes, Showing A Document: A Markdown plan.
  it("renders a Markdown plan and offers its source", () => {
    const { shown } = view(PLAN);
    shown.update(read("# Plan\n\n- [ ] one\n"));
    const host = shown.element.querySelector(".viewer-rendered")!;
    expect(host.shadowRoot!.querySelector("h1")!.textContent).toBe("Plan");
    expect(notice(shown).hidden).toBe(true);
    expect(toggle(shown).hidden).toBe(false);
    expect(toggle(shown).textContent).toBe("Show Source");
    expect(shown.switchableMode).toBe("rendered");

    toggle(shown).click();
    expect(shown.element.querySelector(".viewer-rendered")).toBeNull();
    expect(shown.element.querySelector(".cm-editor")!.textContent).toContain("# Plan");
    expect(toggle(shown).textContent).toBe("Show Rendered");
    expect(shown.wrapping).toBe(true);
  });

  // viewer-panes, Showing A Document: A binary file.
  it("shows a binary file as its size and none of its bytes", () => {
    const { shown } = view({ source: { kind: "file", path: "/home/me/app/target/app" }, revision: 1 });
    const program = encoder.encode(`\u007fELF\u0000secret-symbol${"x".repeat(2000)}`);
    shown.update(read(program));
    expect(shown.element.querySelector(".viewer-binary")!.textContent).toBe("A binary file of 2.0 KB. Its content is not shown.");
    expect(shown.element.textContent).not.toContain("secret-symbol");
    expect(toggle(shown).hidden).toBe(true);
  });

  it("shows an image from a data URL only", () => {
    const { shown } = view({ source: { kind: "file", path: "/home/me/app/logo.png" }, revision: 1 });
    shown.update(read(new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2])));
    expect(shown.element.querySelector("img")!.getAttribute("src")).toMatch(/^data:image\/png;base64,/);
  });

  it("shows a text document as plain text, never rendered", () => {
    const { shown } = view({ source: { kind: "text", title: "Selection from claude" }, revision: 1 });
    shown.update(read("# The answer\n"));
    expect(shown.element.querySelector(".viewer-rendered")).toBeNull();
    expect(shown.element.querySelector(".cm-editor")!.textContent).toContain("# The answer");
    expect(shown.element.querySelector(".viewer-bar-label")!.textContent).toBe("Selection from claude");
  });

  // viewer-panes, Keeping A Shown Document Current: a failed read says why and
  // offers to read again.
  it("says why a read failed and offers to read again", () => {
    const { shown, reload } = view(PLAN);
    expect(notice(shown).textContent).toBe("Reading…");
    shown.update({ failure: { code: "not_found", message: "The file does not exist." }, reading: false });
    expect(notice(shown).hidden).toBe(false);
    expect(notice(shown).querySelector(".viewer-notice-text")!.textContent).toBe("The file does not exist.");
    notice(shown).querySelector<HTMLButtonElement>(".viewer-notice-button")!.click();
    expect(reload).toHaveBeenCalledTimes(1);

    shown.update(read("# Plan\n"));
    expect(notice(shown).hidden).toBe(true);
  });

  it("shows a new version in place of the old one", () => {
    const { shown } = view(PLAN);
    shown.update(read("# Plan\n", "v1"));
    const host = shown.element.querySelector(".viewer-rendered");
    shown.update(read("# Plan v2\n", "v2"));
    expect(shown.element.querySelector(".viewer-rendered")).toBe(host);
    expect(host!.shadowRoot!.querySelector("h1")!.textContent).toBe("Plan v2");
  });

  // viewer-panes, Viewer Capability: a pane of the role viewer that cannot be shown says so.
  it("says that a pane cannot be shown without a record or on a server without viewers", () => {
    const missing = view(undefined).shown;
    expect(notice(missing).textContent).toBe("This pane cannot be shown: its document is missing or malformed.");
    missing.update(read("ignored"));
    expect(notice(missing).hidden).toBe(false);

    const old = view(PLAN, { supported: false }).shown;
    expect(notice(old).textContent).toContain("has no viewer panes");
    old.update(read("ignored"));
    expect(old.element.querySelector(".viewer-rendered")).toBeNull();
  });

  it("follows a relative link of a rendered document, and reports one it refuses", () => {
    const { shown, openFile, refuseLink } = view(PLAN);
    shown.update(read("[tasks](TASKS.md) [far](../../../../x.md)\n"));
    const links = shown.element.querySelector(".viewer-rendered")!.shadowRoot!.querySelectorAll("[data-splitty-href]");
    links[0].dispatchEvent(new MouseEvent("click", { bubbles: true, composed: true, cancelable: true }));
    expect(openFile).toHaveBeenCalledWith("/home/me/app/TASKS.md");
    links[1].dispatchEvent(new MouseEvent("click", { bubbles: true, composed: true, cancelable: true }));
    expect(refuseLink).toHaveBeenCalledTimes(1);
  });

  // The bar says where a link leads before it is followed.
  it("shows where a link under the pointer or with the focus leads in place of the path", () => {
    const { shown, openFile, openExternal, refuseLink } = view(PLAN);
    shown.update(read([
      "[tasks](TASKS.md) [far](../../../../x.md) [web](https://example.com/a) [mail](mailto:me@example.com)",
      "[top](#plan) [**bold** tasks](./docs/../TASKS.md) plain text",
    ].join("\n\n")));
    const host = shown.element.querySelector<HTMLElement>(".viewer-rendered")!;
    const links = [...host.shadowRoot!.querySelectorAll<HTMLElement>("[data-splitty-href]")];
    const bar = shown.element.querySelector<HTMLElement>(".viewer-bar-label")!;
    const over = (target: Element) => target.dispatchEvent(new PointerEvent("pointerover", { bubbles: true, composed: true }));
    const showsPath = () => {
      expect(bar.textContent).toBe("/home/me/app/PLAN.md");
      expect(bar.classList.contains("viewer-bar-link")).toBe(false);
    };
    showsPath();

    over(links[0]);
    expect(bar.textContent).toBe("/home/me/app/TASKS.md");
    expect(bar.title).toBe("/home/me/app/TASKS.md");
    expect(bar.classList.contains("viewer-bar-link")).toBe(true);
    over(links[1]);
    expect(bar.textContent).toBe(UNFOLLOWED_LINK);
    over(links[2]);
    expect(bar.textContent).toBe("https://example.com/a");
    over(links[3]);
    expect(bar.textContent).toBe(UNFOLLOWED_LINK);
    over(links[4]);
    expect(bar.textContent).toBe("/home/me/app/PLAN.md#plan");
    // An element inside a link stands for the link.
    over(links[5].querySelector("strong")!);
    expect(bar.textContent).toBe("/home/me/app/TASKS.md");

    // Off the links: over the document's text, over the host itself, and out of the host.
    over(links[0]);
    over(host.shadowRoot!.querySelectorAll("p")[1]);
    showsPath();
    over(links[0]);
    over(host);
    showsPath();
    over(links[2]);
    host.dispatchEvent(new PointerEvent("pointerleave"));
    showsPath();

    // The keyboard focus, while the pointer is over no link.
    links[2].focus();
    expect(bar.textContent).toBe("https://example.com/a");
    links[0].focus();
    expect(bar.textContent).toBe("/home/me/app/TASKS.md");
    // The pointer comes first while it is over a link.
    over(links[1]);
    expect(bar.textContent).toBe(UNFOLLOWED_LINK);
    host.dispatchEvent(new PointerEvent("pointerleave"));
    expect(bar.textContent).toBe("/home/me/app/TASKS.md");
    links[0].blur();
    showsPath();

    // Showing a target follows nothing.
    expect(openFile).not.toHaveBeenCalled();
    expect(openExternal).not.toHaveBeenCalled();
    expect(refuseLink).not.toHaveBeenCalled();
  });

  it("shows the path again when a new version replaces the link under the pointer", () => {
    const { shown } = view(PLAN);
    shown.update(read("[tasks](TASKS.md)\n", "v1"));
    const host = shown.element.querySelector<HTMLElement>(".viewer-rendered")!;
    const bar = shown.element.querySelector<HTMLElement>(".viewer-bar-label")!;
    host.shadowRoot!.querySelector("[data-splitty-href]")!.dispatchEvent(new PointerEvent("pointerover", { bubbles: true, composed: true }));
    expect(bar.textContent).toBe("/home/me/app/TASKS.md");
    shown.update(read("[notes](NOTES.md)\n", "v2"));
    expect(bar.textContent).toBe("/home/me/app/PLAN.md");
    toggle(shown).click();
    expect(bar.textContent).toBe("/home/me/app/PLAN.md");
  });

  it("describes a fragment without the characters that could disguise it", () => {
    expect(linkDescription({ kind: "fragment", id: "a‮b\u0007c" }, "/home/me/app/PLAN.md")).toBe("/home/me/app/PLAN.md#a�b�c");
    expect(linkDescription({ kind: "refused" }, "/home/me/app/PLAN.md")).toBe(UNFOLLOWED_LINK);
    expect(linkDescription({ kind: "none" }, "/home/me/app/PLAN.md")).toBe(UNFOLLOWED_LINK);
  });
});
