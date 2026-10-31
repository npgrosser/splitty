/**
 * Files offered by a pane's process (`terminal-sessions`, File Offers and
 * Saving An Offered File): `splitty download <file>` in a pane makes its
 * server offer the file to the windows showing the pane. An offer is
 * untrusted data, a host's too: the window asks, reads nothing before the
 * user's Save, reads only from the connection that offered it, and saves the
 * file only as a download of this window. A desktop shell can then show the
 * saved file, named by the download's URL only (`access-control`, Showing A
 * Saved Download).
 */
import { toast } from "svelte-sonner";
import { hostOf, type ServerConnection } from "./connection/server_connection.ts";
import { isRemoteSession, shellKind } from "./platform.ts";
import { fromBase64 } from "./protocol/base64.ts";
import { plainText } from "./protocol/untrusted.ts";
import { formatBytes } from "./terminal/image_paste.ts";
import type { FileChunk, FileOffer, IncomingMessage } from "./protocol/types.ts";

export const FILE_OFFER_CAPABILITY = "file-offer-v1";
/** The largest file an offer may name; the server offers none larger. */
export const MAX_OFFER_BYTES = 512 * 1024 * 1024;
/** The most a `file.chunk` carries, decoded. */
const MAX_CHUNK_BYTES = 1024 * 1024;
/** Offers asked or read at once, across every connection of this window. */
export const MAX_VISIBLE_OFFERS = 3;
/** How long a prompt waits at most, whatever the offer says. */
const MAX_PROMPT_MS = 60_000;
/** How long the answer to one `file.read` may take before the download is given up. */
export const READ_TIMEOUT_MS = 30_000;
const MAX_NAME = 255;

/** How long the notice of a saved file stays when it offers to show the file. */
const SAVED_NOTICE_MS = 10_000;

/** Save `blob` as a download named `name`; returns the download's URL. */
export type SaveDownload = (name: string, blob: Blob) => string;
/** Ask the desktop shell to show the download of `url` in the file manager; rejects when it cannot. */
export type RevealDownload = (url: string) => Promise<unknown>;

/** The desktop shell's, which knows where it saved its downloads; a browser has none. */
export const revealDownload: RevealDownload | null =
  shellKind === "tauri" ? (url) => (window as any).__TAURI_INTERNALS__.invoke("reveal_download", { url }) :
  shellKind === "electron" ? (url) => (window as any).__SPLITTY_ELECTRON__.invoke("download:reveal", url) :
  null;

/** A download of the window: the browser, Electron and the Tauri shell save it as they are set up to. */
export function saveAsDownload(name: string, blob: Blob): string {
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.rel = "noopener";
  link.style.display = "none";
  document.body.appendChild(link);
  link.click();
  link.remove();
  // The download has started from the URL; give slow shells time before it goes.
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
  return url;
}

/**
 * The name a download is saved under: the last path component, as plain text,
 * without characters file systems refuse. Null when nothing is left.
 */
