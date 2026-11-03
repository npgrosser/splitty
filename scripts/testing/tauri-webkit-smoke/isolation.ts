// Actual WebViews and production policy; only the two backend locations are fixtures.
import { NativeSocket } from '../../../frontend/src/lib/protocol/native_socket.ts';
import { openTauriChannel } from '../../../frontend/src/lib/protocol/tauri_channel.ts';
export const targets=['fixture-one','fixture-two'];
const mainName='splitty-unix-fixture-main';
const key='splitty-unix-fixture-storage';
const wait=(ms:number)=>new Promise(resolve=>setTimeout(resolve,ms));
const assert=(value:unknown,message:string)=>{if(!value)throw Error(message)};
export const fixtureState=(value?:object)=>(window as any).__TAURI_INTERNALS__.invoke('smoke_state',{value});
export function probeBridge(native:any){return {...native,open:(options:unknown,id:number)=>native.open(options,id+0x40000000),cancelOpen:(id:number)=>native.cancelOpen(id+0x40000000)}}
async function references(names:string[]){
 assert(window.opener===null,'isolated remote has opener');
 for(const name of names)for(const url of ['', '/index.html']){
  const reference=window.open(url,name);await wait(100);
  assert(!reference||reference.closed,'named/shared window reference escaped: '+name);
 }
 return {openerAbsent:true,namedReferencesAbsent:true};
}
export async function remoteIsolation(capsule:any,metadata:any){
 const target=metadata.target;assert(targets.includes(target),'remote target not fixed');
 window.name='splitty-unix-'+target;
 assert(localStorage.getItem(key)===null&&!document.cookie.includes('splitty_unix='),'remote inherited Main storage');
 localStorage.setItem(key,target);document.cookie='splitty_unix='+target+';Path=/;SameSite=Strict';
 const socket=new NativeSocket();let error:Error|undefined;socket.onError=value=>{error=value};
 try{
  const channel=await openTauriChannel(probeBridge(capsule.native));
  assert(channel.assignment.role==='remote'&&channel.assignment.target===target,'remote assignment changed');
  // Spoof only the frontend convenience role; native authority remains Remote.
  socket.adopt({...channel,assignment:{...channel.assignment,role:'main'}} as any);
  const build=await socket.request({operation:'build.get',args:{}});assert(build.status===200,'remote API unavailable');
  const hub=await socket.request({operation:'host.list',args:{}});assert(hub.status===403,'remote granted LocalHub operation');
  let localDenied=false,localDenial='';try{await capsule.invoke('open_local_editor',{editor:'vscode',cwd:'/tmp'})}catch(error){localDenial=String(error);assert(/not allowed|denied|rejected|not permitted/i.test(localDenial),'unexpected editor failure: '+localDenial);localDenied=true}
  assert(localDenied,'remote granted local editor command');
  const report={target,kind:'remote',instance:channel.assignment.instance,handle:channel.assignment.handle,localDenied,localDenial,hubDenied:true};
  await fixtureState(report);
  const deadline=Date.now()+30000;
  while(Date.now()<deadline){
   const state=await fixtureState();const sibling=(Object.values(state.reports)as any[]).find(value=>value.kind==='remote'&&value.target!==target);
   if(!sibling?.handle){await wait(25);continue}
   let copiedDenied=false;try{await capsule.native.write(sibling.handle,new Uint8Array([0]))}catch{copiedDenied=true}
   let targetDenied=false;try{await openTauriChannel(probeBridge(capsule.native),{target:sibling.target})}catch{targetDenied=true}
   assert(copiedDenied&&targetDenied,'remote selected sibling handle/target');
   assert(sibling.instance!==report.instance,'remote backend instances merged');
   assert(localStorage.getItem(key)===target,'sibling changed remote local storage');
   assert(!document.cookie.includes('splitty_unix=main')&&!document.cookie.includes('splitty_unix='+sibling.target),'sibling/Main cookie shared');
   if(error)throw error;
   await fixtureState({...report,copiedHandleCapsuleDenied:copiedDenied,targetDenied,localStoragePrivate:true,cookieOwnObserved:document.cookie.includes('splitty_unix='+target),complete:true,...await references([mainName,'splitty-unix-'+sibling.target])});return;
  }
  throw Error('sibling remote checks did not settle');
 }finally{socket.dispose()}
}
export async function mainIsolation(capsule:any,mainInstance:string){
 window.name=mainName;localStorage.setItem(key,'main');document.cookie='splitty_unix=main;Path=/;SameSite=Strict';
 for(const target of targets)await capsule.invoke('remote_open',{target});
 const deadline=Date.now()+30000;
 while(Date.now()<deadline){
  const state=await fixtureState();const reports=Object.values(state.reports)as any[];
  const failed=reports.find(value=>value.error);if(failed)throw Error(failed.error);
  const remotes=reports.filter(value=>value.kind==='remote'&&value.complete);
  if(remotes.length===2){
   assert(remotes.every(value=>value.instance!==mainInstance)&&remotes[0].instance!==remotes[1].instance,'Main/remote backend identities merged');
   assert(localStorage.getItem(key)==='main','remote changed Main storage');
   assert(state.externalEffects.length===0,'denied operation attempted an external opener');
   return {targets,instances:[mainInstance,...remotes.map(value=>value.instance)],reports:remotes,backendSubstitution:'two native-fixed real embedded services; no SSH/host filesystem claim'};
  }
  await wait(25);
 }
 throw Error('Main/two-remote isolation did not settle');
}
