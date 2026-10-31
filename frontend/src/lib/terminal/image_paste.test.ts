import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  attachImageInsert, attachImagePaste, CTRL_V_FALLBACK_MS, droppedFiles, formatBytes, isImage, isPlainCtrlV, MAX_FILE_BYTES, MAX_IMAGE_BYTES,
  pastedFiles, pasteFiles, toBase64, type ImagePasteTarget, type TransferData,
} from "./image_paste.ts";

const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
const png = (name = "screenshot.png") => new File([PNG], name, { type: "image/png" });
const pdf = (name = "spec.pdf") => new File(["%PDF-1.7"], name, { type: "application/pdf" });
/** A file of `size` bytes without allocating them; only its size is read. */
const sized = (name: string, type: string, size: number) => Object.defineProperty(new File([], name, { type }), "size", { value: size });

function transfer(files: File[], text = ""): TransferData {
  return {
    files,
    types: [...(files.length ? ["Files"] : []), ...(text ? ["text/plain"] : [])],
    getData: (format) => (format === "text/plain" ? text : ""),
  };
}

function target(paths: string[] = ["/home/me/.cache/splitty/paste/paste-1.png"]) {
  const queue = [...paths];
  return {
    saveImage: vi.fn(async (_data: string) => queue.shift() ?? "/tmp/x.png"),
    uploadFile: vi.fn(async (_file: File, onProgress?: (received: number) => void) => {
      onProgress?.(_file.size);
      return queue.shift() ?? "/tmp/upload-1/x";
    }),
    paste: vi.fn(),
    sendCtrlV: vi.fn(),
    failed: vi.fn(),
  } satisfies ImagePasteTarget;
}

/** A pane's container: its capture listeners, called the way the browser would. */
function pane(activeTag = "TEXTAREA") {
  const listeners = new Map<string, (e: Event) => void>();
  const container = {
    ownerDocument: { activeElement: { tagName: activeTag } },
    addEventListener: (type: string, fn: (e: Event) => void) => listeners.set(type, fn),
    removeEventListener: (type: string) => listeners.delete(type),
  } as unknown as HTMLElement;
  const fire = (type: string, fields: Record<string, unknown>) => {
    const event = { preventDefault: vi.fn(), stopPropagation: vi.fn(), stopImmediatePropagation: vi.fn(), ...fields };
    listeners.get(type)?.(event as unknown as Event);
    return event;
  };
  const ctrlV = (extra: Record<string, unknown> = {}) =>
    fire("keydown", { ctrlKey: true, shiftKey: false, altKey: false, metaKey: false, code: "KeyV", repeat: false, ...extra });
  return { container, listeners, fire, ctrlV };
}

describe("which files a paste or drop carries", () => {
  it("takes the files of a paste only when it carries no text", () => {
    expect(pastedFiles(transfer([png()]))).toHaveLength(1);
    expect(pastedFiles(transfer([png()], "some text"))).toEqual([]);
    expect(pastedFiles(transfer([new File(["x"], "a.bmp", { type: "image/bmp" })]))).toHaveLength(1);
    expect(pastedFiles(null)).toEqual([]);
  });

  it("takes every file of a drop, in its order", () => {
    const script = new File(["echo"], "run.sh", { type: "text/x-sh" });
    expect(droppedFiles(transfer([script, png("a.png")])).map((f) => f.name)).toEqual(["run.sh", "a.png"]);
  });

  it("sends PNG, JPEG, GIF and WebP up to 10 MB as images, everything else as a file", () => {
    expect(isImage(png())).toBe(true);
    expect(isImage(new File(["x"], "a.bmp", { type: "image/bmp" }))).toBe(false);
    expect(isImage(pdf())).toBe(false);
    expect(isImage(sized("huge.png", "image/png", MAX_IMAGE_BYTES + 1))).toBe(false);
  });

  it("names sizes for people", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(3 * 1024 * 1024)).toBe("3.0 MB");
  });

  it("treats Ctrl+V as the paste except on macOS, where Cmd+V is", () => {
    const key = { ctrlKey: true, shiftKey: false, altKey: false, metaKey: false, code: "KeyV" };
    expect(isPlainCtrlV(key, false)).toBe(true);
    expect(isPlainCtrlV(key, true)).toBe(false);
    expect(isPlainCtrlV({ ...key, shiftKey: true }, false)).toBe(false);
  });

  it("encodes the image as standard base64", () => {
    expect(toBase64(PNG)).toBe("iVBORw0KGgo=");
  });
});

