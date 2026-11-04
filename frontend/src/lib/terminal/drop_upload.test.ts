import { fakeServer } from "../testing/fake_server.ts";
import { describe, expect, it, vi } from "vitest";
import { quoteUploadPath, uploadDroppedFiles, type DropProgress } from "./drop_upload.ts";

const files = () => [new File(["abc"], "report.pdf"), new File(["xy"], "image.png", { type: "image/png" })];

describe("confirmed drop uploads", () => {
  it("uploads every file including images in order and pastes literal paths once", async () => {
    const progress: DropProgress[] = [];
    const paste = vi.fn();
    const uploadFile = vi.fn(async (file: File, report?: (n: number) => void) => {
      report?.(file.size);
      return `/my project/${file.name}`;
    });
    await uploadDroppedFiles(files(), { uploadFile, paste }, { insert: true }, (p) => progress.push(p));
    expect(uploadFile.mock.calls.map(([file]) => file.name)).toEqual(["report.pdf", "image.png"]);
    expect(paste).toHaveBeenCalledExactlyOnceWith("'/my project/report.pdf' '/my project/image.png'");
    expect(progress.at(-1)).toEqual({ stored: 5, total: 5, index: 1, completed: 2 });
  });

  it("stores a durable file without inserting its path when unchecked", async () => {
    const target = { uploadFile: vi.fn(async () => "/work/report.pdf"), paste: vi.fn() };
    await uploadDroppedFiles(files().slice(0, 1), target, { dir: "/work", insert: false }, vi.fn());
    expect(target.uploadFile).toHaveBeenCalledWith(expect.any(File), expect.any(Function), expect.objectContaining({ dir: "/work" }));
    expect(target.paste).not.toHaveBeenCalled();
  });

  it("stops the batch on cancellation, retains completed files and pastes nothing", async () => {
    const controller = new AbortController();
    const progress: DropProgress[] = [];
    const target = { uploadFile: vi.fn(async (file: File) => {
      if (file.name === "image.png") controller.abort();
      controller.signal.throwIfAborted();
      return "/tmp/report.pdf";
    }), paste: vi.fn() };
    await expect(uploadDroppedFiles([...files(), new File(["z"], "third")], target,
      { insert: true, signal: controller.signal }, (p) => progress.push(p))).rejects.toMatchObject({ name: "AbortError" });
    expect(target.uploadFile).toHaveBeenCalledTimes(2);
    expect(progress.at(-1)?.completed).toBe(1);
    expect(target.paste).not.toHaveBeenCalled();
  });

  it("rejects an oversized batch before sending its first file", async () => {
    const huge = Object.defineProperty(new File([], "big"), "size", { value: 512 * 1024 * 1024 + 1 });
    const target = { uploadFile: vi.fn(), paste: vi.fn() };
    await expect(uploadDroppedFiles([...files(), huge], target, { insert: true }, vi.fn())).rejects.toThrow("larger than");
    expect(target.uploadFile).not.toHaveBeenCalled();
  });

  it("does not insert partial paths when any file fails", async () => {
    const target = { uploadFile: vi.fn().mockResolvedValueOnce("/tmp/first").mockRejectedValueOnce(new Error("exists")), paste: vi.fn() };
    await expect(uploadDroppedFiles(files(), target, { insert: true }, vi.fn())).rejects.toThrow("exists");
    expect(target.paste).not.toHaveBeenCalled();
  });

  it("quotes shell metacharacters without changing safe paths", () => {
    expect(quoteUploadPath("/tmp/report.pdf")).toBe("/tmp/report.pdf");
    expect(quoteUploadPath("/work/$(touch x)/a")).toBe("'/work/$(touch x)/a'");
    expect(quoteUploadPath("/it's/a")).toBe("'/it'\"'\"'s/a'");
  });
});

it("keeps a confirmed batch on its original connection between files", async () => {
  const { conn, socket } = fakeServer("local");
  const destination = { captureConnection: () => conn.sessions.captureConnection(), uploadFile: vi.fn(async () => "/tmp/first"), paste: vi.fn() };
  await expect(uploadDroppedFiles(files(), destination, { insert: true }, progress => { if (progress.completed === 1) socket.drop(); })).rejects.toThrow("connection changed");
  expect(destination.uploadFile).toHaveBeenCalledOnce(); expect(destination.paste).not.toHaveBeenCalled(); conn.dispose();
});
