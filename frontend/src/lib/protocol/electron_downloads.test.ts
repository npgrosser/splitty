import { expect, it } from "vitest";
import { isPackagedBlob, rememberDownload, savedDownload } from "../../../../frontend-electron/src/downloads.ts";
const url = "blob:splitty://app/00000000-0000-0000-0000-000000000001";
it("binds a saved path to the actual document/window and never accepts a supplied path", () => {
  const a = {}, b = {};
  rememberDownload(a, "document-a", url, "/tmp/saved");
  expect(savedDownload(a, "document-a", url)).toBe("/tmp/saved");
  expect(savedDownload(b, "document-a", url)).toBeUndefined();
  expect(savedDownload(a, "document-b", url)).toBeUndefined();
  expect(savedDownload(a, "document-a", "/tmp/saved")).toBeUndefined();
  rememberDownload(a, "document-b", url, "/tmp/new-document");
  expect(savedDownload(a, "document-a", url)).toBeUndefined();
});
it("allows only the packaged application's exact blob origin", () => {
  expect(isPackagedBlob(url)).toBe(true);
  for (const input of ["file:///tmp/x", "blob:http://127.0.0.1/00000000-0000-0000-0000-000000000001", url + "?x", url.replace("app/", "app:80/"), url.replace("app/", "other/"), url.replace("app/", "user@app/"), "blob:null/id"]) {
    expect(isPackagedBlob(input)).toBe(false);
  }
});
it("retains only the latest 32 finished downloads of a document", () => {
  const owner = {};
  for (let i = 0; i < 33; i++) rememberDownload(owner, "document", url.slice(0, -12) + i.toString(16).padStart(12, "0"), `/tmp/saved-${i}`);
  expect(savedDownload(owner, "document", url.slice(0, -12) + "000000000000")).toBeUndefined();
  expect(savedDownload(owner, "document", url)).toBe("/tmp/saved-1");
});
