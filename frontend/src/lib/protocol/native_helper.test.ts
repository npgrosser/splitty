import { expect, it } from "vitest";
import { setImmediate as nextTurn } from "node:timers/promises";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { NativeProcessBudget } from "../../../../frontend-electron/src/native_budget.ts";
import { startNativeHelper } from "../../../../frontend-electron/src/native_helper.ts";

it("rejects invalid targets and worker floods before process startup", () => {
  const budget = new NativeProcessBudget();
  const events = { closed() {} };
  for (const target of ["--local", "host\ncommand", "", "x".repeat(4097)]) {
    expect(() => startNativeHelper("unreachable", {}, events, target, budget)).toThrow("Invalid");
  }
  expect(() => startNativeHelper("unreachable", {}, events, undefined, budget, false, true)).toThrow("requires an SSH helper");
  expect(() => startNativeHelper("unreachable", {}, events, undefined, budget, false, false, true)).toThrow("requires an SSH helper");
  expect(budget.counts.workers).toBe(0);
  const workers = Array.from({ length: 32 }, () => budget.worker());
  expect(() => startNativeHelper("unreachable", {}, events, undefined, budget)).toThrow("worker");
  workers.forEach(release => release()); expect(budget.counts).toEqual({ data: 0, controls: 0, workers: 0 });
});

it.runIf(process.platform !== "win32")("passes explicit update intent only to its assigned SSH child", async () => {
  const fixture = mkdtempSync(join(tmpdir(), "splitty-helper-migration-"));
  const executable = join(fixture, "helper"), output = join(fixture, "args.json");
  writeFileSync(executable, `#!${process.execPath}\nrequire('node:fs').writeFileSync(process.env.ARGS_OUTPUT, JSON.stringify(process.argv.slice(2)));\n`, { mode: 0o700 });
  const budget = new NativeProcessBudget();
  try {
    for (const update of [true, false]) {
      const helper = startNativeHelper(executable, { ARGS_OUTPUT: output }, { closed() {} }, "host.invalid", budget, false, true, update);
      void helper.pipe.ready.catch(() => {});
      await helper.exited; await nextTurn();
      const args = JSON.parse(readFileSync(output, "utf8"));
      expect(args).toContain("--target"); expect(args).toContain("host.invalid");
      expect(args.includes("--update")).toBe(update);
    }
    expect(budget.counts).toEqual({ data: 0, controls: 0, workers: 0 });
  } finally { rmSync(fixture, { recursive: true, force: true }); }
});

it("cleans up a failed OS spawn without claiming there was an exited child", async () => {
  const budget = new NativeProcessBudget(), failures: unknown[] = [];
  const helper = startNativeHelper("/nonexistent/splitty-native-helper", {}, { closed(reason) { failures.push(reason); } }, undefined, budget);
  await expect(helper.pipe.ready).rejects.toThrow();
  await helper.exited;
  expect(helper.child.pid).toBeUndefined(); expect(failures).toHaveLength(1);
  expect(budget.counts).toEqual({ data: 0, controls: 0, workers: 0 });
});

it("reaps the child when initial framing reservations exceed the remaining control budget", async () => {
  const budget = new NativeProcessBudget(), failures: unknown[] = [];
  const releasePressure = budget.reserve(8 * 1024 * 1024 - 8192, true);
  const helper = startNativeHelper(process.execPath, {}, { closed(reason) { failures.push(reason); } }, undefined, budget);
  await expect(helper.pipe.ready).rejects.toThrow("Process transport byte limit");
  expect(budget.counts.workers).toBe(1);
  await helper.exited; await nextTurn(); releasePressure();
  expect(failures).toHaveLength(1); expect(helper.child.stderr.destroyed).toBe(true);
  expect(helper.child.stderr.listenerCount("data")).toBe(0);
  expect(budget.counts).toEqual({ data: 0, controls: 0, workers: 0 });
});

it.runIf(process.platform !== "win32")("revokes inherited stderr before releasing diagnostic bytes on process exit", async () => {
  const fixture = mkdtempSync(join(tmpdir(), "splitty-inherited-stderr-"));
  const executable = join(fixture, "helper");
  // The descendant outlives its parent and keeps fd 2. It is bounded and owns
  // no resources outside this fixture; a broken pipe ends it normally too.
  const descendant = "process.stderr.on('error',()=>process.exit(0));setTimeout(()=>process.stderr.write('x'.repeat(4096)),150);setTimeout(()=>process.exit(0),300)";
  writeFileSync(executable, `#!${process.execPath}\nrequire('node:child_process').spawn(process.execPath,['-e',${JSON.stringify(descendant)}],{stdio:['ignore','ignore',2]}).unref();\n`, { mode: 0o700 });
  const budget = new NativeProcessBudget();
  let helper: ReturnType<typeof startNativeHelper> | undefined;
  try {
    helper = startNativeHelper(executable, {}, { closed() {} }, undefined, budget);
    void helper.pipe.ready.catch(() => {});
    await helper.exited; await nextTurn();
    expect(helper.child.stderr.destroyed).toBe(true);
    expect(helper.child.stderr.listenerCount("data")).toBe(0);
    expect(budget.counts).toEqual({ data: 0, controls: 0, workers: 0 });
  } finally {
    helper?.close(); if (helper) await helper.exited;
    rmSync(fixture, { recursive: true, force: true });
  }
});
