import { editorDefaults, editorPreferences, type EditorPreferences } from "./editor_preferences.ts";
import { apiFetch } from "./protocol/http.ts";

/** Extract a typed value from a select element's change event. */
export function selectValue<T extends string>(e: Event): T {
  return (e.target as HTMLSelectElement).value as T;
}

/** Extract a numeric value from a range/number input's change event. */
export function inputNumber(e: Event): number {
  return Number((e.target as HTMLInputElement).value);
}

// --- Types ---

export type TerminalRenderer = "xterm" | "ghostty";
export type CloseOnExit = "always" | "never" | "graceful";
export type CloseButtonMode = "always" | "active";
// LinkTarget and WebviewMode removed with the web-app panes (security review
// SR-01/SR-02). Terminal links always open in the system browser;
// there is no in-app rendering mode left to choose between.
/** Which clients decide the PTY size of a session several clients show (this server's sessions). */
export type SizePolicy = "active" | "latest" | "smallest";
export type FontWeight = "normal" | "bold" | "100" | "200" | "300" | "400" | "500" | "600" | "700" | "800" | "900";
export type GpuAcceleration = "on" | "off";

export interface XtermSettings {
  gpuAcceleration: GpuAcceleration;
  scrollbarWidth: number;
  scrollbarOverlay: boolean;
  scrollbarFade: boolean;
  scrollSensitivity: number;
  fastScrollSensitivity: number;
  lineHeight: number;
  letterSpacing: number;
  fontWeight: FontWeight;
  fontWeightBold: FontWeight;
  drawBoldTextInBrightColors: boolean;
  minimumContrastRatio: number;
  customGlyphs: boolean;
  rescaleOverlappingGlyphs: boolean;
  allowTransparency: boolean;
  cursorBlink: boolean;
}

export interface TerminalSettings {
  renderer: TerminalRenderer;
  theme: string;
  closeOnExit: CloseOnExit;
  ligatures: boolean;
  windowPaddingBalance: boolean;
  scrollToBottom: boolean;
  /**
   * Allow terminal output to write the system clipboard via OSC 52.
   *
   * Enabled by default for copy in terminal applications. Users can disable
   * writes; reads are always refused. Successful writes show a short notice.
   */
  allowOsc52Write: boolean;
  sizePolicy: SizePolicy;
  /** How long after its user's last action a client still counts as active. */
  sizeIdleSeconds: number;
  xterm: XtermSettings;
}

export type ThemeMode = "fixed" | "auto";
export type ZoomScope = "window" | "terminals";

export interface AppearanceSettings {
  theme: string;
  themeMode: ThemeMode;
  themeDark: string;
  themeLight: string;
  borderRadius: number;
  fontSize: number;
  fontFamily: string;
  /** The zoom level of this computer's windows (display-zoom). */
  zoom: number;
  /** What zooms: the whole window or only the terminals' font size. */
  zoomScope: ZoomScope;
}

export interface TabSettings {
  autoHide: boolean;
  autoHideMulti: boolean;
  syncRow: boolean;
  locked: boolean;
  closeButton: CloseButtonMode;
  minWidth: number;
  showSplit: boolean;
  showFloat: boolean;
  showMaximize: boolean;
  showPopout: boolean;
}

export interface WindowSettings {
  nativeTitleBar: boolean;
  titleBarColorMode: TitleBarColorMode;
  titleBarHeight: number;
  titleBarScroll: boolean;
}

export type TitleBarColorMode =
  | "tabBar"
  | "native"
  | "terminal"
  | "activeTab";

export interface DrawerSettings {
  pinned: boolean;
  edgeReveal: boolean;
  showTips: boolean;
  resizable: boolean;
  sidebarWidth: number;
}

export interface NotificationSettings {
  enabled: boolean;
  toasts: boolean;
  desktop: boolean;
  processFinished: boolean;
  processExited: boolean;
  processCrashed: boolean;
}

export interface ExperimentalSettings {
  linkedSashes: boolean;
  resizeDebounce: number;
}

export interface RemoteSettings {
  /**
   * Forward a port that starts listening on a connected host without asking
   * (`remote-screens`, Automatic Port Forwarding). Off by default: a forward
   * opens a port on this machine that any local process can reach.
   */
  autoForwardPorts: boolean;
}

export interface SplittySettings {
  editors: EditorPreferences;
  terminal: TerminalSettings;
  appearance: AppearanceSettings;
  tabs: TabSettings;
  drawer: DrawerSettings;
  window: WindowSettings;
  notifications: NotificationSettings;
  experimental: ExperimentalSettings;
  remote: RemoteSettings;
}

