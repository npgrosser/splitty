import { assignPlatform } from "../platform.ts";
import { assignBrowserConnection, rootAssignment, type BrowserAssignment } from "./connection.ts";
import { assignBrowserGateway, exchangeToken } from "./http.ts";

async function readAssignment(): Promise<BrowserAssignment | null> {
  const response = await fetch("/api/client-assignment", {
    credentials: "same-origin", cache: "no-store", signal: AbortSignal.timeout(5_000),
  });
  if (response.status === 401) return null;
  if (!response.ok) throw Error("The browser gateway could not assign this workspace");
  const value: unknown = await response.json();
  if (!value || typeof value !== "object") throw Error("Invalid browser gateway assignment");
  const { gateway, role, target } = value as Record<string, unknown>;
  if (typeof gateway !== "string" || !/^splitty-session-[a-f0-9]{32}$/.test(gateway)) {
    throw Error("Invalid browser gateway assignment");
  }
  if (role === "main" && target === undefined) return { gateway, role };
  if (role === "remote" && typeof target === "string" && target.length > 0 && target.length <= 1024
    && !/[\x00-\x20\x7f]/.test(target)) return { gateway, role, target };
  throw Error("Invalid browser gateway assignment");
}

/** Authenticate before mounting the workspace: even its root connection's
 * command policy must follow the gateway's assignment, not a URL parameter. */
export async function bootstrapBrowser(
  login: (submit: (token: string) => Promise<boolean>) => Promise<void> = showLogin,
): Promise<void> {
  if (rootAssignment) return;
  let assignment = await readAssignment();
  if (!assignment) await login(async token => {
    if (!await exchangeToken(token)) return false;
    assignment = await readAssignment();
    return assignment !== null;
  });
  if (!assignment) throw Error("The browser gateway did not assign this workspace");
  assignBrowserGateway(assignment.gateway);
  assignBrowserConnection(assignment);
  assignPlatform("web", assignment.role);
}

async function showLogin(submit: (token: string) => Promise<boolean>): Promise<void> {
  const [{ mount, unmount }, { default: BrowserLogin }] = await Promise.all([
    import("svelte"), import("../../components/dialogs/BrowserLogin.svelte"),
  ]);
  const target = document.getElementById("app");
  if (!target) throw Error("Application mount is unavailable");
  let complete!: () => void, fail!: (error: unknown) => void;
  const done = new Promise<void>((resolve, reject) => { complete = resolve; fail = reject; });
  const login = mount(BrowserLogin, { target, props: {
    submit: async (token: string) => {
      try { const accepted = await submit(token); if (accepted) complete(); return accepted; }
      catch (error) { fail(error); return false; }
    },
  } });
  try { await done; } finally { await unmount(login); }
}