describe("pasting files", () => {
  it("uploads the images one after the other and pastes their paths once", async () => {
    const t = target(["/p/a.png", "/p/b.png"]);
    await pasteFiles([png("a.png"), png("b.png")], t);
    expect(t.saveImage.mock.calls.map(([data]) => data)).toEqual(["iVBORw0KGgo=", "iVBORw0KGgo="]);
    expect(t.paste).toHaveBeenCalledExactlyOnceWith("/p/a.png /p/b.png");
  });

  it("sends images with save_image and other files as uploads, pasting the paths in the files' order", async () => {
    const t = target(["/p/upload-1/spec.pdf", "/p/paste-1.png", "/p/upload-2/notes.txt"]);
    await pasteFiles([pdf(), png(), new File(["n"], "notes.txt", { type: "text/plain" })], t);
    expect(t.saveImage).toHaveBeenCalledOnce();
    expect(t.uploadFile.mock.calls.map(([file]) => file.name)).toEqual(["spec.pdf", "notes.txt"]);
    expect(t.paste).toHaveBeenCalledExactlyOnceWith("/p/upload-1/spec.pdf /p/paste-1.png /p/upload-2/notes.txt");
  });

  it("refuses a file over 512 MB before anything is sent", async () => {
    const t = target();
    await pasteFiles([pdf(), sized("disk.img", "", MAX_FILE_BYTES + 1)], t);
    expect(t.uploadFile).not.toHaveBeenCalled();
    expect(t.paste).not.toHaveBeenCalled();
    expect(t.failed).toHaveBeenCalledExactlyOnceWith("disk.img is larger than 512 MB.", false);
  });

  it("pastes nothing when a file is refused, and says why", async () => {
    const t = target();
    t.saveImage.mockRejectedValueOnce(Object.assign(new Error("too old"), { needsUpdate: true }));
    await pasteFiles([png()], t);
    t.uploadFile.mockRejectedValueOnce(new Error("disk full"));
    await pasteFiles([pdf()], t);
    expect(t.paste).not.toHaveBeenCalled();
    expect(t.failed.mock.calls).toEqual([["too old", true], ["disk full", false]]);
  });

  it("reports the stored bytes and the end of a paste", async () => {
    const progress = vi.fn();
    const t = { ...target(["/p/a", "/p/b"]), progress };
    await pasteFiles([pdf("a.pdf"), pdf("b.pdf")], t);
    expect(progress.mock.calls.at(-1)).toEqual([16, 16, true]);
    expect(progress.mock.calls.filter(([, , done]) => done)).toHaveLength(1);
    t.uploadFile.mockRejectedValueOnce(new Error("lost"));
    await pasteFiles([pdf()], t);
    expect(progress.mock.calls.at(-1)).toEqual([0, 8, true]);
  });
});

describe("a pane's paste and drop", () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  it("lets Ctrl+V's paste upload an image instead of sending ^V", async () => {
    const t = target();
    const p = pane();
    attachImagePaste(p.container, t);
    const key = p.ctrlV();
    expect(key.stopImmediatePropagation).toHaveBeenCalled();
    expect(key.preventDefault).not.toHaveBeenCalled();
    const paste = p.fire("paste", { clipboardData: transfer([png()]) });
    expect(paste.preventDefault).toHaveBeenCalled();
    await vi.runAllTimersAsync();
    expect(t.paste).toHaveBeenCalledExactlyOnceWith("/home/me/.cache/splitty/paste/paste-1.png");
    expect(t.sendCtrlV).not.toHaveBeenCalled();
  });

  it("sends ^V for Ctrl+V when the clipboard holds text, or when no paste follows", () => {
    const t = target();
    const p = pane();
    attachImagePaste(p.container, t);
    p.ctrlV();
    const paste = p.fire("paste", { clipboardData: transfer([], "hello") });
    expect(paste.preventDefault).toHaveBeenCalled();
    expect(t.sendCtrlV).toHaveBeenCalledOnce();
    p.ctrlV();
    vi.advanceTimersByTime(CTRL_V_FALLBACK_MS);
    expect(t.sendCtrlV).toHaveBeenCalledTimes(2);
    expect(t.saveImage).not.toHaveBeenCalled();
  });

  it("leaves Ctrl+V to the terminal where the browser would not paste", () => {
    const t = target();
    const p = pane("CANVAS");
    attachImagePaste(p.container, t);
    const key = p.ctrlV();
    expect(key.stopImmediatePropagation).not.toHaveBeenCalled();
    vi.advanceTimersByTime(CTRL_V_FALLBACK_MS);
    expect(t.sendCtrlV).not.toHaveBeenCalled();
  });

  it("keeps a text paste of the paste shortcut as it was", () => {
    const t = target();
    const p = pane();
    attachImagePaste(p.container, t);
    const paste = p.fire("paste", { clipboardData: transfer([], "ls -la") });
    expect(paste.preventDefault).not.toHaveBeenCalled();
    expect(t.sendCtrlV).not.toHaveBeenCalled();
    expect(t.saveImage).not.toHaveBeenCalled();
  });

  it("uploads dropped images and other files", async () => {
    const t = target();
    const p = pane();
    attachImagePaste(p.container, t);
    const over = p.fire("dragover", { dataTransfer: { ...transfer([png()]), dropEffect: "none" } });
    expect(over.preventDefault).toHaveBeenCalled();
    p.fire("drop", { dataTransfer: transfer([png()]) });
    await vi.runAllTimersAsync();
    expect(t.paste).toHaveBeenCalledOnce();
    p.fire("drop", { dataTransfer: transfer([new File(["echo"], "run.sh", { type: "text/x-sh" })]) });
    await vi.runAllTimersAsync();
    expect(t.uploadFile).toHaveBeenCalledOnce();
    expect(t.paste).toHaveBeenCalledTimes(2);
    expect(t.failed).not.toHaveBeenCalled();
  });

  it("PDF into Claude Code on a host: a dropped PDF is uploaded and its path pasted", async () => {
    const t = target(["/home/me/.cache/splitty/paste/upload-20261029-1/spec.pdf"]);
    const p = pane("CANVAS");
    attachImagePaste(p.container, t);
    p.fire("drop", { dataTransfer: transfer([pdf()]) });
    await vi.runAllTimersAsync();
    expect(t.saveImage).not.toHaveBeenCalled();
    expect(t.paste).toHaveBeenCalledExactlyOnceWith("/home/me/.cache/splitty/paste/upload-20261029-1/spec.pdf");
  });

  it("lets Ctrl+V's paste upload a copied file instead of sending ^V", async () => {
    const t = target(["/p/upload-1/spec.pdf"]);
    const p = pane();
    attachImagePaste(p.container, t);
    p.ctrlV();
    p.fire("paste", { clipboardData: transfer([pdf()]) });
    await vi.runAllTimersAsync();
    expect(t.paste).toHaveBeenCalledExactlyOnceWith("/p/upload-1/spec.pdf");
    expect(t.sendCtrlV).not.toHaveBeenCalled();
  });

  it("stops listening once its pane is gone", () => {
    const p = pane();
    const detach = attachImagePaste(p.container, target());
    detach();
    expect(p.listeners.size).toBe(0);
  });
});

