import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fakeServer, fakeEmulator } from "./testing/fake_server.ts";
import { TerminalSession } from "./terminal/terminal_session.ts";
import { downloadName, MAX_OFFER_BYTES, READ_TIMEOUT_MS, watchFileOffers, type RevealDownload } from "./file_offers.ts";
import { toBase64 } from "./protocol/base64.ts";

const state = vi.hoisted(() => ({ remote: false }));
type ToastOptions = {
  description?: string;
  duration?: number;
  action?: { label: string; onClick: () => void };
  cancel?: { onClick: () => void };
  onDismiss?: () => void;
  onAutoClose?: () => void;
};
const notice = vi.hoisted(() => {
  let next = 0;
  return {
    show: vi.fn((_title: string, _options?: unknown) => `toast-${++next}`),
    loading: vi.fn((_title: string, options?: { id?: string }) => options?.id ?? `toast-${++next}`),
    success: vi.fn(),
    error: vi.fn(),
    dismiss: vi.fn(),
  };
});
vi.mock("svelte-sonner", () => ({
  toast: Object.assign(notice.show, { loading: notice.loading, success: notice.success, error: notice.error, dismiss: notice.dismiss }),
}));
vi.mock("./platform.ts", () => ({ get isRemoteSession() { return state.remote; }, shellKind: "web" }));

const cleanups: (() => void)[] = [];
const id = "b".repeat(32);
const savedUrl = "blob:http://127.0.0.1:45299/0d1c6f2e-6a4b-4f63-9d8e-2b7a1c3e5f90";

function fixture(key: "local" | `host:${string}` = "host:vm1", capabilities = ["file-offer-v1"], reveal: RevealDownload | null = null) {
  const server = fakeServer(key);
  const session = new TerminalSession(server.conn.sessions, "session", fakeEmulator());
  session.wire();
  const save = vi.fn((_name: string, _blob: Blob) => savedUrl);
  const cleanup = watchFileOffers(server.conn, save, reveal);
  cleanups.push(cleanup, () => server.conn.dispose());
  server.socket.message({ type: "mux.welcome", capabilities });
  return { ...server, session, save, cleanup };
}
function offer(overrides: Record<string, unknown> = {}) {
  return { type: "file.offer", offerId: id, sessionId: "session", name: "report.pdf", size: 5, expiresAt: Date.now() + 60_000, ...overrides };
}
/** The options of the latest prompt. */
function prompt(): ToastOptions { return notice.show.mock.calls.at(-1)![1] as ToastOptions; }
function chunk(offset: number, text: string, eof: boolean, offerId = id) {
  return { type: "file.chunk", offerId, offset, data: toBase64(new TextEncoder().encode(text)), eof };
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date", "performance", "setTimeout", "clearTimeout"] });
  vi.clearAllMocks();
  state.remote = false;
});
afterEach(() => {
  for (const cleanup of cleanups.splice(0)) cleanup();
  vi.useRealTimers();
});

