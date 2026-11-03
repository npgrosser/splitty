import { expect, it } from "vitest";
import { NativeProcessBudget } from "../../../../frontend-electron/src/native_budget.ts";
import { NativeFrameDecoder, NativeKind, dataFrame, encodeNativeFrame } from "./native_wire.ts";
it("retains reassembly ownership until the consumer releases complete delivery", () => {
 const process = new NativeProcessBudget(), budget = process.connection();
 const decoder = new NativeFrameDecoder(bytes => budget.reserve(bytes));
 const bytes = encodeNativeFrame(dataFrame(NativeKind.binary, {generation:1,receipt:new Uint8Array(16)},new Uint8Array(100)));
 decoder.feed(bytes.subarray(0,5), () => { throw Error('incomplete'); });
 expect(budget.counts.data).toBe(bytes.length);
 let consumed!: () => void;
 decoder.feed(bytes.subarray(5), (_frame,release) => { consumed=release; });
 expect(decoder.bufferedBytes).toBe(0); expect(process.counts.data).toBe(bytes.length);
 consumed(); consumed(); expect(process.counts.data).toBe(0);
});
it("releases incomplete or rejected frames, and refuses allocation before exceeding a budget", () => {
 const process = new NativeProcessBudget(), budget = process.connection();
 const decoder = new NativeFrameDecoder(bytes => budget.reserve(bytes));
 const bytes = encodeNativeFrame(dataFrame(NativeKind.binary,{generation:1,receipt:new Uint8Array(16)},new Uint8Array(100)));
 decoder.feed(bytes.subarray(0,6), () => {}); expect(() => decoder.end()).toThrow('Truncated'); expect(process.counts.data).toBe(0);
 const denied = new NativeFrameDecoder(() => { throw Error('busy'); }); expect(() => denied.feed(bytes, () => {})).toThrow('busy'); expect(denied.bufferedBytes).toBe(0);
 const rejected = new NativeFrameDecoder(n => budget.reserve(n)); expect(() => rejected.feed(bytes, () => {throw Error('invalid application');})).toThrow(); expect(process.counts.data).toBe(0);
});
it("reserves controls while data is full, shares process ceilings and holds worker slots until explicit exit", () => {
 const process = new NativeProcessBudget(); const release: (()=>void)[]=[];
 for(let i=0;i<4;i++) release.push(process.connection().reserve(30*1024*1024));
 expect(()=>process.connection().reserve(1)).toThrow('Process');
 const control = process.connection().reserve(4096,true); expect(process.counts.controls).toBe(4096); control();
 const workers = Array.from({length:32},()=>process.worker()); expect(()=>process.worker()).toThrow('worker');
 workers[0]!();workers[0]!();const next=process.worker();expect(process.counts.workers).toBe(32);next();workers.forEach(exit=>exit());
 release.forEach(done=>done()); expect(process.counts).toEqual({data:0,controls:0,workers:0});
});
