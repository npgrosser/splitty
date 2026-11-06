// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { HOST_STYLE, RenderedDocument } from "./render.ts";
import { cssValueAllowed, documentSelector, sanitizeHtml } from "./sanitize.ts";

/** A script that marks the document when it runs, `data-ran-<name>` on its root element. */
function run(name: string): string {
  return `document.documentElement.setAttribute('data-ran-${name}', '')`;
}

/** The names of the scripts that ran. */
function ran(): string[] {
  return document.documentElement.getAttributeNames().filter((name) => name.startsWith("data-ran-")).map((name) => name.slice(9));
}

function setup(documentPath = "/home/me/app/docs/report.html") {
  const options = { documentPath, openExternal: vi.fn(), openFile: vi.fn(), refuseLink: vi.fn() };
  const rendered = new RenderedDocument(document, options);
  document.body.append(rendered.host);
  return { rendered, options, root: rendered.shadow };
}

function click(element: Element, init: MouseEventInit = {}): MouseEvent {
  const event = new MouseEvent("click", { bubbles: true, composed: true, cancelable: true, button: 0, ...init });
  element.dispatchEvent(event);
  return event;
}

/** The document's own styles as the shadow root holds them; the first sheet is the pane's. */
function documentCss(root: ShadowRoot): string {
  return [...root.querySelectorAll("style")].slice(1).map((style) => style.textContent).join("\n");
}

afterEach(() => {
  document.body.replaceChildren();
  for (const name of ran()) document.documentElement.removeAttribute(`data-ran-${name}`);
});

