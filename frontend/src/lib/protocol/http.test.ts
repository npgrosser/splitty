import { afterEach, expect, it, vi } from "vitest";
import { apiFetch, assignNativeApi, exchangeBootstrap, exchangeToken, fetchWsTicket } from "./http.ts";
afterEach(() => vi.unstubAllGlobals());
it("keeps native assignment through failure and revocation without browser authentication or fetch", async () => {
  const fetch = vi.fn(); vi.stubGlobal("fetch", fetch);
  const request = vi.fn().mockResolvedValue({ id: 1, status: 200, contentType: "json", body: '{"native":true}' });
  const revoke = assignNativeApi({ request });
  expect(await (await apiFetch("/api/build")).json()).toEqual({ native: true });
  request.mockRejectedValue(new Error("Backend lost")); await expect(apiFetch("/api/build")).rejects.toThrow("Backend lost");
  await expect(exchangeBootstrap("b")).rejects.toThrow("Browser authentication");
  await expect(exchangeToken("t")).rejects.toThrow("Browser authentication");
  await expect(fetchWsTicket()).rejects.toThrow("Browser authentication");
  revoke(); await expect(apiFetch("/api/build")).rejects.toThrow("revoked"); expect(fetch).not.toHaveBeenCalled();
});
