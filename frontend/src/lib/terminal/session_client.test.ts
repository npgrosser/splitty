import { describe, expect, it, vi } from "vitest";
import { fakeEmulator, fakeServer } from "../testing/fake_server.ts";
import { TerminalSession } from "./terminal_session.ts";
import { MAX_TITLE } from "../protocol/untrusted.ts";

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

function wiredSession(conn: ReturnType<typeof fakeServer>["conn"], sessionId: string) {
  const emulator = fakeEmulator();
  const session = new TerminalSession(conn.sessions, sessionId, emulator);
  session.wire();
  return { session, emulator };
}

describe("SessionClient", () => {
  it("sends one create at a time, in request order", async () => {
    const { socket, conn } = fakeServer("local");
    const a = conn.sessions.requestSession({ cols: 80, rows: 24, paneId: "terminal-a" });
    const b = conn.sessions.requestSession({ cols: 80, rows: 24, paneId: "terminal-b" });
    await flush();
    expect(socket.sentOfType("create").map((m) => m.paneId)).toEqual(["terminal-a"]);

    socket.message({ type: "created", sessionId: "sess-a" });
    await expect(a).resolves.toEqual({ sessionId: "sess-a", shellProfileId: undefined });
    await flush();
    expect(socket.sentOfType("create").map((m) => m.paneId)).toEqual(["terminal-a", "terminal-b"]);

    socket.message({ type: "created", sessionId: "sess-b" });
    await expect(b).resolves.toMatchObject({ sessionId: "sess-b" });
  });

  it("fails a create on an error without a session, and goes on with the next", async () => {
    const { socket, conn } = fakeServer("local");
    const a = conn.sessions.requestSession({ cols: 80, rows: 24 });
    const b = conn.sessions.requestSession({ cols: 80, rows: 24 });
    await flush();
    socket.message({ type: "error", sessionId: "sess-other", error: "not for this create" });
    socket.message({ type: "error", error: "no shell" });
    await expect(a).rejects.toThrow("no shell");
    await flush();
    socket.message({ type: "created", sessionId: "sess-b" });
    await expect(b).resolves.toMatchObject({ sessionId: "sess-b" });
  });

  it("keeps the home directory per connection", async () => {
    const local = fakeServer("local");
    const host = fakeServer("host:vm1");
    const created = host.conn.sessions.requestSession({ cols: 80, rows: 24 });
    await flush();
    host.socket.message({ type: "created", sessionId: "sess-1", homeDir: "/home/remote" });
    await created;

    expect(host.conn.sessions.homeDir).toBe("/home/remote");
    expect(local.conn.sessions.homeDir).toBe("");
  });

  it("keeps panes, screens and sessions per connection", () => {
    const local = fakeServer("local");
    const host = fakeServer("host:vm1");
    const { session } = wiredSession(local.conn, "sess-1");
    local.conn.sessions.linkPane("terminal-1", "sess-1", "screen-1");

    expect(local.conn.sessions.forPane("terminal-1")).toBe(session);
    expect(local.conn.sessions.screenForPane("terminal-1")).toBe("screen-1");
    expect(local.conn.sessions.paneForSession("sess-1")).toBe("terminal-1");
    expect(host.conn.sessions.forPane("terminal-1")).toBeUndefined();
    expect(host.conn.sessions.paneForSession("sess-1")).toBeUndefined();
    expect(host.conn.sessions.get("sess-1")).toBeUndefined();
  });

  it("routes title and closed events to the session, as plain text", () => {
    const { socket, conn } = fakeServer("local");
    const { session, emulator } = wiredSession(conn, "sess-1");
    const titles: string[] = [];
    session.onTitleChange((title) => titles.push(title));

    socket.message({
      type: "title", sessionId: "sess-1", title: "vim\u0007‮", cwd: "/src", baseShell: false,
      oscTitle: `<b>${"x".repeat(MAX_TITLE + 50)}</b>`,
    });
    socket.message({ type: "closed", sessionId: "sess-1", exitCode: 3 });

    expect(session.lastTitle).toBe("vim");
    expect(session.lastCwd).toBe("/src");
    expect(titles[0].startsWith("<b>")).toBe(true);
    expect(titles[0]).toHaveLength(MAX_TITLE);
    expect(emulator.written.join("")).toContain("[session ended]");
  });

  it("does not report successful exit when only a signal or unknown result is available", () => {
    const { socket, conn } = fakeServer("local");
    const { session } = wiredSession(conn, "signal-session");
    let result: number | undefined;
    session.onEnd(code => { result = code; });
    socket.message({ type: "closed", sessionId: "signal-session", signal: "SIGTERM" });
    expect(result).toBe(-1);
  });

  it("sends input to the session's own server", () => {
    const local = fakeServer("local");
    const host = fakeServer("host:vm1");
    const { emulator } = wiredSession(local.conn, "sess-1");

    emulator.type("ls\r");

    expect(local.socket.sentOfType("input")).toEqual([{ type: "input", id: "sess-1", data: "ls\r" }]);
    expect(host.socket.sent).toEqual([]);
  });
});

