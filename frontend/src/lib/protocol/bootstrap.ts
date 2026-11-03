import { assignPlatform } from "../platform.ts";
import { assignNativeApi } from "./http.ts";
import { assignRootConnection, assignNativeHosts } from "./connection.ts";
import { openElectronChannel, type ElectronNativeBridge } from "./electron_channel.ts";
import { openTauriChannel, type TauriNativeBridge } from "./tauri_channel.ts";
import { NativeConnection } from "./native_connection.ts";
import { NativeSocket } from "./native_socket.ts";

/** The packaged entry asks the shell to assign a backend before importing UI
 * modules. A global only locates the adapter; the native caller check grants
 * the connection. HTTP pages always use the explicit browser gateway. */
export async function bootstrapConnection(): Promise<() => void> {
  const packagedTauri = (window.location.protocol === "tauri:" && window.location.hostname === "localhost")
    || (["http:", "https:"].includes(window.location.protocol) && window.location.hostname === "tauri.localhost" && !window.location.port);
  const tauri = (window as unknown as { __SPLITTY_TAURI__?: { native: TauriNativeBridge; ready: Promise<{ role: string; target?: string }> } }).__SPLITTY_TAURI__;
  if (window.location.protocol !== "splitty:" && !packagedTauri) return () => {};
  const startup = new NativeSocket();
  // Latch native mode before awaiting IPC: failure must never fetch a token,
  // ticket, settings or fallback endpoint over HTTP.
  assignNativeApi(startup);
  const bridge = (window as unknown as { __SPLITTY_ELECTRON__?: { native?: ElectronNativeBridge } }).__SPLITTY_ELECTRON__?.native;
  if (packagedTauri ? !tauri : !bridge) throw Error("The packaged application has no native connection adapter");
  if (packagedTauri) await tauri!.ready;
  const open = (signal?: AbortSignal) => packagedTauri
    ? openTauriChannel(tauri!.native, undefined, signal)
    : openElectronChannel(bridge!, undefined, signal);
  try {
    const channel = await open();
    startup.adopt(channel);
    const assignment = startup.assignment;
    if (!assignment) throw Error("Native startup connection closed before assignment");
    const root = new NativeConnection(open, assignment);
    if (assignment.role === "remote") {
      if (packagedTauri) {
        root.onStateChange = state => tauri!.native.state(state);
        window.addEventListener("splitty-native-retry", () => { if (root.connectionState === "disconnected") root.reconnect(); });
      }
      else {
        root.onStateChange = state => bridge!.state(state);
        bridge!.onRetry(() => { if (root.connectionState === "disconnected") root.reconnect(); });
      }
    }
    assignRootConnection(root, assignment);
    assignPlatform(packagedTauri ? "tauri" : "electron", assignment.role);
    if (assignment.role === "main") assignNativeHosts((target, update, signal) => packagedTauri ? openTauriChannel(tauri!.native, { target, update }, signal) : openElectronChannel(bridge!, { target, update }, signal));
    // Startup reads use a short-lived independent client. The application's
    // client starts only after all welcome/session handlers are installed.
    return () => { assignNativeApi(root); startup.dispose(); };
  } catch (error) { startup.dispose(); bridge?.state?.("disconnected"); throw error; }
}
