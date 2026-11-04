import { describe, expect, it, vi } from "vitest";
import { nativeApiFetch, nativeOperations, nativeRequest } from "./native_api.ts";

describe("enumerated native API", () => {
  it("maps every published operation without sending paths, methods or authority", () => {
    for (const operation of nativeOperations) {
      let path = operation.path.replaceAll(/\{([^}]+)\}/g, "example%20id");
      const query = operation.arguments.filter((arg) => arg.endsWith("?"));
      if (query.length) path += "?" + query.map((arg) => `${arg.slice(0, -1)}=value`).join("&");
      const result = nativeRequest(path, { method: operation.method, ...(operation.arguments.includes("body") ? { body: '{"example":true}' } : {}) });
      expect(result.operation).toBe(operation.operation);
      expect(Object.keys(result)).toEqual(["operation", "args"]);
      expect(Object.keys(result.args).sort()).toEqual(operation.arguments.map((arg) => arg.replace(/\?$/, "")).sort());
    }
  });
  it.each([
    "https://evil.example/api/build", "//localhost/api/build", "/api/../api/build", "/api/%2e%2e/build",
    "/api/build#x", "/api/build?target=vm2", "/api/home/theme?name=x&name=y", "/api/session",
    "/api/ws-ticket", "/api/bootstrap", "/api/agents", "/api/daemon/stop", "/api/templates/a%2fb",
    "/api/templates/a%5cb", "/api/templates/%00", "/api/templates/%xx", "/proxy/anything",
  ])("rejects unclassified paths: %s", (path) => expect(() => nativeRequest(path)).toThrow());
  it("rejects credential headers, non-JSON bodies, wrong methods and extra bodies", () => {
    expect(() => nativeRequest("/api/build", { headers: { Authorization: "Bearer token" } })).toThrow();
    expect(() => nativeRequest("/api/build", { body: "{}" })).toThrow();
    expect(() => nativeRequest("/api/build", { method: "DELETE" })).toThrow();
    expect(() => nativeRequest("/api/templates/test", { method: "POST", body: new Blob(["{}"]) })).toThrow();
    expect(() => nativeRequest("/api/home/settings", { method: "PATCH", body: "bad JSON" })).toThrow();
  });
  it("returns identical JSON/text/status results without a network fallback", async () => {
    const request = vi.fn().mockResolvedValueOnce({ id: 1, status: 200, contentType: "json", body: '{"version":"test"}' })
      .mockResolvedValueOnce({ id: 2, status: 409, contentType: "text", body: "Template already exists" })
      .mockResolvedValueOnce({ id: 3, status: 204, contentType: "empty", body: "" });
    const fetch = nativeApiFetch({ request });
    expect(await (await fetch("/api/build")).json()).toEqual({ version: "test" });
    const conflict = await fetch("/api/templates/test", { method: "POST", body: "{}" });
    expect(conflict.ok).toBe(false); expect(conflict.status).toBe(409); expect(await conflict.text()).toBe("Template already exists");
    const empty = await fetch("/api/templates/test", { method: "DELETE" });
    expect(empty.status).toBe(204); expect(await empty.text()).toBe("");
    expect(request.mock.calls.map(([value]) => value.operation)).toEqual(["build.get", "template.save", "template.delete"]);
  });
  it("retains AbortSignal and uncertainty instead of retrying mutations", async () => {
    const controller = new AbortController();
    const request = vi.fn().mockResolvedValue({ id: 1, status: 200, contentType: "json", body: "{}", uncertain: true });
    const fetch = nativeApiFetch({ request });
    await expect(fetch("/api/templates/test", { method: "POST", body: "{}", signal: controller.signal })).rejects.toMatchObject({ uncertain: true });
    expect(request.mock.calls[0]![1]).toEqual({ signal: controller.signal });
    controller.abort(); await expect(fetch("/api/build", { signal: controller.signal })).rejects.toThrow();
    expect(request).toHaveBeenCalledTimes(1);
  });
});
