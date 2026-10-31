import { describe, expect, it, vi } from "vitest";
import { fakeEmulator, fakeServer } from "../testing/fake_server.ts";
import { AttachTransferredError, TerminalSession } from "./terminal_session.ts";

describe("TerminalSession: attach dimensions and redraw", () => {
  it("forces a redraw at the same size after replay, on its own server", async () => {
    const local = fakeServer("local");
    const host = fakeServer("host:vm1");
    const pending = host.conn.sessions.attach("sess-1", 80, 24);
    host.socket.output("sess-1", "replay");
    host.socket.message({ type: "attached", sessionId: "sess-1" });

    const emulator = fakeEmulator();
    const session = new TerminalSession(host.conn.sessions, "sess-1", emulator);
    const originalFit = emulator.fit.bind(emulator);
    vi.spyOn(emulator, "fit").mockImplementation(() => {
      expect(emulator.written).toEqual(["replay"]);
      return originalFit();
    });
    session.wire(await pending);

    expect(host.socket.sentOfType("resize")).toEqual([
      { type: "resize", id: "sess-1", cols: 80, rows: 25 },
      { type: "resize", id: "sess-1", cols: 80, rows: 24 },
    ]);
    expect([emulator.cols, emulator.rows]).toEqual([80, 24]);
    expect(local.socket.sent).toEqual([]);

    host.socket.output("sess-1", "redraw");
    expect(emulator.written).toEqual(["replay", "redraw"]);
    emulator.type("x");
    expect(host.socket.sentOfType("input")).toEqual([{ type: "input", id: "sess-1", data: "x" }]);
    // An older server without `pty-size-v1` gets none of the size policy's messages.
    expect(host.socket.sentOfType("hide")).toEqual([]);
    expect(host.socket.sentOfType("activity")).toEqual([]);
    expect(host.socket.sentOfType("resize").some((m) => m.redraw)).toBe(false);
    session.detach();
  });

  it("uses the new owner's current layout when an attach is taken over", async () => {
    const { socket, conn } = fakeServer("host:vm1");
    const first = conn.sessions.attach("sess-1", 80, 24);
    socket.message({ type: "attached", sessionId: "sess-1" });
    const stale = await first;
    const newer = await conn.sessions.attach("sess-1", 100, 30);

    const oldSession = new TerminalSession(conn.sessions, "sess-1", fakeEmulator());
    expect(() => oldSession.wire(stale)).toThrow(AttachTransferredError);
    expect(socket.sentOfType("resize")).toEqual([]);

    const emulator = fakeEmulator();
    vi.spyOn(emulator, "fit").mockReturnValue({ cols: 120, rows: 40 });
    const session = new TerminalSession(conn.sessions, "sess-1", emulator);
    session.wire(newer);

    expect(socket.sentOfType("attach")).toHaveLength(1);
    expect(socket.sentOfType("resize")).toEqual([
      { type: "resize", id: "sess-1", cols: 120, rows: 41 },
      { type: "resize", id: "sess-1", cols: 120, rows: 40 },
    ]);
    session.detach();
  });

  it("keeps the emulator's dimensions when fitting is unavailable", async () => {
    const { socket, conn } = fakeServer("host:vm1");
    const pending = conn.sessions.attach("sess-1", 80, 24);
    socket.message({ type: "attached", sessionId: "sess-1" });
    const emulator = fakeEmulator();
    vi.spyOn(emulator, "fit").mockReturnValue(undefined);
    const session = new TerminalSession(conn.sessions, "sess-1", emulator);
    session.wire(await pending);

    expect(socket.sentOfType("resize").at(-1)).toEqual({ type: "resize", id: "sess-1", cols: 80, rows: 24 });
    session.detach();
  });

  it("keeps the temporary row count within the protocol's u16 range", async () => {
    const { socket, conn } = fakeServer("host:vm1");
    const pending = conn.sessions.attach("sess-1", 80, 65535);
    socket.message({ type: "attached", sessionId: "sess-1" });
    const emulator = fakeEmulator();
    vi.spyOn(emulator, "fit").mockReturnValue({ cols: 80, rows: 65535 });
    const session = new TerminalSession(conn.sessions, "sess-1", emulator);
    session.wire(await pending);

    expect(socket.sentOfType("resize")).toEqual([
      { type: "resize", id: "sess-1", cols: 80, rows: 65534 },
      { type: "resize", id: "sess-1", cols: 80, rows: 65535 },
    ]);
    session.detach();
  });

  it("leaves newly created sessions at their initial size", () => {
    const { socket, conn } = fakeServer("host:vm1");
    const session = new TerminalSession(conn.sessions, "sess-1", fakeEmulator());
    session.wire();
    expect(socket.sentOfType("resize")).toEqual([]);
    session.detach();
  });
});

/** A host whose server decides PTY sizes (`pty-size-v1`). */
function sizingServer() {
  const server = fakeServer("host:vm1");
  server.socket.message({ type: "mux.welcome", clientId: "c1", screens: [], capabilities: ["pty-size-v1"] });
  return server;
}

