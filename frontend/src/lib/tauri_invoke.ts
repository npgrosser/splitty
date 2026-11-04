/** The main-document capsule supplies the private current-document proof.
 * Public command callers never receive or reconstruct that proof. */
export function invokeTauri<T = unknown>(command: string, args?: Record<string, unknown>): Promise<T> {
  const capsule = (window as unknown as { __SPLITTY_TAURI__?: { invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> } }).__SPLITTY_TAURI__;
  if (!capsule) return Promise.reject(Error("Native document is unavailable; reload this window"));
  return capsule.invoke<T>(command, args);
}
