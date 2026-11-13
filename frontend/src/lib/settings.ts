import type { EditorPreferences } from "./editor_preferences.ts";
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
  /** Notifications for program status reports; absent from an older server, where it is on. */
  programStatus?: boolean;
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
  /**
   * The hosts a window connects at its start when it brings back remembered
   * screens of them; others connect when one of their screens is shown.
   * Absent on older servers.
   */
  reconnectHosts?: string[];
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

export interface NumericLimit { min: number; max: number; step: number; values?: number[] }
export interface ChoiceLimit { values: (string | number)[] }
type FieldLimit = NumericLimit | ChoiceLimit;
let limits: Record<string, FieldLimit> = {};
/** Server-provided defaults. On older hosts, the loaded settings are the reset baseline. */
export let defaults = {} as SplittySettings;
export function numericLimit(path: string, fallback: NumericLimit): NumericLimit {
  const limit = limits[path];
  return limit && "min" in limit ? limit : fallback;
}
export function allowedValues(path: string): (string | number)[] | undefined {
  return limits[path]?.values;
}
export function isAllowed(path: string, value: string | number): boolean {
  const values = allowedValues(path);
  return !values || values.includes(value);
}

// --- Module State ---

let current: SplittySettings | undefined;
let confirmed: SplittySettings | undefined;
let changeRevision = 0;
let lastError: { field?: string; error: string } | null = null;
const listeners = new Set<() => void>();

function notify(): void { for (const cb of listeners) cb(); }

function merge(base: unknown, patch: unknown): unknown {
  if (base && patch && typeof base === "object" && typeof patch === "object" && !Array.isArray(base) && !Array.isArray(patch)) {
    const result = { ...base } as Record<string, unknown>;
    for (const [key, value] of Object.entries(patch)) result[key] = merge(result[key], value);
    return result;
  }
  return patch ?? base;
}

function syncTheme(previous: SplittySettings | undefined, next: SplittySettings): void {
  const before = previous?.appearance;
  const after = next.appearance;
  if (!before || !after) return;
  if (before.theme !== after.theme || before.themeMode !== after.themeMode ||
      before.themeDark !== after.themeDark || before.themeLight !== after.themeLight) {
    void import("./theme.ts").then((theme) => theme.reloadTheme(theme.resolveActiveThemeName()));
  }
}

export function applyServerSettings(data: SplittySettings): void {
  const previous = current;
  // A settings.changed event can be the first authoritative payload. Older
  // hosts also have no metadata endpoint, so their values are the reset base.
  if (!Object.keys(defaults).length) defaults = structuredClone(data);
  confirmed = merge(defaults, data) as SplittySettings;
  current = confirmed;
  changeRevision++;
  lastError = null;
  notify();
  syncTheme(previous, current);
}

export function getSettingsError(): { field?: string; error: string } | null { return lastError; }

/** Read current settings first; metadata is an optional newer-server capability. */
export async function loadSettings(): Promise<SplittySettings> {
  const res = await apiFetch("/api/home/settings");
  if (!res.ok) throw new Error(`Failed to load settings (${res.status})`);
  const data = await res.json() as SplittySettings;
  try {
    const metadata = await apiFetch("/api/home/settings/metadata");
    if (metadata.ok) {
      const body = await metadata.json() as { defaults: SplittySettings; limits: Record<string, FieldLimit> };
      if (body.defaults && body.limits) { defaults = body.defaults; limits = body.limits; }
      else defaults = structuredClone(data);
    } else defaults = structuredClone(data);
  } catch { defaults = structuredClone(data); }
  applyServerSettings(data);
  return getSettings();
}

export function getSettings(): SplittySettings {
  if (!current) throw new Error("Settings have not loaded");
  return current;
}
export function subscribe(cb: () => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

async function savePatch(patch: object): Promise<void> {
  const startRevision = changeRevision;
  try {
    const res = await apiFetch("/api/home/settings", {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(patch),
    });
    if (!res.ok) {
      const body = await res.json().catch(() => ({})) as { field?: string; error?: string };
      throw Object.assign(new Error(body.error ?? `Failed to save settings (${res.status})`), { field: body.field });
    }
    const saved = await res.json() as SplittySettings;
    // Another window's event may have arrived during this request; fetch the newest state.
    if (changeRevision !== startRevision) await loadSettings();
    else applyServerSettings(saved);
  } catch (error) {
    const previous = getSettings();
    current = confirmed ?? previous;
    lastError = { field: error instanceof Error && "field" in error ? String(error.field) : undefined,
      error: error instanceof Error ? error.message : String(error) };
    notify();
    syncTheme(previous, current);
  }
}

export async function updateSetting<
  S extends keyof SplittySettings,
  K extends keyof SplittySettings[S],
>(section: S, key: K, value: SplittySettings[S][K]): Promise<void> {
  const previous = getSettings();
  current = { ...previous, [section]: { ...previous[section], [key]: value } };
  lastError = null;
  notify();
  await savePatch({ [section]: { [key]: value } });
}

export async function updateXtermSetting<K extends keyof XtermSettings>(key: K, value: XtermSettings[K]): Promise<void> {
  const previous = getSettings();
  current = { ...previous, terminal: { ...previous.terminal, xterm: { ...previous.terminal.xterm, [key]: value } } };
  lastError = null;
  notify();
  await savePatch({ terminal: { xterm: { [key]: value } } });
}
