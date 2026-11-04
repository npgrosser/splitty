import { EventEmitter } from "node:events";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { transpileModule, ModuleKind, ScriptTarget } from "typescript";
import { afterEach, expect, it, vi } from "vitest";
import type { ElectronNativeBridge } from "./electron_channel.ts";

const source = transpileModule(readFileSync(new URL("../../../../frontend-electron/src/preload.ts", import.meta.url), "utf8"), {
  compilerOptions: { module: ModuleKind.CommonJS, target: ScriptTarget.ES2022 },
}).outputText;
afterEach(() => vi.useRealTimers());

it("routes preload progress only to the current document's pending request and forgets cancelled listeners", async () => {
  vi.useFakeTimers();
  const ipcRenderer = Object.assign(new EventEmitter(), { send: vi.fn(), invoke: vi.fn() });
  let bridge!: ElectronNativeBridge;
  const electron = { ipcRenderer, contextBridge: { exposeInMainWorld: (_name: string, value: { native: ElectronNativeBridge }) => { bridge = value.native; } } };
  runInNewContext(source, { exports: {}, require: (name: string) => { if (name !== "electron") throw Error(name); return electron; },
    process: { platform: "linux" }, setTimeout, clearTimeout, setInterval, clearInterval });
  ipcRenderer.emit("native:document", {}, { document: "doc-one" });
  const first = vi.fn(), second = vi.fn();
  const opening = bridge.open({ target: "vm1" }, "first", first);
  const other = bridge.open({ target: "vm2" }, "second", second);
  const failed = expect(opening).rejects.toThrow("failed");
  const cancelled = expect(other).rejects.toThrow("cancelled");
  await Promise.resolve();
  const report = (request: string, document = "doc-one", message = "Installing") => ipcRenderer.emit("native:progress", {}, { request, document, phase: "install", message });
  report("first"); expect(first).toHaveBeenCalledWith({ phase: "install", message: "Installing" }); expect(second).not.toHaveBeenCalled();
  report("unknown"); report("first", "wrong"); report("first", "doc-one", "x".repeat(4097));
  expect(first).toHaveBeenCalledOnce();
  bridge.cancelOpen("second"); report("second"); await cancelled; expect(second).not.toHaveBeenCalled();
  ipcRenderer.emit("native:document", {}, { document: "doc-two" });
  report("first"); report("first", "doc-two"); expect(first).toHaveBeenCalledOnce();
  ipcRenderer.emit("native:failed", {}, { request: "first", message: "failed" }); await failed;
  report("first"); expect(first).toHaveBeenCalledOnce(); expect(vi.getTimerCount()).toBe(0);
});
