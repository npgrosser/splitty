// The order of the host groups is the client's own: the local hub keeps it
// (remote-hosts, Host Group Order); a host never learns of it.
import { validateTarget } from "./api.ts";

const PATH = "/api/remote/host-order";
/** The most targets the hub keeps. */
export const MAX_HOST_ORDER = 256;

/** The valid targets of a stored order, each once; null for an answer without its shape. */
export function parseHostOrder(value: unknown): string[] | null {
  if (!value || typeof value !== "object") return null;
  const { version, order } = value as Record<string, unknown>;
  if (version !== 1 || !Array.isArray(order)) return null;
  const targets: string[] = [];
  for (const row of order) {
    if (targets.length === MAX_HOST_ORDER) break;
    if (typeof row === "string" && validateTarget(row) === row && !targets.includes(row)) targets.push(row);
  }
  return targets;
}

interface Effects {
  fetch(path: string, init?: RequestInit): Promise<Response>;
  /** The order to show. */
  changed(order: string[]): void;
}

/** Loads and stores the order through the local hub. */
export class HostOrderStore {
  /** Only the answer to the latest request is shown. */
  private sequence = 0;

  constructor(private readonly fx: Effects) {}

  async load(): Promise<void> {
    const sequence = ++this.sequence;
    try {
      const response = await this.fx.fetch(PATH);
      if (!response.ok) return;
      const order = parseHostOrder(await response.json());
      if (order && sequence === this.sequence) this.fx.changed(order);
    } catch { /* Keep the order shown while the hub is unavailable. */ }
  }

  /** Store `order`: shown at once; the hub's answer, or its stored order after a refusal, wins. */
  async save(order: string[]): Promise<void> {
    const sequence = ++this.sequence;
    this.fx.changed(order);
    try {
      const response = await this.fx.fetch(PATH, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ order }),
      });
      if (sequence !== this.sequence) return;
      if (!response.ok) {
        await this.load();
        return;
      }
      const stored = parseHostOrder(await response.json());
      if (stored) this.fx.changed(stored);
    } catch { /* Keep the order shown; the next load corrects it. */ }
  }
}
