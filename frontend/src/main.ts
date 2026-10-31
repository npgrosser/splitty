import { mount } from "svelte";
import { establishSession } from "./stores/auth.svelte.ts";
import { reloadTheme, resolveActiveThemeName, initAutoThemeListener } from "./lib/theme.ts";
import { loadSettings } from "./lib/settings.ts";
import { loadShellProfiles } from "./lib/shell_profiles.ts";
import "./app.css";
import App from "./App.svelte";

// Redeem the bootstrap code before anything else: the loads below are
// authenticated API calls, and the session cookie has to exist by then.
await establishSession();

// Load settings first so theme resolution can read themeMode, then load/apply theme.
Promise.all([loadSettings(), loadShellProfiles()]).then(async () => {
  await reloadTheme(resolveActiveThemeName());
  initAutoThemeListener();
  mount(App, {
    target: document.getElementById("app")!,
  });
});
