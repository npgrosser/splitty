import { NativeConnection } from '../../../frontend/src/lib/protocol/native_connection.ts';
import { openTauriChannel } from '../../../frontend/src/lib/protocol/tauri_channel.ts';
import { SessionClient } from '../../../frontend/src/lib/terminal/session_client.ts';
import { AttachRegistry } from '../../../frontend/src/lib/terminal/attach_registry.ts';
const capsule=(window as any).__SPLITTY_TAURI__;
const report=(value:unknown)=>(window as any).__TAURI_INTERNALS__.invoke('windows_smoke_report',{value});
const wait=(ms:number)=>new Promise(resolve=>setTimeout(resolve,ms));
const assert=(value:unknown,message:string)=>{if(!value)throw Error(message)};
let stage='bootstrap';const timer=setTimeout(()=>{void report({error:'remote fixture deadline',stage})},120000);
void(async()=>{
 const metadata=await capsule.ready;
 if(metadata.role==='setup'){await capsule.invoke('remote_open',{target:'splitty-native-windows-fixture'});clearTimeout(timer);return}
 assert(metadata.role==='remote'&&metadata.target==='splitty-native-windows-fixture','fixed remote assignment missing');
 assert(window.opener===null,'remote has opener');assert(window.open('/index.html','main')===null,'named window gained opener');
 let deniedMain=false,deniedLauncher=false;
 try{await capsule.invoke('wsl_open_backend')}catch{deniedMain=true}
 try{await capsule.invoke('remote_open',{target:'other-host'})}catch{deniedLauncher=true}
 assert(deniedMain&&deniedLauncher,'remote granted unrelated shell authority');
 stage='native remote open';const first=await openTauriChannel(capsule.native);
 const socket=new NativeConnection(signal=>openTauriChannel(capsule.native,undefined,signal),first.assignment);
 const states:string[]=[];let lostInputRejected=false;let session:string|undefined;
 socket.onStateChange=state=>{states.push(state);capsule.native.state(state);if(state==='reconnecting'){
   void socket.request({operation:'session.send',args:{id:session,body:{data:'printf REPLAY_BUG > /home/splitty-fixture/replayed\n'}}}).catch(()=>{lostInputRejected=true})
 }};
 const events:any[]=[],bytes:number[]=[];socket.onMessage(value=>events.push(value));socket.onBinaryMessage((_id,data)=>bytes.push(...data));socket.adopt(first);
 const sessions=new SessionClient(socket,new AttachRegistry(socket));
 const request=async(operation:string,args={})=>{const reply=await socket.request({operation,args});assert(reply.status>=200&&reply.status<300,operation+': '+reply.body);return JSON.parse(reply.body)};
 await request('build.get');await request('home.get');
 stage='remote PTY';socket.send({type:'create',cols:80,rows:24});let end=Date.now()+15000;
 while(!session&&Date.now()<end){session=events.find(event=>event.type==='created')?.sessionId;await wait(50)}assert(session,'PTY create missing');
 await request('session.send',{id:session,body:{data:"printf 'WINDOWS_REMOTE_OK\\000\\377\\200\\n'; printf 'REMOTE_SHELL_%s\\n' \"$$\"\n"}});
 let pid:RegExpMatchArray|null=null,exact=false;const marker=[...new TextEncoder().encode('WINDOWS_REMOTE_OK'),0,255,128];
 while((!pid||!exact)&&Date.now()<end){pid=new TextDecoder().decode(new Uint8Array(bytes)).match(/REMOTE_SHELL_(\d+)/);exact=bytes.some((_b,index)=>marker.every((b,offset)=>bytes[index+offset]===b));await wait(50)}assert(pid&&exact,'remote binary PTY corrupted');
 stage='host daemon restart';await report({phase:'restart',terminalPid:Number(pid![1]),session});
 end=Date.now()+40000;while((!states.includes('reconnecting')||socket.connectionState!=='connected')&&Date.now()<end)await wait(100);
 assert(states.includes('reconnecting')&&socket.connectionState==='connected','remote did not reconnect after host restart');assert(lostInputRejected,'disconnected mutation did not reject');
 await request('home.get');await request('session.send',{id:session,body:{data:"printf 'REMOTE_AFTER_RESTART_%s\\n' \"$$\"\n"}});
 end=Date.now()+15000;while(!new TextDecoder().decode(new Uint8Array(bytes)).includes('REMOTE_AFTER_RESTART_'+pid![1])&&Date.now()<end)await wait(50);
 assert(new TextDecoder().decode(new Uint8Array(bytes)).includes('REMOTE_AFTER_RESTART_'+pid![1]),'original PTY did not survive host restart');
 stage='heartbeat';await wait(17000);assert(socket.connectionState==='connected','heartbeat lost connection');sessions.dispose();socket.dispose();clearTimeout(timer);
 await report({phase:'final',terminalPid:Number(pid![1]),session,terminalBinary:true,states,deniedMain,deniedLauncher,lostInputRejected});
})().catch(error=>{clearTimeout(timer);void report({error:String(error),stage,stack:error?.stack})});
