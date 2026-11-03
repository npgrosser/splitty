/** Finished downloads belong to the native window/document that saved them.
 * The renderer supplies only its blob URL; native paths never cross the bridge. */
const KEEP = 32;
const saved = new WeakMap<object, { document: string; files: Map<string, string> }>();

export function isPackagedBlob(url: unknown): url is string {
  return typeof url === "string" && /^blob:splitty:\/\/app\/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(url);
}
export function rememberDownload(owner: object, document: string, url: string, path: string): void {
  if (!isPackagedBlob(url) || !path) return;
  let current = saved.get(owner);
  if (!current || current.document !== document) {
    current = { document, files: new Map() }; saved.set(owner, current);
  }
  current.files.delete(url); current.files.set(url, path);
  if (current.files.size > KEEP) current.files.delete(current.files.keys().next().value!);
}
export function savedDownload(owner: object, document: string, url: unknown): string | undefined {
  const current = saved.get(owner);
  return current?.document === document && isPackagedBlob(url) ? current.files.get(url) : undefined;
}