describe("SessionClient: pasted images", () => {
  const requestIdOf = (socket: ReturnType<typeof fakeServer>["socket"]) =>
    (socket.sentOfType("save_image").at(-1) as { requestId: string }).requestId;

  it("sends the image and resolves with the path its own request names", async () => {
    const { socket, conn } = fakeServer("host:vm1");
    const saved = conn.sessions.saveImage("iVBORw0KGgo=");
    expect(socket.sentOfType("save_image")).toEqual([{ type: "save_image", requestId: expect.stringMatching(/^img-[0-9a-f]{16}$/), data: "iVBORw0KGgo=" }]);
    socket.message({ type: "image_saved", requestId: "img-other", path: "/tmp/other.png" });
    socket.message({ type: "image_saved", requestId: requestIdOf(socket), path: "/home/me/.cache/splitty/paste/paste-1.png" });
    await expect(saved).resolves.toBe("/home/me/.cache/splitty/paste/paste-1.png");
  });

  it("rejects what the server refuses, and a path that is not a plain absolute one", async () => {
    const { socket, conn } = fakeServer("host:vm1");
    const refused = conn.sessions.saveImage("eA==");
    socket.message({ type: "image_failed", requestId: requestIdOf(socket), error: "not an image" });
    await expect(refused).rejects.toThrow("not an image");
    for (const path of ["paste-1.png", "/tmp/a.png\x1b[201~rm -rf ~\r"]) {
      const odd = conn.sessions.saveImage("iVBORw0KGgo=");
      socket.message({ type: "image_saved", requestId: requestIdOf(socket), path });
      await expect(odd).rejects.toThrow("no usable file");
    }
  });

  it("says that a server older than save_image needs an update, and keeps a waiting create", async () => {
    const { socket, conn } = fakeServer("host:vm1");
    const created = conn.sessions.requestSession({ cols: 80, rows: 24 });
    await flush();
    const saved = conn.sessions.saveImage("iVBORw0KGgo=");
    socket.message({ type: "error", error: "unknown message type: save_image" });
    await expect(saved).rejects.toMatchObject({ needsUpdate: true });
    socket.message({ type: "created", sessionId: "sess-1" });
    await expect(created).resolves.toMatchObject({ sessionId: "sess-1" });
  });

  it("takes the answer only from its own connection", async () => {
    const local = fakeServer("local");
    const host = fakeServer("host:vm1");
    const saved = host.conn.sessions.saveImage("iVBORw0KGgo=");
    const id = requestIdOf(host.socket);
    local.socket.message({ type: "image_saved", requestId: id, path: "/evil.png" });
    expect(local.socket.sent).toEqual([]);
    host.socket.message({ type: "image_saved", requestId: id, path: "/home/me/.cache/splitty/paste/paste-2.png" });
    await expect(saved).resolves.toBe("/home/me/.cache/splitty/paste/paste-2.png");
  });

  it("gives up when the server does not answer", async () => {
    const { conn } = fakeServer("host:vm1");
    await expect(conn.sessions.saveImage("iVBORw0KGgo=", 10)).rejects.toThrow("did not answer");
  });
});

