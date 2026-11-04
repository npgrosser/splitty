/** Exact packaged resource surface. Paths are looked up, never resolved from a
 * request URL. The manifest is emitted by the desktop build from frontend/dist. */
export const APP_ORIGIN = "splitty://app";
export const ASSET_HEADERS = {
  "Content-Security-Policy": "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; frame-src 'none'; frame-ancestors 'none'; object-src 'none'; base-uri 'none'; form-action 'none'",
  "X-Content-Type-Options": "nosniff",
  "Referrer-Policy": "no-referrer",
  "Permissions-Policy": "camera=(), microphone=(), geolocation=(), clipboard-read=()",
  "Cross-Origin-Opener-Policy": "same-origin",
  "Cross-Origin-Resource-Policy": "same-origin",
} as const;
export interface Asset { path: string; type: string; bytes: number; sha256: string }
export interface AssetManifest { format: 1; assets: Asset[] }
export const MIME: Readonly<Record<string, string>> = {
  html: "text/html; charset=utf-8", js: "text/javascript; charset=utf-8", css: "text/css; charset=utf-8",
  json: "application/json", webmanifest: "application/manifest+json", wasm: "application/wasm",
  png: "image/png", ico: "image/x-icon", svg: "image/svg+xml", jpg: "image/jpeg", jpeg: "image/jpeg", webp: "image/webp",
  ttf: "font/ttf", otf: "font/otf", woff: "font/woff", woff2: "font/woff2",
};
export function assetType(path: string): string | undefined {
  if (!/^\/(?:[A-Za-z0-9_-][A-Za-z0-9_.-]*\/)*[A-Za-z0-9_-][A-Za-z0-9_.-]*$/.test(path)) return undefined;
  const extension = path.slice(path.lastIndexOf(".") + 1);
  if (extension === "html" && path !== "/index.html" && path !== "/setup.html" && path !== "/popout.html") return undefined;
  return Object.hasOwn(MIME, extension) ? MIME[extension] : undefined;
}
export function assetIndex(raw: unknown): ReadonlyMap<string, Readonly<Asset>> {
  if (!raw || typeof raw !== "object") throw new Error("Missing packaged asset manifest");
  const manifest = raw as AssetManifest;
  if (manifest.format !== 1 || !Array.isArray(manifest.assets) || manifest.assets.length > 4096) throw new Error("Invalid packaged asset manifest");
  const index = new Map<string, Readonly<Asset>>();
  for (const asset of manifest.assets) {
    if (!asset || typeof asset.path !== "string" || assetType(asset.path) !== asset.type || !asset.type
      || !Number.isSafeInteger(asset.bytes) || asset.bytes < 0 || asset.bytes > 32 * 1024 * 1024
      || typeof asset.sha256 !== "string" || !/^[a-f0-9]{64}$/.test(asset.sha256) || index.has(asset.path)) throw new Error("Invalid packaged asset entry");
    index.set(asset.path, Object.freeze({ ...asset }));
  }
  if (!index.has("/index.html") || !index.has("/setup.html") || !index.has("/popout.html")) throw new Error("Missing packaged entry document");
  return index;
}
/** Validate the raw path before WHATWG URL normalization can erase traversal. */
export function resolveAsset(url: string, method: string, index: ReadonlyMap<string, Readonly<Asset>>): Readonly<Asset> | undefined {
  if (method !== "GET" && method !== "HEAD") return undefined;
  const match = /^splitty:\/\/app(\/[^?#]*)?(?:[?#].*)?$/.exec(url);
  if (!match) return undefined;
  const path = match[1] || "/";
  // Build output uses this ASCII vocabulary: no decoding, alternate separator,
  // authority, directory listing or SPA fallback can add another resource.
  if (path === "/") return index.get("/index.html");
  if (!assetType(path)) return undefined;
  return index.get(path);
}
