/** The browser route vocabulary is translated once, before crossing native IPC.
 * The broker and service independently enforce this same checked catalog. */
import catalog from "../../../../docs/native-operations.json";

export interface NativeRequest { operation: string; args: Record<string, unknown> }
export interface NativeReply {
  id: number;
  status: number;
  contentType: "json" | "text" | "empty";
  body: string;
  uncertain?: boolean;
}
export interface NativeApi {
  request(request: NativeRequest, options?: { signal?: AbortSignal }): Promise<NativeReply>;
}
export type ApiFetch = (path: string, init?: RequestInit) => Promise<Response>;
export const nativeOperations = catalog.operations;

export function isNativeReply(value: unknown): value is NativeReply {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const reply = value as Record<string, unknown>;
  return typeof reply.id === "number" && Number.isInteger(reply.id) && reply.id > 0 && reply.id <= 0xffffffff
    && typeof reply.status === "number" && Number.isInteger(reply.status) && reply.status >= 200 && reply.status <= 599
    && typeof reply.contentType === "string" && ["json", "text", "empty"].includes(reply.contentType) && typeof reply.body === "string"
    && (reply.uncertain === undefined || typeof reply.uncertain === "boolean")
    && Object.keys(reply).every((key) => ["id", "status", "contentType", "body", "uncertain"].includes(key));
}

/** A lost mutation response never establishes whether the effect happened. */
export class NativeRequestError extends Error {
  constructor(message: string, readonly uncertain: boolean) { super(message); }
}

export function describeRequest(request: NativeRequest) {
  const description = nativeOperations.find((entry) => entry.operation === request.operation);
  if (!description) throw new Error("Unknown native operation");
  const allowed = description.arguments.map((key) => key.replace(/\?$/, ""));
  if (Object.keys(request.args).some((key) => !allowed.includes(key))) throw new Error("Unknown native argument");
  for (const argument of description.arguments) {
    const key = argument.replace(/\?$/, "");
    if (!argument.endsWith("?") && !Object.hasOwn(request.args, key)) throw new Error(`Missing native argument: ${key}`);
    if (Object.hasOwn(request.args, key) && key !== "body" && typeof request.args[key] !== "string") {
      throw new Error(`Invalid native argument: ${key}`);
    }
  }
  return description;
}

/** No URL resolution: normalization must not turn a forbidden path into an API. */
export function nativeRequest(path: string, init: RequestInit = {}): NativeRequest {
  if (!path.startsWith("/api/") || path.includes("#") || path.includes("\\") || /[\x00-\x20\x7f]/.test(path)) {
    throw new Error("Expected an enumerated local API path");
  }
  const split = path.indexOf("?");
  const pathname = split < 0 ? path : path.slice(0, split);
  const query = new URLSearchParams(split < 0 ? "" : path.slice(split + 1));
  const segments = pathname.split("/").map((segment) => {
    const decoded = decodeURIComponent(segment);
    if (decoded === "." || decoded === ".." || /[\x00-\x1f\x7f/\\]/.test(decoded)) throw new Error("Invalid API path segment");
    return decoded;
  });
  const method = (init.method ?? "GET").toUpperCase();
  const description = nativeOperations.find((entry) => {
    const parts = entry.path.split("/");
    return entry.method === method && parts.length === segments.length
      && parts.every((part, index) => part.startsWith("{") ? segments[index]!.length > 0 : part === segments[index]);
  });
  if (!description) throw new Error("Path/method has no native operation");
  const headers = new Headers(init.headers);
  for (const [name, value] of headers) {
    if (name !== "content-type" || value.toLowerCase() !== "application/json") throw new Error("Native API does not accept HTTP headers");
  }
  const args: Record<string, unknown> = Object.create(null);
  description.path.split("/").forEach((part, index) => {
    if (part.startsWith("{")) args[part.slice(1, -1)] = segments[index];
  });
  for (const [key, value] of query) {
    if (!description.arguments.includes(`${key}?`) || Object.hasOwn(args, key)) throw new Error("Unknown or repeated API query argument");
    args[key] = value;
  }
  if (init.body !== undefined && init.body !== null) {
    if (!description.arguments.includes("body") || typeof init.body !== "string") throw new Error("Native API requires a JSON body");
    if (new TextEncoder().encode(init.body).length > description.maxBodyBytes) throw new Error("API body exceeds its limit");
    args.body = JSON.parse(init.body);
  }
  const request = { operation: description.operation, args };
  describeRequest(request);
  if (new TextEncoder().encode(JSON.stringify(request)).length > description.maxBodyBytes) throw new Error("Native request exceeds its limit");
  return request;
}

/** Bound to a particular connection, including during a later default change. */
export function nativeApiFetch(api: NativeApi): ApiFetch {
  return async (path, init) => {
    const request = nativeRequest(path, init);
    init?.signal?.throwIfAborted();
    const reply = await api.request(request, { signal: init?.signal ?? undefined });
    if (!isNativeReply(reply)) {
      throw new NativeRequestError("Invalid native API reply", describeRequest(request).cancellation !== "read");
    }
    if (reply.uncertain) throw new NativeRequestError("The operation may have completed; its result is unknown.", true);
    const empty = reply.contentType === "empty" || [204, 205, 304].includes(reply.status);
    return new Response(empty ? null : reply.body, {
      status: reply.status,
      headers: reply.contentType === "json" ? { "Content-Type": "application/json" }
        : reply.contentType === "text" ? { "Content-Type": "text/plain; charset=utf-8" } : {},
    });
  };
}
