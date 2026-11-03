import { observePressure } from '../tauri-windows-native-smoke/pressure.mjs';
import { NativeSocket } from '../../../frontend/src/lib/protocol/native_socket.ts';
import { openTauriChannel } from '../../../frontend/src/lib/protocol/tauri_channel.ts';
import { fixtureState, probeBridge } from './isolation.ts';
const wait=(ms:number)=>new Promise(resolve=>setTimeout(resolve,ms));
const assert=(value:unknown,message:string)=>{if(!value)throw Error(message)};
export async function consumerAdmission(capsule:any,healthy:NativeSocket,request:(operation:string,args?:object)=>Promise<any>,upload:(home:string)=>Promise<string>){
 const bridge=probeBridge(capsule.native);let paused=false,closed=false,packets=0;let firstPausedAt:number|undefined,closedAt:number|undefined;
 const stalled={...bridge,listen:(handle:string,data:any,ended:any)=>bridge.listen(handle,(bytes:Uint8Array,token:string)=>{
  if(paused){packets++;firstPausedAt??=Date.now();return}data(bytes,token);
 },(reason:any)=>{closed=true;closedAt=Date.now();ended(reason)})};
 const baseline=await fixtureState();const identity=healthy.assignment?.handle;assert(identity,'healthy identity absent');
 const slow=new NativeSocket();const events:any[]=[];slow.onMessage(value=>events.push(value));const extras:NativeSocket[]=[];
 try{
  slow.adopt(await openTauriChannel(stalled));slow.send({type:'create',shell:'/bin/sh',cols:80,rows:24});
  let deadline=Date.now()+10000;let session:string|undefined;
  while(!session&&Date.now()<deadline){session=events.find(value=>value.type==='created')?.sessionId;await wait(25)}assert(session,'slow PTY absent');
  paused=true;
  const {healthyFile,chronology,high}=await observePressure({now:Date.now,wait,closed:()=>closed,pausedPackets:()=>packets,firstPausedAt:()=>firstPausedAt,closedAt:()=>closedAt,
   alive:()=>healthy.assignment?.handle===identity,request:()=>request('home.get'),upload:()=>upload(baseline.home),
   output:()=>request('session.send',{id:session,body:{data:"i=0; while [ $i -lt 5000 ]; do printf 'SLOW_CAPTURE_%s\\n' \"$i\"; i=$((i+1)); done; printf 'CAPTURE_%s\\n' 'CONTINUES'\n"}}),
   sample:async()=>{const current=await fixtureState();assert(current.bytes<=128*1024*1024&&current.admission.streams<=128&&current.admission.workers<=32,'native ceiling exceeded');return current},
  });
  await fixtureState({phase:'pressure-observed',healthyFile,chronology,high,firstPausedAt,closedAt,packets});
  slow.dispose();
  await request('session.send',{id:session,body:{data:"printf 'AFTER_%s\\n' 'SLOW_CLOSE'\n"}});
  const replay=new NativeSocket();extras.push(replay);const bytes:number[]=[];const replayEvents:any[]=[];let replayError:string|undefined;
  const replayErrors:{message:string,stack?:string}[]=[];
  const recordReplayError=(error:Error)=>{const message=String(error).slice(0,512);replayError??=message;
   if(replayErrors.length<4)replayErrors.push({message,stack:error.stack?.slice(0,1024)})};
  replay.onError=recordReplayError;
  replay.onMessage((value:any)=>{if(replayEvents.length<16)replayEvents.push({type:value.type,sessionId:value.sessionId,error:value.error})});
  replay.onBinaryMessage((_id,data)=>{if(bytes.length+data.length>1024*1024){recordReplayError(Error('replay byte bound'));replay.dispose();return}bytes.push(...data)});
  replay.adopt(await openTauriChannel(bridge));const replayHandle=replay.assignment?.handle;replay.send({type:'attach',id:session!,cols:80,rows:24});deadline=Date.now()+10000;
  while(!new TextDecoder().decode(new Uint8Array(bytes)).includes('AFTER_SLOW_CLOSE')&&!replayError&&Date.now()<deadline)await wait(25);
  const marker=new TextDecoder().decode(new Uint8Array(bytes)).includes('AFTER_SLOW_CLOSE');
  await fixtureState({phase:'pressure-replay',session,replayHandle,replayEvents,replayError,replayErrors,bytes:bytes.length,lastBytes:bytes.slice(-96),marker});
  assert(marker,'post-close capture missing: '+replayError);replay.dispose();extras.pop();
  deadline=Date.now()+10000;while((await fixtureState()).broker.mainHandles!==baseline.broker.mainHandles&&Date.now()<deadline)await wait(25);
  const restored=await fixtureState();assert(restored.broker.mainHandles===baseline.broker.mainHandles,'slow channel admission leaked');
  for(let count=restored.broker.mainHandles;count<32;count++){
   const peer=new NativeSocket();peer.adopt(await openTauriChannel(bridge));extras.push(peer);
  }
  const saturated=await fixtureState();assert(saturated.broker.mainHandles===32,'window32 ceiling not reached');
  let denied=false;try{const excess=await openTauriChannel(bridge);excess.close()}catch{denied=true}assert(denied,'excess window stream admitted');
  await request('home.get');extras.pop()!.dispose();deadline=Date.now()+10000;
  while((await fixtureState()).broker.mainHandles!==31&&Date.now()<deadline)await wait(25);
  const replacement=new NativeSocket();replacement.adopt(await openTauriChannel(bridge));extras.push(replacement);
  assert(healthy.assignment?.handle===identity,'healthy identity changed');
  return {healthyFile,chronology,high,saturated,captureReplay:true,window32:true,excessRefused:true,replacementAdmitted:true,
   scope:'receipt starvation with concurrent healthy work; no engine freeze, RSS or independent global/worker ceiling claim'};
 }finally{
  slow.dispose();for(const peer of extras)peer.dispose();const deadline=Date.now()+10000;
  while((await fixtureState()).broker.mainHandles!==baseline.broker.mainHandles&&Date.now()<deadline)await wait(25);
  assert((await fixtureState()).broker.mainHandles===baseline.broker.mainHandles,'admission did not recover');
 }
}
