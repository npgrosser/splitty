// Updating a host's Splitty from the window (`remote-screens`, Host Update).
//
// The hub replaces a daemon of another version only for a socket opened with
// `update=1`. Its `update` metadata is the plan: whether an update is
// available, whether the daemon keeps its sessions, and the version it gets
// (`toVersion`). An available update keeps every session, so the window
// offers one only when `available` and `keepsSessions` are both true, and its
// warning names nothing that ends. A legacy daemon is migrated instead
// (`MigrationOffer`), keeping the keeper-backed sessions the hub checked.

/** An offer from native discovery, never a process selector or authority token. */
export interface MigrationOffer {
  available: true;
  fromVersion: string;
  toVersion: string;
  sessions: number;
}

export function migrationOffer(value: unknown): MigrationOffer | undefined {
  if (!value || typeof value !== "object") return undefined;
  const offer = value as Partial<MigrationOffer>;
  if (offer.available !== true || typeof offer.fromVersion !== "string" || !offer.fromVersion || offer.fromVersion.length > 64
    || typeof offer.toVersion !== "string" || !offer.toVersion || offer.toVersion.length > 64
    || !Number.isInteger(offer.sessions) || offer.sessions! < 0 || offer.sessions! > 0xffffffff) return undefined;
  return { available: true, fromVersion: offer.fromVersion, toVersion: offer.toVersion, sessions: offer.sessions! };
}

/** The local hub's update plan; version strings in the window carry no authority. */
export interface UpdateAvailability {
  available: boolean;
  keepsSessions: boolean;
  /** The version the host's daemon gets; older hubs do not send it. */
  toVersion?: string;
}
export function updateAvailability(value: unknown): UpdateAvailability | undefined {
  if (!value || typeof value !== "object") return undefined;
  const item = value as Record<string, unknown>;
  if (typeof item.available !== "boolean" || typeof item.keepsSessions !== "boolean") return undefined;
  const plan: UpdateAvailability = { available: item.available, keepsSessions: item.keepsSessions };
  // eslint-disable-next-line no-control-regex
  if (typeof item.toVersion === "string" && item.toVersion && item.toVersion.length <= 64 && !/[\u0000-\u001f\u007f]/.test(item.toVersion)) {
    plan.toVersion = item.toVersion;
  }
  return plan;
}

/** Whether the plan offers an ordinary update: available, and every session kept. */
export function updateOffered(update: UpdateAvailability | undefined): boolean {
  return update?.available === true && update.keepsSessions;
}

export interface UpdateWarning {
  title: string;
  /** Which versions: the host's now and the one it gets. */
  versions: string;
  /** What happens to the host's sessions. */
  ends: string;
  action?: string;
}

export interface UpdateVersions {
  migration?: MigrationOffer;
  /** The host daemon's version and protocol, from its `remote.error` or `remote.ready`. */
  hostVersion?: string;
  hostProtocol?: number;
  /** The version the host gets, from the hub's plan. */
  toVersion?: string;
  /** The local server's version, named when the hub's plan has none. */
  localVersion?: string;
}

/** The warning the user confirms before the window opens the host's hub socket with `update=1`. */
export function updateWarning(target: string, versions: UpdateVersions): UpdateWarning {
  if (versions.migration) {
    const offer = versions.migration;
    return {
      title: `Update server and connect to ${target}?`,
      versions: `${target} runs Splitty ${offer.fromVersion}. Splitty will verify it again and start ${offer.toVersion}.`,
      ends: `The ${count(offer.sessions, "keeper-backed session")} checked on ${target} ${offer.sessions === 1 ? "keeps" : "keep"} running. Connections are interrupted briefly. If the server can no longer be verified, the update stops.`,
      action: "Update server and connect",
    };
  }
  const running = versions.hostVersion
    ? `Splitty ${versions.hostVersion}${versions.hostProtocol !== undefined ? ` (protocol ${versions.hostProtocol})` : ""}`
    : "another Splitty";
  const next = versions.toVersion
    ? `Splitty ${versions.toVersion}`
    : versions.localVersion ? `Splitty ${versions.localVersion}, the version of this app` : "the version of this app";
  return {
    title: `Update Splitty on ${target}?`,
    versions: `${target} runs ${running}. Updating stops it and starts ${next}.`,
    ends: `The sessions on ${target} keep running, with every process in them: `
      + `updating restarts its Splitty, and the connection drops for a moment.`,
  };
}

/**
 * The tooltip's note for a host whose daemon runs another version than the
 * local server: an update to the local version is available.
 */
export function updateNote(hostVersion: string | undefined, localVersion: string | undefined, update?: UpdateAvailability): string | undefined {
  const next = update?.toVersion ?? localVersion;
  if (!hostVersion || !next || !updateOffered(update)) return undefined;
  return `update to ${next} available`;
}

function count(n: number, noun: string): string {
  return `${n} ${noun}${n === 1 ? "" : "s"}`;
}