// access-control, Rendered Documents Are Inert.
describe("RenderedDocument", () => {
  // A script in an HTML report.
  it("runs no script, event handler or javascript: link of an HTML report", () => {
    // Scripts that reach this document run.
    const script = document.createElement("script");
    script.textContent = run("probe");
    document.body.append(script);
    expect(ran()).toEqual(["probe"]);

    const { rendered, root, options } = setup();
    rendered.render("html", `<!doctype html><html><head><script>${run("head")}</script>
      <base href="https://example.com/"><meta http-equiv="refresh" content="0;url=https://example.com/">
      <link rel="stylesheet" href="/api/mux/state"></head>
      <body onload="${run("onload")}">
      <script>${run("body")}</script>
      <img src="chart.png" onerror="${run("onerror")}" alt="chart">
      <div onclick="${run("onclick")}" onmouseover="${run("over")}">box</div>
      <a id="js" href="javascript:${run("link")}">run</a>
      <a id="js2" href=" JaVaScRiPt:${run("link2")}">run</a>
      <svg><a id="svgjs" xlink:href="javascript:${run("svg")}"><text>t</text></a><script>${run("svgscript")}</script>
        <animate attributeName="href" to="javascript:${run("animate")}"/></svg>
      <iframe srcdoc="<script>parent.${run("frame")}</script>"></iframe>
      <object data="x.swf"></object><embed src="x.swf">
      <form action="/api/mux/op" method="post"><input name="op"><button>go</button></form>
      <math><mi xlink:href="javascript:${run("math")}">m</mi></math>
      </body></html>`);

    expect(root.querySelectorAll("script, iframe, object, embed, form, input, button, base, meta, link")).toHaveLength(0);
    for (const element of root.querySelectorAll("*")) {
      for (const attribute of element.attributes) expect(attribute.name.startsWith("on"), attribute.name).toBe(false);
      element.dispatchEvent(new Event("error"));
      element.dispatchEvent(new Event("load"));
      click(element);
    }
    for (const id of ["js", "js2", "svgjs"]) {
      const link = root.getElementById(id)!;
      expect(link.hasAttribute("href"), id).toBe(false);
      expect(link.hasAttribute("data-splitty-href"), id).toBe(false);
      expect(click(link).defaultPrevented, id).toBe(true);
    }
    expect(ran()).toEqual(["probe"]);
    expect(options.openExternal).not.toHaveBeenCalled();
    expect(options.openFile).not.toHaveBeenCalled();
    rendered.destroy();
  });

  // An image from the server's origin.
  it("requests no image from the server's origin and shows its alt text", () => {
    const { rendered, root } = setup("/home/me/app/PLAN.md");
    rendered.render("markdown", "![x](/api/mux/state)\n\n![](https://example.com/a.png)\n\n![dot](data:image/png;base64,iVBORw0KGgo=)\n");
    expect([...root.querySelectorAll("img")].map((image) => image.getAttribute("src"))).toEqual(["data:image/png;base64,iVBORw0KGgo="]);
    expect([...root.querySelectorAll(".missing-image")].map((span) => span.textContent)).toEqual(["x", "Image"]);
    expect(root.innerHTML).not.toContain("/api/mux/state");
    expect(root.innerHTML).not.toContain("example.com");
  });

  it("drops what would load before it reaches a live document", () => {
    const fragment = sanitizeHtml(`<img src="/api/mux/state" alt="x"><p style="background:url(/api/mux/state);color:red">p</p>
      <svg><image href="/api/mux/state"/><path id="curve" d="M0 0L9 9"/><text><textPath href="#curve">a</textPath><textPath href="https://example.com/s.svg#a">b</textPath></text><use href="#curve"/></svg>
      <video src="/v.mp4" poster="/p.png"></video><img srcset="/a.png 1x" src="data:image/png;base64,AAAA" alt="">
      <div style="--x: \\75 rl(/api)">v</div>`);
    // Parsed and cleaned in an inert document.
    expect(fragment.ownerDocument).not.toBe(document);
    expect(fragment.querySelectorAll("img")).toHaveLength(1);
    expect(fragment.querySelector("img")!.hasAttribute("srcset")).toBe(false);
    // Only the declaration that loads goes; a shorthand's other parts stay.
    const style = fragment.querySelector("p")!.getAttribute("style")!;
    expect(style).not.toMatch(/url|api/);
    expect(style).toContain("color: red");
    expect(fragment.querySelector("image")!.hasAttribute("href")).toBe(false);
    // References stay within the document; `use` is not kept at all.
    expect([...fragment.querySelectorAll("textPath")].map((path) => path.getAttribute("href"))).toEqual(["#curve", null]);
    expect(fragment.querySelector("use")).toBeNull();
    expect(fragment.querySelector("video")).toBeNull();
    expect(fragment.querySelector("div")!.hasAttribute("style")).toBe(false);
  });

  it("keeps no style rule that loads something or reaches outside the document", () => {
    const { rendered, root } = setup();
    rendered.render("html", `<html><head><style>
      @import url(/api/mux/state);
      @font-face { font-family: x; src: url(/font.woff); }
      .a { background: url(/api/mux/state); color: red; }
      .b { background-image: image-set("/x.png" 1x); }
      .c { fill: url(#grad); }
      .d { background: url("data:image/png;base64,AAAA"); }
      :host { position: fixed; inset: 0; }
      @media screen { :host(.x) { color: red; } .e { color: blue; } }
      ::slotted(p), x::part(y) { color: red; }
      html { background: blue; }
      body.dark { color: white; }
      </style></head><body class="dark"><p class="a" style="background:url(https://example.com/t.png);color:green">a</p></body></html>`);
    const css = documentCss(root);
    expect(css).not.toMatch(/@import|@font-face|\/api|:host|slotted|::part|image-set|example\.com/);
    expect(css).toContain("color: red");
    expect(css).toMatch(/url\(["']?#grad/);
    expect(css).toContain("data:image/png;base64,AAAA");
    expect(css).toContain(".e");
    expect(css).toMatch(/\[data-splitty-html\]\s*\{\s*background: blue/);
    expect(css).toContain("[data-splitty-body].dark");
    expect(root.querySelector("[data-splitty-body]")!.className).toBe("dark");
    const style = root.querySelector("p")!.getAttribute("style")!;
    expect(style).not.toMatch(/url|example/);
    expect(style).toContain("color: green");
  });

  // An overlay.
  it("keeps a fixed overlay inside the pane", () => {
    const { rendered, root } = setup();
    rendered.render("html", `<style>.overlay{position:fixed;inset:0;z-index:2147483647} :host{contain:none!important;overflow:visible!important}</style><div class="overlay">covering</div>`);
    // The host, which no rule of the document reaches, contains and clips
    // whatever the document positions.
    expect(rendered.host.getAttribute("style")).toBe(HOST_STYLE);
    for (const declaration of ["contain:layout paint", "overflow:auto", "isolation:isolate"]) expect(HOST_STYLE).toContain(declaration);
    expect(rendered.host.shadowRoot).toBe(root);
    expect(document.querySelector(".overlay")).toBeNull();
    expect(root.querySelector("[data-splitty-html] .overlay")).not.toBeNull();
    expect(documentCss(root)).toContain("position: fixed");
    expect(documentCss(root)).not.toContain(":host");
  });

  // A link to a website.
  it("opens a website through the external opener and never navigates", () => {
    const { rendered, root, options } = setup("/home/me/app/PLAN.md");
    rendered.render("markdown", "[site](https://example.com/docs) [tasks](TASKS.md) [up](#plan) [far](../../../../x.md) [mail](mailto:me@example.com)\n\n# Plan\n");
    const links = [...root.querySelectorAll("[data-splitty-href]")];
    expect(links.map((link) => link.getAttribute("data-splitty-href"))).toEqual(["https://example.com/docs", "TASKS.md", "#plan", "../../../../x.md", "mailto:me@example.com"]);
    expect(root.querySelectorAll("[href]")).toHaveLength(0);
    const before = window.location.href;

    expect(click(links[0]).defaultPrevented).toBe(true);
    expect(options.openExternal).toHaveBeenCalledWith("https://example.com/docs");
    expect(window.location.href).toBe(before);

    click(links[1]);
    expect(options.openFile).toHaveBeenCalledWith("/home/me/app/TASKS.md");
    // A relative link that climbs above the root opens nothing and says so.
    click(links[3]);
    expect(options.refuseLink).toHaveBeenCalledTimes(1);
    // Any other link does nothing.
    expect(click(links[4]).defaultPrevented).toBe(true);
    expect(options.openExternal).toHaveBeenCalledTimes(1);
    expect(options.openFile).toHaveBeenCalledTimes(1);

    // Neither a middle click nor a click with another button opens anything.
    const middle = new MouseEvent("auxclick", { bubbles: true, composed: true, cancelable: true, button: 1 });
    links[0].dispatchEvent(middle);
    expect(middle.defaultPrevented).toBe(true);
    expect(click(links[0], { button: 2 }).defaultPrevented).toBe(true);
    expect(options.openExternal).toHaveBeenCalledTimes(1);

    // The keyboard follows a focused link.
    const enter = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, composed: true, cancelable: true });
    links[1].dispatchEvent(enter);
    expect(enter.defaultPrevented).toBe(true);
    expect(options.openFile).toHaveBeenCalledTimes(2);
    expect(links[1].getAttribute("tabindex")).toBe("0");
  });

  it("scrolls to an anchor within the document", () => {
    const { rendered, root } = setup("/home/me/app/PLAN.md");
    rendered.render("markdown", "[down](#step-2)\n\n## Step 2\n");
    let top = 0;
    Object.defineProperty(rendered.host, "scrollTop", { configurable: true, get: () => top, set: (value: number) => { top = value; } });
    rendered.host.getBoundingClientRect = () => ({ top: 100 }) as DOMRect;
    root.getElementById("step-2")!.getBoundingClientRect = () => ({ top: 500 }) as DOMRect;
    click(root.querySelector("[data-splitty-href]")!);
    expect(top).toBe(392);
  });

  it("lets no document set the pane's own attributes", () => {
    const { rendered, root, options } = setup("/home/me/app/PLAN.md");
    rendered.render("markdown", `<span data-splitty-href="https://example.com/">fake</span> <a href="x.md" data-splitty-href="https://example.com/">real</a> <p data-splitty-html>p</p>`);
    expect(root.querySelector("span")!.hasAttribute("data-splitty-href")).toBe(false);
    expect(root.querySelector(".markdown-body [data-splitty-html]")).toBeNull();
    click(root.querySelector("span")!);
    expect(options.openExternal).not.toHaveBeenCalled();
    expect(root.querySelector("a")!.getAttribute("data-splitty-href")).toBe("x.md");
  });

  it("forbids attributes that take focus, open popovers or name other trees", () => {
    const { rendered, root } = setup();
    rendered.render("html", `<div id="a" tabindex="1" autofocus accesskey="x" contenteditable popover part="p" slot="s">x</div><button popovertarget="a">b</button>`);
    const div = root.getElementById("a")!;
    for (const name of ["tabindex", "autofocus", "accesskey", "contenteditable", "popover", "part", "slot"]) expect(div.hasAttribute(name), name).toBe(false);
  });

  it("shows Markdown with its source lines and HTML on a page of its own", () => {
    const { rendered, root } = setup("/home/me/app/PLAN.md");
    rendered.render("markdown", "# Plan\n\n- [x] done\n");
    expect(root.querySelector(".markdown-body h1#plan")!.getAttribute("data-source-line")).toBe("1");
    expect(root.querySelector(".task-box")!.getAttribute("aria-checked")).toBe("true");
    rendered.render("html", "<p>report</p>");
    expect(root.querySelector(".markdown-body")).toBeNull();
    expect(root.querySelector("[data-splitty-html] > [data-splitty-body] > p")!.textContent).toBe("report");
    rendered.destroy();
    expect(rendered.host.isConnected).toBe(false);
    expect(root.childNodes).toHaveLength(0);
  });
});

describe("cssValueAllowed", () => {
  it("allows values that load nothing", () => {
    expect(cssValueAllowed("color", "red")).toBe(true);
    expect(cssValueAllowed("background", "url(\"data:image/png;base64,AAAA\")")).toBe(true);
    expect(cssValueAllowed("fill", "url(#grad)")).toBe(true);
    expect(cssValueAllowed("content", "attr(title)")).toBe(true);
    expect(cssValueAllowed("--accent", "#fff")).toBe(true);
  });

  it("refuses values that load or could spell a load", () => {
    for (const [property, value] of [
      ["background", "url(/api/mux/state)"],
      ["background", "URL( 'https://example.com/x.png' )"],
      ["background-image", "-webkit-image-set(\"/x.png\" 1x)"],
      ["background-image", "image(\"/x.png\")"],
      ["background-image", "cross-fade(url(a.png), red)"],
      ["background-image", "element(#x)"],
      ["background-image", "paint(worklet)"],
      ["cursor", "url(/c.cur), auto"],
      ["list-style", "url(data:text/html,x)"],
      ["background", "url(#grad)"],
      ["width", "attr(data-w px)"],
      ["--x", "\\75 rl(/api)"],
      ["--x", "url(/api)"],
    ]) {
      expect(cssValueAllowed(property, value), `${property}: ${value}`).toBe(false);
    }
  });
});

describe("documentSelector", () => {
  it("puts the wrappers in place of html, :root and body", () => {
    expect(documentSelector("html body .x")).toBe("[data-splitty-html] [data-splitty-body] .x");
    expect(documentSelector(":root, body > p, .body, #html, tbody")).toBe("[data-splitty-html], [data-splitty-body] > p, .body, #html, tbody");
  });
});
