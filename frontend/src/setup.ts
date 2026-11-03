import "./setup.css";

try {
  const capsule = (window as unknown as { __SPLITTY_TAURI__?: { ready: Promise<{ role: string }> } }).__SPLITTY_TAURI__;
  if (!capsule || (await capsule.ready).role !== "setup") throw Error("Native setup document is unavailable");
  const { assignPlatform } = await import("./lib/platform.ts");
  assignPlatform("tauri", "main");
  // Import the setup UI only after native assignment so its shell surfaces
  // remain available for remote-only use without starting WSL.
  const [{ mount }, { default: Setup }] = await Promise.all([import("svelte"), import("./Setup.svelte")]);
  mount(Setup, { target: document.getElementById("app")! });
} catch (error) {
  const message = document.createElement("p");
  message.textContent = `Splitty setup could not open: ${error instanceof Error ? error.message : String(error)}`;
  document.getElementById("app")?.replaceChildren(message);
}
