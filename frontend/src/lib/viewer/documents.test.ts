import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { toBase64 } from "../protocol/base64.ts";
import type { IncomingMessage, PaneState, ScreenState } from "../protocol/types.ts";
import { FakeSocket } from "../testing/fake_server.ts";
import {
  CACHE_ENTRIES, MAX_RESTARTS, POLL_MS, READ_TIMEOUT_MS, ViewerDocuments,
  type DocumentState, type DocumentWatch, type Visibility,
} from "./documents.ts";
import { CHUNK_BYTES, MAX_FILE_BYTES, MAX_TEXT_BYTES } from "./limits.ts";

type ViewerRead = Extract<IncomingMessage, { type: "viewer.read" }>;

const encoder = new TextEncoder();
const decoder = new TextDecoder();

class FakeVisibility implements Visibility {
  private shown = true;
  private readonly listeners = new Set<() => void>();
  visible(): boolean { return this.shown; }
  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }
  set(shown: boolean): void {
    this.shown = shown;
    for (const listener of [...this.listeners]) listener();
  }
}

let socket: FakeSocket;
let visibility: FakeVisibility;
let docs: ViewerDocuments;

beforeEach(() => {
  vi.useFakeTimers();
  socket = new FakeSocket();
  visibility = new FakeVisibility();
  docs = new ViewerDocuments(socket, { visibility });
});

afterEach(() => {
  docs.dispose();
  vi.useRealTimers();
});

function welcome(capabilities: string[] = ["viewer-v1"]): void {
  socket.message({ type: "mux.welcome", clientId: "c1", screens: [], capabilities });
}

function reads(): ViewerRead[] {
  return socket.sentOfType("viewer.read");
}

function lastRead(): ViewerRead {
  const all = reads();
  expect(all.length).toBeGreaterThan(0);
  return all[all.length - 1];
}

function bytesOf(content: string | Uint8Array): Uint8Array {
  return typeof content === "string" ? encoder.encode(content) : content;
}

/** The server answers `read` with one chunk. */
function chunk(read: ViewerRead, content: string | Uint8Array, fields: { size?: number; version?: string; eof?: boolean; offset?: number } = {}): void {
  const bytes = bytesOf(content);
  socket.message({
    type: "viewer.chunk",
    requestId: read.requestId,
    offset: fields.offset ?? read.offset,
    size: fields.size ?? bytes.length,
    version: fields.version ?? "v1",
    data: toBase64(bytes),
    eof: fields.eof ?? true,
  });
}

function failed(read: ViewerRead, code: string, error = "it failed"): void {
  socket.message({ type: "viewer.failed", requestId: read.requestId, code, error });
}

function unchanged(read: ViewerRead, version: string): void {
  socket.message({ type: "viewer.unchanged", requestId: read.requestId, version });
}

interface Watched {
  watch: DocumentWatch;
  states: DocumentState[];
}

function watchFile(paneId = "pane-1", limit = MAX_FILE_BYTES): Watched {
  const states: DocumentState[] = [];
  const watch = docs.watch(paneId, { poll: true, limit, onChange: (state) => states.push(state) });
  return { watch, states };
}

function textOf(state: DocumentState | undefined): string | undefined {
  return state?.document ? decoder.decode(state.document.bytes) : undefined;
}

function last(watched: Watched): DocumentState {
  return watched.watch.state;
}

/** A shown file pane whose document was read once, as `text` at `version`. */
function shownFile(text = "# Plan\n", version = "v1", paneId = "pane-1"): Watched {
  const watched = watchFile(paneId);
  watched.watch.setShown(true);
  chunk(lastRead(), text, { version });
  expect(textOf(last(watched))).toBe(text);
  return watched;
}

function screenWith(panes: PaneState[]): ScreenState {
  return {
    id: "screen-1",
    revision: 1,
    root: { type: "slot", id: "s1", paneIds: panes.map((pane) => pane.id) },
    floating: [],
    panes: Object.fromEntries(panes.map((pane) => [pane.id, pane])),
    sessions: {},
  };
}

