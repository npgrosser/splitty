import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: vi.fn() }));
const notice = vi.hoisted(() => ({ error: vi.fn() }));
vi.mock("svelte-sonner", () => ({ toast: notice }));

async function browser(globals: Record<string, unknown> = {}) {
  vi.resetModules();
  const popup = {
    opener: {} as unknown,
    closed: false,
    close: vi.fn(),
    location: { replace: vi.fn() },
    document: { createElement: vi.fn(() => ({ name: "", content: "" })), head: { append: vi.fn() }, title: "", body: { textContent: "" } },
  };
  const open = vi.fn(() => popup);
  vi.stubGlobal("window", { location: { search: "", origin: "http://127.0.0.1:9800" }, matchMedia: () => ({ matches: false }), open, ...globals });
  return { popup, open, ...(await import("./window.ts")) };
}

afterEach(() => vi.unstubAllGlobals());

describe("reserved external browser tabs", () => {
  it("reserves synchronously, cuts the opener and sends no service navigation until verified", async () => {
    const { popup, open, reserveExternalOpen } = await browser();
    const reserved = reserveExternalOpen()!;
    expect(open).toHaveBeenCalledWith("", "_blank");
    expect(popup.opener).toBeNull();
    expect(popup.document.head.append).toHaveBeenCalledWith({ name: "referrer", content: "no-referrer" });
    expect(popup.location.replace).not.toHaveBeenCalled();
    expect(reserved.open("https://localhost:15173/app?x=1#top")).toBe(true);
    expect(popup.location.replace).toHaveBeenCalledWith("https://localhost:15173/app?x=1#top");
    reserved.close();
    expect(popup.close).not.toHaveBeenCalled();
  });

  it("closes failed/cancelled reservations idempotently and refuses later navigation", async () => {
    const { popup, reserveExternalOpen } = await browser();
    const reserved = reserveExternalOpen()!;
    reserved.close();
    reserved.close();
    expect(popup.close).toHaveBeenCalledTimes(1);
    expect(reserved.open("http://localhost:15173/")).toBe(false);
    expect(popup.location.replace).not.toHaveBeenCalled();
  });

  it.each(["javascript:alert(1)", "file:///tmp/example", "data:text/html,hello", "vscode://vscode-remote/ssh-remote+vm1/srv/", "zed://file/srv/", "zed://ssh/vm1/srv/"])("rejects %s even after reservation", async (url) => {
    const { popup, reserveExternalOpen } = await browser();
    expect(reserveExternalOpen()!.open(url)).toBe(false);
    expect(popup.close).toHaveBeenCalledTimes(1);
    expect(popup.location.replace).not.toHaveBeenCalled();
  });

  it("refuses editor links from hub remote panes through the production URL adapter", async () => {
    const { open, openExternalTarget } = await browser();
    const { PortForwarding } = await import("./remote/port_forwarding.ts");
    const fetch = vi.fn();
    const ports = new PortForwarding({ fetch, open: (url) => openExternalTarget({ url }),
      autoForward: () => false, notify: vi.fn(), changed: vi.fn() });
    ports.openLink("host:vm1", "vscode://vscode-remote/ssh-remote+vm1/srv/");
    expect(open).not.toHaveBeenCalled();
    expect(fetch).not.toHaveBeenCalled();
    ports.dispose();
  });

  it("allows blocked-popup fallback and never reserves a browser tab in a native shell", async () => {
    const blocked = await browser({ open: vi.fn(() => null) });
    expect(blocked.reserveExternalOpen()).toBeUndefined();
    const native = await browser({ __SPLITTY_ELECTRON__: {} });
    expect(native.reserveExternalOpen()).toBeUndefined();
    expect(native.open).not.toHaveBeenCalled();
  });

  it("returns to the notice fallback when the browser refuses the initial popup", async () => {
    const { reserveExternalOpen } = await browser({ open: vi.fn(() => { throw new Error("Blocked"); }) });
    expect(reserveExternalOpen()).toBeUndefined();
  });

  it("closes a blank tab if it cannot detach its opener, without navigating it", async () => {
    const { popup, reserveExternalOpen } = await browser();
    Object.defineProperty(popup, "opener", { set() { throw new Error("Denied"); } });
    expect(reserveExternalOpen()).toBeUndefined();
    expect(popup.close).toHaveBeenCalledOnce();
    expect(popup.location.replace).not.toHaveBeenCalled();
  });

  it("never navigates a tab the user already closed", async () => {
    const { popup, reserveExternalOpen } = await browser();
    const reserved = reserveExternalOpen()!;
    popup.closed = true;
    expect(reserved.open("http://localhost:15173")).toBe(false);
    expect(popup.location.replace).not.toHaveBeenCalled();
  });

  it("returns to the notice fallback if navigation fails, even when closing is refused", async () => {
    const { popup, reserveExternalOpen } = await browser();
    const reserved = reserveExternalOpen()!;
    popup.location.replace.mockImplementation(() => { throw new Error("Navigation refused"); });
    popup.close.mockImplementation(() => { throw new Error("Access revoked"); });
    expect(reserved.open("http://localhost:15173")).toBe(false);
    expect(reserved.open("http://localhost:15173")).toBe(false);
    expect(popup.location.replace).toHaveBeenCalledOnce();
  });

  it.each(["electron", "tauri"])("keeps the %s system-browser opener and never reserves a native popup", async (shell) => {
    const invoke = vi.fn(() => Promise.resolve());
    const bridge = shell === "electron" ? "__SPLITTY_ELECTRON__" : "__TAURI_INTERNALS__";
    const { open, reserveExternalOpen, openExternalTarget, tauriReady } = await browser({ [bridge]: { invoke } });
    await tauriReady;
    expect(reserveExternalOpen()).toBeUndefined();
    expect(openExternalTarget({ url: "https://localhost:15173/app?x=1#top" })).toBe(true);
    expect(invoke).toHaveBeenCalledWith(...(shell === "electron"
      ? ["shell:open-external", "https://localhost:15173/app?x=1#top"]
      : ["plugin:opener|open_url", { url: "https://localhost:15173/app?x=1#top" }]));
    expect(openExternalTarget({ url: "javascript:alert(1)" })).toBe(false);
    expect(openExternalTarget({ url: "vscode://vscode-remote/ssh-remote+vm1/srv/" })).toBe(false);
    expect(invoke).toHaveBeenCalledOnce();
    expect(open).not.toHaveBeenCalled();
  });

  it.each(["electron", "tauri"])("says when the %s opener refused a link instead of doing nothing", async (shell) => {
    notice.error.mockClear();
    const invoke = vi.fn(() => Promise.reject(new Error("Not allowed to open url")));
    const bridge = shell === "electron" ? "__SPLITTY_ELECTRON__" : "__TAURI_INTERNALS__";
    const { openExternalTarget, tauriReady } = await browser({ [bridge]: { invoke } });
    await tauriReady;
    vi.spyOn(console, "warn").mockImplementation(() => {});
    expect(openExternalTarget({ url: "https://example.com/" })).toBe(true);
    await vi.waitFor(() => expect(notice.error).toHaveBeenCalledOnce());
    expect(notice.error.mock.calls[0][1].description).toContain("Not allowed to open url");
  });
});