describe("SessionClient: uploaded files", () => {
  const MiB = 1024 * 1024;
  const chunks = (socket: ReturnType<typeof fakeServer>["socket"]) => socket.sentOfType("upload");
  const file = (size: number, name = "notes.txt") => new File([new Uint8Array(size).fill(7)], name, { type: "text/plain" });
  /** Wait until the client sent its `n`th chunk. */
  const sent = (socket: ReturnType<typeof fakeServer>["socket"], n: number) => vi.waitFor(() => expect(chunks(socket)).toHaveLength(n));

  it("Upload in two chunks: sends the next chunk only after the answer to the last", async () => {
    const { socket, conn } = fakeServer("host:vm1");
    const progress = vi.fn();
    const saved = conn.sessions.uploadFile(file(1.5 * MiB), progress);
    await sent(socket, 1);
    const [first] = chunks(socket);
    expect(first).toMatchObject({ type: "upload", offset: 0, name: "notes.txt", size: 1.5 * MiB });
    expect(first.requestId).toMatch(/^up-[0-9a-f]{16}$/);
    expect(atob(first.data)).toHaveLength(MiB);
    await flush();
    expect(chunks(socket)).toHaveLength(1);
    socket.message({ type: "upload_progress", requestId: first.requestId, received: MiB });
    await sent(socket, 2);
    const second = chunks(socket)[1];
    expect(second).toEqual({ type: "upload", requestId: first.requestId, offset: MiB, data: expect.any(String) });
    expect(atob(second.data)).toHaveLength(0.5 * MiB);
    socket.message({ type: "upload_saved", requestId: first.requestId, path: "/home/me/.cache/splitty/paste/upload-1/notes.txt" });
    await expect(saved).resolves.toBe("/home/me/.cache/splitty/paste/upload-1/notes.txt");
    expect(progress.mock.calls).toEqual([[MiB], [1.5 * MiB]]);
  });

  it("sends an empty file as one first chunk", async () => {
    const { socket, conn } = fakeServer("local");
    const saved = conn.sessions.uploadFile(file(0, "empty"));
    await sent(socket, 1);
    expect(chunks(socket)[0]).toMatchObject({ offset: 0, data: "", name: "empty", size: 0 });
    socket.message({ type: "upload_saved", requestId: chunks(socket)[0].requestId, path: "/c/upload-1/empty" });
    await expect(saved).resolves.toBe("/c/upload-1/empty");
  });

  it("rejects a refusal, a server out of step and a path that is not a plain absolute one", async () => {
    const { socket, conn } = fakeServer("host:vm1");
    const refused = conn.sessions.uploadFile(file(10));
    await sent(socket, 1);
    socket.message({ type: "upload_failed", requestId: chunks(socket)[0].requestId, error: "disk full" });
    await expect(refused).rejects.toThrow("disk full");
    const odd = conn.sessions.uploadFile(file(2 * MiB));
    await sent(socket, 2);
    socket.message({ type: "upload_progress", requestId: chunks(socket)[1].requestId, received: 5 });
    await expect(odd).rejects.toThrow("lost track");
    const early = conn.sessions.uploadFile(file(2 * MiB));
    await sent(socket, 3);
    socket.message({ type: "upload_saved", requestId: chunks(socket)[2].requestId, path: "/c/x" });
    await expect(early).rejects.toThrow("before all of it");
    const hostile = conn.sessions.uploadFile(file(10));
    await sent(socket, 4);
    socket.message({ type: "upload_saved", requestId: chunks(socket)[3].requestId, path: "/tmp/a\x1b[201~rm -rf ~\r" });
    await expect(hostile).rejects.toThrow("no usable file");
  });

  it("Host with an older Splitty: says that a server without upload needs an update, and keeps a waiting create", async () => {
    const { socket, conn } = fakeServer("host:vm1");
    const created = conn.sessions.requestSession({ cols: 80, rows: 24 });
    await flush();
    const saved = conn.sessions.uploadFile(file(10));
    await sent(socket, 1);
    socket.message({ type: "error", error: "unknown message type: upload" });
    await expect(saved).rejects.toMatchObject({ needsUpdate: true });
    socket.message({ type: "created", sessionId: "sess-1" });
    await expect(created).resolves.toMatchObject({ sessionId: "sess-1" });
  });

  it("refuses a file over 512 MB before sending, and gives up on a lost connection or a silent server", async () => {
    const { socket, conn } = fakeServer("host:vm1");
    const huge = Object.defineProperty(new File([], "disk.img"), "size", { value: 512 * MiB + 1 });
    await expect(conn.sessions.uploadFile(huge)).rejects.toThrow("larger than 512 MB");
    expect(chunks(socket)).toEqual([]);
    const lost = conn.sessions.uploadFile(file(10));
    await sent(socket, 1);
    socket.drop();
    await expect(lost).rejects.toThrow("connection was lost");
    await expect(conn.sessions.uploadFile(file(10), undefined, 10)).rejects.toThrow("did not answer");
  });

  it("takes the answer only from its own connection", async () => {
    const local = fakeServer("local");
    const host = fakeServer("host:vm1");
    const saved = host.conn.sessions.uploadFile(file(10));
    await sent(host.socket, 1);
    const id = chunks(host.socket)[0].requestId;
    local.socket.message({ type: "upload_saved", requestId: id, path: "/evil" });
    host.socket.message({ type: "upload_saved", requestId: id, path: "/c/upload-1/notes.txt" });
    await expect(saved).resolves.toBe("/c/upload-1/notes.txt");
    expect(local.socket.sent).toEqual([]);
  });
});

