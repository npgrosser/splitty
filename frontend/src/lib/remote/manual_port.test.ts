import { describe, expect, it } from "vitest";
import { manualPort, portNumber } from "./manual_port.ts";

const input = { target: " vm1 ", remote: "5173", local: "", remember: false, label: "" };

describe("manual port entry", () => {
  it.each(["", "0", "65536", "12.3", "-80", "80x", "1e3", "0x50", "3000:4000", "http://localhost:3000"])("rejects %s as a port", (text) => {
    expect(portNumber(text)).toBeNull();
    expect(manualPort({ ...input, remote: text }).error).toContain("Remote port");
  });
  it("allows exact boundary ports and validates a supplied local port", () => {
    expect(portNumber(" 65535 ")).toBe(65535);
    expect(portNumber("1")).toBe(1);
    expect(manualPort({ ...input, local: "bad" }).error).toContain("Local port");
  });
  it.each(["", "-oProxyCommand=x", "vm1 another", "vm1\u001b", "x".repeat(256)])("rejects an invalid target", (target) => {
    expect(manualPort({ ...input, target }).error).toContain("SSH target");
  });
  it("defaults to ephemeral forwarding and omits unsupplied local and label metadata", () => {
    expect(manualPort({ ...input, label: "ignored" }).value).toEqual({ target: "vm1", port: 5173, remember: false });
  });
  it("accepts a manual SSH target and explicit alternate, sanitizing opt-in labels", () => {
    expect(manualPort({ ...input, target: "dev@lab:2222", local: "15173", remember: true, label: "Frontend\u202e\u0000" }).value)
      .toEqual({ target: "dev@lab:2222", port: 5173, local: 15173, remember: true, label: "Frontend" });
  });
  it("counts label Unicode characters rather than UTF-16 units", () => {
    expect(manualPort({ ...input, remember: true, label: "🚀".repeat(200) }).value?.label).toBe("🚀".repeat(200));
    expect(manualPort({ ...input, remember: true, label: "🚀".repeat(201) }).error).toContain("200");
  });
});
