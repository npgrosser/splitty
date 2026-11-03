/** Shared process authority, never keyed by a renderer's supplied IDs. Leases
 * remain charged until both pipe delivery and renderer consumption complete. */
const PROCESS_DATA = 120 * 1024 * 1024;
const PROCESS_CONTROL = 8 * 1024 * 1024;
const CONNECTION_DATA = 32 * 1024 * 1024 - 64 * 1024;
const CONNECTION_CONTROL = 64 * 1024;
export class NativeProcessBudget {
  private data = 0;
  private controls = 0;
  private workers = 0;
  get counts() { return { data: this.data, controls: this.controls, workers: this.workers }; }
  connection(): NativeConnectionBudget { return new NativeConnectionBudget(this); }
  worker(): () => void {
    if (this.workers >= 32) throw new Error("Transport worker capacity exhausted");
    this.workers++; let active = true;
    return () => { if (active) { active = false; this.workers--; } };
  }
  reserve(bytes: number, control: boolean): () => void {
    if (!Number.isSafeInteger(bytes) || bytes < 0) throw new Error("Invalid byte reservation");
    const used = control ? this.controls : this.data;
    if (used + bytes > (control ? PROCESS_CONTROL : PROCESS_DATA)) throw new Error("Process transport byte limit exceeded");
    if (control) this.controls += bytes; else this.data += bytes;
    let active = true;
    return () => { if (active) { active = false; if (control) this.controls -= bytes; else this.data -= bytes; } };
  }
}
export class NativeConnectionBudget {
  private data = 0;
  private controls = 0;
  constructor(private readonly process: NativeProcessBudget) {}
  get counts() { return { data: this.data, controls: this.controls }; }
  reserve(bytes: number, control = false): () => void {
    const used = control ? this.controls : this.data;
    if (!Number.isSafeInteger(bytes) || bytes < 0 || used + bytes > (control ? CONNECTION_CONTROL : CONNECTION_DATA)) throw new Error("Connection transport byte limit exceeded");
    const release = this.process.reserve(bytes, control);
    if (control) this.controls += bytes; else this.data += bytes;
    let active = true;
    return () => { if (active) { active = false; release(); if (control) this.controls -= bytes; else this.data -= bytes; } };
  }
}
export const nativeProcessBudget = new NativeProcessBudget();