describe("images in a text field", () => {
  function field() {
    const listeners = new Map<string, (e: Event) => void>();
    const el = {
      addEventListener: (type: string, fn: (e: Event) => void) => listeners.set(type, fn),
      removeEventListener: (type: string) => listeners.delete(type),
    } as unknown as HTMLTextAreaElement;
    const fire = (type: string, fields: Record<string, unknown>) => {
      const event = { preventDefault: vi.fn(), ...fields };
      listeners.get(type)?.(event as unknown as Event);
      return event;
    };
    return { el, listeners, fire };
  }

  const store = (saveImage = vi.fn(async (_data: string) => "/x.png"), uploadFile = vi.fn(async (_file: File) => "/cache/upload-1/x")) => ({ saveImage, uploadFile });

  it("inserts the stored path of a pasted image", async () => {
    const { el, fire } = field();
    const save = vi.fn(async (_data: string) => "/cache/paste-1.png");
    const insert = vi.fn();
    attachImageInsert(el, store(save), insert);
    const event = fire("paste", { clipboardData: transfer([png()]) });
    await vi.waitFor(() => expect(insert).toHaveBeenCalledWith("/cache/paste-1.png"));
    expect(event.preventDefault).toHaveBeenCalled();
    expect(save).toHaveBeenCalledWith(toBase64(PNG));
  });

  it("leaves a paste with text to the field", () => {
    const { el, fire } = field();
    const save = vi.fn(async (_data: string) => "/x.png");
    attachImageInsert(el, store(save), vi.fn());
    const event = fire("paste", { clipboardData: transfer([png()], "some text") });
    expect(event.preventDefault).not.toHaveBeenCalled();
    expect(save).not.toHaveBeenCalled();
  });

  it("inserts the paths of dropped images, separated by spaces", async () => {
    const { el, fire } = field();
    const paths = ["/cache/a.png", "/cache/b.png"];
    const insert = vi.fn();
    attachImageInsert(el, store(vi.fn(async () => paths.shift() ?? "")), insert);
    fire("drop", { dataTransfer: transfer([png("a.png"), png("b.png")]) });
    await vi.waitFor(() => expect(insert).toHaveBeenCalledWith("/cache/a.png /cache/b.png"));
  });

  it("A log dropped into the scratchpad: the file is uploaded and its path inserted", async () => {
    const { el, fire } = field();
    const upload = vi.fn(async (_file: File) => "/cache/upload-1/build.log");
    const insert = vi.fn();
    attachImageInsert(el, store(undefined, upload), insert);
    const drop = fire("drop", { dataTransfer: transfer([new File(["ok"], "build.log", { type: "text/plain" })]) });
    expect(drop.preventDefault).toHaveBeenCalled();
    await vi.waitFor(() => expect(insert).toHaveBeenCalledWith("/cache/upload-1/build.log"));
    expect(upload.mock.calls[0][0].name).toBe("build.log");
  });

  it("stops listening when cleaned up", () => {
    const { el, listeners } = field();
    attachImageInsert(el, store(), vi.fn())();
    expect(listeners.size).toBe(0);
  });
});