export function downloadName(name: unknown): string | null {
  if (typeof name !== "string" || name.length > MAX_NAME) return null;
  const last = name.split(/[/\\]/).at(-1) ?? "";
  const cleaned = plainText(last, MAX_NAME).replace(/[<>:"|?*]/g, "_").replace(/^\.+/, "").trim();
  return cleaned && cleaned !== "." && cleaned !== ".." ? cleaned : null;
}

// Across all connections in this window, even a malicious host cannot create
// unbounded prompts or reads.
let visible = 0;

interface Offer {
  readonly id: string;
  readonly name: string;
  readonly size: number;
  phase: "asking" | "reading" | "done";
  readonly chunks: Uint8Array[];
  received: number;
  prompt?: string | number;
  progress?: string | number;
  timer?: ReturnType<typeof setTimeout>;
}

/**
 * Receive the file offers of `conn`'s server. Returns the cleanup.
 *
 * Remote windows (UI delivered by a host) take no offers: the hub's window
 * does, and the desktop shells save downloads only from the backend's page.
 */
export function watchFileOffers(
  conn: ServerConnection,
  save: SaveDownload = saveAsDownload,
  reveal: RevealDownload | null = revealDownload,
): () => void {
  if (isRemoteSession) return () => {};
  let connected = false;
  let disposed = false;
  const offers = new Map<string, Offer>();
  const seen = new Map<string, number>();
  const origin = hostOf(conn.key) ?? "this computer";

  const send = (msg: IncomingMessage) => {
    if (connected && !disposed) conn.socket.send(msg);
  };

  /**
   * Forget `offer` and take its notices down; `decline` tells the server,
   * which ends an accepted offer's transfer as well.
   */
  const end = (offer: Offer, decline = false) => {
    if (offer.phase === "done") return;
    offer.phase = "done";
    offers.delete(offer.id);
    visible--;
    clearTimeout(offer.timer);
    if (offer.prompt !== undefined) toast.dismiss(offer.prompt);
    if (offer.progress !== undefined) toast.dismiss(offer.progress);
    if (decline) send({ type: "file.decline", offerId: offer.id });
  };

  /** The user dismissed the prompt; the prompt's own dismissal after Save is not one. */
  const decline = (offer: Offer) => {
    if (offer.phase === "asking") end(offer, true);
  };

  /** The user stopped a download that runs. */
  const cancel = (offer: Offer) => {
    if (offer.phase !== "reading") return;
    end(offer, true);
    toast(`${offer.name} was not saved`, { description: "The download was cancelled." });
  };

  /** `tell`: the server still holds the offer and should end it. */
  const failed = (offer: Offer, reason: string, tell = true) => {
    if (offer.phase === "done") return;
    end(offer, tell);
    toast.error(`${offer.name} was not saved`, { description: reason });
  };

  const read = (offer: Offer) => {
    clearTimeout(offer.timer);
    offer.timer = setTimeout(() => failed(offer, "The server stopped sending the file."), READ_TIMEOUT_MS);
    send({ type: "file.read", offerId: offer.id, offset: offer.received });
  };

  const showProgress = (offer: Offer) => {
    const percent = offer.size === 0 ? 100 : Math.floor((offer.received / offer.size) * 100);
    offer.progress = toast.loading(`Downloading ${offer.name}`, {
      id: offer.progress,
      description: `${percent} % of ${formatBytes(offer.size)} from ${origin}`,
      cancel: { label: "Cancel", onClick: () => cancel(offer) },
    });
  };

  const accept = (offer: Offer, sessionId: string) => {
    if (offer.phase !== "asking") return;
    if (!connected || disposed || !conn.sessions.get(sessionId)) {
      end(offer);
      return;
    }
    // Before the prompt goes: its dismissal must not decline what was taken.
    offer.phase = "reading";
    clearTimeout(offer.timer);
    if (offer.prompt !== undefined) toast.dismiss(offer.prompt);
    offer.prompt = undefined;
    send({ type: "file.accept", offerId: offer.id });
    showProgress(offer);
    read(offer);
  };

  const chunk = (msg: FileChunk) => {
    const offer = offers.get(msg.offerId);
    if (!offer || offer.phase !== "reading" || msg.offset !== offer.received) return;
    const bytes = typeof msg.data === "string" && msg.data.length <= Math.ceil(MAX_CHUNK_BYTES / 3) * 4 ? fromBase64(msg.data) : null;
    if (!bytes || offer.received + bytes.length > offer.size) {
      failed(offer, "The server sent more than it offered.");
      return;
    }
    offer.chunks.push(bytes);
    offer.received += bytes.length;
    const complete = offer.received === offer.size;
    if (msg.eof !== complete || (!complete && bytes.length === 0)) {
      failed(offer, "The server sent a file of another size than it offered.");
      return;
    }
    if (!complete) {
      showProgress(offer);
      read(offer);
      return;
    }
    const blob = new Blob(offer.chunks as BlobPart[], { type: "application/octet-stream" });
    end(offer);
    const url = save(offer.name, blob);
    const description = `${formatBytes(offer.size)} from ${origin}`;
    if (!reveal) {
      toast.success(`Saved ${offer.name}`, { description });
      return;
    }
    const name = offer.name;
    toast.success(`Saved ${name}`, {
      description,
      duration: SAVED_NOTICE_MS,
      action: { label: "Show in folder", onClick: () => {
        reveal(url).catch(() => toast.error(`Could not show ${name}`, { description: "The app has no finished download of it." }));
      } },
    });
  };

  const prompt = (msg: FileOffer) => {
    if (!connected || disposed) return;
    if (typeof msg.offerId !== "string" || !/^[a-f0-9]{32}$/.test(msg.offerId) || seen.has(msg.offerId)) return;
    const now = performance.now();
    for (const [id, until] of seen) if (now >= until) seen.delete(id);
    if (seen.size >= 256) {
      conn.socket.send({ type: "file.decline", offerId: msg.offerId });
      return;
    }
    seen.set(msg.offerId, now + 300_000);
    const name = downloadName(msg.name);
    const valid = name !== null
      && Number.isSafeInteger(msg.size) && msg.size >= 0 && msg.size <= MAX_OFFER_BYTES
      && typeof msg.sessionId === "string" && msg.sessionId.length <= 256 && conn.sessions.get(msg.sessionId) !== undefined
      && Number.isFinite(msg.expiresAt);
    const duration = valid ? Math.min(MAX_PROMPT_MS, msg.expiresAt - Date.now()) : 0;
    if (!valid || duration <= 0 || visible >= MAX_VISIBLE_OFFERS) {
      conn.socket.send({ type: "file.decline", offerId: msg.offerId });
      return;
    }
    const offer: Offer = { id: msg.offerId, name: name!, size: msg.size, phase: "asking", chunks: [], received: 0 };
    offers.set(offer.id, offer);
    visible++;
    const sessionId = msg.sessionId;
    // The server closes it then too; a prompt left open would offer nothing.
    offer.timer = setTimeout(() => end(offer), duration);
    offer.prompt = toast(`Save ${offer.name}?`, {
      description: `${formatBytes(offer.size)} from ${origin}`,
      duration,
      closeButton: true,
      action: { label: "Save", onClick: () => accept(offer, sessionId) },
      cancel: { label: "Dismiss", onClick: () => decline(offer) },
      onDismiss: () => decline(offer),
      onAutoClose: () => { if (offer.phase === "asking") end(offer); },
    });
  };

  /** The connection changed: its offers are gone on the server. */
  const dropAll = () => {
    for (const offer of [...offers.values()]) {
      const reading = offer.phase === "reading";
      end(offer);
      if (reading) toast.error(`${offer.name} was not saved`, { description: "The connection was lost." });
    }
  };

  const unsubDisconnect = conn.socket.onDisconnect(() => {
    connected = false;
    dropAll();
  });
  const unsubMessage = conn.socket.onMessage((msg) => {
    switch (msg.type) {
      case "mux.welcome":
        dropAll();
        connected = Array.isArray(msg.capabilities) && msg.capabilities.includes(FILE_OFFER_CAPABILITY);
        if (connected && !disposed) conn.socket.send({ type: "file.register" });
        break;
      case "file.offer":
        prompt(msg);
        break;
      case "file.chunk":
        chunk(msg);
        break;
      case "file.closed": {
        const offer = offers.get(msg.offerId);
        if (!offer) break;
        if (offer.phase === "reading") failed(offer, "The server ended the download.", false);
        else end(offer);
        break;
      }
    }
  });
  return () => {
    for (const offer of [...offers.values()]) end(offer, true);
    disposed = true;
    connected = false;
    unsubMessage();
    unsubDisconnect();
  };
}
