import { apiFetch } from "./protocol/http.ts";

export interface TerminalConfig {
  fontSize: number;
  fontFamily: string;
  cursorStyle: "bar" | "block" | "underline";
}

const defaults: TerminalConfig = {
  fontSize: 14,
  fontFamily: "'JetBrainsMono Nerd Font Mono', 'JetBrains Mono', monospace",
  cursorStyle: "bar",
};

let resolved: TerminalConfig | null = null;

export async function loadConfig(): Promise<TerminalConfig> {
  if (resolved) return resolved;

  try {
    const res = await apiFetch("/api/config");
    if (res.ok) {
      const overrides: Partial<TerminalConfig> = await res.json();
      resolved = { ...defaults, ...overrides };
      return resolved;
    }
  } catch {
    // Backend unavailable — use defaults
  }

  resolved = defaults;
  return resolved;
}

export function getConfig(): TerminalConfig {
  return resolved ?? defaults;
}