/** Attach `sess-1`, whose server says its PTY is `pty`, to a pane that fits `pane`. */
async function attachedAt(
  server: ReturnType<typeof sizingServer>,
  pane: { cols: number; rows: number },
  pty: { cols: number; rows: number } | null,
  shown: () => boolean = () => true,
) {
  const pending = server.conn.sessions.attach("sess-1", pane.cols, pane.rows);
  if (pty) server.socket.message({ type: "size", sessionId: "sess-1", ...pty, policy: "active" });
  server.socket.output("sess-1", "replay");
  server.socket.message({ type: "attached", sessionId: "sess-1" });
  const emulator = fakeEmulator();
  emulator.resize(pane.cols, pane.rows);
  emulator.pane = { ...pane };
  const sizesAtReplay: number[][] = [];
  const write = emulator.write.bind(emulator);
  vi.spyOn(emulator, "write").mockImplementation((data) => {
    sizesAtReplay.push([emulator.cols, emulator.rows]);
    write(data);
  });
  const session = new TerminalSession(server.conn.sessions, "sess-1", emulator, { shown });
  session.wire(await pending);
  return { session, emulator, sizesAtReplay };
}

describe("TerminalSession: a PTY size decided by the server", () => {
  it("shows a narrower PTY at its size and reports the pane's (Narrower PTY)", async () => {
    const server = sizingServer();
    const { session, emulator, sizesAtReplay } = await attachedAt(server, { cols: 120, rows: 40 }, { cols: 100, rows: 30 });

    expect(sizesAtReplay).toEqual([[100, 30]]);
    expect([emulator.cols, emulator.rows]).toEqual([100, 30]);
    expect(server.socket.sentOfType("resize")).toEqual([
      { type: "resize", id: "sess-1", cols: 120, rows: 40, redraw: true },
    ]);
    expect(session.sizeView()).toEqual({
      pane: { cols: 120, rows: 40 },
      pty: { cols: 100, rows: 30, policy: "active" },
    });
    session.detach();
  });

  it("reports when the pane is shown again and hides it when it is not", async () => {
    const server = sizingServer();
    let shown = false;
    const { session } = await attachedAt(server, { cols: 120, rows: 40 }, null, () => shown);
    expect(server.socket.sentOfType("hide")).toEqual([{ type: "hide", id: "sess-1" }]);
    expect(server.socket.sentOfType("resize")).toEqual([]);

    shown = true;
    session.setShown(true);
    expect(server.socket.sentOfType("resize")).toEqual([
      { type: "resize", id: "sess-1", cols: 120, rows: 40, redraw: true },
    ]);
    session.setShown(false);
    expect(server.socket.sentOfType("hide")).toHaveLength(2);
    session.fit();
    expect(server.socket.sentOfType("resize")).toHaveLength(1);
    session.detach();
  });

  it("never reports the pinned size as the pane's", async () => {
    const server = sizingServer();
    const { session, emulator } = await attachedAt(server, { cols: 120, rows: 40 }, { cols: 100, rows: 30 });

    // A settings change refits the emulator to the pane on its own.
    emulator.pane = { cols: 130, rows: 45 };
    emulator.resize(130, 45);
    expect(server.socket.sentOfType("resize").map((m) => [m.cols, m.rows])).toEqual([[120, 40], [130, 45]]);
    expect([emulator.cols, emulator.rows]).toEqual([100, 30]);
    session.detach();
  });

  it("resizes right away while its own size decides, and follows the answer", async () => {
    const server = sizingServer();
    const { session, emulator } = await attachedAt(server, { cols: 120, rows: 40 }, { cols: 120, rows: 40 });

    emulator.pane = { cols: 140, rows: 50 };
    session.fit();
    expect([emulator.cols, emulator.rows]).toEqual([140, 50]);
    expect(server.socket.sentOfType("resize").at(-1)).toEqual({ type: "resize", id: "sess-1", cols: 140, rows: 50 });

    server.socket.message({ type: "size", sessionId: "sess-1", cols: 130, rows: 50, policy: "active" });
    expect([emulator.cols, emulator.rows]).toEqual([130, 50]);
    session.detach();
  });

  it("keeps the PTY size while another client decides it", async () => {
    const server = sizingServer();
    const { session, emulator } = await attachedAt(server, { cols: 120, rows: 40 }, { cols: 100, rows: 30 });
    const views: unknown[] = [];
    session.onSizeView((view) => views.push(view));

    emulator.pane = { cols: 140, rows: 50 };
    session.fit();
    expect([emulator.cols, emulator.rows]).toEqual([100, 30]);
    expect(views.at(-1)).toEqual({ pane: { cols: 140, rows: 50 }, pty: { cols: 100, rows: 30, policy: "active" } });
    session.detach();
  });

  it("takes the size over with an activity", async () => {
    const server = sizingServer();
    const { session } = await attachedAt(server, { cols: 120, rows: 40 }, { cols: 100, rows: 30 });
    session.takeOver();
    expect(server.socket.sentOfType("activity")).toEqual([{ type: "activity", takeover: true }]);
    session.detach();
  });

  it("reports a created session's size and shows the size that follows `created`", () => {
    const server = sizingServer();
    const emulator = fakeEmulator();
    const session = new TerminalSession(server.conn.sessions, "sess-1", emulator);
    session.wire();
    expect(server.socket.sentOfType("resize")).toEqual([{ type: "resize", id: "sess-1", cols: 80, rows: 24 }]);
    server.socket.message({ type: "size", sessionId: "sess-1", cols: 70, rows: 20, policy: "smallest" });
    expect([emulator.cols, emulator.rows]).toEqual([70, 20]);
    session.detach();
  });
});
