import { describe, it, expect } from "vitest";
import { filterCommands, listCommands, registerCommands, type PaletteCommand } from "./commands.ts";

const command = (id: string, label: string): PaletteCommand => ({ id, label, run: () => {} });

describe("registerCommands", () => {
  it("offers the commands until they are unregistered", () => {
    const workspace = [command("screen.new", "New Screen"), command("templates", "Templates…")];
    const remote = [command("remote.open", "Open host in new window…")];

    const unregisterWorkspace = registerCommands(workspace);
    const unregisterRemote = registerCommands(remote);
    expect(listCommands().map((c) => c.id)).toEqual(["screen.new", "templates", "remote.open"]);

    unregisterWorkspace();
    expect(listCommands().map((c) => c.id)).toEqual(["remote.open"]);

    unregisterRemote();
    expect(listCommands()).toEqual([]);
  });
});

describe("filterCommands", () => {
  const all = [command("screen.new", "New Screen"), command("remote.open", "Open host in new window…")];

  it("matches every word of the query anywhere in the label", () => {
    expect(filterCommands(all, "").map((c) => c.id)).toEqual(["screen.new", "remote.open"]);
    expect(filterCommands(all, "host win").map((c) => c.id)).toEqual(["remote.open"]);
    expect(filterCommands(all, "NEW").map((c) => c.id)).toEqual(["screen.new", "remote.open"]);
    expect(filterCommands(all, "new tab")).toEqual([]);
  });
});
