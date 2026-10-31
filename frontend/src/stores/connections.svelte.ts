// The window's host connections as the UI shows them (`remote-screens`): per
// host its connection's phase, progress, error and daemon version, the host's
// screens the window does not show (the reopen offer and "On hosts"), and per
// remote screen its flags; and the local server's version, which an updated
// host gets. Fed by the plain classes that own the state (`RemoteSocket`,
// `RemoteScreenSync`); nothing here sends anything.

import type { ConnKey } from "../lib/connection/server_connection.ts";
import type { HostConnectionView } from "../lib/remote/on_hosts.ts";
import type { HostScreen, RemoteSocket, RemoteState } from "../lib/remote/remote_socket.ts";
import type { RemoteScreenFlags } from "../lib/remote/screen_status.ts";

interface HostView {
  state: RemoteState;
  available: HostScreen[];
}

const NO_FLAGS: RemoteScreenFlags = { pending: false, needsChoice: false, missing: false };

let hosts = $state<Record<string, HostView>>({});
let flags = $state<Record<string, RemoteScreenFlags>>({});
let localVersion = $state<string | undefined>(undefined);
let localWslDistro: string | undefined;

/** Follow a host's socket until the returned function is called. */
export function trackHost(key: ConnKey, socket: RemoteSocket): () => void {
  hosts = { ...hosts, [key]: { state: socket.state, available: [] } };
  const unsubscribe = socket.onState((state) => {
    const current = hosts[key];
    if (current) hosts = { ...hosts, [key]: { ...current, state } };
  });
  return () => {
    unsubscribe();
    const { [key]: _gone, ...rest } = hosts;
    hosts = rest;
  };
}

/** The state of a host's connection; undefined for the local server or a host no longer connected. */
export function hostState(key: ConnKey): RemoteState | undefined {
  return hosts[key]?.state;
}

/** The host's screens the window does not show. */
export function availableScreens(key: ConnKey): HostScreen[] {
  return hosts[key]?.available ?? [];
}

/** Every host connection of the window. */
export function hostViews(): HostConnectionView[] {
  return Object.entries(hosts).map(([key, view]) => ({ key: key as ConnKey, state: view.state, available: view.available }));
}

/** The local server's version (`/api/build`), once known. */
export function getLocalVersion(): string | undefined {
  return localVersion;
}

export function setLocalVersion(version: string): void {
  localVersion = version;
}

/** The WSL distribution the local server runs in (`/api/build`), if it runs in one. */
let localEditorContextReady = false;
export function hasLocalEditorContext(): boolean { return localEditorContextReady; }

export function getLocalWslDistro(): string | undefined {
  return localWslDistro;
}

export function setLocalWslDistro(distro: string | undefined): void {
  localWslDistro = distro;
  localEditorContextReady = true;
}

/** What a host's screen sync knows changed: its available screens and its screens' flags. */
export function setHostScreens(key: ConnKey, available: HostScreen[], screenFlags: Record<string, RemoteScreenFlags>): void {
  const current = hosts[key];
  if (current) hosts = { ...hosts, [key]: { ...current, available } };
  flags = { ...flags, ...screenFlags };
}

export function remoteScreenFlags(screenKey: string): RemoteScreenFlags {
  return flags[screenKey] ?? NO_FLAGS;
}

/** The screen left the window. */
export function forgetScreenFlags(screenKey: string): void {
  if (!(screenKey in flags)) return;
  const { [screenKey]: _gone, ...rest } = flags;
  flags = rest;
}
