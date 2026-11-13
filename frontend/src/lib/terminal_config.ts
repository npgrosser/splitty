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

export function getConfig(): TerminalConfig {
  return defaults;
}
