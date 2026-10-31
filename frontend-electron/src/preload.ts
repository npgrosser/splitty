import { contextBridge, ipcRenderer, type IpcRendererEvent } from "electron";

const ALLOWED_CHANNELS = [
  "window:minimize",
  "window:maximize",
  "window:unmaximize",
  "window:toggle-maximize",
  "window:close",
  "window:force-close",
  "window:is-maximized",
  "window:start-drag",
  "window:set-zoom",
  "shell:open-external",
  "editor:open-remote",
  "editor:open-local",
  "download:reveal",
  // Remote windows (desktop-remote). Remote windows themselves get no preload.
  "remote:hosts",
  "remote:open",
  "remote:cancel",
];

contextBridge.exposeInMainWorld("__SPLITTY_ELECTRON__", {
  invoke: (channel: string, ...args: unknown[]) => {
    if (!ALLOWED_CHANNELS.includes(channel)) {
      return Promise.reject(new Error(`IPC channel not allowed: ${channel}`));
    }
    return ipcRenderer.invoke(channel, ...args);
  },
  onCloseRequested: (callback: () => void) => {
    const listener = () => callback();
    ipcRenderer.on("window:close-requested", listener);
    return () => { ipcRenderer.removeListener("window:close-requested", listener); };
  },
  /** Progress of a `remote:open` this window started: `{ target, step, message }`. */
  onRemoteProgress: (callback: (progress: unknown) => void) => {
    const listener = (_event: IpcRendererEvent, progress: unknown) => callback(progress);
    ipcRenderer.on("remote:progress", listener);
    return () => { ipcRenderer.removeListener("remote:progress", listener); };
  },
  platform: process.platform,
});
