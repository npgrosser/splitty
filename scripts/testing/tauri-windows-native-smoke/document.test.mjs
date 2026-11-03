import test from 'node:test';
import assert from 'node:assert/strict';
import { isWindowsFixtureDocument, probeBridge } from './document.mjs';
function view(href, child=false) { const value={location:{href}};value.top=child?{}:value;return value; }
test('only actual top Windows packaged workspace owns UI probe',()=>{
 for(const path of ['/','/index.html'])assert.equal(isWindowsFixtureDocument(view('http://tauri.localhost'+path)),true);
 for(const url of ['about:blank','http://tauri.localhost/popout.html','http://other.localhost/index.html','http://tauri.localhost:123/index.html','https://tauri.localhost/index.html'])assert.equal(isWindowsFixtureDocument(view(url)),false);
 assert.equal(isWindowsFixtureDocument(view('http://tauri.localhost/index.html',true)),false);
 assert.equal(isWindowsFixtureDocument(view('http://tauri.localhost/setup.html')),false);
 assert.equal(isWindowsFixtureDocument(view('http://tauri.localhost/setup.html'),true),true);
});
test('independently bundled adapter cannot collide with App native open IDs',async()=>{
 const calls=[];const native={};for(const method of ['state','open','cancelOpen','listen','start','write','consumed','close'])native[method]=function(...args){assert.equal(this,native);calls.push([method,...args]);return 'result';};
 const bridge=probeBridge(native);assert.equal(await bridge.open(undefined,1),'result');bridge.cancelOpen(1);bridge.close('handle');
 assert.deepEqual(calls,[['open',undefined,0x40000001],['cancelOpen',0x40000001],['close','handle']]);
 assert.throws(()=>bridge.open(undefined,0x40000000));
});

test('fixture blocks modern and legacy clipboard in main and popout before UI', async()=>{
 const {readFile}=await import('node:fs/promises');const vm=await import('node:vm');
 const helpers=(await readFile(new URL('./document.mjs',import.meta.url),'utf8')).replaceAll('export function','function');
 const observer=await readFile(new URL('./full-ui-init.js',import.meta.url),'utf8');
 for(const path of ['/index.html','/popout.html']) {
  let originalClipboard=0,legacy=0;const parent=[];
  const clipboard={};for(const name of ['read','readText','write','writeText'])clipboard[name]=()=>{originalClipboard++;};
  const window=view('http://tauri.localhost'+path);window.opener={__SPLITTY_WINDOWS_UI_PROBE__:{clipboard:parent}};
  window.fetch=()=>Promise.resolve({});window.WebSocket=function(){};
  const document={execCommand:()=>{legacy++;return true;},querySelector:()=>null};
  vm.runInNewContext('(function(){'+helpers+'\n'+observer+'})();',{window,document,navigator:{clipboard},URL,location:{origin:'http://tauri.localhost'},MutationObserver:class{observe(){}}});
  await assert.rejects(clipboard.readText());await assert.rejects(clipboard.writeText('never-global'));
  for(const command of ['copy','CUT','paste'])assert.equal(document.execCommand(command),false);
  assert.equal(originalClipboard,0);assert.equal(legacy,0);assert.equal(document.execCommand('selectAll'),true);assert.equal(legacy,1);
  if(path==='/popout.html'){assert.equal(window.__SPLITTY_WINDOWS_UI_PROBE__,undefined);assert.equal(parent.length,5);}
 }
});
