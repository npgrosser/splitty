import { applyServerSettings, type SplittySettings } from "../settings.ts";

/** Seed the server-owned sections used by component and connection tests. */
export function seedSettingsForTest(): void {
  applyServerSettings({
    drawer: { pinned: false, edgeReveal: false, showTips: false, resizable: false, sidebarWidth: 200 },
    notifications: {
      enabled: true, toasts: true, desktop: false,
      processFinished: true, processExited: true, processCrashed: true, programStatus: true,
    },
  } as SplittySettings);
}
