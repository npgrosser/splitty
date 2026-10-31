import {
  getSettings,
  subscribe,
  updateSetting as _updateSetting,
  updateXtermSetting as _updateXtermSetting,
  type SplittySettings,
  type XtermSettings,
} from "../lib/settings.ts";

let current = $state<SplittySettings>(getSettings());

subscribe(() => {
  current = getSettings();
});

/** Reactive settings — automatically updates when settings change. */
export function settings(): SplittySettings {
  return current;
}

/** Update a setting within a section. Optimistic + persisted to backend. */
export async function updateSetting<
  S extends keyof SplittySettings,
  K extends keyof SplittySettings[S],
>(section: S, key: K, value: SplittySettings[S][K]): Promise<void> {
  return _updateSetting(section, key, value);
}

/** Update a single xterm sub-setting. */
export async function updateXtermSetting<K extends keyof XtermSettings>(
  key: K,
  value: XtermSettings[K],
): Promise<void> {
  return _updateXtermSetting(key, value);
}
