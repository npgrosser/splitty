import { afterEach, describe, expect, it, vi } from "vitest";
const { fetchWsTicket } = vi.hoisted(() => ({ fetchWsTicket: vi.fn() }));
vi.mock("./http.ts", () => ({ fetchWsTicket }));
import { SplittyWebSocket } from "./websocket.ts";
class BrowserSocket {
  static readonly OPEN = 1;
  static opened: BrowserSocket[] = [];
  readyState = 0;
  binaryType = "blob";
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onerror: (() => void) | null = null;
  sent: string[] = [];
  constructor(readonly url: string) { BrowserSocket.opened.push(this); }
  send(value: string) { this.sent.push(value); }
  close() { this.readyState = 3; this.onclose?.(); }
  open() { this.readyState = 1; this.onopen?.(); }
}
function setup() {
  vi.stubGlobal("window", { location: { search: "" }, setTimeout }); vi.stubGlobal("WebSocket", BrowserSocket);
  BrowserSocket.opened = []; fetchWsTicket.mockReset().mockResolvedValue("ticket");
  return new SplittyWebSocket(() => "ws://127.0.0.1:1234/ws");
}
afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });
describe("browser generations", () => {
  it("discards offline terminal input and mutations, resuming only deliberate sends", async () => {
    const socket = setup(); socket.send({ type: "input", id: "s", data: "first\r" });
    await socket.connect(); BrowserSocket.opened[0]!.open();
    expect(BrowserSocket.opened[0]!.sent).toEqual([]);
    socket.send({ type: "input", id: "s", data: "live\r" });
    expect(BrowserSocket.opened[0]!.sent).toHaveLength(1);
    socket.reconnect(); socket.send({ type: "create", command: "old effect" });
    await Promise.resolve(); BrowserSocket.opened[1]!.open();
    expect(BrowserSocket.opened[1]!.sent).toEqual([]); socket.dispose();
  });
  it("ignores old socket messages after replacement", async () => {
    const socket = setup(); const receive = vi.fn(); socket.onMessage(receive);
    await socket.connect(); const old = BrowserSocket.opened[0]!; old.open(); const oldReceive = old.onmessage;
    socket.reconnect(); await Promise.resolve(); BrowserSocket.opened[1]!.open();
    oldReceive?.({ data: '{"type":"bell"}' }); expect(receive).not.toHaveBeenCalled(); socket.dispose();
  });
  it("does not open a socket for a ticket resolving after disposal", async () => {
    const socket = setup(); let finish!: (value: string) => void;
    fetchWsTicket.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
    const connecting = socket.connect(); socket.dispose(); finish("late-ticket"); await connecting;
    expect(BrowserSocket.opened).toEqual([]);
  });
});
