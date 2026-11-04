import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { NativeProcessBudget, nativeProcessBudget } from "./native_budget";
import { NativePipe, type PipeEvents } from "./native_pipe";
import { isAcceptableTarget } from "./remote";

/** The main process supplies the executable/environment and fixed target.
 * There is no renderer-controlled command, executable path or stdio endpoint. */
export function startNativeHelper(binary: string, env: NodeJS.ProcessEnv, events: PipeEvents,
  target?: string, processBudget: NativeProcessBudget = nativeProcessBudget, owner = false, rememberTarget = false, update = false): {
    pipe: NativePipe; child: ChildProcessWithoutNullStreams; exited: Promise<void>; close(): void;
  } {
  if (target !== undefined && (!isAcceptableTarget(target) || target.length > 4096)) throw Error("Invalid native helper target");
  if (owner && target !== undefined) throw Error("An owner helper must be embedded");
  if (rememberTarget && target === undefined) throw Error("Remembering a host requires an SSH helper");
  if (update && target === undefined) throw Error("Updating a host requires an SSH helper");
  const releaseWorker = processBudget.worker();
  const budget = processBudget.connection();
  let releaseDiagnostics: () => void;
  try { releaseDiagnostics = budget.reserve(8192, true); } catch (error) { releaseWorker(); throw error; }
  let child: ChildProcessWithoutNullStreams;
  try {
    child = spawn(binary, ["desktop-backend", "--transport", "native-v1", "--backend", target === undefined ? "embedded" : "ssh",
      ...(target === undefined ? [] : ["--target", target]), ...(owner ? ["--owner"] : []), ...(rememberTarget ? ["--remember-target"] : []), ...(update ? ["--update"] : [])], { env, stdio: ["pipe", "pipe", "pipe"], windowsHide: true });
  } catch (error) { releaseDiagnostics(); releaseWorker(); throw error; }
  let done = false, tail = Buffer.alloc(0), kill: ReturnType<typeof setTimeout> | undefined;
  let pipe: NativePipe | undefined;
  let resolveExit!: () => void;
  const exited = new Promise<void>(resolve => { resolveExit = resolve; });
  const observedExit = () => {
    if (done) return;
    done = true; clearTimeout(kill); tail = Buffer.alloc(0);
    // Process exit does not imply stdio EOF: descendants can inherit stderr.
    // Revoke our reader before returning its byte lease or worker capacity.
    child.stderr.removeAllListeners("data");
    child.stderr.destroy();
    releaseDiagnostics(); releaseWorker(); resolveExit();
  };
  child.stderr.on("data", (chunk: Buffer) => {
    // Keep only a bounded diagnostic suffix; stdout never goes through text.
    const suffix = chunk.subarray(Math.max(0, chunk.length - 4096));
    const retained = tail.subarray(Math.max(0, tail.length - (4096 - suffix.length)));
    tail = Buffer.concat([retained, suffix]);
  });
  child.stderr.on("error", () => {});
  const stop = () => {
    if (done || kill) return;
    // EOF first lets Rust shut down its owned sessions. Escalation targets only
    // this child and never releases its worker slot until actual exit.
    child.stdin.destroy();
    kill = setTimeout(() => { if (!done) child.kill("SIGKILL"); }, 3000);
  };
  // Install the reaper before constructing any fallible protocol state.
  child.once("error", error => {
    pipe?.close({ code: "backendLost", retryable: false, message: `Could not start native backend: ${error.message}` });
    // A failed spawn has no process to reap. Other errors do not establish exit.
    if (child.pid === undefined) observedExit();
  });
  child.once("exit", (code, signal) => {
    const diagnostic = tail.toString("utf8").trim();
    observedExit();
    pipe?.close({ code: "backendLost", retryable: true,
      message: diagnostic || `Native backend exited (${signal ?? code ?? "unknown"})` });
  });
  try {
    pipe = new NativePipe(child.stdout, child.stdin, budget, {
      progress: events.progress,
      closed: reason => { stop(); events.closed(reason); },
    }, target, owner);
  } catch (error) {
    child.stdout.destroy(); stop();
    throw error;
  }
  const connection = pipe;
  return { pipe: connection, child, exited, close: () => connection.close() };
}
