import { apiFetch } from "./protocol/http.ts";
import { getSettings, type TitleBarColorMode } from "./settings.ts";
import { getDesktopPlatform } from "./platform.ts";
import { windowAPI } from "./window.ts";

export interface ResolvedTheme {
  name: string;
  variant: "dark" | "light";
  ui: Record<string, string>;
  terminal: Record<string, string>;
  effects?: Record<string, string>;
  layout?: Record<string, string>;
}

let resolved: ResolvedTheme | null = null;
const listeners = new Set<() => void>();

// --- OS color-scheme detection ---

const darkMq = window.matchMedia("(prefers-color-scheme: dark)");

/** Returns the OS-level preference: "dark" or "light". */
export function getSystemColorScheme(): "dark" | "light" {
  return darkMq.matches ? "dark" : "light";
}

/** Resolves which theme name to use based on settings + OS preference. */
export function resolveActiveThemeName(): string {
  const s = getSettings().appearance;
  if (s.themeMode === "auto") {
    return getSystemColorScheme() === "dark" ? s.themeDark : s.themeLight;
  }
  return s.theme;
}

let osListenerAttached = false;

/** Start listening for OS color-scheme changes. Call once after initial load. */
export function initAutoThemeListener(): void {
  if (osListenerAttached) return;
  osListenerAttached = true;

  darkMq.addEventListener("change", () => {
    const s = getSettings().appearance;
    if (s.themeMode === "auto") {
      reloadTheme(resolveActiveThemeName());
    }
  });
}

const fallback: ResolvedTheme = {
  name: "Splitty Dark",
  variant: "dark",
  ui: {
    "surface-base": "#0f0f0f",
    "surface-raised": "#1a1a1a",
    "surface-overlay": "#1e1e1e",
    "text-primary": "#c8c8c8",
    "text-secondary": "#6b6b6b",
    "text-muted": "#505050",
    "border-default": "#2a2a2a",
    "border-subtle": "#1e1e1e",
    "hover-bg": "rgba(255, 255, 255, 0.06)",
    "active-bg": "rgba(255, 255, 255, 0.10)",
    "accent": "#63a0ff",
    "accent-muted": "#63a0ff66",
    "danger": "#e06c75",
    "danger-hover": "#e06c7529",
    "warning": "#e5c07b",
    "success": "#98c379",
    "shadow": "rgba(0, 0, 0, 0.45)",
    "scrollbar": "rgba(255, 255, 255, 0.08)",
  },
  terminal: {
    bg: "#0f0f0f",
    fg: "#c8c8c8",
    cursor: "#c8c8c8",
    "cursor-accent": "#0f0f0f",
    "selection-bg": "#2a2a2a",
    "selection-fg": "#c8c8c8",
    black: "#0f0f0f",
    red: "#e06c75",
    green: "#98c379",
    yellow: "#e5c07b",
    blue: "#63a0ff",
    magenta: "#c678dd",
    cyan: "#56b6c2",
    white: "#c8c8c8",
    "bright-black": "#505050",
    "bright-red": "#e88891",
    "bright-green": "#a9d48b",
    "bright-yellow": "#ecd08e",
    "bright-blue": "#7db4ff",
    "bright-magenta": "#d48ce6",
    "bright-cyan": "#6bc5d0",
    "bright-white": "#fafafa",
  },
};

/** Boolean-ish effect keys that get mirrored as data-effect-* attributes. */
const BOOLEAN_EFFECTS = ["glow", "scanlines"];