function viewerPane(id: string, revision: number): PaneState {
  return { id, role: "viewer", title: "PLAN.md", viewer: { source: { kind: "file", path: "/home/me/app/PLAN.md" }, revision } };
}

describe("ViewerDocuments", () => {
  beforeEach(() => welcome());

  // viewer-panes, Keeping A Shown Document Current: a pane that is not shown reads nothing.
  it("reads nothing before the pane is shown, then the whole document from offset 0", () => {
    const watched = watchFile();
    expect(reads()).toEqual([]);
    expect(last(watched)).toEqual({ document: undefined, failure: undefined, reading: false });

    watched.watch.setShown(true);
    expect(reads()).toHaveLength(1);
    const read = lastRead();
    expect(read).toEqual({ type: "viewer.read", requestId: expect.stringMatching(/^vr-[0-9a-f]{16}$/), paneId: "pane-1", offset: 0 });
    expect(last(watched).reading).toBe(true);

    chunk(read, "# Plan\n", { version: "f-7-1-2" });
    expect(last(watched)).toEqual({ document: { bytes: encoder.encode("# Plan\n"), version: "f-7-1-2" }, failure: undefined, reading: false });
  });

  // viewer-panes, Reading A Document: A file in two chunks.
  it("reads a file of 1.5 MiB in two chunks, the second with the first one's version", () => {
    const watched = watchFile();
    watched.watch.setShown(true);
    const size = CHUNK_BYTES + CHUNK_BYTES / 2;
    const first = lastRead();
    chunk(first, new Uint8Array(CHUNK_BYTES).fill(0x61), { size, version: "f-1572864-5-9", eof: false });

    const second = lastRead();
    expect(second).toMatchObject({ offset: CHUNK_BYTES, version: "f-1572864-5-9", paneId: "pane-1" });
    expect(second).not.toHaveProperty("ifVersion");
    expect(second.requestId).not.toBe(first.requestId);
    expect(last(watched).document).toBeUndefined();

    chunk(second, new Uint8Array(CHUNK_BYTES / 2).fill(0x62), { size, version: "f-1572864-5-9", eof: true });
    const document = last(watched).document!;
    expect(document.version).toBe("f-1572864-5-9");
    expect(document.bytes).toHaveLength(size);
    expect(document.bytes[CHUNK_BYTES - 1]).toBe(0x61);
    expect(document.bytes[CHUNK_BYTES]).toBe(0x62);
    expect(reads()).toHaveLength(2);
  });

  // viewer-panes, Reading A Document: Nothing changed; Keeping A Shown Document Current.
  it("checks a shown file every two seconds with ifVersion and keeps the content when nothing changed", () => {
    const watched = shownFile("# Plan\n", "v1");
    const before = last(watched).document;

    vi.advanceTimersByTime(POLL_MS - 1);
    expect(reads()).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(reads()).toHaveLength(2);
    expect(lastRead()).toMatchObject({ offset: 0, ifVersion: "v1" });
    expect(lastRead()).not.toHaveProperty("version");

    unchanged(lastRead(), "v1");
    expect(last(watched)).toEqual({ document: before, failure: undefined, reading: false });
    vi.advanceTimersByTime(POLL_MS);
    expect(reads()).toHaveLength(3);
  });

  // viewer-panes, Keeping A Shown Document Current: An agent rewrites the plan.
  it("shows a rewritten file within a few seconds", () => {
    const watched = shownFile("# Plan\n\n- [ ] one\n", "v1");
    vi.advanceTimersByTime(POLL_MS);
    chunk(lastRead(), "# Plan\n\n- [x] one\n- [ ] two\n", { version: "v2" });
    expect(textOf(last(watched))).toBe("# Plan\n\n- [x] one\n- [ ] two\n");
    expect(last(watched).document?.version).toBe("v2");
    expect(watched.states.map(textOf).filter(Boolean)).toContain("# Plan\n\n- [x] one\n- [ ] two\n");
  });

  // viewer-panes, Keeping A Shown Document Current: Reopened from the CLI.
  it("reads a shown pane at once when a snapshot raises its revision", () => {
    shownFile();
    docs.snapshot(screenWith([viewerPane("pane-1", 1)]));
    expect(reads()).toHaveLength(1);
    docs.snapshot(screenWith([viewerPane("pane-1", 1)]));
    expect(reads()).toHaveLength(1);

    docs.snapshot(screenWith([viewerPane("pane-1", 2)]));
    expect(reads()).toHaveLength(2);
    expect(lastRead()).toMatchObject({ paneId: "pane-1", offset: 0, ifVersion: "v1" });
  });

  it("does not read a pane that is not shown when its revision rises", () => {
    const watched = shownFile();
    watched.watch.setShown(false);
    docs.snapshot(screenWith([viewerPane("pane-1", 1)]));
    docs.snapshot(screenWith([viewerPane("pane-1", 2)]));
    vi.advanceTimersByTime(POLL_MS * 3);
    expect(reads()).toHaveLength(1);
  });

  // viewer-panes, Keeping A Shown Document Current: a pane that is not shown,
  // and every pane of a hidden window, sends no reads.
  it("stops polling while the pane is not shown or the window is hidden", () => {
    const watched = shownFile();
    watched.watch.setShown(false);
    vi.advanceTimersByTime(POLL_MS * 5);
    expect(reads()).toHaveLength(1);

    watched.watch.setShown(true);
    expect(reads()).toHaveLength(2);
    unchanged(lastRead(), "v1");

    visibility.set(false);
    vi.advanceTimersByTime(POLL_MS * 5);
    expect(reads()).toHaveLength(2);

    visibility.set(true);
    expect(reads()).toHaveLength(3);
    expect(lastRead()).toMatchObject({ ifVersion: "v1" });
  });

  it("does not read in a hidden window, and reads once it is visible again", () => {
    visibility.set(false);
    const watched = watchFile();
    watched.watch.setShown(true);
    watched.watch.reload();
    expect(reads()).toEqual([]);
    visibility.set(true);
    expect(reads()).toHaveLength(1);
  });

  it("reads a text document once and does not poll it", () => {
    const states: DocumentState[] = [];
    const watch = docs.watch("viewer-0123456789abcdef", { poll: false, limit: MAX_TEXT_BYTES, onChange: (state) => states.push(state) });
    watch.setShown(true);
    chunk(lastRead(), "the selected text", { version: "t-17-ab" });
    expect(textOf(watch.state)).toBe("the selected text");
    vi.advanceTimersByTime(POLL_MS * 5);
    expect(reads()).toHaveLength(1);
  });

  it("runs at most one read per pane, and a reload that waited reads the whole document", () => {
    const watched = shownFile();
    const other = docs.watch("pane-1", { poll: true, limit: MAX_FILE_BYTES, onChange: () => {} });
    other.setShown(true);
    vi.advanceTimersByTime(POLL_MS);
    expect(reads()).toHaveLength(2);

    watched.watch.reload();
    other.reload();
    expect(reads()).toHaveLength(2);
    unchanged(lastRead(), "v1");
    expect(reads()).toHaveLength(3);
    expect(lastRead()).toMatchObject({ offset: 0 });
    expect(lastRead()).not.toHaveProperty("ifVersion");
    other.close();
  });

  // viewer-panes, Reading A Document: Changed between two chunks.
  it("starts over when the file changes between two chunks, and gives up after a few tries", () => {
    const watched = watchFile();
    watched.watch.setShown(true);
    chunk(lastRead(), "a".repeat(10), { size: 20, version: "v1", eof: false });
    failed(lastRead(), "changed", "the file changed");
    expect(lastRead()).toEqual({ type: "viewer.read", requestId: expect.any(String), paneId: "pane-1", offset: 0 });
    chunk(lastRead(), "b".repeat(20), { version: "v2" });
    expect(textOf(last(watched))).toBe("b".repeat(20));

    watched.watch.reload();
    for (let i = 0; i <= MAX_RESTARTS; i++) failed(lastRead(), "changed");
    expect(last(watched).failure).toEqual({ code: "changed", message: "The file kept changing while it was read." });
    expect(reads()).toHaveLength(3 + MAX_RESTARTS + 1);
  });

  it("starts over on a later chunk of another version or size", () => {
    const watched = watchFile();
    watched.watch.setShown(true);
    chunk(lastRead(), "a".repeat(10), { size: 20, version: "v1", eof: false });
    chunk(lastRead(), "b".repeat(10), { size: 20, version: "v2", eof: true });
    expect(lastRead()).toMatchObject({ offset: 0 });
    expect(last(watched).document).toBeUndefined();
    chunk(lastRead(), "c".repeat(10), { size: 10, version: "v2" });
    expect(textOf(last(watched))).toBe("c".repeat(10));
  });

  // viewer-panes, Reading A Document: Too large.
  it("fails a document over the pane's limit without reading it further", () => {
    const watched = watchFile("pane-1", MAX_FILE_BYTES);
    watched.watch.setShown(true);
    chunk(lastRead(), new Uint8Array(16), { size: 9 * 1024 * 1024, eof: false });
    expect(reads()).toHaveLength(1);
    expect(last(watched).failure).toEqual({ code: "too_large", message: "The document is larger than 8.0 MB." });

    watched.watch.reload();
    failed(lastRead(), "too_large", "the file is larger than 8 MiB");
    expect(last(watched).failure?.code).toBe("too_large");
    expect(last(watched).document).toBeUndefined();
  });

  // viewer-panes, Keeping A Shown Document Current: a failed read says why and
  // can be read again.
  it("says why a read failed, keeps checking a missing file, and shows it once it exists", () => {
    const watched = watchFile();
    watched.watch.setShown(true);
    failed(lastRead(), "not_found", "no such file");
    expect(last(watched)).toMatchObject({ failure: { code: "not_found", message: "The file does not exist." }, reading: false });

    vi.advanceTimersByTime(POLL_MS);
    chunk(lastRead(), "# Plan\n");
    expect(last(watched)).toMatchObject({ failure: undefined });
    expect(textOf(last(watched))).toBe("# Plan\n");
  });

  it("stops polling a pane the server does not know until it is read again", () => {
    const watched = watchFile();
    watched.watch.setShown(true);
    failed(lastRead(), "unknown_pane");
    expect(last(watched).failure).toEqual({ code: "unknown_pane", message: "The server does not know this pane." });
    vi.advanceTimersByTime(POLL_MS * 5);
    expect(reads()).toHaveLength(1);

    watched.watch.reload();
    expect(reads()).toHaveLength(2);
  });

  it("names the server's own words, as plain text, for a code it does not know", () => {
    const watched = watchFile();
    watched.watch.setShown(true);
    failed(lastRead(), "on_fire", "disk\u0007 on‮ fire");
    expect(last(watched).failure).toEqual({ code: "malformed", message: "disk on fire" });
  });

  it("fails a read nobody answers, and ignores the late answer", () => {
    const watched = watchFile();
    watched.watch.setShown(true);
    const read = lastRead();
    vi.advanceTimersByTime(READ_TIMEOUT_MS);
    expect(last(watched).failure).toEqual({ code: "timeout", message: "The server did not answer." });
    chunk(read, "late");
    expect(last(watched).document).toBeUndefined();
  });

  it("fails a malformed chunk", () => {
    const watched = watchFile();
    watched.watch.setShown(true);
    socket.message({ type: "viewer.chunk", requestId: lastRead().requestId, offset: 0, size: 4, version: "v1", data: "not base64!", eof: true });
    expect(last(watched).failure?.code).toBe("malformed");

    watched.watch.reload();
    chunk(lastRead(), "text", { offset: 5 });
    expect(last(watched).failure?.code).toBe("malformed");

    watched.watch.reload();
    chunk(lastRead(), "text", { version: "" });
    expect(last(watched).failure?.code).toBe("malformed");
  });

  it("ignores answers to requests it did not send", () => {
    const watched = watchFile();
    watched.watch.setShown(true);
    socket.message({ type: "viewer.chunk", requestId: "vr-0000000000000000", offset: 0, size: 1, version: "v1", data: toBase64(encoder.encode("x")), eof: true });
    socket.message({ type: "viewer.failed", requestId: 7, code: "not_found", error: "" });
    expect(last(watched)).toMatchObject({ document: undefined, failure: undefined, reading: true });
  });

  it("keeps the content and asks again later when the server is busy", () => {
    const watched = shownFile();
    watched.watch.reload();
    failed(lastRead(), "busy");
    expect(last(watched)).toMatchObject({ failure: undefined, reading: false });
    expect(textOf(last(watched))).toBe("# Plan\n");
    expect(reads()).toHaveLength(2);
    vi.advanceTimersByTime(POLL_MS);
    expect(reads()).toHaveLength(3);
    expect(lastRead()).not.toHaveProperty("ifVersion");
  });

  // A popout window or a snapshot rebuild builds the pane again: the content
  // is there at once.
  it("keeps the last content of a pane for the next watch", () => {
    const watched = shownFile("# Plan\n", "v1");
    watched.watch.close();
    const again = watchFile();
    expect(textOf(again.watch.state)).toBe("# Plan\n");
    again.watch.setShown(true);
    expect(lastRead()).toMatchObject({ ifVersion: "v1" });
  });

  it("forgets the oldest contents nobody shows beyond the cache's bounds", () => {
    for (let i = 0; i <= CACHE_ENTRIES; i++) {
      const watched = shownFile(`doc ${i}`, "v1", `pane-${i}`);
      vi.advanceTimersByTime(1);
      watched.watch.close();
    }
    expect(watchFile("pane-0").watch.state.document).toBeUndefined();
    expect(textOf(watchFile(`pane-${CACHE_ENTRIES}`).watch.state)).toBe(`doc ${CACHE_ENTRIES}`);
  });

  it("keeps the content through a lost connection and reads again after the next welcome", () => {
    const watched = shownFile();
    watched.watch.reload();
    socket.drop();
    expect(last(watched)).toMatchObject({ failure: undefined, reading: false });
    expect(textOf(last(watched))).toBe("# Plan\n");
    vi.advanceTimersByTime(POLL_MS * 5);
    expect(reads()).toHaveLength(2);

    welcome();
    expect(reads()).toHaveLength(3);
    expect(lastRead()).not.toHaveProperty("ifVersion");
  });

  it("says the connection was lost when there is no content to keep", () => {
    const watched = watchFile();
    watched.watch.setShown(true);
    socket.drop();
    expect(last(watched).failure).toEqual({ code: "disconnected", message: "The connection was lost." });
  });

  it("reads nothing for a pane that is not shown when the connection comes back", () => {
    const watched = shownFile();
    socket.drop();
    watched.watch.reload();
    watched.watch.setShown(false);
    welcome();
    expect(reads()).toHaveLength(1);
    watched.watch.setShown(true);
    expect(reads()).toHaveLength(2);
    expect(lastRead()).not.toHaveProperty("ifVersion");
  });

  it("stops everything when disposed", () => {
    const watched = shownFile();
    docs.dispose();
    vi.advanceTimersByTime(POLL_MS * 5);
    expect(reads()).toHaveLength(1);
    watched.watch.close();
  });
});

// viewer-panes, Viewer Capability: a server without viewer panes is never read.
it("reads nothing from a server that does not announce viewer panes", () => {
  welcome(["scratchpad-v1"]);
  const watched = watchFile();
  watched.watch.setShown(true);
  expect(reads()).toEqual([]);
  expect(last(watched).failure?.code).toBe("unsupported");

  welcome(["scratchpad-v1", "viewer-v1"]);
  expect(reads()).toHaveLength(1);
});

it("waits for the connection's welcome before it reads", () => {
  const watched = watchFile();
  watched.watch.setShown(true);
  expect(reads()).toEqual([]);
  expect(last(watched).failure).toBeUndefined();
  welcome();
  expect(reads()).toHaveLength(1);
});
