const path = require('node:path');
const { app, protocol, BrowserWindow, session } = require('electron');
const { installAssets } = require('../compiled/asset_protocol.js');
protocol.registerSchemesAsPrivileged([{scheme:'splitty',privileges:{standard:true,secure:true,supportFetchAPI:true}}]);
const timeout = setTimeout(() => { console.error('asset smoke timed out'); app.exit(1); }, 20000);
app.whenReady().then(async () => {
 installAssets(session.defaultSession, path.resolve(__dirname, '../compiled/ui'));
 const window = new BrowserWindow({show:false,webPreferences:{sandbox:true,contextIsolation:true,nodeIntegration:false}});
 await window.loadURL('splitty://app/index.html');
 const result = await window.webContents.executeJavaScript(`(async () => {
  const font = new FontFace('SmokeFont', 'url("splitty://app/JetBrainsMonoNerdFontMono-Regular.ttf")');
  await font.load();
  await WebAssembly.compile(new Uint8Array([0,97,115,109,1,0,0,0]));
  const blocked = directive => new Promise(resolve => {
   const timer = setTimeout(() => resolve(false), 3000);
   const listener = event => { if(event.violatedDirective.startsWith(directive)) {
    clearTimeout(timer); document.removeEventListener('securitypolicyviolation', listener); resolve(true);
   }};
   document.addEventListener('securitypolicyviolation', listener);
  });
  const scriptBlocked = blocked('script-src');
  const script = document.createElement('script'); script.src = 'http://127.0.0.1:1/blocked.js'; document.head.append(script);
  const connectBlocked = blocked('connect-src');
  fetch('http://127.0.0.1:1/blocked').catch(() => {});
  return {scriptBlocked:await scriptBlocked,connectBlocked:await connectBlocked,styles:document.styleSheets.length,font:font.status,missing:(await fetch('splitty://app/private')).status,
   node:typeof require,origin:location.origin};
 })()`);
 if (!result.scriptBlocked || !result.connectBlocked || !result.styles || result.font !== 'loaded' || result.missing !== 404 || result.node !== 'undefined' || result.origin !== 'splitty://app') throw Error(JSON.stringify(result));
 console.log(JSON.stringify(result)); clearTimeout(timeout); window.destroy(); app.quit();
}).catch(error => { console.error(error); clearTimeout(timeout); app.exit(1); });
