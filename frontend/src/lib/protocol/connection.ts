/** Selected once before loading the application module graph. Browser globals
 * are not grants: only a successful native broker assignment selects IPC. */
import { socket as browserSocket } from "./websocket.ts";
import type { NativeConnection } from "./native_connection.ts";
import type { NativeAssignment } from "./native_socket.ts";

export let socket: typeof browserSocket | NativeConnection = browserSocket;
export let rootAssignment: Readonly<NativeAssignment> | undefined;
export interface BrowserAssignment { readonly gateway: string; readonly role: "main" | "remote"; readonly target?: string }
export let browserAssignment: Readonly<BrowserAssignment> | undefined;
export function assignBrowserConnection(assignment: BrowserAssignment): void {
  if (rootAssignment || browserAssignment) throw Error("The document already has a root connection");
  browserAssignment = Object.freeze({ ...assignment });
}
export function assignRootConnection(connection: NativeConnection, assignment: NativeAssignment): void {
  if (rootAssignment || browserAssignment) throw Error("The document already has a root connection");
  rootAssignment = Object.freeze({ ...assignment });
  socket = connection;
}

export let nativeHostOpener: import("../remote/native_hub.ts").NativeHostOpener | undefined;
export function assignNativeHosts(open: import("../remote/native_hub.ts").NativeHostOpener): void {
  if (!rootAssignment || rootAssignment.role !== "main" || nativeHostOpener) throw Error("Native host opener requires the assigned main document");
  nativeHostOpener = open;
}
