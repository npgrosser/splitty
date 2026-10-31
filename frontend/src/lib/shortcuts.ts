// Centralized keyboard shortcut definitions.
// Single source of truth for all Splitty shortcuts.
// Used by keyboard handlers and UI components (tooltips, labels).

export const isMac = navigator.platform.toUpperCase().includes("MAC");
export const modKey = (e: KeyboardEvent) => (isMac ? e.metaKey : e.altKey);

const mod = isMac ? "⌘" : "Alt+";
const shift = isMac ? "⇧" : "Shift+";
const ctrl = isMac ? "⌃" : "Ctrl+";

export const shortcuts = {
  drawer:           { label: "Drawer",             key: `${ctrl}B` },
  newTab:           { label: "New Tab",            key: `${mod}T` },
  splitRight:       { label: "Split Right",        key: `${mod}D` },
  splitDown:        { label: "Split Down",         key: `${mod}${shift}D` },
  closePane:        { label: "Close Tab",          key: `${mod}W` },
  newScreen:        { label: "New Screen",          key: `${ctrl}${shift}Space` },
  settings:         { label: "Settings",           key: `${ctrl},` },
  templates:        { label: "Templates",          key: `${mod}P` },
  commandPalette:   { label: "Command Palette",    key: `${mod}${shift}P` },
  toggleTabBar:     { label: "Toggle Tab Bar",     key: `${mod}${shift}X` },
  holdRevealTabBar: { label: "Reveal Tab Bar",     key: `${mod}${ctrl}X` },
  cycleNext:        { label: "Next Pane",          key: `${mod}${shift}→` },
  cyclePrev:        { label: "Previous Pane",      key: `${mod}${shift}←` },
  focusUp:          { label: "Focus Up",           key: `${mod}${shift}↑` },
  focusDown:        { label: "Focus Down",         key: `${mod}${shift}↓` },
  resizePane:       { label: "Resize Pane",        key: `${ctrl}${shift}Arrow` },
  maximizePane:     { label: "Maximize Pane",      key: `${mod}M` },
  openWebPage:      { label: "Open Web Page",      key: `${mod}O` },
  scratchpad:       { label: "Scratchpad",         key: `${mod}J` },
} as const;

export type ShortcutId = keyof typeof shortcuts;

/** Get the display key combo, e.g. "⌘T" or "Alt+T" */
export function shortcutKey(id: ShortcutId): string {
  return shortcuts[id].key;
}

/** Get "Label (Key)" string for use in title attributes */
export function shortcutTitle(id: ShortcutId): string {
  return `${shortcuts[id].label} (${shortcutKey(id)})`;
}

/** Get shortcut key for switching to screen N (1-9) */
export function screenShortcutKey(n: number): string {
  return `${mod}${shift}${n}`;
}