describe("SessionClient: PTY sizes", () => {
  const size = (fields: Record<string, unknown>) => ({ type: "size", sessionId: "sess-1", cols: 100, rows: 30, policy: "active", ...fields });

  it("learns from each welcome whether the server decides sizes", () => {
    const { socket, conn } = fakeServer("host:vm1");
    expect(conn.sessions.sizing).toBe(false);
    socket.message({ type: "mux.welcome", clientId: "c1", screens: [], capabilities: ["managed-jobs-v1", "pty-size-v1"] });
    expect(conn.sessions.sizing).toBe(true);
    socket.message({ type: "mux.welcome", clientId: "c2", screens: [] });
    expect(conn.sessions.sizing).toBe(false);
  });

  it("keeps the latest size per session, also before its view is wired, and routes it", () => {
    const { socket, conn } = fakeServer("host:vm1");
    socket.message(size({}));
    expect(conn.sessions.sizeOf("sess-1")).toEqual({ cols: 100, rows: 30, policy: "active" });

    const { session } = wiredSession(conn, "sess-1");
    const handled = vi.spyOn(session, "handleSize");
    socket.message(size({ cols: 90 }));
    expect(handled).toHaveBeenCalledWith({ cols: 90, rows: 30, policy: "active" });

    socket.message({ type: "closed", sessionId: "sess-1" });
    expect(conn.sessions.sizeOf("sess-1")).toBeUndefined();
    socket.message(size({}));
    socket.drop();
    expect(conn.sessions.sizeOf("sess-1")).toBeUndefined();
  });

  it("ignores sizes out of bounds and malformed ones", () => {
    const { socket, conn } = fakeServer("host:vm1");
    for (const fields of [
      { cols: 1001 },
      { rows: 501 },
      { cols: 0 },
      { rows: 2.5 },
      { cols: "100" },
      { policy: "largest" },
      { sessionId: 7 },
    ]) {
      socket.message(size(fields));
    }
    expect(conn.sessions.sizeOf("sess-1")).toBeUndefined();
    socket.message(size({ cols: 1000, rows: 500 }));
    expect(conn.sessions.sizeOf("sess-1")).toEqual({ cols: 1000, rows: 500, policy: "active" });
  });
});
