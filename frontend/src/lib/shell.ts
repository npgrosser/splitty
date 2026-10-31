const SHELLS = new Set(["bash", "zsh", "sh", "fish", "dash", "ksh", "csh", "tcsh", "nu", "pwsh"]);

function normalizeExecutable(token: string): string {
  const cleaned = token.trim();
  if (!cleaned) return "";

  const parts = cleaned.split(/[\\/]/);
  const base = parts[parts.length - 1] ?? cleaned;
  return base.replace(/^-+/, "").toLowerCase();
}

export function normalizeKnownShellName(name?: string): string | undefined {
  if (!name) return undefined;
  const normalized = normalizeExecutable(name);
  if (!SHELLS.has(normalized)) return undefined;
  return normalized;
}
