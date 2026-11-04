import { bootstrapConnection } from "./lib/protocol/bootstrap.ts";
import "./app.css";

try {
  const finishStartup = await bootstrapConnection();
  try {
    // Platform-dependent modules must load after the native assignment.
    const { establishSession } = await import("./stores/auth.svelte.ts");
    await establishSession();
    const { bootstrapBrowser } = await import("./lib/protocol/browser_assignment.ts");
    await bootstrapBrowser();
    const [{ loadSettings }, { loadShellProfiles }, theme] = await Promise.all([
      import("./lib/settings.ts"), import("./lib/shell_profiles.ts"), import("./lib/theme.ts"),
    ]);
    await Promise.all([loadSettings(), loadShellProfiles()]);
    await theme.reloadTheme(theme.resolveActiveThemeName());
    theme.initAutoThemeListener();
  } finally { finishStartup(); }
  const [{ mount }, { default: App }] = await Promise.all([import("svelte"), import("./App.svelte")]);
  mount(App, { target: document.getElementById("app")! });
} catch (error) {
  // Fail visibly without starting a different transport or rendering backend
  // diagnostic text as HTML.
  const message = document.createElement("p");
  message.textContent = `Splitty could not open this workspace: ${error instanceof Error ? error.message : String(error)}`;
  document.getElementById("app")?.replaceChildren(message);
}
