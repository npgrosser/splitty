import { afterEach, describe, expect, it, vi } from "vitest";
import type { DockviewApi, DockviewGroupPanel } from "dockview-core";
import { addTerminalPane } from "./panes.ts";
import { ScreenBinding, registerScreenApi, unregisterScreenApi } from "./connection/screen_binding.ts";
import { fakeServer } from "./testing/fake_server.ts";

vi.mock("./shell_profiles.ts", () => ({ getDefaultShellProfileId: () => "local-default" }));

const layouts: DockviewApi[] = [];
afterEach(() => { for (const api of layouts.splice(0)) unregisterScreenApi(api); });

function setup() {
  const { socket, conn } = fakeServer("host:vm1");
  const binding = new ScreenBinding("remote-screen", conn, "host-screen");
  let activePanel = { id: "active-pane" };
  const group = { panels: [activePanel] };
  const api = {
    id: "tab-test", get activePanel() { return activePanel; }, panels: [activePanel],
    addPanel: vi.fn(() => { activePanel = { id: "new-pane" }; group.panels.push(activePanel); }),
    getPanel: (id: string) => group.panels.some((panel) => panel.id === id) ? { id, group } : undefined,
  } as unknown as DockviewApi;
  registerScreenApi(api, binding);
  layouts.push(api);
  return { api, socket, binding };
}

describe("terminal pane layout operations", () => {
  it("opens a keyboard tab in the active pane's slot on its host", () => {
    const { api, socket } = setup();
    addTerminalPane(api, { id: "new-pane", dir: "/work" });
    expect(socket.sentOfType("mux.op")).toEqual([{
      type: "mux.op", screenId: "host-screen",
      op: { type: "new_tab", paneId: "active-pane", newPaneId: "new-pane", cwd: "/work", shellProfileId: undefined },
    }]);
  });

  // mux-protocol, Selected Tabs Reach the Server: new_tab makes the new tab active on the server.
  it("knows the new tab as its slot's active one on the server", () => {
    const { api, binding } = setup();
    binding.noteActiveTab(["active-pane"], "active-pane");
    addTerminalPane(api, { id: "new-pane" });
    expect([...binding.activeTabs]).toEqual(["new-pane"]);
  });

  it("targets the clicked group's tab even when another group is active", () => {
    const { api, socket } = setup();
    const group = { activePanel: { id: "other-pane" } } as DockviewGroupPanel;
    addTerminalPane(api, { id: "new-pane", group });
    expect(api.addPanel).toHaveBeenCalledWith(expect.objectContaining({ position: { referenceGroup: group, direction: "within" } }));
    expect(socket.sentOfType("mux.op")[0].op).toMatchObject({ type: "new_tab", paneId: "other-pane" });
  });

  it.each([["right", "horizontal"], ["below", "vertical"]] as const)("still splits %s along the %s axis", (direction, axis) => {
    const { api, socket } = setup();
    addTerminalPane(api, { id: "new-pane", direction });
    expect(socket.sentOfType("mux.op")[0].op).toMatchObject({ type: "split_pane", paneId: "active-pane", axis });
  });
});
