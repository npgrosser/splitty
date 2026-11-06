import {
  getSettings,
  subscribe,
  updateSetting as _updateSetting,
  updateXtermSetting as _updateXtermSetting,
  type SplittySettings,
  type XtermSettings,
} from "../lib/settings.ts";

// The store may be imported before startup has fetched settings. The app only
// reads it after loadSettings succeeds; direct component tests can seed it.
let current = $state<SplittySettings | undefined>();

subscribe(() => {
  current = getSettings();
});

/** Reactive settings — automatically updates when settings change. */
export function settings(): SplittySettings {
  return current ?? getSettings();
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
