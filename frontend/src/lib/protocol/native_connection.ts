/** A document's reconnectable native connection. Every attempt is assigned by
 * the shell again; neither failed opens nor lost mutations fall back to HTTP. */
import { NativeSocket, NativeConnectionError, type NativeAssignment, type NativeChannel } from "./native_socket.ts";

export type NativeConnectionState = "connected" | "reconnecting" | "disconnected";
export type OpenNativeChannel = (signal?: AbortSignal) => Promise<NativeChannel>;
export class NativeConnection extends NativeSocket {
  private epoch = 0;
  private opening?: Promise<void>;
  private disposed = false;
  private preparation?: AbortController;
  private lostAt?: number;
  private retries = 0;
  private retryTimer?: ReturnType<typeof setTimeout>;
  private deadlineTimer?: ReturnType<typeof setTimeout>;
  private state: NativeConnectionState = "disconnected";
  onStateChange: ((state: NativeConnectionState) => void) | null = null;
  get connectionState(): NativeConnectionState { return this.state; }
  // Browser sockets expose this callback. Native failures deliberately never
  // call it: an SSH or IPC failure cannot be repaired with a browser token.
  onAuthError: (() => void) | null = null;
  onConnectionError: ((error: Error) => void) | null = null;

  constructor(private readonly open: OpenNativeChannel,
    private readonly expected: Pick<NativeAssignment, "role" | "target" | "document">) {
    super();
    this.onError = error => this.failed(error);
  }

  connect(): Promise<void> {
    if (this.disposed || this.assignment) return Promise.resolve();
    if (this.recoveryExpired()) { this.expire(); return Promise.resolve(); }
    if (this.opening) return this.opening;
    this.setState("reconnecting");
    const epoch = this.epoch;
    const preparation = new AbortController(); this.preparation = preparation;
    const attempt = this.open(preparation.signal).then(channel => {
      if (this.disposed || epoch !== this.epoch) { channel.close(); return; }
      if (this.recoveryExpired()) { channel.close(); this.expire(); return; }
      const assigned = channel.assignment;
      if (assigned.role !== this.expected.role || assigned.target !== this.expected.target
        || assigned.document !== this.expected.document) {
        channel.close(); throw new NativeConnectionError("Native connection assignment changed; reload this window", "assignment", false);
      }
      this.adopt(channel);
      if (this.assignment) { this.clearRecovery(); this.setState("connected"); }
    }).catch(error => {
      if (!this.disposed && epoch === this.epoch) this.failed(error instanceof Error ? error : Error(String(error)));
    }).finally(() => { if (this.opening === attempt) this.opening = undefined; });
    this.opening = attempt;
    return attempt;
  }

  reconnect(): void {
    if (this.disposed) return;
    // Do not overlap an outstanding open. Its quota remains charged until the
    // shell confirms cleanup, even if the caller no longer wants its result.
    if (this.opening) {
      // A deliberate retry renews the recovery window, but keeps the one live
      // attempt. An already cancelled attempt must first confirm its cleanup.
      if (!this.preparation?.signal.aborted && this.state === "reconnecting") {
        this.clearRecovery(); this.lostAt = Date.now();
        this.deadlineTimer = setTimeout(() => this.expire(), this.recoveryLimit);
      }
      return;
    }
    this.clearRecovery();
    this.epoch++; this.revoke(); void this.connect();
  }
  private setState(state: NativeConnectionState): void {
    if (this.state === state) return;
    this.state = state; this.onStateChange?.(state);
  }
  private get recoveryLimit(): number { return this.expected.role === "remote" ? 600_000 : 120_000; }
  private recoveryExpired(): boolean {
    return this.lostAt !== undefined && Date.now() - this.lostAt >= this.recoveryLimit;
  }
  private clearRecovery(): void {
    clearTimeout(this.retryTimer); clearTimeout(this.deadlineTimer);
    this.retryTimer = undefined; this.deadlineTimer = undefined;
    this.lostAt = undefined; this.retries = 0;
  }
  private expire(): void {
    if (this.disposed) return;
    this.clearRecovery(); this.epoch++; this.preparation?.abort();
    this.setState("disconnected");
    this.onConnectionError?.(new NativeConnectionError("Reconnection timed out. Retry to connect again.", "reconnectTimeout", false));
  }
  private failed(error: Error): void {
    this.onConnectionError?.(error);
    if (this.disposed) return;
    if (!(error instanceof NativeConnectionError) || !error.retryable) {
      this.clearRecovery(); this.setState("disconnected"); return;
    }
    // Separate remote windows preserve their established ten-minute recovery
    // policy. Main-window local clients retain the shorter two-minute bound.
    const limit = this.recoveryLimit;
    if (this.lostAt === undefined) {
      this.lostAt = Date.now();
      this.deadlineTimer = setTimeout(() => this.expire(), limit);
    }
    const remaining = limit - (Date.now() - this.lostAt);
    if (remaining <= 0) { this.expire(); return; }
    this.setState("reconnecting");
    const delay = Math.min(1000 * Math.pow(1.5, this.retries++), 10_000);
    clearTimeout(this.retryTimer);
    // The deadline cancels an outstanding preparation too; no last retry can
    // silently extend the recovery window or adopt a late result after expiry.
    if (delay < remaining) this.retryTimer = setTimeout(() => { this.retryTimer = undefined; void this.connect(); }, delay);
  }
  override dispose(): void {
    this.disposed = true; this.clearRecovery(); this.epoch++; this.preparation?.abort(); super.dispose();
  }
}
