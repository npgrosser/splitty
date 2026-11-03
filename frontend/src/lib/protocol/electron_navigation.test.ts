import { expect, it } from "vitest";
import { isPackagedDocument } from "../../../../frontend-electron/src/navigation.ts";

it("keeps main/remote documents on their assigned entry and popouts on theirs", () => {
  for (const role of ["main", "remote"] as const) {
    expect(isPackagedDocument(role, "splitty://app/index.html?host=forged#view")).toBe(true);
    expect(isPackagedDocument(role, "splitty://app/")).toBe(true);
    expect(isPackagedDocument(role, "splitty://app/popout.html")).toBe(false);
  }
  expect(isPackagedDocument("popout", "splitty://app/popout.html#dockview")).toBe(true);
  expect(isPackagedDocument("popout", "splitty://app/index.html")).toBe(false);
});

it.each([
  "splitty://app/setup.html", "splitty://app/assets/main.js", "splitty://user@app/index.html",
  "splitty://app:1/index.html", "splitty://other/index.html", "splitty://app//index.html",
  "splitty://app/assets/../index.html", "splitty://app/%69ndex.html", "splitty://app/index.html\n",
  "https://example.com/", "file:///tmp/index.html", "about:blank", "javascript:alert(1)",
])("rejects other documents and non-canonical targets: %s", url => {
  for (const role of ["main", "remote", "popout"] as const) expect(isPackagedDocument(role, url)).toBe(false);
});
