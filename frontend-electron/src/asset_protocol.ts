import { readFileSync, statSync } from "node:fs";
import { createHash } from "node:crypto";
import path from "node:path";
import type { Session } from "electron";
import { assetIndex, resolveAsset, ASSET_HEADERS } from "./assets";

/** Each isolated Electron session needs its own handler. No net.fetch/file URL
 * bridge is used: only manifest entries can reach this filesystem operation. */
export function installAssets(session: Session, root: string): void {
  const manifestPath = path.join(root, "manifest.json");
  if (statSync(manifestPath).size > 1024 * 1024) throw new Error("Packaged asset manifest exceeds its limit");
  const index = assetIndex(JSON.parse(readFileSync(manifestPath, "utf8")));
  session.protocol.handle("splitty", request => {
    const asset = resolveAsset(request.url, request.method, index);
    if (!asset) return new Response(null, { status: 404, headers: ASSET_HEADERS });
    try {
      const filename = path.join(root, "files", asset.path.slice(1));
      if (statSync(filename).size !== asset.bytes) throw new Error("Packaged asset size mismatch");
      const bytes = readFileSync(filename);
      if (createHash("sha256").update(bytes).digest("hex") !== asset.sha256) throw new Error("Packaged asset digest mismatch");
      return new Response(request.method === "HEAD" ? null : bytes, {
        headers: { ...ASSET_HEADERS, "Content-Type": asset.type, "Content-Length": String(asset.bytes) },
      });
    } catch {
      return new Response(null, { status: 404, headers: ASSET_HEADERS });
    }
  });
}
