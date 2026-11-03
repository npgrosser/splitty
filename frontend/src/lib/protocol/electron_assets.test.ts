import { describe, expect, it } from "vitest";
import { assetIndex, assetType, resolveAsset, ASSET_HEADERS } from "../../../../frontend-electron/src/assets.ts";
const paths = ["/index.html", "/setup.html", "/popout.html", "/assets/main.js", "/assets/main.css", "/assets/terminal.wasm", "/font.woff2"];
const manifest = { format: 1, assets: paths.map(path => ({ path, type: assetType(path), bytes: 8, sha256: "a".repeat(64) })) };
const index = assetIndex(manifest);
describe("packaged Electron resource surface", () => {
  it("serves only declared entry documents and assets with their actual content type", () => {
    for (const path of paths) expect(resolveAsset(`splitty://app${path}`, "GET", index)?.path).toBe(path);
    expect(resolveAsset("splitty://app/", "HEAD", index)?.path).toBe("/index.html");
    expect(resolveAsset("splitty://app/index.html?host=vm2#main", "GET", index)?.path).toBe("/index.html");
    expect(resolveAsset("splitty://app/assets/terminal.wasm", "GET", index)?.type).toBe("application/wasm");
  });
  it.each([
    "splitty://evil/index.html", "splitty://app.evil/index.html", "splitty://user@app/index.html", "splitty://app:80/index.html",
    "https://app/index.html", "file:///index.html", "splitty://app/../index.html", "splitty://app/assets/../index.html",
    "splitty://app/%2e%2e/index.html", "splitty://app/assets%2fmain.js", "splitty://app/assets%5cmain.js",
    "splitty://app/%252e%252e/index.html", "splitty://app/%xx", "splitty://app//index.html", "splitty://app/compiled/main.js",
    "splitty://app/manifest.json", "splitty://app/package.json", "splitty://app/assets/", "splitty://app/missing.html",
    "splitty://app/other-route", "splitty://app/assets\\main.js", "splitty://app/index.html\n",
  ])("rejects %s without fallback", url => expect(resolveAsset(url, "GET", index)).toBeUndefined());
  it("rejects mutations and unsafe or duplicate build entries", () => {
    expect(resolveAsset("splitty://app/index.html", "POST", index)).toBeUndefined();
    for (const path of ["/../../private.js", "/.env", "/assets/arbitrary.html", "/main.js.map"]) expect(assetType(path)).toBeUndefined();
    expect(() => assetIndex({ ...manifest, assets: [...manifest.assets, manifest.assets[0]] })).toThrow();
    expect(() => assetIndex({ ...manifest, assets: manifest.assets.slice(1) })).toThrow();
  });
  it("keeps network scripts, generic loopback and frames outside CSP", () => {
    const csp = ASSET_HEADERS["Content-Security-Policy"];
    expect(csp).toContain("script-src 'self' 'wasm-unsafe-eval'");
    expect(csp).toContain("connect-src 'self';"); expect(csp).toContain("frame-src 'none'");
    expect(csp).not.toMatch(/https?:|\*/);
  });
});

it("the installed handler reads packaged bytes only and refuses missing or changed content", async () => {
  const { mkdtempSync, mkdirSync, writeFileSync, rmSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const { createHash } = await import("node:crypto");
  const { installAssets } = await import("../../../../frontend-electron/src/asset_protocol.ts");
  const root = mkdtempSync(join(tmpdir(), "splitty-assets-"));
  try {
    mkdirSync(join(root, "files", "assets"), { recursive: true });
    const body = new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0]);
    const assets = paths.map(path => ({ path, type: assetType(path), bytes: body.length, sha256: createHash("sha256").update(body).digest("hex") }));
    writeFileSync(join(root, "manifest.json"), JSON.stringify({ format: 1, assets }));
    for (const asset of assets) writeFileSync(join(root, "files", asset.path.slice(1)), body);
    let handler!: (request: Request) => Response;
    const session = { protocol: { handle: (_scheme: string, callback: typeof handler) => { handler = callback; } } };
    installAssets(session as unknown as Parameters<typeof installAssets>[0], root);
    const reply = handler(new Request("splitty://app/assets/terminal.wasm"));
    expect(reply.status).toBe(200); expect(reply.headers.get("content-type")).toBe("application/wasm");
    expect(new Uint8Array(await reply.arrayBuffer())).toEqual(body);
    expect(handler(new Request("splitty://app/manifest.json")).status).toBe(404);
    expect(handler(new Request("splitty://app/missing")).status).toBe(404);
    writeFileSync(join(root, "files", "assets", "terminal.wasm"), new Uint8Array(8));
    expect(handler(new Request("splitty://app/assets/terminal.wasm")).status).toBe(404);
    rmSync(join(root, "files", "setup.html"));
    expect(handler(new Request("splitty://app/setup.html")).status).toBe(404);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
