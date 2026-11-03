/** The shell needs one setting before creating its title bar. Use an ordinary
 * bounded native client; no bootstrap credential or HTTP endpoint is involved. */
import { randomBytes } from "node:crypto";
import { parseKnownHosts, type KnownHost } from "./remote";
import type { NativeOwner } from "./native_owner";
import { application, dataFrame, NativeKind as K } from "./native_wire";

export async function readWindowSettings(owner: NativeOwner): Promise<{ nativeTitleBar: boolean }> {
  try { return await read(owner, "home.get", body => ({ nativeTitleBar: JSON.parse(body)?.window?.nativeTitleBar === true })); }
  catch { return { nativeTitleBar: false }; }
}
export function readKnownHosts(owner: NativeOwner): Promise<KnownHost[]> { return read(owner, "host.list", parseKnownHosts); }

async function read<T>(owner: NativeOwner, operation: "home.get" | "host.list", consume: (body: string) => T): Promise<T> {
  let rejectReply!: (error: Error) => void;
  let resolveReply!: (value: T) => void;
  const reply = new Promise<T>((resolve, reject) => { resolveReply = resolve; rejectReply = reject; });
  void reply.catch(() => {});
  const connection = owner.connect({ closed: reason => rejectReply(Error(reason.message)) });
  const timer = setTimeout(() => rejectReply(Error("Native metadata read timed out")), 5000);
  try {
    const ready = await connection.pipe.ready;
    connection.pipe.activate(frame => {
      if (frame.kind === K.credit) return;
      const { delivery, bytes } = application(frame);
      try {
        if (frame.kind === K.reply) {
          const response = JSON.parse(new TextDecoder("utf8", { fatal: true }).decode(bytes));
          if (response.id !== 1 || response.status !== 200 || response.contentType !== "json") throw Error("Native metadata was not returned");
          resolveReply(consume(response.body));
        }
      } catch (error) { rejectReply(error instanceof Error ? error : Error(String(error))); }
      finally { connection.pipe.send({ kind: K.credit, payload: Buffer.from(JSON.stringify({ generation: ready.generation,
        receipt: Buffer.from(delivery.receipt).toString("hex") })) }); }
    });
    connection.pipe.send(dataFrame(K.request, { generation: ready.generation, receipt: randomBytes(16) },
      Buffer.from(JSON.stringify({ id: 1, operation, args: {} }))));
    return await reply;
  } finally { clearTimeout(timer); connection.pipe.close(); await connection.released; }
}
