import { beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "svelte/server";
import HostPorts from "./HostPorts.svelte";
import HostShares from "./HostShares.svelte";
import ManualPortForm from "./ManualPortForm.svelte";
import Port from "./PortRow.svelte";
import ShareForm from "./ShareForm.svelte";
import Share from "./ShareRow.svelte";
import { portForwarding, reverseForwarding } from "../../stores/ports.svelte.ts";
import {
  hostSummary, portMenu, portPrimary, portsSummary, shareMenu, sharePrimary, shareStatus,
} from "./remote_view.ts";
import type { PortAction } from "../../lib/remote/ports.ts";
import type { ShareAction } from "../../lib/remote/reverse_forwards.ts";

vi.mock("../../stores/ports.svelte.ts", () => ({ portForwarding: vi.fn(), reverseForwarding: vi.fn() }));
vi.mock("../../stores/ui.svelte.ts", () => ({ showContextMenu: vi.fn() }));

describe("drawer port controls", () => {
  beforeEach(() => vi.clearAllMocks());

  it("offers Add port for a host with no detected, active or saved ports", () => {
    const { body } = render(HostPorts, { props: { target: "vm1", rows: [], targets: ["vm1"] } });
    expect(body).toContain("Add port");
    expect(body).toContain("No ports detected.");
    expect(body).toMatch(/<section[^>]*aria-label="Ports on vm1"/);
    expect(portForwarding).not.toHaveBeenCalled();
  });

  it("keeps per-host forms in their host context while global forms allow unknown targets", () => {
    const { body } = render(ManualPortForm, { props: { target: "disconnected-vm", targets: ["other-vm"], onclose: vi.fn() } });
    expect(body).toContain('aria-label="Add port on disconnected-vm"');
    expect(body).not.toContain("<datalist");
    expect(body).not.toContain("SSH target");
    expect(body).not.toMatch(/\schecked(?:\s|=|>)/);
    expect(portForwarding).not.toHaveBeenCalled();
  });

  it("offers manual target input and known choices, with Remember initially unchecked", () => {
    const { body } = render(ManualPortForm, { props: { targets: ["known-vm", "saved-vm"], onclose: vi.fn() } });
    expect(body).toContain("SSH target");
    expect(body).toContain('value="known-vm"');
    expect(body).toContain('value="saved-vm"');
    expect(body).toContain("Remote port");
    expect(body).toContain("Local port");
    expect(body).toContain('type="checkbox"');
    expect(body).not.toMatch(/\schecked(?:\s|=|>)/);
    expect(body).not.toContain("Save without starting");
  });

  it("shows paused undiscovered mappings safely, and explains Forget on a live mapping", () => {
    const saved = { target: "vm1", remote: 5173, local: 15173, label: "<script>API</script>", enabled: false };
    const paused = render(HostPorts, { props: { target: "vm1", targets: [], rows: [{ port: 5173, saved }] } }).body;
    expect(paused).toContain("Paused");
    expect(paused).toContain("Resume");
    expect(paused).toContain("localhost:15173");
    expect(paused).toContain("&lt;script>API&lt;/script>");
    expect(paused).not.toContain("not listening");
    const live = render(HostPorts, { props: { target: "vm1", targets: [], rows: [{ port: 5173, saved: { ...saved, enabled: true },
      forward: { target: "vm1", remote: 5173, local: 15173, state: "up", url: "http://localhost:15173" },
    }] } }).body;
    expect(live).toMatch(/>Open(?:<!--[^>]*-->)?\s*<span aria-hidden="true">↗/);
    expect(live).not.toContain("Use another local port");
    expect(live).not.toContain("href="); // Open must still go through controller validation.
  });

  it("offers Retry and an explicit alternate for a saved conflict without a direct service link", () => {
    const error = { code: "local_port_conflict", message: "Port is occupied", hint: "Choose an unused port" };
    const saved = { target: "vm1", remote: 5173, local: 15173, label: "Frontend", enabled: true, lastError: error };
    const { body } = render(HostPorts, { props: { target: "vm1", targets: [], rows: [{ port: 5173, saved, error }] } });
    expect(body).toContain("Local port in use");
    expect(body).toContain("Port is occupied Choose an unused port");
    expect(body).toContain("Retry");
    expect(portMenu({ port: 5173, saved, error }).map((item) => item.label)).toContain("Stop");
    expect(body).toContain("Use another local port…");
    expect(body).not.toContain("href=");
    expect(body).not.toMatch(/>\s*Open\s*</);
  });

  it.each<PortAction>(["forward", "stop", "save", "forget"])("disables every row action while %s is pending", (pending) => {
    const saved = { target: "vm1", remote: 5173, local: 15173, label: "Frontend", enabled: true };
    const { body } = render(Port, { props: { target: "vm1", row: { port: 5173, saved, pending } } });
    expect(body).toContain('aria-busy="true"');
    const buttons = body.match(/<button\b[^>]*>/g)!;
    expect(buttons.length).toBeGreaterThan(0);
    expect(buttons.every((button) => /\sdisabled(?:\s|=|>)/.test(button))).toBe(true);
  });

  it("keeps a row's rarer actions in its menu, each consequence beside it", () => {
    const saved = { target: "vm1", remote: 5173, local: 15173, label: "API", enabled: true };
    const forward = { target: "vm1", remote: 5173, local: 15173, state: "up" as const, url: "http://localhost:15173" };
    expect(portMenu({ port: 5173, saved, forward })).toEqual([
      { action: "stop", label: "Stop", detail: "Pauses the saved rule until you resume it" },
      { action: "label", label: "Edit label…" },
      { action: "forget", label: "Forget", detail: "Keeps this forward running; Stop pauses it" },
    ]);
    expect(portMenu({ port: 8000, process: "python" }).map((item) => item.action)).toEqual(["label"]);
    expect(portMenu({ port: 5173, saved: { ...saved, enabled: false } }).map((item) => item.action)).toEqual(["label", "local", "forget"]);
    expect([portPrimary({ port: 1, forward }), portPrimary({ port: 1, saved: { ...saved, enabled: false } }), portPrimary({ port: 1, process: "x" })])
      .toEqual(["Open", "Resume", "Forward"]);
  });

  it("lists forwarded and saved ports before the ones only detected", () => {
    const saved = { target: "vm1", remote: 8080, local: 18080, label: "API", enabled: true };
    const { body } = render(HostPorts, { props: { target: "vm1", targets: [], rows: [{ port: 3000, process: "node" }, { port: 8080, saved }] } });
    expect(body.indexOf("8080")).toBeLessThan(body.indexOf("Detected"));
    expect(body.indexOf("Detected")).toBeLessThan(body.indexOf("3000"));
  });

  it("distinguishes screen connectivity from independently running or rebuilding forwards", () => {
    const group = { key: "host:vm1" as const, target: "vm1", open: [], screens: [], stale: true };
    expect(hostSummary(group)).toBe("Not connected");
    expect(hostSummary({ ...group, state: { phase: "reconnecting", progress: [], attempt: 2 } })).toBe("Reconnecting…");
    const forward = { target: "vm1", remote: 5173, local: 15173, state: "up" as const, url: "http://localhost:15173" };
    expect(portsSummary([{ port: 5173, forward }])).toBe("1 active");
    expect(portsSummary([{ port: 5173, forward: { ...forward, state: "down" } }])).toBe("Reconnecting…");
    expect(portsSummary([{ port: 5173, forward, error: { code: "storage_failure", message: "Could not save" } }])).toBe("1 needs attention");
  });
});

describe("drawer share controls", () => {
  beforeEach(() => vi.clearAllMocks());
  const shared = { target: "vm1", remote: 11434, local: 21434, state: "up" as const };
  const saved = { target: "vm1", remote: 11434, local: 21434, enabled: true };

  it("lists a host's shares apart from its ports, with one way to share another", () => {
    const { body } = render(HostShares, { props: { target: "vm1", rows: [{ remote: 11434, local: 21434, forward: shared }] } });
    expect(body).toMatch(/<section[^>]*aria-label="Shared with vm1"/);
    expect(body).toContain("localhost:21434 → vm1:11434");
    expect(body).toMatch(/role="status">Shared</);
    expect(body).toMatch(/>Stop</);
    expect(body).toContain("Share a local port");
    // The form opens only on the user's click.
    expect(body).not.toContain("<form");
    expect(reverseForwarding).not.toHaveBeenCalled();
  });

  it("states who can use the port before Share, with Remember unchecked and no stronger warning yet", () => {
    const { body } = render(ShareForm, { props: { target: "vm1", onclose: vi.fn() } });
    expect(body).toContain('aria-label="Share a local port with vm1"');
    expect(body).toContain("Local port");
    expect(body).toContain("Port on host");
    expect(body).toContain("Programs running as you on vm1 can use the local port you share. Most local services ask for no password.");
    expect(body.indexOf("Most local services ask for no password")).toBeLessThan(body.indexOf(">Share<"));
    expect(body).not.toContain("control of this machine");
    expect(body).not.toMatch(/\schecked(?:\s|=|>)/);
    expect(reverseForwarding).not.toHaveBeenCalled();
  });

  it("shows a paused share with Resume and a failed restore with its error, hint and Retry", () => {
    const paused = render(Share, { props: { target: "vm1", row: { remote: 11434, local: 21434, saved: { ...saved, enabled: false } } } }).body;
    expect(paused).toContain("Paused");
    expect(paused).toMatch(/>Resume</);
    const error = { code: "host_port_conflict", message: "Port 11434 is in use on vm1.", hint: "Stop the program using it there." };
    const failed = render(Share, { props: { target: "vm1", row: { remote: 11434, local: 21434, saved: { ...saved, lastError: error }, error } } }).body;
    expect(failed).toContain("Port in use on the host");
    expect(failed).toContain("Port 11434 is in use on vm1. Stop the program using it there.");
    expect(failed).toMatch(/>Retry</);
  });

  it.each<ShareAction>(["share", "stop", "save", "forget"])("disables every share action while %s is pending", (pending) => {
    const { body } = render(Share, { props: { target: "vm1", row: { remote: 11434, local: 21434, saved, pending } } });
    expect(body).toContain('aria-busy="true"');
    const buttons = body.match(/<button\b[^>]*>/g)!;
    expect(buttons.length).toBe(2);
    expect(buttons.every((button) => /\sdisabled(?:\s|=|>)/.test(button))).toBe(true);
  });

  it("offers Stop, Resume, Start or Retry as the primary action, Remember and Forget in the menu", () => {
    expect(sharePrimary({ remote: 1, local: 1, forward: shared })).toBe("Stop");
    expect(sharePrimary({ remote: 1, local: 1, saved: { ...saved, enabled: false } })).toBe("Resume");
    expect(sharePrimary({ remote: 1, local: 1, saved })).toBe("Start");
    expect(sharePrimary({ remote: 1, local: 1, saved, error: { code: "forward", message: "x" } })).toBe("Retry");

    expect(shareMenu({ remote: 1, local: 1, forward: shared })).toEqual([
      { action: "remember", label: "Remember", detail: "Share it again when this host connects" },
    ]);
    expect(shareMenu({ remote: 1, local: 1, forward: shared, saved })).toEqual([
      { action: "forget", label: "Forget", detail: "Keeps sharing now; Stop ends it" },
    ]);
    expect(shareMenu({ remote: 1, local: 1, saved }).map((item) => item.action)).toEqual(["stop", "forget"]);
    expect(shareMenu({ remote: 1, local: 1, saved: { ...saved, enabled: false } }).map((item) => item.action)).toEqual(["forget"]);
  });

  it("reports state: shared, reconnecting, paused, waiting for the host, or failed", () => {
    expect(shareStatus({ remote: 1, local: 1, forward: shared })).toBe("Shared");
    expect(shareStatus({ remote: 1, local: 1, forward: { ...shared, state: "down" } })).toBe("Reconnecting…");
    expect(shareStatus({ remote: 1, local: 1, saved: { ...saved, enabled: false } })).toBe("Paused");
    expect(shareStatus({ remote: 1, local: 1, saved })).toBe("Starts when host connects");
    expect(shareStatus({ remote: 1, local: 1, saved, error: { code: "unsupported_host", message: "x" } })).toBe("Not supported on this host");
    expect(portsSummary([{ remote: 1, local: 1, forward: shared }, { remote: 2, local: 2, saved: { ...saved, enabled: false } }])).toBe("1 active");
    expect(portsSummary([{ remote: 1, local: 1, saved: { ...saved, enabled: false } }])).toBe("1 paused");
  });
});
