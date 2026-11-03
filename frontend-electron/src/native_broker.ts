/** Native authority is a registered WebContents plus its actual main-frame
 * document. A request id only correlates an answer; it is never a capability. */
import { randomUUID } from "node:crypto";
import type { IpcMainEvent, MessageChannelMain, WebContents, WebFrameMain } from "electron";
import type { NativePipe, PipeEvents, NativeClosed } from "./native_pipe";
import { NativePort } from "./native_port";
import { isPackagedDocument } from "./navigation";
import type { NativeAssignment } from "./native_types";

interface Connection { pipe: NativePipe; released: Promise<void> }
export interface NativeWindowGrant {
  readonly role: "main" | "remote";
  readonly target?: string;
  /** The composition root fixes the service. Only a main grant may ask that
   * service's Hub for another target; remote grants ignore all target input. */
  open(events: PipeEvents, host?: string, update?: boolean): Connection;
}
interface Attempt {
  request: string;
  document: string;
  opening: boolean;
  revoked: boolean;
  released: boolean;
  pipe?: NativePipe;
  port?: NativePort;
  failure?: NativeClosed;
}
interface Grant {
  policy: NativeWindowGrant;
  document: string;
  attempts: Set<Attempt>;
  navigating: boolean;
}
function entry(url: string): boolean { return isPackagedDocument("main", url); }

export class NativeBroker {
  private grants = new WeakMap<WebContents, Grant>();
  private attempts = new Set<Attempt>();
  private pending = 0;
  constructor(private readonly channel: () => Pick<MessageChannelMain, "port1" | "port2">,
    /** The composition root's synchronous navigation guard, never renderer
     * input. Destinations it will prevent never change the document lifetime. */
    private readonly navigationAllowed?: (contents: WebContents, url: string) => boolean) {}
  get counts() { return { connections: this.attempts.size, opening: this.pending }; }

  grant(contents: WebContents, policy: NativeWindowGrant): void {
    if (this.grants.has(contents)) throw Error("Window already has a native grant");
    if (policy.role === "remote" && !policy.target) throw Error("Remote window needs a fixed target");
    const grant: Grant = { policy, document: randomUUID(), attempts: new Set(), navigating: contents.isLoadingMainFrame() };
    this.grants.set(contents, grant);
    contents.on("did-start-navigation", details => {
      if (this.navigationAllowed && !this.navigationAllowed(contents, details.url)) return;
      if (details.isMainFrame && !details.isSameDocument) { grant.navigating = true; this.revoke(contents); }
    });
    contents.on("did-navigate", () => { this.revoke(contents); grant.navigating = false; });
    contents.on("did-stop-loading", () => {
      // An admitted navigation can also be aborted before commit. Once the
      // native loader confirms it has stopped on an authorized entry, publish
      // a fresh challenge; old handles/queued requests remain revoked.
      if (grant.navigating && !contents.isDestroyed() && !contents.isLoadingMainFrame()
        && entry(contents.mainFrame.url)) {
        grant.navigating = false; this.revoke(contents);
      }
    });
    contents.on("render-process-gone", () => this.revoke(contents));
    contents.once("destroyed", () => { this.revoke(contents); this.grants.delete(contents); });
  }

  /** A fresh native challenge reaches only the currently committed document's
   * private preload. Queued IPC from an old document still carries its old
   * challenge, even when Electron reuses the WebFrameMain object at commit. */
  document(event: Pick<IpcMainEvent, "sender" | "senderFrame">): void {
    const caller = this.caller(event);
    if (caller) this.notify(caller.frame, "native:document", { document: caller.grant.document });
  }

  /** Used by native long-running commands to retain the same caller binding. */
  callerDocument(event: Pick<IpcMainEvent, "sender" | "senderFrame">): string | undefined {
    const caller = this.caller(event);
    return caller?.grant.policy.role === "main" ? caller.grant.document : undefined;
  }

  /** Native shell commands carry the private preload document challenge too. */
  authorized(event: Pick<IpcMainEvent, "sender" | "senderFrame">, document: unknown): boolean {
    const caller = this.caller(event);
    return !!caller && caller.grant.policy.role === "main" && caller.grant.document === document;
  }

  /** Presentation-only status for this exact remote window. No target/title,
   * backend operation or main-window command is accepted in the payload. */
  remoteState(event: Pick<IpcMainEvent, "sender" | "senderFrame">, input: unknown): "connected" | "reconnecting" | "disconnected" | undefined {
    const caller = this.caller(event);
    if (!caller || caller.grant.policy.role !== "remote" || !input || typeof input !== "object" || Array.isArray(input)) return;
    const value = input as Record<string, unknown>;
    if (value.document !== caller.grant.document || Object.keys(value).some(key => !["document", "state"].includes(key))) return;
    if (value.state === "connected" || value.state === "reconnecting" || value.state === "disconnected") return value.state;
  }

  /** Only native window orchestration calls this; the event reaches the
   * current private preload and cannot address another document/backend. */
  retryRemote(contents: WebContents): boolean {
    try {
      const caller = this.caller({ sender: contents, senderFrame: contents.mainFrame });
      if (!caller || caller.grant.policy.role !== "remote") return false;
      this.notify(caller.frame, "native:retry", { document: caller.grant.document });
      return true;
    } catch { return false; }
  }

  revoke(contents: WebContents): void {
    const grant = this.grants.get(contents);
    if (!grant) return;
    // Keep old attempts charged across the document change until their real
    // helper/stream cleanup completes. Reload cannot reset a window's quota.
    grant.document = randomUUID();
    for (const attempt of grant.attempts) this.stop(attempt, {
      code: "assignment", retryable: false, message: "The document connection was revoked. Reload this window.",
    });
    if (!grant.navigating) {
      try { this.document({ sender: contents, senderFrame: contents.mainFrame }); } catch { /* Destroyed frame. */ }
    }
  }

