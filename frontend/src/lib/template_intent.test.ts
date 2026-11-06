import { describe, expect, it } from "vitest";
import { templateIntent } from "./template_intent.ts";
import { ScreenViews } from "./connection/screen_sync.ts";
import { ServerConnection } from "./connection/server_connection.ts";
import { FakeSocket } from "./testing/fake_server.ts";
import type { DockviewApi } from "dockview-core";
import type { Template } from "./protocol/types.ts";

const template: Template = { name: "Work", panes: { a: { command: "editor", dir: "src", restore_mode: "close", shell_profile: "fish" }, b: {}, c: {} }, layout: { split: "horizontal", children: [{ slot: "a", ratio: 3 }, { slot: ["b"], ratio: 1 }] }, floating: [{ layout: { slot: ["c"] }, bounds: { x: .1, y: .2, w: .4, h: .3 } }] };

describe("template intent", () => {
  it("preserves raw launch intent and projects ratios and floats without session IDs", () => {
    const { op, screen } = templateIntent(template, "screen", "/project", true);
    expect(op).toMatchObject({ type: "apply_template", template, root: "/project", createScreen: true });
    expect(Object.keys(screen.panes)).toEqual(Object.values(op.paneIds));
    expect(Object.values(screen.panes).every(p => p.sessionId === undefined)).toBe(true);
    expect(screen.sessions).toEqual({});
    expect(screen.root.sizes).toEqual([.75, .25]);
    expect(screen.floating[0]).toMatchObject({ id: "template-floating-0", bounds: template.floating![0].bounds });
  });
  it("skips legacy web panes, collapses their branches and rejects empty or invalid layouts", () => {
    const mixed = structuredClone(template);
    mixed.panes.b = { type: "webview", url: "https://example.invalid" };
    const projected = templateIntent(mixed, "s");
    expect(projected.skippedWeb).toBe(true);
    expect(projected.screen.root.type).toBe("slot");
    expect(Object.keys(projected.op.paneIds)).toEqual(["a", "c"]);
    mixed.panes.a = mixed.panes.b;
    expect(() => templateIntent(mixed, "s")).toThrow("no tiled terminals");
    expect(() => templateIntent({ ...template, layout: { slot: ["a", "a"] } }, "s")).toThrow();
    expect(() => templateIntent({ ...template, layout: { slot: ["a"] } }, "s")).toThrow("unused");
  });
  it("queues the optimistic snapshot until a new screen mounts and sends exactly one op", () => {
    const socket = new FakeSocket();
    const conn = new ServerConnection("local", "Local", socket);
    socket.message({ type: "mux.welcome", capabilities: ["server-templates-v1"], screens: [] });
    const applied: string[] = [];
    const views = new ScreenViews((_api, _binding, screen) => applied.push(screen.id));
    const { screen, op } = templateIntent(template, "new", "", true);
    views.snapshot("local", "new", screen);
    conn.mux.sendOp("new", op);
    expect(applied).toEqual([]);
    views.bind({ id: "new", conn: "local", wireId: "new" }, conn, true);
    expect(views.ready("new", {} as DockviewApi)).toBe(true);
    expect(applied).toEqual(["new"]);
    expect(socket.sent.map(m => m.type)).toEqual(["mux.op"]);
    expect(socket.sentOfType("mux.op")[0].op.type).toBe("apply_template");
    views.clear(); conn.dispose();
  });
});
