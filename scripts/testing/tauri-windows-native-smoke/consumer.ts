import { fixtureError } from './error.mjs';
import { observePressure } from './pressure.mjs';
import { NativeSocket } from '../../../frontend/src/lib/protocol/native_socket.ts';
import { openTauriChannel } from '../../../frontend/src/lib/protocol/tauri_channel.ts';
import { probeBridge } from './document.mjs';
import { fixtureState } from './isolation.ts';
const wait=(ms:number)=>new Promise(resolve=>setTimeout(resolve,ms));
const assert=(value:unknown,message:string)=>{if(!value)throw Error(message)};
export async function consumerAdmission(capsule:any,healthyRequest:(operation:string,args?:object)=>Promise<any>,healthyUpload:()=>Promise<string>,healthyIdentity:()=>string|undefined,checkpoint:(phase:string,value:object)=>Promise<unknown>){
 const bridge=probeBridge(capsule.native);let paused=false,closed=false;let pausePackets=0;let firstPausedAt:number|undefined,closedAt:number|undefined;let slowCloseReason:string|undefined;
 const stalled={...bridge,listen:(handle:string,data:any,ended:any)=>bridge.listen(handle,(bytes:Uint8Array,token:string)=>{
  if(paused){pausePackets++;firstPausedAt??=Date.now();return}data(bytes,token);
 },(reason:any)=>{closed=true;closedAt=Date.now();slowCloseReason=fixtureError(reason);ended(reason)})};
 const baseline=await fixtureState();const slow=new NativeSocket();const events:any[]=[];slow.onMessage(value=>events.push(value));
 const extras:NativeSocket[]=[];const identity=healthyIdentity();assert(identity,'healthy native identity missing');
 try{
  slow.adopt(await openTauriChannel(stalled));slow.send({type:'create',cols:80,rows:24});
  let end=Date.now()+10000;let session:string|undefined;
  while(!session&&Date.now()<end){session=events.find(value=>value.type==='created')?.sessionId;await wait(25)}assert(session,'slow client PTY missing');
  paused=true;
  const {healthyFile,chronology,high}=await observePressure({
   now:Date.now,wait,closed:()=>closed,pausedPackets:()=>pausePackets,firstPausedAt:()=>firstPausedAt,closedAt:()=>closedAt,
   alive:()=>healthyIdentity()===identity,
   request:()=>healthyRequest('home.get'),upload:healthyUpload,
   output:()=>healthyRequest('session.send',{id:session,body:{data:"i=0; while [ $i -lt 5000 ]; do printf 'SLOW_CAPTURE_%s\\n' \"$i\"; i=$((i+1)); done; printf 'CAPTURE_%s\\n' 'CONTINUES'\n"}}),
   sample:async()=>{const current=await fixtureState();assert(current.bytes<=128*1024*1024&&current.admission.streams<=128&&current.admission.workers<=32,'native counter ceiling exceeded');return current},
  });
  const pressureChronology={...chronology,firstPausedAt,closedAt,slowCloseReason,healthyHandle:identity};
  await checkpoint('pressure-observed',{healthyFile,pressureChronology,high,slowClientClosed:true,pausedPackets:pausePackets});
  slow.dispose();
  await healthyRequest('session.send',{id:session,body:{data:"printf 'AFTER_%s\\n' 'SLOW_CLOSE'\n"}});
  const writeAcceptedAt=Date.now();
  // Reattach to observe snapshot/live output. Missing bytes alone do not
  // distinguish capture failure from an attach or transport failure.
  const replay=new NativeSocket();extras.push(replay);const bytes:number[]=[];const replayEvents:any[]=[];let replayFailure:string|undefined;let replayBytes=0;
  replay.onError=error=>{replayFailure=fixtureError(error)};
  replay.onMessage((value:any)=>{if(replayEvents.length<16)replayEvents.push({type:value.type,sessionId:value.sessionId,error:value.error?fixtureError(value.error):undefined})});
  replay.onBinaryMessage((_id,data)=>{replayBytes+=data.length;if(bytes.length+data.length>1024*1024){replayFailure='fixture replay byte bound exceeded';replay.dispose();return}bytes.push(...data)});
  const replayOpenStartedAt=Date.now();
  // Preserve the original critical sequence: write -> open -> attach. Read-only
  // diagnostics and checkpoint IPC begin only after attach has been sent.
  replay.adopt(await openTauriChannel(bridge));replay.send({type:'attach',id:session!,cols:80,rows:24});const attachSentAt=Date.now();end=attachSentAt+10000;
  const catalog=await healthyRequest('job.list');
  const jobs=Array.isArray(catalog)?catalog:Array.isArray(catalog?.jobs)?catalog.jobs:[];
  const job=jobs.find((value:any)=>value.sessionId===session);
  const sessionState:any={sessionId:session,postCloseWriteAccepted:true,writeAcceptedAt,replayOpenStartedAt,attachSentAt,jobCatalogued:!!job};
  if(job?.id){const inspected=await healthyRequest('job.inspect',{id:job.id});sessionState.job={id:job.id,state:inspected.state,sessionId:inspected.sessionId};}
  await checkpoint('capture-requested',{sessionState});
  while(!new TextDecoder().decode(new Uint8Array(bytes)).includes('AFTER_SLOW_CLOSE')&&!replayFailure&&Date.now()<end)await wait(25);
  const markerPresent=new TextDecoder().decode(new Uint8Array(bytes)).includes('AFTER_SLOW_CLOSE');
  await checkpoint('pressure-replay',{sessionState,replay:{events:replayEvents,error:replayFailure,bytes:replayBytes,firstBytesHex:bytes.slice(0,96).map(value=>value.toString(16).padStart(2,'0')).join(''),lastBytesHex:bytes.slice(-96).map(value=>value.toString(16).padStart(2,'0')).join(''),markerPresent,completedAt:Date.now()}});
  assert(markerPresent,'post-close capture marker missing on reattach'+(replayFailure?': '+replayFailure:''));replay.dispose();extras.pop();
  end=Date.now()+10000;while((await fixtureState()).broker.mainHandles!==baseline.broker.mainHandles&&Date.now()<end)await wait(25);
  const restored=await fixtureState();assert(restored.broker.mainHandles===baseline.broker.mainHandles,'slow channel admission leaked');
  // Reach only the independently testable per-window ceiling; do not claim
  // global/backend/worker ceilings from this one-domain runtime case.
  const count=32-restored.broker.mainHandles;
  for(let index=0;index<count;index++){const peer=new NativeSocket();peer.adopt(await openTauriChannel(bridge));extras.push(peer)}
  const saturated=await fixtureState();assert(saturated.broker.mainHandles===32,'per-window live stream ceiling not reached');
  let denied=false;try{const excess=await openTauriChannel(bridge);excess.close()}catch{denied=true}assert(denied,'excess per-window stream admitted');
  await healthyRequest('home.get');
  extras.pop()!.dispose();end=Date.now()+10000;while((await fixtureState()).broker.mainHandles!==31&&Date.now()<end)await wait(25);
  const replacement=new NativeSocket();replacement.adopt(await openTauriChannel(bridge));extras.push(replacement);
  return {healthyFile,nonConsumingClientClosed:true,healthyReplies:chronology.apiReplies,pressureChronology,captureReplay:true,perWindow32:true,excessRefused:true,replacementAdmitted:true,baseline,high,saturated,
   scope:'actual receipt-starved client; no JS-engine suspension, RSS claim or independent global/worker ceiling test'};
 }finally{slow.dispose();for(const peer of extras)peer.dispose();const end=Date.now()+10000;while((await fixtureState()).broker.mainHandles!==baseline.broker.mainHandles&&Date.now()<end)await wait(25);assert((await fixtureState()).broker.mainHandles===baseline.broker.mainHandles,'admission did not recover to baseline')}
}
