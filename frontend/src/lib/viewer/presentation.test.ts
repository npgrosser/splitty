import { describe, expect, it } from "vitest";
import { RENDER_LIMIT_BYTES } from "./limits.ts";
import { BINARY_PROBE_BYTES, decodeText, directoryOf, fileName, imageDataUrl, imageType, presentationOf } from "./presentation.ts";

const encoder = new TextEncoder();
const file = (path: string) => ({ kind: "file" as const, path });
const text = (content: string) => encoder.encode(content);

const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13]);
const JPEG = new Uint8Array([0xff, 0xd8, 0xff, 0xe0, 0, 0x10]);
const GIF = text("GIF89a\u0001\u0000");
const WEBP = new Uint8Array([0x52, 0x49, 0x46, 0x46, 1, 0, 0, 0, 0x57, 0x45, 0x42, 0x50]);
const SVG = text(`<?xml version="1.0"?>\n<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"></svg>`);

// viewer-panes, Showing A Document.
describe("presentationOf", () => {
  it("renders a Markdown plan and offers its source", () => {
    // A Markdown plan.
    expect(presentationOf(file("/home/me/app/PLAN.md"), text("# Plan\n"))).toEqual({ kind: "rendered", format: "markdown" });
    for (const name of ["notes.markdown", "NOTES.MDOWN"]) {
      expect(presentationOf(file(`/home/me/${name}`), text("x"))).toEqual({ kind: "rendered", format: "markdown" });
    }
    for (const name of ["report.html", "REPORT.HTM"]) {
      expect(presentationOf(file(`/home/me/${name}`), text("<p>x</p>"))).toEqual({ kind: "rendered", format: "html" });
    }
  });

  it("shows a document over 2 MiB as source", () => {
    const big = new Uint8Array(RENDER_LIMIT_BYTES + 1).fill(0x61);
    expect(presentationOf(file("/home/me/PLAN.md"), big)).toEqual({ kind: "source", fileName: "PLAN.md" });
    expect(presentationOf(file("/home/me/PLAN.md"), big.subarray(0, RENDER_LIMIT_BYTES))).toEqual({ kind: "rendered", format: "markdown" });
  });

  it("shows images named and made so", () => {
    expect(presentationOf(file("/home/me/a.png"), PNG)).toEqual({ kind: "image", type: "image/png" });
    expect(presentationOf(file("/home/me/a.JPG"), JPEG)).toEqual({ kind: "image", type: "image/jpeg" });
    expect(presentationOf(file("/home/me/a.jpeg"), JPEG)).toEqual({ kind: "image", type: "image/jpeg" });
    expect(presentationOf(file("/home/me/a.gif"), GIF)).toEqual({ kind: "image", type: "image/gif" });
    expect(presentationOf(file("/home/me/a.webp"), WEBP)).toEqual({ kind: "image", type: "image/webp" });
    expect(presentationOf(file("/home/me/a.svg"), SVG)).toEqual({ kind: "image", type: "image/svg+xml" });
    // The type is the content's, also under another image name.
    expect(presentationOf(file("/home/me/a.jpg"), PNG)).toEqual({ kind: "image", type: "image/png" });
  });

  it("does not take a file for an image by its name alone", () => {
    expect(presentationOf(file("/home/me/a.png"), text("not an image"))).toEqual({ kind: "source", fileName: "a.png" });
    // SVG markup under a raster name is not shown as an image, and the other way round.
    expect(presentationOf(file("/home/me/a.png"), SVG)).toEqual({ kind: "source", fileName: "a.png" });
    expect(presentationOf(file("/home/me/a.svg"), PNG)).toEqual({ kind: "binary", size: PNG.length });
    // An image's bytes under another name are a binary file.
    expect(presentationOf(file("/home/me/a.dat"), PNG)).toEqual({ kind: "binary", size: PNG.length });
  });

  it("shows a binary file as its size", () => {
    // A binary file.
    const program = new Uint8Array(20_000).fill(0x41);
    program.set([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1, 0]);
    expect(presentationOf(file("/home/me/app/target/debug/app"), program)).toEqual({ kind: "binary", size: 20_000 });
  });

  it("looks for NUL bytes only in the first 8 KiB", () => {
    const late = new Uint8Array(BINARY_PROBE_BYTES + 10).fill(0x61);
    late[BINARY_PROBE_BYTES + 1] = 0;
    expect(presentationOf(file("/home/me/log.txt"), late)).toEqual({ kind: "source", fileName: "log.txt" });
    late[BINARY_PROBE_BYTES - 1] = 0;
    expect(presentationOf(file("/home/me/log.txt"), late)).toEqual({ kind: "binary", size: late.length });
  });

  it("shows other text as source by its name", () => {
    expect(presentationOf(file("/home/me/app/src/main.rs"), text("fn main() {}\n"))).toEqual({ kind: "source", fileName: "main.rs" });
    expect(presentationOf(file("/home/me/Makefile"), text("all:\n"))).toEqual({ kind: "source", fileName: "Makefile" });
    expect(presentationOf(file("/home/me/empty.txt"), new Uint8Array())).toEqual({ kind: "source", fileName: "empty.txt" });
  });

  it("shows a text document as plain text, whatever it holds", () => {
    expect(presentationOf({ kind: "text", title: "Selection from claude" }, text("# Not rendered\n"))).toEqual({ kind: "source" });
    expect(presentationOf({ kind: "text", title: "PLAN.md" }, text("# Not rendered\n"))).toEqual({ kind: "source" });
  });

  // viewer-panes, Showing A Document (A plan in Markdown).
  it("renders a text document whose format is Markdown", () => {
    expect(presentationOf({ kind: "text", title: "Plan", format: "markdown" }, text("# Plan\n"))).toEqual({ kind: "rendered", format: "markdown" });
  });
});

describe("helpers", () => {
  it("names a file and its directory", () => {
    expect(fileName("/home/me/app/PLAN.md")).toBe("PLAN.md");
    expect(directoryOf("/home/me/app/PLAN.md")).toBe("/home/me/app");
    expect(directoryOf("/PLAN.md")).toBe("/");
  });

  it("recognizes images by their content", () => {
    expect(imageType(PNG)).toBe("image/png");
    expect(imageType(text("<svg>"))).toBe("image/svg+xml");
    expect(imageType(text("<svgx>"))).toBeUndefined();
    expect(imageType(new Uint8Array())).toBeUndefined();
  });

  it("decodes UTF-8 with replacement characters", () => {
    expect(decodeText(new Uint8Array([0x61, 0xff, 0x62]))).toBe("a�b");
    expect(decodeText(text("﻿plan"))).toBe("plan");
  });

  it("makes data URLs of images", () => {
    expect(imageDataUrl("image/png", new Uint8Array([1, 2, 3]))).toBe("data:image/png;base64,AQID");
  });
});