describe("file offers", () => {
  it("registers only with a server that announces file-offer-v1", () => {
    const server = fixture();
    expect(server.socket.sentOfType("file.register")).toHaveLength(1);
    const old = fixture("local", []);
    expect(old.socket.sentOfType("file.register")).toHaveLength(0);
  });

  it("takes no offers in a remote window", () => {
    state.remote = true;
    const server = fixture();
    expect(server.socket.sent).toEqual([]);
  });

  it("Save from a host: reads the file from that host's connection only and saves it as a download", async () => {
    const local = fixture("local");
    const host = fixture("host:vm1");
    host.socket.message(offer({ size: 11 }));
    expect(notice.show).toHaveBeenCalledOnce();
    expect(notice.show.mock.calls[0][0]).toBe("Save report.pdf?");
    expect(prompt().description).toBe("11 B from vm1");
    expect(host.socket.sentOfType("file.read")).toEqual([]);
    prompt().action!.onClick();
    prompt().onDismiss!(); // The prompt goes after Save; that is no decline.
    expect(host.socket.sentOfType("file.accept")).toEqual([{ type: "file.accept", offerId: id }]);
    expect(host.socket.sentOfType("file.decline")).toEqual([]);
    expect(host.socket.sentOfType("file.read")).toEqual([{ type: "file.read", offerId: id, offset: 0 }]);
    local.socket.message(chunk(0, "hello world", true));
    expect(host.save).not.toHaveBeenCalled();
    host.socket.message(chunk(0, "hello ", false));
    expect(host.socket.sentOfType("file.read").at(-1)).toEqual({ type: "file.read", offerId: id, offset: 6 });
    host.socket.message(chunk(6, "world", true));
    expect(host.save).toHaveBeenCalledOnce();
    const [name, blob] = host.save.mock.calls[0] as [string, Blob];
    expect(name).toBe("report.pdf");
    expect(await blob.text()).toBe("hello world");
    expect(notice.success).toHaveBeenCalledOnce();
    expect((notice.success.mock.calls[0] as unknown[])[1]).not.toHaveProperty("action"); // A browser shows its own downloads.
    expect(local.socket.sentOfType("file.accept")).toEqual([]);
  });

  it("Show the saved file: asks the desktop shell to show exactly the download it saved", async () => {
    const reveal = vi.fn((_url: string) => Promise.resolve());
    const server = fixture("local", ["file-offer-v1"], reveal);
    server.socket.message(offer());
    prompt().action!.onClick();
    server.socket.message(chunk(0, "hello", true));
    const [title, options] = notice.success.mock.calls[0] as unknown as [string, ToastOptions];
    expect(title).toBe("Saved report.pdf");
    expect(options.action!.label).toBe("Show in folder");
    expect(options.duration).toBe(10_000);
    expect(reveal).not.toHaveBeenCalled();
    options.action!.onClick();
    expect(reveal).toHaveBeenCalledExactlyOnceWith(savedUrl);
    await Promise.resolve();
    expect(notice.error).not.toHaveBeenCalled();
  });

  it("says so when the desktop shell cannot show the saved file", async () => {
    const server = fixture("local", ["file-offer-v1"], () => Promise.reject(new Error("no finished download of this URL")));
    server.socket.message(offer());
    prompt().action!.onClick();
    server.socket.message(chunk(0, "hello", true));
    (notice.success.mock.calls[0] as unknown as [string, ToastOptions])[1].action!.onClick();
    await vi.waitFor(() => expect(notice.error).toHaveBeenCalledOnce());
    expect((notice.error.mock.calls[0] as unknown[])[0]).toBe("Could not show report.pdf");
  });

  it("Dismiss: declines and reads nothing", () => {
    const server = fixture("local");
    server.socket.message(offer());
    expect(prompt().description).toBe("5 B from this computer");
    prompt().cancel!.onClick();
    prompt().onDismiss!();
    expect(server.socket.sentOfType("file.decline")).toEqual([{ type: "file.decline", offerId: id }]);
    expect(server.socket.sentOfType("file.accept")).toEqual([]);
    expect(server.socket.sentOfType("file.read")).toEqual([]);
  });

  it("Two windows show the session: drops the prompt when another window took the offer", () => {
    const server = fixture();
    server.socket.message(offer());
    const shown = notice.show.mock.results[0].value;
    server.socket.message({ type: "file.closed", offerId: id, reason: "taken" });
    expect(notice.dismiss).toHaveBeenCalledWith(shown);
    prompt().onDismiss!();
    prompt().action!.onClick();
    expect(server.socket.sentOfType("file.decline")).toEqual([]);
    expect(server.socket.sentOfType("file.accept")).toEqual([]);
  });

  it("Offer without Save: reads nothing, and the prompt goes when the offer expires", () => {
    const server = fixture();
    server.socket.message(offer({ expiresAt: Date.now() + 10_000 }));
    vi.advanceTimersByTime(10_000);
    expect(notice.dismiss).toHaveBeenCalled();
    expect(server.socket.sentOfType("file.accept")).toEqual([]);
    expect(server.socket.sentOfType("file.read")).toEqual([]);
  });

  it("declines malformed offers, offers of sessions it does not show and offers beyond three", () => {
    const server = fixture();
    const bad = [
      offer({ offerId: "c".repeat(32), name: 42 }),
      offer({ offerId: "d".repeat(32), name: "" }),
      offer({ offerId: "e".repeat(32), size: MAX_OFFER_BYTES + 1 }),
      offer({ offerId: "f".repeat(32), size: -1 }),
      offer({ offerId: "1".repeat(32), sessionId: "other" }),
      offer({ offerId: "2".repeat(32), expiresAt: Date.now() - 1 }),
      offer({ offerId: "3".repeat(32), expiresAt: "soon" }),
    ];
    for (const msg of bad) server.socket.message(msg);
    expect(notice.show).not.toHaveBeenCalled();
    expect(server.socket.sentOfType("file.decline")).toHaveLength(bad.length);
    server.socket.message(offer({ offerId: "not hex" }));
    expect(server.socket.sentOfType("file.decline")).toHaveLength(bad.length);
    for (const n of ["4", "5", "6", "7"]) server.socket.message(offer({ offerId: n.repeat(32) }));
    expect(notice.show).toHaveBeenCalledTimes(3);
    expect(server.socket.sentOfType("file.decline").at(-1)).toEqual({ type: "file.decline", offerId: "7".repeat(32) });
    server.socket.message(offer({ offerId: "4".repeat(32) }));
    expect(notice.show).toHaveBeenCalledTimes(3);
  });

  it("names the download after the offer's last path component, as plain text", () => {
    expect(downloadName("../../.ssh/id_ed25519")).toBe("id_ed25519");
    expect(downloadName("C:\\Users\\x\\a?.txt")).toBe("a_.txt");
    expect(downloadName("evil\u202Etxt.exe")).toBe("eviltxt.exe");
    expect(downloadName("..")).toBeNull();
    expect(downloadName("a".repeat(256))).toBeNull();
  });

  it("stops a download whose chunks do not fit the offer, and one the server ends", () => {
    const server = fixture();
    server.socket.message(offer({ size: 5 }));
    prompt().action!.onClick();
    server.socket.message(chunk(3, "late", false));
    expect(server.socket.sentOfType("file.read")).toHaveLength(1);
    server.socket.message(chunk(0, "too long!", false));
    expect(notice.error).toHaveBeenCalledOnce();
    expect(server.save).not.toHaveBeenCalled();

    server.socket.message(offer({ offerId: "9".repeat(32), size: 5 }));
    prompt().action!.onClick();
    server.socket.message(chunk(0, "abc", true, "9".repeat(32)));
    expect(notice.error).toHaveBeenCalledTimes(2);

    server.socket.message(offer({ offerId: "8".repeat(32), size: 5 }));
    prompt().action!.onClick();
    server.socket.message({ type: "file.closed", offerId: "8".repeat(32), reason: "disconnected" });
    expect(notice.error).toHaveBeenCalledTimes(3);
    expect(server.save).not.toHaveBeenCalled();
  });

  it("gives up on a server that stops sending, and on a lost connection", () => {
    const server = fixture();
    server.socket.message(offer());
    prompt().action!.onClick();
    vi.advanceTimersByTime(READ_TIMEOUT_MS);
    expect(notice.error).toHaveBeenCalledOnce();

    server.socket.message(offer({ offerId: "a".repeat(32) }));
    prompt().action!.onClick();
    server.socket.drop();
    expect(notice.error).toHaveBeenCalledTimes(2);
    server.socket.message(chunk(0, "hello", true, "a".repeat(32)));
    expect(server.save).not.toHaveBeenCalled();
  });

  it("Dismiss during a download: cancels the transfer on the server and saves nothing", () => {
    const server = fixture();
    server.socket.message(offer({ size: 11 }));
    prompt().action!.onClick();
    server.socket.message(chunk(0, "hello ", false));
    const progress = notice.loading.mock.calls.at(-1)![1] as ToastOptions;
    progress.cancel!.onClick();
    expect(server.socket.sentOfType("file.decline")).toEqual([{ type: "file.decline", offerId: id }]);
    server.socket.message(chunk(6, "world", true));
    expect(server.save).not.toHaveBeenCalled();
    expect(server.socket.sentOfType("file.read")).toHaveLength(2);
  });

  it("tells the server when it gives up on a download, but not when the server ended it", () => {
    const server = fixture();
    server.socket.message(offer({ size: 5 }));
    prompt().action!.onClick();
    server.socket.message(chunk(0, "too long!", false));
    expect(server.socket.sentOfType("file.decline")).toEqual([{ type: "file.decline", offerId: id }]);

    server.socket.message(offer({ offerId: "8".repeat(32), size: 5 }));
    prompt().action!.onClick();
    server.socket.message({ type: "file.closed", offerId: "8".repeat(32), reason: "disconnected" });
    expect(server.socket.sentOfType("file.decline")).toHaveLength(1);
  });

  it("saves an empty file from one empty last chunk", () => {
    const server = fixture();
    server.socket.message(offer({ size: 0, name: "empty.txt" }));
    prompt().action!.onClick();
    server.socket.message(chunk(0, "", true));
    expect(server.save).toHaveBeenCalledOnce();
  });
});
