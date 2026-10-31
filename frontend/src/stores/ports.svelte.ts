// The Ports lists of the window's hosts as the UI shows them (`remote-screens`,
// Detected Ports In The Window), fed by the one `PortForwarding` of the window
// (`lib/remote/port_forwarding.ts`), and the way a terminal pane opens a link.

import type { ConnKey } from "../lib/connection/server_connection.ts";
import { PortForwarding, type PortForwardingDeps } from "../lib/remote/port_forwarding.ts";
import type { HostPorts } from "../lib/remote/ports.ts";
import { openExternalTarget } from "../lib/window.ts";

let lists = $state<HostPorts[]>([]);
let forwarding = $state.raw<PortForwarding>();

/** Start the window's port forwarding and read the local server's forwards. */
export function startPortForwarding(deps: Omit<PortForwardingDeps, "changed">): PortForwarding {
  forwarding?.dispose();
  const instance: PortForwarding = new PortForwarding({
    ...deps,
    changed: () => {
      if (forwarding === instance) lists = instance.lists();
    },
  });
  lists = [];
  forwarding = instance;
  void instance.refresh();
  return instance;
}

export function stopPortForwarding(): void {
  forwarding?.dispose();
  forwarding = undefined;
  lists = [];
}

/** The window's port forwarding, while it runs. */
export function portForwarding(): PortForwarding | undefined {
  return forwarding;
}

/** Per host its ports and forwards. */
export function hostPortLists(): HostPorts[] {
  return lists;
}

/**
 * A link a terminal pane of `conn` opened: a host's loopback links open at
 * their forward, every other link as it is.
 */
export function openPaneLink(conn: ConnKey, url: string): void {
  if (forwarding) forwarding.openLink(conn, url);
  else openExternalTarget({ url });
}
