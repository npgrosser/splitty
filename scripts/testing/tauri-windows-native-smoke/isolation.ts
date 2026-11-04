import { fixtureError } from './error.mjs';
import { NativeSocket } from '../../../frontend/src/lib/protocol/native_socket.ts';
import { openTauriChannel } from '../../../frontend/src/lib/protocol/tauri_channel.ts';
import { probeBridge } from './document.mjs';
export const mainName='splitty-fixture-isolation-main';
const key='splitty-fixture-isolation-storage';
const wait=(ms:number)=>new Promise(resolve=>setTimeout(resolve,ms));
const assert=(value:unknown,message:string)=>{if(!value)throw Error(message)};
export const fixtureState=(value?:object)=>(window as any).__TAURI_INTERNALS__.invoke('windows_fixture_state',{value});
async function references(names:string[]) {
 assert(window.opener===null,'isolated window has opener');
 for(const name of names) for(const url of ['', '/index.html']) {
  const ref=window.open(url,name);await wait(100);
  assert(!ref||ref.closed,'named window reference escaped isolation: '+name);
 }
 return {openerAbsent:true,namedReferencesAbsent:true};
}
export async function setupIsolation(capsule:any,aliases:string[]) {
 window.name=mainName;localStorage.setItem(key,'main');document.cookie='splitty_fixture=main;Path=/;SameSite=Strict';
 await fixtureState({kind:'setup',documentReady:true});
 for(const target of aliases){try{await capsule.invoke('remote_open',{target})}catch(cause){throw {message:'remote_open failed for '+target,cause}}}
 const end=Date.now()+60000;
 while(Date.now()<end){const state=await fixtureState();const reports=Object.values(state.reports) as any[];const failure=reports.find(value=>value.error);if(failure)throw Error(failure.error);if(reports.filter(value=>value.kind==='remote'&&value.setupChecked).length===2)return;await wait(50)}
 throw Error('remote Setup isolation checks did not settle');
}
export async function remoteIsolation(capsule:any,metadata:any,aliases:string[]) {
 const own=metadata.target;assert(aliases.includes(own),'unexpected remote fixture target');
 window.name='splitty-fixture-'+own;
 assert(localStorage.getItem(key)===null&&!document.cookie.includes('splitty_fixture='),'remote inherited storage/cookie');
 localStorage.setItem(key,own);document.cookie='splitty_fixture='+own+';Path=/;SameSite=Strict';
 const names=[mainName,'splitty-fixture-'+aliases.find(value=>value!==own)];
 const channel=await openTauriChannel(probeBridge(capsule.native));
 assert(channel.assignment.role==='remote'&&channel.assignment.target===own,'native remote assignment changed');
 const socket=new NativeSocket();socket.adopt(channel);
 const build=await socket.request({operation:'build.get',args:{}});assert(build.status===200,'remote native API unavailable');
 let localDenied=false;try{await capsule.invoke('wsl_open_backend')}catch{localDenied=true}assert(localDenied,'remote granted main command');
 const report:any={kind:'remote',target:own,publicHandle:channel.assignment.handle,localDenied,setupChecked:true,...await references(names)};
 await fixtureState(report);
 const end=Date.now()+60000;
 while(Date.now()<end){const state=await fixtureState();if(state.phase==='main'){
  const sibling=(Object.values(state.reports) as any[]).find(value=>value.kind==='remote'&&value.target!==own);
  if(!sibling?.publicHandle){await wait(50);continue}
  let copiedDenied=false;try{await capsule.native.write(sibling.publicHandle,new Uint8Array([0]))}catch{copiedDenied=true}
  assert(copiedDenied,'sibling public handle usable');
  let targetDenied=false;try{await openTauriChannel(probeBridge(capsule.native),{target:aliases.find(value=>value!==own)})}catch{targetDenied=true}
  assert(targetDenied,'remote selected sibling target');
  assert(localStorage.getItem(key)===own&&document.cookie.includes('splitty_fixture='+own),'remote storage changed across contexts');
  await fixtureState({...report,mainChecked:true,copiedDenied,targetDenied,storagePrivate:true,...await references(names)});return;
 }await wait(50)}throw Error('Main isolation transition did not settle');
}
export async function mainIsolation(){
 window.name=mainName;assert(localStorage.getItem(key)==='main','Setup/Main own storage continuity missing');
 const end=Date.now()+60000;
 while(Date.now()<end){const state=await fixtureState();const reports=Object.values(state.reports) as any[];const failure=reports.find(value=>value.error);if(failure)throw Error(failure.error);
  if(reports.filter(value=>value.kind==='remote'&&value.mainChecked).length===2){assert(state.oldSetupDenied,'old Setup proof revived after Main transition');assert(state.externalOpens.length===0,'policy attempted system opener');
   const end=Date.now()+15000;while(!document.querySelector('#app textarea')&&Date.now()<end)await wait(25);assert(document.querySelector('#app textarea'),'healthy production App not mounted');
   return state}
  await wait(50)
 }throw Error('sibling remote Main checks did not settle');
}