  open(event: Pick<IpcMainEvent, "sender" | "senderFrame">, input: unknown): void {
    const caller = this.caller(event);
    if (!caller) return;
    const { grant, frame } = caller;
    if (!input || typeof input !== "object" || Array.isArray(input)) return;
    const value = input as Record<string, unknown>;
    if (typeof value.request !== "string" || !/^[a-zA-Z0-9-]{1,64}$/.test(value.request)) return;
    const request = value.request;
    const fail = (message: string, code = "protocol", retryable = false) => this.notify(frame, "native:failed", { request, message, code, retryable });
    if (value.document !== grant.document) { fail("Native document challenge is stale"); return; }
    if (Object.keys(value).some(key => !["request", "document", "target", "update"].includes(key))
      || (value.target !== undefined && (typeof value.target !== "string" || value.target.length > 4096))
      || (value.update !== undefined && typeof value.update !== "boolean")
      || (value.update === true && value.target === undefined)) { fail("Invalid native open intent"); return; }
    if (grant.policy.role === "remote" && (value.target !== undefined || value.update !== undefined)) {
      fail("This window can only use its assigned host"); return;
    }
    if (Array.from(grant.attempts).some(attempt => attempt.request === request && attempt.document === grant.document)) {
      fail("Duplicate native open request"); return;
    }
    if (grant.attempts.size >= 32 || this.attempts.size >= 128 || this.pending >= 16
      || Array.from(grant.attempts).filter(attempt => attempt.opening).length >= 4) {
      fail("Native connection capacity exhausted", "busy", true); return;
    }
    const attempt: Attempt = { request, document: grant.document, opening: true, revoked: false, released: false };
    grant.attempts.add(attempt); this.attempts.add(attempt); this.pending++;
    const current = () => {
      try { return !attempt.revoked && !attempt.released && !grant.navigating && this.grants.get(event.sender) === grant
        && grant.document === attempt.document && !event.sender.isDestroyed()
        && event.sender.mainFrame === frame && frame.parent === null && entry(frame.url); }
      catch { return false; }
    };
    const release = () => {
      if (attempt.released) return;
      attempt.released = true;
      if (attempt.opening) { attempt.opening = false; this.pending--; }
      grant.attempts.delete(attempt); this.attempts.delete(attempt);
    };
    try {
      const connected = grant.policy.open({
        progress: progress => { if (current()) this.notify(frame, "native:progress", { request, ...progress }); },
        closed: reason => {
          attempt.failure = reason;
          attempt.port?.close(reason);
          if (!attempt.port && current()) this.notify(frame, "native:failed", { request, ...reason });
        },
      }, value.target as string | undefined, value.update === true);
      attempt.pipe = connected.pipe;
      // Factory cleanup is evidence, rather than renderer cancellation or a
      // failed ready promise, that its resource slot can be returned.
      void connected.released.then(release, () => { this.stop(attempt); });
      void connected.pipe.ready.then(ready => {
        if (!current() || attempt.failure) { this.stop(attempt); return; }
        if (ready.purpose.kind === "owner") throw Error("Owner carriers cannot be assigned to a document");
        const target = grant.policy.role === "remote" ? grant.policy.target : value.target as string | undefined;
        if (target === undefined ? ready.purpose.kind !== "direct" : ready.purpose.kind !== "host" || ready.purpose.target !== target) {
          throw Error("Native service differs from the window assignment");
        }
        const assignment: NativeAssignment = Object.freeze({ handle: randomUUID(), document: attempt.document,
          generation: ready.generation, instance: ready.instance, role: target === undefined ? "main" : "remote",
          ...(target === undefined ? {} : { target }) });
        const channel = this.channel();
        try {
          attempt.port = new NativePort(channel.port1, connected.pipe, assignment, current);
          if (!current()) { channel.port2.close(); this.stop(attempt); return; }
          frame.postMessage("native:assigned", { request, assignment }, [channel.port2]);
          attempt.opening = false; this.pending--;
        } catch (error) { channel.port1.close(); channel.port2.close(); throw error; }
      }).catch(error => {
        if (current() && !attempt.failure) fail(error instanceof Error ? error.message : String(error));
        this.stop(attempt);
      });
    } catch (error) {
      if (current()) fail(error instanceof Error ? error.message : String(error));
      this.stop(attempt); release();
    }
  }

  cancel(event: Pick<IpcMainEvent, "sender" | "senderFrame">, input: unknown): void {
    const caller = this.caller(event);
    if (!caller || !input || typeof input !== "object") return;
    const value = input as Record<string, unknown>;
    if (value.document !== caller.grant.document || typeof value.request !== "string") return;
    for (const attempt of caller.grant.attempts) {
      if (attempt.request === value.request && attempt.document === caller.grant.document) this.stop(attempt);
    }
  }
  private caller(event: Pick<IpcMainEvent, "sender" | "senderFrame">): { grant: Grant; frame: WebFrameMain } | undefined {
    try {
      const grant = this.grants.get(event.sender), frame = event.senderFrame;
      if (!grant || grant.navigating || !frame || event.sender.isDestroyed() || frame.parent !== null
        || frame !== event.sender.mainFrame || !entry(frame.url)) return;
      return { grant, frame };
    } catch { return; }
  }
  private stop(attempt: Attempt, reason?: NativeClosed): void {
    if (attempt.revoked) return;
    attempt.revoked = true;
    try { attempt.port?.close(reason); } finally { attempt.pipe?.close(reason); }
  }
  private notify(frame: WebFrameMain, channel: string, value: unknown): void {
    try { frame.send(channel, value); } catch { /* The exact old document may already be destroyed. */ }
  }
}