function applyCssVariables(theme: ResolvedTheme): void {
  const root = document.documentElement;

  for (const [key, val] of Object.entries(theme.ui)) {
    root.style.setProperty(`--splitty-${key}`, val);
  }

  // Clear previous effect variables and data attributes.
  for (const name of BOOLEAN_EFFECTS) {
    root.removeAttribute(`data-effect-${name}`);
  }
  root.dataset.effectList?.split(",").forEach((k) => {
    if (k) root.style.removeProperty(`--splitty-effect-${k}`);
  });

  const effectKeys: string[] = [];
  if (theme.effects) {
    for (const [key, val] of Object.entries(theme.effects)) {
      root.style.setProperty(`--splitty-effect-${key}`, val);
      effectKeys.push(key);

      if (BOOLEAN_EFFECTS.includes(key) && val === "true") {
        root.setAttribute(`data-effect-${key}`, "true");
      }
    }
  }
  // Stash key list so we can clean them up on next theme switch.
  root.dataset.effectList = effectKeys.join(",");

  // Tab accent position (top/bottom) — drives CSS selector for accent line.
  const accentPos = theme.ui["tab-accent-position"];
  if (accentPos && accentPos !== "top") {
    root.dataset.tabAccentPosition = accentPos;
  } else {
    delete root.dataset.tabAccentPosition;
  }

  syncNativeTitleBarAppearance(theme);
}

function parseHexRgb(hex: string): [number, number, number] {
  const h = hex.replace("#", "");
  return [
    parseInt(h.slice(0, 2), 16) || 0,
    parseInt(h.slice(2, 4), 16) || 0,
    parseInt(h.slice(4, 6), 16) || 0,
  ];
}

function resolveTitleBarColor(theme: ResolvedTheme, mode: TitleBarColorMode): [number, number, number] | null {
  switch (mode) {
    case "native":
      return null;
    case "terminal":
      return parseHexRgb(theme.terminal.bg);
    case "activeTab":
      return parseHexRgb(theme.ui["surface-base"] ?? fallback.ui["surface-base"]);
    case "tabBar":
    default:
      return parseHexRgb(theme.ui["surface-raised"] ?? fallback.ui["surface-raised"]);
  }
}

export function syncNativeTitleBarAppearance(theme: ResolvedTheme = resolved ?? fallback): void {
  if (getDesktopPlatform() !== "mac") return;

  const windowSettings = getSettings().window;
  if (!windowSettings.nativeTitleBar) return;

  if (windowSettings.titleBarColorMode === "native") {
    windowAPI.setTitleBarStyle("visible");
    return;
  }

  windowAPI.setTitleBarStyle("transparent");
  const rgb = resolveTitleBarColor(theme, windowSettings.titleBarColorMode);
  if (rgb) {
    const [r, g, b] = rgb;
    windowAPI.setBackgroundColor(r, g, b);
  }
}

export function getXtermTheme(): Record<string, string> {
  const t = (resolved ?? fallback).terminal;
  return {
    background: t.bg,
    foreground: t.fg,
    cursor: t.cursor,
    cursorAccent: t["cursor-accent"],
    selectionBackground: t["selection-bg"],
    selectionForeground: t["selection-fg"],
    black: t.black,
    red: t.red,
    green: t.green,
    yellow: t.yellow,
    blue: t.blue,
    magenta: t.magenta,
    cyan: t.cyan,
    white: t.white,
    brightBlack: t["bright-black"],
    brightRed: t["bright-red"],
    brightGreen: t["bright-green"],
    brightYellow: t["bright-yellow"],
    brightBlue: t["bright-blue"],
    brightMagenta: t["bright-magenta"],
    brightCyan: t["bright-cyan"],
    brightWhite: t["bright-white"],
  };
}

export async function loadTheme(): Promise<ResolvedTheme> {
  if (resolved) return resolved;
  try {
    const res = await apiFetch("/api/home/theme");
    if (res.ok) {
      resolved = await res.json();
      applyCssVariables(resolved!);
      return resolved!;
    }
  } catch {
    // Backend unavailable
  }
  resolved = fallback;
  applyCssVariables(resolved);
  return resolved;
}

export async function reloadTheme(name?: string): Promise<ResolvedTheme> {
  try {
    const url = name
      ? `/api/home/theme?name=${encodeURIComponent(name)}`
      : "/api/home/theme";
    const res = await apiFetch(url);
    if (res.ok) {
      resolved = await res.json();
      applyCssVariables(resolved!);
      for (const cb of listeners) cb();
      return resolved!;
    }
  } catch {
    // keep current
  }
  return resolved ?? fallback;
}

export function getTheme(): ResolvedTheme {
  return resolved ?? fallback;
}

export function subscribeTheme(cb: () => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}
