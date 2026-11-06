import { describe, expect, it } from "vitest";
import { cssValueAllowed, declaredNames, sanitizeDeclarations, unescapeCss } from "./sanitize.ts";

interface Shorthand {
  readonly text: string;
  readonly longhands: readonly string[];
}

/**
 * A declaration block as Chromium shows one that holds a shorthand with
 * `var()`: the shorthand's longhands read as `""`, the shorthand reads as its
 * text while all of its longhands are present, and serializes as itself.
 */
class PendingStyle {
  private readonly longhands: Map<string, string>;
  private readonly shorthands: Map<string, Shorthand>;

  constructor(longhands: Record<string, string>, shorthands: Record<string, Shorthand>) {
    this.longhands = new Map(Object.entries(longhands));
    this.shorthands = new Map(Object.entries(shorthands));
  }

  get length(): number {
    return this.longhands.size;
  }

  item(index: number): string {
    return [...this.longhands.keys()][index] ?? "";
  }

  getPropertyValue(name: string): string {
    const value = this.longhands.get(name);
    if (value !== undefined) return value;
    const shorthand = this.shorthands.get(name);
    return shorthand && this.whole(shorthand) ? shorthand.text : "";
  }

  removeProperty(name: string): string {
    const shorthand = this.shorthands.get(name);
    for (const longhand of shorthand?.longhands ?? [name]) this.longhands.delete(longhand);
    return "";
  }

  get cssText(): string {
    const parts: string[] = [];
    const covered = new Set<string>();
    for (const [name, shorthand] of this.shorthands) {
      if (!this.whole(shorthand)) continue;
      parts.push(`${name}: ${shorthand.text};`);
      for (const longhand of shorthand.longhands) covered.add(longhand);
    }
    for (const [name, value] of this.longhands) {
      if (!covered.has(name) && value) parts.push(`${name}: ${value};`);
    }
    return parts.join(" ");
  }

  names(): string[] {
    return [...this.longhands.keys()];
  }

  private whole(shorthand: Shorthand): boolean {
    return shorthand.longhands.every((longhand) => this.longhands.get(longhand) === "");
  }
}

const SHORTHANDS: Record<string, readonly string[]> = {
  background: ["background-image", "background-color"],
  "border-top": ["border-top-width", "border-top-style", "border-top-color"],
};

const expand = (name: string): readonly string[] => SHORTHANDS[name] ?? [name];

function sanitized(style: PendingStyle): string[] {
  sanitizeDeclarations(style as unknown as CSSStyleDeclaration, expand);
  return style.names().sort();
}

describe("unescapeCss", () => {
  it("resolves hex and character escapes as the engine does", () => {
    expect(unescapeCss("u\\72 l(/x)")).toBe("url(/x)");
    expect(unescapeCss("\\55 RL(/x)")).toBe("URL(/x)");
    expect(unescapeCss("ur\\l(/x)")).toBe("url(/x)");
    expect(unescapeCss("\\0 \\110000 \\d800")).toBe("\uFFFD\uFFFD\uFFFD");
    expect(unescapeCss("plain")).toBe("plain");
  });
});

describe("cssValueAllowed", () => {
  it("sees a load that escapes spell inside a substituted value", () => {
    for (const [property, value] of [
      ["background-image", "var(--n, u\\72 l(/api/mux/state))"],
      ["background-image", "if(media(width > 0): u\\72 l(/api); else: none)"],
      ["border-image", "var(--n, url(/api)) 30 round"],
      ["background", "var(--n, image-set(url(/x.png) 1x))"],
      ["content", "var(--n, u\\72 l(/x))"],
    ]) {
      expect(cssValueAllowed(property, value), `${property}: ${value}`).toBe(false);
    }
  });

  it("keeps cursor keywords and no cursor image", () => {
    expect(cssValueAllowed("cursor", "pointer")).toBe(true);
    expect(cssValueAllowed("cursor", "url(\"data:image/png;base64,AAAA\") 4 4, auto")).toBe(false);
    expect(cssValueAllowed("cursor", "image-set(\"data:image/png;base64,AAAA\" 1x), auto")).toBe(false);
  });

  it("keeps substituted values that load nothing", () => {
    expect(cssValueAllowed("border", "1px solid var(--line)")).toBe(true);
    expect(cssValueAllowed("background", "var(--bg, #fff)")).toBe(true);
    expect(cssValueAllowed("background", "var(--bg, url(\"data:image/png;base64,AAAA\"))")).toBe(true);
  });
});

describe("declaredNames", () => {
  it("splits a serialized block at its top-level semicolons", () => {
    expect(declaredNames("color: red; background: var(--n, url(\"a;b\")); --x: { a: b; }; content: \"x;y\";"))
      .toEqual(["color", "background", "--x", "content"]);
    expect(declaredNames("")).toEqual([]);
  });
});

describe("sanitizeDeclarations", () => {
  it("checks a shorthand holding var() by its own text", () => {
    const style = new PendingStyle(
      { "background-image": "", "background-color": "", "border-top-width": "", "border-top-style": "", "border-top-color": "", color: "red" },
      {
        background: { text: "var(--n, url(/api/mux/state))", longhands: SHORTHANDS.background },
        "border-top": { text: "1px solid var(--line)", longhands: SHORTHANDS["border-top"] },
      },
    );
    expect(sanitized(style)).toEqual(["border-top-color", "border-top-style", "border-top-width", "color"]);
  });

  it("drops a longhand whose value it cannot see", () => {
    // `background: var(--n, url(…)); background-color: red`: the shorthand no
    // longer serializes, and its other longhands read as "".
    const style = new PendingStyle(
      { "background-image": "", "background-color": "red" },
      { background: { text: "var(--n, url(/api))", longhands: SHORTHANDS.background } },
    );
    expect(sanitized(style)).toEqual(["background-color"]);
  });

  it("removes a plain declaration that loads", () => {
    const style = new PendingStyle({ "background-image": "url(\"/api/mux/state\")", color: "red" }, {});
    expect(sanitized(style)).toEqual(["color"]);
  });
});
