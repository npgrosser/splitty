/** A role is assigned natively to a WebContents. Sharing the asset origin does
 * not allow it to turn into setup, another entry point or an executable asset. */
export function isPackagedDocument(role: "main" | "remote" | "popout", target: string): boolean {
  if (/[\u0000-\u0020\u007f]/.test(target)) return false;
  // Match the serialized URL too: URL parsing would normalize path traversal
  // and credentials before a pathname-only check could reject them.
  return role === "popout"
    ? /^splitty:\/\/app\/popout\.html(?:[?#].*)?$/.test(target)
    : /^splitty:\/\/app\/(?:index\.html)?(?:[?#].*)?$/.test(target);
}
