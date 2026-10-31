import { settings, updateSetting } from "./settings.svelte.ts";

// "webapps" removed with the web-app panes (security review SR-01).
export type DrawerState = "closed" | "open" | "settings" | "templates" | "hosts";

export interface ContextMenuState {
  items: ContextMenuItem[];
  x: number;
  y: number;
}

export interface ContextMenuItem {
  children?: ContextMenuItem[];
  label: string;
  detail?: string;
  badge?: string;
  action: () => void;
  shortcut?: string;
  danger?: boolean;
  separator?: boolean;
  disabled?: boolean;
}

// --- Drawer ---

let drawerState = $state<DrawerState>("closed");

export function getDrawerState(): DrawerState {
  return drawerState;
}

export function setDrawerState(state: DrawerState): void {
  drawerState = state;
}

export function toggleDrawer(): void {
  drawerState = drawerState === "closed" ? "open" : "closed";
}

export function isDrawerPinned(): boolean {
  return settings().drawer.pinned;
}

export function setDrawerPinned(pinned: boolean): void {
  void updateSetting("drawer", "pinned", pinned);
}

// --- Drawer Resize ---

let drawerDragging = $state(false);
let drawerLiveWidth = $state(0);

export function isDrawerDragging(): boolean {
  return drawerDragging;
}

export function setDrawerDragging(v: boolean): void {
  drawerDragging = v;
}

export function getDrawerLiveWidth(): number {
  return drawerLiveWidth;
}

export function setDrawerLiveWidth(w: number): void {
  drawerLiveWidth = w;
}

// --- Shortcuts Overlay ---

let shortcutsOpen = $state(false);

export function isShortcutsOpen(): boolean {
  return shortcutsOpen;
}

export function setShortcutsOpen(open: boolean): void {
  shortcutsOpen = open;
}

// --- Command Palette ---

let commandPaletteOpen = $state(false);

export function isCommandPaletteOpen(): boolean {
  return commandPaletteOpen;
}

export function setCommandPaletteOpen(open: boolean): void {
  commandPaletteOpen = open;
}

// --- Host Picker (remote windows) ---

let hostPickerOpen = $state(false);
let hostPickerTarget = $state<string | undefined>(undefined);

export function isHostPickerOpen(): boolean {
  return hostPickerOpen;
}

/** `target` opens that host at once instead of offering the list. */
export function setHostPickerOpen(open: boolean, target?: string): void {
  hostPickerOpen = open;
  hostPickerTarget = open ? target : undefined;
}

export function getHostPickerTarget(): string | undefined {
  return hostPickerTarget;
}

// --- Host Picker (remote screens) ---

let remoteScreenPickerOpen = $state(false);

export function isRemoteScreenPickerOpen(): boolean {
  return remoteScreenPickerOpen;
}

export function setRemoteScreenPickerOpen(open: boolean): void {
  remoteScreenPickerOpen = open;
}

// --- Context Menu ---

let contextMenu = $state<ContextMenuState | null>(null);

export function getContextMenu(): ContextMenuState | null {
  return contextMenu;
}

export function showContextMenu(items: ContextMenuItem[], x: number, y: number): void {
  contextMenu = { items, x, y };
}

export function hideContextMenu(): void {
  contextMenu = null;
}
