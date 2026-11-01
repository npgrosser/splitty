// The Ports lists of the window's hosts as the UI shows them (`remote-screens`,
// Detected Ports In The Window), fed by the one `PortForwarding` of the window
// (`lib/remote/port_forwarding.ts`), and the way a terminal pane opens a link.
// Beside them, the local ports shared with each host (`remote-screens`,
// Sharing Local Ports In The Window), read with the forwards by the window's
// `ReverseForwarding` (`lib/remote/reverse_forwards.ts`).

import type { ConnKey } from "../lib/connection/server_connection.ts";
import { PortForwarding, type PortForwardingDeps } from "../lib/remote/port_forwarding.ts";
import type { HostPorts } from "../lib/remote/ports.ts";
import { ReverseForwarding, type HostShares } from "../lib/remote/reverse_forwards.ts";
import { openExternalTarget } from "../lib/window.ts";

let lists = $state<HostPorts[]>([]);
let forwarding = $state.raw<PortForwarding>();
let shares = $state<HostShares[]>([]);
let sharing = $state.raw<ReverseForwarding>();

/** Start the window's port forwarding and sharing, and read the local server's forwards and shares. */
export function startPortForwarding(deps: Omit<PortForwardingDeps, "changed">): PortForwarding {
  sharing?.dispose();
  const shared: ReverseForwarding = new ReverseForwarding({
    fetch: deps.fetch,
    changed: () => {
      if (sharing === shared) shares = shared.lists();
    },
  });
  shares = [];
  sharing = shared;
  void shared.refresh();
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
  sharing?.dispose();
  sharing = undefined;
  shares = [];
}

/** The window's port forwarding, while it runs. */
export function portForwarding(): PortForwarding | undefined {
  return forwarding;
}

/** Per host its ports and forwards. */
export function hostPortLists(): HostPorts[] {
  return lists;
}

/** The window's shares of local ports, while it runs. */
export function reverseForwarding(): ReverseForwarding | undefined {
  return sharing;
}

/** Per host the local ports shared with it and their saved rules. */
export function hostShareLists(): HostShares[] {
  return shares;
}

/**
 * A link a terminal pane of `conn` opened: a host's loopback links open at
 * their forward, every other link as it is.
 */
export function openPaneLink(conn: ConnKey, url: string): void {
  if (forwarding) forwarding.openLink(conn, url);
  else openExternalTarget({ url });
}
