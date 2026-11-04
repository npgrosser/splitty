/** Assigned by the native composition root, never reconstructed from a URL. */
export interface NativeAssignment {
  readonly handle: string;
  readonly document: string;
  readonly generation: number;
  readonly instance: string;
  readonly role: "main" | "remote";
  readonly target?: string;
}
