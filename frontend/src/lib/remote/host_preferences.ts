// Display metadata belongs to the local hub; it never changes an SSH target.
import { plainText } from "../protocol/untrusted.ts";
import { validateTarget } from "./api.ts";

export const MAX_HOST_ALIAS = 200;
const PATH = "/api/remote/host-preferences";

export interface HostPreference {
  target: string;
  alias?: string;
  hidden?: boolean;
}

export function parseHostPreference(value: unknown): HostPreference | undefined {
  if (!value || typeof value !== "object") return;
  const row = value as Record<string, unknown>;
  if (typeof row.target !== "string" || validateTarget(row.target) !== row.target) return;
  const preference: HostPreference = { target: row.target };
  if (typeof row.alias === "string") {
    const alias = plainText(row.alias, MAX_HOST_ALIAS * 2).trim();
    if (alias && [...alias].length <= MAX_HOST_ALIAS) preference.alias = alias;
  }
  if (row.hidden === true) preference.hidden = true;
  return preference;
}

interface Effects {
  fetch(path: string, init?: RequestInit): Promise<Response>;
  changed(): void;
  visibility(target: string, hidden: boolean): void;
}

export class HostPreferences {
  private rows = new Map<string, HostPreference>();
  private revision = 0;
  private readonly revisions = new Map<string, number>();
  private readSequence = 0;
  private disposed = false;
  private readonly pending = new Map<string, Promise<void>>();

  constructor(private readonly fx: Effects) {}

  entries(): HostPreference[] { return [...this.rows.values()].map((row) => ({ ...row })); }
  isHidden(target: string): boolean { return this.rows.get(target)?.hidden === true; }

  async load(): Promise<void> {
    const revision = this.revision;
    const sequence = ++this.readSequence;
    try {
      const response = await this.fx.fetch(PATH);
      if (!response.ok) return;
      const value = await response.json();
      if (this.disposed || sequence !== this.readSequence || value?.version !== 1 || !Array.isArray(value.hosts)) return;
      const rows = new Map<string, HostPreference>();
      for (const valueRow of value.hosts) {
        const row = parseHostPreference(valueRow);
        if (row && !rows.has(row.target)) rows.set(row.target, row);
      }
      const targets = new Set([...this.rows.keys(), ...rows.keys()]);
      for (const target of targets) {
        if ((this.revisions.get(target) ?? 0) <= revision) continue;
        const current = this.rows.get(target);
        if (current) rows.set(target, current);
        else rows.delete(target);
      }
      this.rows = rows;
      for (const target of targets) this.fx.visibility(target, this.isHidden(target));
      this.fx.changed();
    } catch { /* Keep the last successful metadata when the hub is unavailable. */ }
  }

  async setAlias(target: string, input: string): Promise<void> {
    const alias = input.trim();
    if ([...alias].length > MAX_HOST_ALIAS || /[\u0000-\u001f\u007f-\u009f]/u.test(alias)) {
      throw new Error("Use at most 200 characters without control characters.");
    }
    await this.enqueue(target, () => this.update(target, { alias: alias || null }));
  }

  async reveal(target: string): Promise<void> {
    await this.enqueue(target, () => this.update(target, { hidden: false }));
  }

  async forget(target: string): Promise<void> {
    await this.enqueue(target, async () => {
      this.touch(target);
      const response = await this.fx.fetch("/api/remote/hosts", {
        method: "DELETE", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ target }),
      });
      if (!response.ok) throw new Error(`Could not remove host (HTTP ${response.status}).`);
      if (!this.disposed) this.publish({ target, hidden: true });
    });
  }

  dispose(): void { this.disposed = true; }

  private async update(target: string, patch: { alias?: string | null; hidden?: false }): Promise<void> {
    this.touch(target);
    const response = await this.fx.fetch(PATH, {
      method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ target, ...patch }),
    });
    if (!response.ok) throw new Error(`Could not save host (HTTP ${response.status}).`);
    const row = parseHostPreference(await response.json());
    if (!row || row.target !== target) throw new Error("The hub returned an invalid host preference.");
    if (!this.disposed) this.publish(row);
  }

  private enqueue(target: string, operation: () => Promise<void>): Promise<void> {
    const previous = this.pending.get(target);
    const next = previous ? previous.catch(() => {}).then(operation) : operation();
    this.pending.set(target, next);
    void next.finally(() => { if (this.pending.get(target) === next) this.pending.delete(target); }).catch(() => {});
    return next;
  }

  private touch(target: string): void {
    this.revisions.set(target, ++this.revision);
  }

  private publish(row: HostPreference): void {
    this.touch(row.target);
    this.rows.set(row.target, row);
    this.fx.visibility(row.target, row.hidden === true);
    this.fx.changed();
  }
}