export const defaults: SplittySettings = {
  editors: structuredClone(editorDefaults),
  terminal: {
    renderer: "xterm",
    theme: "default",
    closeOnExit: "always",
    ligatures: false,
    windowPaddingBalance: true,
    scrollToBottom: true,
    allowOsc52Write: true,
    sizePolicy: "active",
    sizeIdleSeconds: 120,
    xterm: {
      gpuAcceleration: "on",
      scrollbarWidth: 14,
      scrollbarOverlay: true,
      scrollbarFade: true,
      scrollSensitivity: 1,
      fastScrollSensitivity: 5,
      lineHeight: 1.0,
      letterSpacing: 0,
      fontWeight: "normal",
      fontWeightBold: "bold",
      drawBoldTextInBrightColors: true,
      minimumContrastRatio: 1,
      customGlyphs: true,
      rescaleOverlappingGlyphs: false,
      allowTransparency: false,
      cursorBlink: false,
    },
  },
  appearance: {
    theme: "splitty-dark",
    themeMode: "fixed",
    themeDark: "splitty-dark",
    themeLight: "splitty-light",
    borderRadius: 1,
    fontSize: 0,
    zoom: 1,
    zoomScope: "window",
    fontFamily: "",
  },
  tabs: {
    autoHide: false,
    autoHideMulti: false,
    syncRow: false,
    locked: false,
    closeButton: "active",
    minWidth: 12,
    showSplit: true,
    showFloat: false,
    showMaximize: false,
    showPopout: false,
  },
  drawer: {
    pinned: false,
    edgeReveal: true,
    showTips: true,
    resizable: false,
    sidebarWidth: 240,
  },
  window: {
    nativeTitleBar: true,
    titleBarColorMode: "tabBar",
    titleBarHeight: 0,
    titleBarScroll: true,
  },
  notifications: {
    enabled: true,
    toasts: false,
    desktop: false,
    processFinished: true,
    processExited: false,
    processCrashed: true,
  },
  experimental: {
    linkedSashes: false,
    resizeDebounce: 5,
  },
  remote: {
    autoForwardPorts: false,
  },
};

// --- Module State ---

let current: SplittySettings = structuredClone(defaults);
const listeners = new Set<() => void>();

function mergeFromServer(data: Record<string, unknown>): SplittySettings {
  const d = data as Partial<Record<keyof SplittySettings, Record<string, unknown>>>;
  return {
    editors: editorPreferences(d.editors),
    terminal: {
      ...defaults.terminal,
      ...(d.terminal as Partial<TerminalSettings>),
      xterm: { ...defaults.terminal.xterm, ...(d.terminal as Record<string, unknown>)?.xterm as Partial<XtermSettings> },
    },
    appearance: { ...defaults.appearance, ...d.appearance } as AppearanceSettings,
    tabs: { ...defaults.tabs, ...d.tabs } as TabSettings,
    drawer: { ...defaults.drawer, ...d.drawer } as DrawerSettings,
    window: { ...defaults.window, ...d.window } as WindowSettings,
    notifications: { ...defaults.notifications, ...d.notifications } as NotificationSettings,
    experimental: { ...defaults.experimental, ...d.experimental } as ExperimentalSettings,
    remote: { ...defaults.remote, ...d.remote } as RemoteSettings,
  };
}

function notify() {
  for (const cb of listeners) cb();
}

// --- Public API ---

export async function loadSettings(): Promise<SplittySettings> {
  try {
    const res = await apiFetch("/api/home/settings");
    if (res.ok) {
      current = mergeFromServer(await res.json());
      notify();
    }
  } catch {
    // Backend unavailable — use defaults
  }
  return current;
}

export function getSettings(): SplittySettings {
  return current;
}

export function subscribe(cb: () => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

export async function updateSetting<
  S extends keyof SplittySettings,
  K extends keyof SplittySettings[S],
>(section: S, key: K, value: SplittySettings[S][K]): Promise<void> {
  // Optimistic update
  current = {
    ...current,
    [section]: { ...current[section], [key]: value },
  };
  notify();

  try {
    const res = await apiFetch("/api/home/settings", {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ [section]: { [key]: value } }),
    });
    if (res.ok) {
      current = mergeFromServer(await res.json());
      notify();
    }
  } catch {
    // Keep optimistic value
  }
}

export async function updateXtermSetting<K extends keyof XtermSettings>(
  key: K,
  value: XtermSettings[K],
): Promise<void> {
  current = {
    ...current,
    terminal: {
      ...current.terminal,
      xterm: { ...current.terminal.xterm, [key]: value },
    },
  };
  notify();

  try {
    const res = await apiFetch("/api/home/settings", {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ terminal: { xterm: { [key]: value } } }),
    });
    if (res.ok) {
      current = mergeFromServer(await res.json());
      notify();
    }
  } catch {
    // Keep optimistic value
  }
}
