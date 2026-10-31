import { describe, expect, it } from "vitest";
import { HostOrderStore, MAX_HOST_ORDER, parseHostOrder } from "./host_order.ts";

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });

function store(answers: Response[]) {
  const requests: { path: string; init?: RequestInit }[] = [];
  const shown: string[][] = [];
  const hostOrder = new HostOrderStore({
    fetch: async (path, init) => {
      requests.push({ path, init });
      const answer = answers.shift();
      if (!answer) throw new Error("hub unavailable");
      return answer;
    },
    changed: (order) => shown.push(order),
  });
  return { hostOrder, requests, shown };
}

describe("parseHostOrder", () => {
  it("keeps valid targets once and refuses an answer without the shape", () => {
    expect(parseHostOrder({ version: 1, order: ["vm2", " vm1", 7, "vm2", "vm1"] })).toEqual(["vm2", "vm1"]);
    expect(parseHostOrder({ version: 2, order: [] })).toBeNull();
    expect(parseHostOrder({ version: 1 })).toBeNull();
    expect(parseHostOrder(null)).toBeNull();
    const many = Array.from({ length: MAX_HOST_ORDER + 5 }, (_, i) => `vm${i}`);
    expect(parseHostOrder({ version: 1, order: many })).toHaveLength(MAX_HOST_ORDER);
  });
});

describe("HostOrderStore", () => {
  it("loads the stored order", async () => {
    const { hostOrder, requests, shown } = store([json({ version: 1, order: ["vm2", "vm1"] })]);
    await hostOrder.load();
    expect(requests[0].path).toBe("/api/remote/host-order");
    expect(shown).toEqual([["vm2", "vm1"]]);
  });

  it("shows a saved order at once and then what the hub stored", async () => {
    const { hostOrder, requests, shown } = store([json({ version: 1, order: ["vm2"] })]);
    await hostOrder.save(["vm2", "vm2"]);
    expect(requests[0].init?.method).toBe("PUT");
    expect(JSON.parse(String(requests[0].init?.body))).toEqual({ order: ["vm2", "vm2"] });
    expect(shown).toEqual([["vm2", "vm2"], ["vm2"]]);
  });

  it("goes back to the stored order when the hub refuses one", async () => {
    const { hostOrder, shown } = store([json({ error: "bad" }, 400), json({ version: 1, order: ["vm1"] })]);
    await hostOrder.save(["bad target"]);
    expect(shown).toEqual([["bad target"], ["vm1"]]);
  });

  it("keeps the order shown while the hub is unavailable", async () => {
    const { hostOrder, shown } = store([]);
    await hostOrder.load();
    await hostOrder.save(["vm1"]);
    expect(shown).toEqual([["vm1"]]);
  });
});
