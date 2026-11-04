// Actual sandbox/contextBridge/MessagePort/WebContents identities, plus the
// compiled Node owner and real Rust backend. Unit mocks cannot prove this path.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { app, BrowserWindow, MessageChannelMain, ipcMain, protocol, session } = require('electron');
const { NativeBroker } = require('../compiled/native_broker');
const { NativeOwner } = require('../compiled/native_owner');
const { NativeProcessBudget } = require('../compiled/native_budget');
const { installAssets } = require('../compiled/asset_protocol');
protocol.registerSchemesAsPrivileged([{ scheme: 'splitty', privileges: { standard: true, secure: true, supportFetchAPI: true } }]);

// Values inserted into test scripts stay JSON literals, including when a
// fixture contains HTML delimiters or JavaScript line separators.
const scriptValue = value => JSON.stringify(value).replace(/[<>\u2028\u2029]/g,
  character => '\\u' + character.charCodeAt(0).toString(16).padStart(4, '0'));

const probe = `(async () => {
 const bridge = window.__SPLITTY_ELECTRON__.native;
 const assignment = await bridge.open();
 return await new Promise((resolve,reject) => {
  let buffered = new Uint8Array(), welcome = false;
  const timeout = setTimeout(()=>reject(Error('IPC API reply timed out')),5000);
  const encode = (kind,payload) => { const frame=new Uint8Array(5+payload.length); frame[0]=kind;new DataView(frame.buffer).setUint32(1,payload.length,false);frame.set(payload,5);return frame; };
  const json = value => new TextEncoder().encode(JSON.stringify(value));
  const off = bridge.listen(assignment.handle, chunk => {
   if(!(chunk instanceof Uint8Array)) throw Error('contextBridge lost binary bytes');
   const joined = new Uint8Array(buffered.length+chunk.length);joined.set(buffered);joined.set(chunk,buffered.length);buffered=joined;
   if(buffered.length<5)return;
   const size=new DataView(buffered.buffer).getUint32(1,false);
   if(buffered.length<5+size)return;
   const kind=buffered[0],payload=buffered.subarray(5,5+size);buffered=buffered.slice(5+size);
   if(kind===10)return;
   const value=JSON.parse(new TextDecoder('utf8',{fatal:true}).decode(payload.subarray(4,payload.length-16)));
   if(!welcome){if(kind!==1||value.type!=='mux.welcome')throw Error('welcome ordering changed');welcome=true;}
   const receipt=Array.from(payload.subarray(payload.length-16),b=>b.toString(16).padStart(2,'0')).join('');
   bridge.write(assignment.handle,encode(10,json({generation:assignment.generation,receipt})));
   if(kind===6){
    if(value.id!==1||value.status!==200)throw Error(JSON.stringify(value));
    const status=JSON.parse(value.body);
    clearTimeout(timeout);off();bridge.close(assignment.handle);
    resolve({assignment,welcome,status,node:typeof require,origin:location.origin});
   }
  }, reason=>{clearTimeout(timeout);reject(Error(reason.message||'IPC closed'));});
  const request=json({id:1,operation:'mux.status',args:{}}),payload=new Uint8Array(request.length+20);
  new DataView(payload.buffer).setUint32(0,assignment.generation,false);payload.set(request,4);
  payload.set(crypto.getRandomValues(new Uint8Array(16)),payload.length-16);
  bridge.write(assignment.handle,encode(5,payload)); // bounded pre-start write
  bridge.start(assignment.handle);
 });
})()`;

const fixture = fs.mkdtempSync(path.join(os.tmpdir(), 'splitty-native-ipc-'));
let owner, stoppedRenderer, finishing = false;
const windows = [];
const timeout = setTimeout(() => { console.error('native IPC smoke timed out'); void finish(1); }, 90_000);
async function finish(code) {
  if (finishing) return; finishing = true;
  clearTimeout(timeout);
  const escalation = setTimeout(() => { console.error('native IPC cleanup exceeded its deadline'); owner?.helper.child.kill('SIGKILL'); }, 5000);
  const giveUp = setTimeout(() => { console.error('native IPC cleanup could not observe child exit; fixture retained'); app.exit(1); }, 10_000);
  if (stoppedRenderer) { try { process.kill(stoppedRenderer, 'SIGCONT'); } catch {} stoppedRenderer = undefined; }
  windows.forEach(window => { if (!window.isDestroyed()) window.destroy(); });
  if (owner) { owner.close(); await owner.helper.exited; }
  clearTimeout(escalation); clearTimeout(giveUp);
  fs.rmSync(fixture, { recursive: true, force: true });
  app.exit(code);
}
app.whenReady().then(async () => {
  const env = { ...process.env, HOME: fixture, XDG_CONFIG_HOME: path.join(fixture, 'config'),
    XDG_DATA_HOME: path.join(fixture, 'data'), XDG_RUNTIME_DIR: path.join(fixture, 'run') };
  for (const name of Object.keys(env)) if (name.startsWith('SPLITTY_')) delete env[name];
  fs.mkdirSync(env.XDG_RUNTIME_DIR, { mode: 0o700 });
  const budget = new NativeProcessBudget();
  owner = new NativeOwner(path.resolve(process.argv[2] || '../target/debug/splitty'), env, { closed() {} }, budget);
  await owner.ready;
  const broker = new NativeBroker(() => new MessageChannelMain());
  const captures = [];
  ipcMain.on('native:document', event => broker.document(event));
  ipcMain.on('native:open', (event, intent) => {
    captures.push({ event: { sender: event.sender, senderFrame: event.senderFrame }, intent });
    broker.open(event, intent);
  });
  ipcMain.on('native:cancel-open', (event, intent) => broker.cancel(event, intent));
  ipcMain.handle('window:is-maximized', () => false); // Harmless UI metadata for the hidden smoke window.
  // Delay one real navigation response while retaining the production handler
  // and its manifest checks. The old document remains able to send IPC here.
  const assets = session.defaultSession.protocol;
  const handle = assets.handle.bind(assets); let releaseNavigation;
  assets.handle = (scheme, handler) => handle(scheme, async request => {
    if (request.url.includes('hold=1')) await new Promise(resolve => { releaseNavigation = resolve; });
    return handler(request);
  });
  installAssets(session.defaultSession, path.resolve(__dirname, '../compiled/ui')); assets.handle = handle;
  // This harness controls its own protocol clients; the separate smoke:app
  // runs the production application module and terminal lifecycle.
  session.defaultSession.webRequest.onBeforeRequest((details, callback) => callback({ cancel: details.resourceType === "script" }));
  const create = () => {
    const window = new BrowserWindow({ show: false, webPreferences: {
      preload: path.resolve(__dirname, '../compiled/preload.js'), sandbox: true, contextIsolation: true, nodeIntegration: false,
    } }); windows.push(window); return window;
  };
  const main = create();
  let starts = 0;
  broker.grant(main.webContents, { role: 'main', open: (events, target, update) => { starts++; return owner.connect(events, target, update); } });
  await main.loadURL('splitty://app/index.html?host=forged&desktop=1');
  const cancelledStarts = starts;
  const cancelled = await main.webContents.executeJavaScript(`(async () => {
    const bridge = window.__SPLITTY_ELECTRON__.native;
    const request = 'cancel-before-dispatch';
    const result = bridge.open({}, request).then(()=>false,()=>true);
    bridge.cancelOpen(request); return await result;
  })()`);
  assert.equal(cancelled, true); assert.equal(starts, cancelledStarts);
  const failureMessage = await main.webContents.executeJavaScript(`window.__SPLITTY_ELECTRON__.native.open({target:'invalid target'}).then(()=>'',error=>error.message)`);
  assert.ok(failureMessage.startsWith('SplittyNativeFailure:'));
  assert.equal(JSON.parse(failureMessage.slice('SplittyNativeFailure:'.length)).retryable, false);
  const first = await main.webContents.executeJavaScript(probe);
  assert.equal(first.assignment.role, 'main'); assert.equal(first.assignment.target, undefined);
  assert.equal(first.status.instance, first.assignment.instance); assert.equal(first.status.listenAddr, null);
  assert.equal(first.node, 'undefined'); assert.equal(first.origin, 'splitty://app');
  await main.webContents.executeJavaScript('window.__SPLITTY_ELECTRON__.onRemoteProgress(()=>{window.__SPLITTY_ELECTRON__.native.open().catch(()=>{});}); true');
  const oldFrame = main.webContents.mainFrame;
  const navigationStarted = new Promise(resolve => main.webContents.once('did-start-navigation', resolve));
  const navigating = main.loadURL('splitty://app/index.html?reload=1&hold=1');
  await navigationStarted;
  while (!releaseNavigation) await new Promise(resolve => setImmediate(resolve));
  const before = captures.length, oldStarts = starts;
  // executeJavaScript waits for navigation completion; an already-installed
  // old-document callback instead emits real IPC while the response is held.
  oldFrame.send('remote:progress', { phase: 'navigation-probe' });
  while (captures.length === before) await new Promise(resolve => setImmediate(resolve));
  const stale = captures.at(-1);
  assert.equal(starts, oldStarts, 'old document cannot open while navigation is pending');
  releaseNavigation(); await navigating;
  const frameReused = oldFrame === main.webContents.mainFrame;
  broker.open(stale.event, stale.intent); // the actual old IPC, delivered after commit
  assert.equal(starts, oldStarts, 'queued old challenge is invalid after navigation commits');
  const next = await main.webContents.executeJavaScript(probe);
  assert.notEqual(next.assignment.document, first.assignment.document); assert.notEqual(next.assignment.handle, first.assignment.handle);
  assert.equal(next.assignment.instance, first.assignment.instance);
  const beforeAbort = await main.webContents.executeJavaScript(`(async () => {
    const bridge = window.__SPLITTY_ELECTRON__.native, assignment = await bridge.open();
    bridge.listen(assignment.handle, () => {}, reason => { window.abortReason = reason; });
    bridge.start(assignment.handle); return assignment;
  })()`);
  releaseNavigation = undefined;
  const abortStarted = new Promise(resolve => main.webContents.once('did-start-navigation', resolve));
  const aborted = main.loadURL('splitty://app/index.html?abort=1&hold=1').then(() => false, () => true);
  await abortStarted;
  while (!releaseNavigation) await new Promise(resolve => setImmediate(resolve));
  const stopped = new Promise(resolve => main.webContents.once('did-stop-loading', resolve));
  main.webContents.stop(); await stopped; releaseNavigation();
  assert.equal(await aborted, true);
  const abortReason = await main.webContents.executeJavaScript('window.abortReason');
  assert.equal(abortReason.code, 'assignment'); assert.equal(abortReason.retryable, false);
  const recovered = await main.webContents.executeJavaScript(probe);
  assert.notEqual(recovered.assignment.document, beforeAbort.document);
  assert.equal(recovered.assignment.instance, first.assignment.instance);
  const staleStarts = starts;
  broker.open(stale.event, stale.intent);
  assert.equal(starts, staleStarts, 'navigation abort cannot revive a queued challenge from the old lifetime');
  const unassigned = create(); await unassigned.loadURL('splitty://app/index.html');
  const denied = await unassigned.webContents.executeJavaScript(`window.__SPLITTY_ELECTRON__.native.open().then(()=>false,()=>true)`);
  assert.equal(denied, true, 'a packaged but unassigned document has no backend authority');
  // A separate renderer process actually stops consuming MessagePorts. The
  // healthy client transfers files while the native service captures output.
  if (process.platform !== 'linux') throw Error('Frozen-renderer OS smoke requires Linux');
  const clientProbe = fs.readFileSync(path.join(__dirname, 'native-ipc-probe.js'), 'utf8');
  await main.webContents.executeJavaScript(clientProbe);
  const evaluate = expression => main.webContents.executeJavaScript(expression);
  const hostileValue = {text: '\"\\\n</script><script>globalThis.injected=true</script>\u2028\u2029'};
  assert.deepEqual(await evaluate(`(${scriptValue(hostileValue)})`), hostileValue);
  assert.equal(await evaluate('globalThis.injected'), undefined);
  const until = async (check, description) => {
    const deadline = Date.now() + 40_000;
    while (Date.now() < deadline) { if (await check()) return; await new Promise(resolve => setTimeout(resolve, 25)); }
    throw Error(`Timed out: ${description}`);
  };
  const isolated = session.fromPartition('frozen-native-ipc');
  installAssets(isolated, path.resolve(__dirname, '../compiled/ui'));
  isolated.webRequest.onBeforeRequest((details, callback) => callback({cancel: details.resourceType === 'script'}));
  const frozen = new BrowserWindow({show: false, webPreferences: {partition: 'frozen-native-ipc',
    preload: path.resolve(__dirname, '../compiled/preload.js'), sandbox: true, contextIsolation: true, nodeIntegration: false}});
  windows.push(frozen);
  let frozenConnection, frozenReason;
  broker.grant(frozen.webContents, {role: 'main', open: events => {
    frozenConnection = owner.connect({...events, closed(reason) { frozenReason = reason; events.closed(reason); }});
    return frozenConnection;
  }});
  await frozen.loadURL('splitty://app/index.html');
  const frozenAssignment = await frozen.webContents.executeJavaScript(clientProbe);
  assert.equal(frozenAssignment.instance, first.assignment.instance);
  const frozenSession = await frozen.webContents.executeJavaScript(`(async () => {
    const created = smokeClient.next('created');
    smokeClient.send({type:'create',shell:'/bin/sh',cols:80,rows:24}); return (await created).sessionId;
  })()`);
  const renderer = frozen.webContents.getOSProcessId();
  assert.notEqual(renderer, main.webContents.getOSProcessId(), 'the frozen document has its own renderer');
  stoppedRenderer = renderer; process.kill(renderer, 'SIGSTOP');
  let maximumData = 0, maximumControl = 0, maximumConnection = 0;
  const sample = () => {
    maximumData = Math.max(maximumData, budget.counts.data);
    maximumControl = Math.max(maximumControl, budget.counts.controls);
    const count = frozenConnection.pipe.budget.counts;
    maximumConnection = Math.max(maximumConnection, count.data + count.controls);
  };
  const sampling = setInterval(sample, 5);
  try {
    const completed = path.join(fixture, 'capture-completed');
    const command = `head -c 67108864 /dev/zero; printf '\\nCAPTURE_COMPLETED\\n'; touch '${completed}'\r`;
    const sent = await evaluate(`smokeClient.request('session.send', ${scriptValue({id: frozenSession, body: {data: command}})})`);
    assert.equal(sent.status, 200, sent.body);
    // A multi-chunk arbitrary binary file crosses the real preload/port in
    // both directions while the other renderer remains suspended.
    const destination = path.join(fixture, 'uploads'); fs.mkdirSync(destination);
    const source = Buffer.alloc(1024 * 1024 + 257);
    for (let i = 0; i < source.length; i++) source[i] = i % 256;
    let saved;
    for (let offset = 0; offset < source.length; offset += 512 * 1024) {
      const bytes = source.subarray(offset, offset + 512 * 1024), last = offset + bytes.length === source.length;
      const upload = {type:'upload',requestId:'file-roundtrip',offset,data:bytes.toString('base64'),
        ...(offset === 0 ? {name:'all-bytes.bin',size:source.length,dir:destination} : {})};
      const answer = await evaluate(`(async () => { const answer = smokeClient.next('${last ? 'upload_saved' : 'upload_progress'}', 'requestId', 'file-roundtrip'); smokeClient.send(${scriptValue(upload)}); return await answer; })()`);
      if (last) saved = answer.path; else assert.equal(answer.received, offset + bytes.length);
    }
    assert.deepEqual(fs.readFileSync(saved), source);
    const ownSession = await evaluate(`(async () => { smokeClient.send({type:'file.register'}); const created=smokeClient.next('created'); smokeClient.send({type:'create',shell:'/bin/sh',cols:80,rows:24}); const id=(await created).sessionId; const attached=smokeClient.next('attached'); smokeClient.send({type:'attach',id,cols:80,rows:24}); await attached; return id; })()`);
    await evaluate(`smokeClient.send({type:'input',id:${scriptValue(ownSession)},data:${scriptValue("printf '\\000\\377\\376\\001'\r")}}); true`);
    await until(() => evaluate('smokeClient.binary.tail.some((byte,i,bytes) => byte === 0 && bytes[i+1] === 255 && bytes[i+2] === 254 && bytes[i+3] === 1)'), 'byte-exact terminal IPC including NUL/non-UTF-8');
    const partial = {type:'upload',requestId:'explicit-cancel',offset:0,name:'cancelled.bin',size:100,dir:destination,data:'AP8='};
    await evaluate(`(async () => { const progress=smokeClient.next('upload_progress','requestId','explicit-cancel'); smokeClient.send(${scriptValue(partial)}); await progress; smokeClient.send({type:'upload_cancel',requestId:'explicit-cancel'}); const attached=smokeClient.next('attached'); smokeClient.send({type:'attach',id:${scriptValue(ownSession)},cols:80,rows:24}); await attached; })()`);
    assert.deepEqual(fs.readdirSync(destination), ['all-bytes.bin'], 'explicit cancel removes the partial file');
    await evaluate(`window.offered = smokeClient.request('file.offer', ${scriptValue({body: {sessionId: ownSession, path: saved}})}); true`);
    const offer = await evaluate("smokeClient.next('file.offer')");
    const received = [];
    let offset = 0;
    await evaluate(`smokeClient.send({type:'file.accept',offerId:${scriptValue(offer.offerId)}}); true`);
    for (;;) {
      const chunk = await evaluate(`(async () => { const chunk=smokeClient.next('file.chunk'); smokeClient.send({type:'file.read',offerId:${scriptValue(offer.offerId)},offset:${offset}}); return await chunk; })()`);
      assert.equal(chunk.offset, offset); const bytes = Buffer.from(chunk.data, 'base64'); received.push(bytes); offset += bytes.length;
      if (chunk.eof) break;
    }
    assert.deepEqual(Buffer.concat(received), source);
    assert.equal((await evaluate('window.offered')).status, 200);
    await until(() => frozenReason !== undefined, 'slow native client disconnect');
    await frozenConnection.released;
    assert.equal(owner.helper.child.exitCode, null, 'one frozen client cannot close the embedded owner');
    await until(() => fs.existsSync(completed), 'PTY capture continues after the consumer is disconnected');
    await evaluate(`(async () => { const attached=smokeClient.next('attached','sessionId',${scriptValue(frozenSession)}); smokeClient.send({type:'attach',id:${scriptValue(frozenSession)},cols:80,rows:24}); await attached; })()`);
    assert.ok(await evaluate("String.fromCharCode(...smokeClient.binary.tail).includes('CAPTURE_COMPLETED')"), 'reattach replays output captured after the slow consumer was gone');
    assert.equal((await evaluate("smokeClient.request('mux.status')")).status, 200);
    assert.ok(maximumData <= 120 * 1024 * 1024 && maximumControl <= 8 * 1024 * 1024);
    assert.ok(maximumConnection <= 32 * 1024 * 1024);
    assert.ok(maximumConnection > 64 * 1024, 'the real delivery path was exercised');
    process.kill(renderer, 'SIGCONT'); stoppedRenderer = undefined;
    await until(() => frozen.webContents.executeJavaScript('!!smokeClient.closed'), 'resumed renderer sees connection failure');
    const lost = await frozen.webContents.executeJavaScript('smokeClient.closed');
    assert.ok(lost.message);
    console.log(JSON.stringify({frozenConsumerDisconnected:true,healthyFileRoundtrip:source.length,
      captureContinues:true,maximumData,maximumControl,maximumConnection,closeReason:lost.message}));
  } finally { clearInterval(sampling); if (stoppedRenderer) { process.kill(stoppedRenderer, 'SIGCONT'); stoppedRenderer = undefined; } }
  // Closing a native document client must clean partial uploads without an
  // application cancellation message and return its real stream allocation.
  await evaluate('window.healthyClient = smokeClient; true');
  await main.webContents.executeJavaScript(clientProbe);
  const abandoned = path.join(fixture, 'abandoned'); fs.mkdirSync(abandoned);
  await evaluate(`(async () => { const progress=smokeClient.next('upload_progress','requestId','abandoned'); smokeClient.send(${scriptValue({type:'upload',requestId:'abandoned',offset:0,name:'abandoned.bin',size:100,dir:abandoned,data:'AP8='})}); await progress; smokeClient.close(); window.smokeClient=window.healthyClient; })()`);
  await until(() => owner.counts.streams === 1, 'closed upload stream cleanup');
  assert.deepEqual(fs.readdirSync(abandoned), [], 'disconnect removes the unpublished upload');
  const admission = await evaluate(`(async () => {
    const bridge=window.__SPLITTY_ELECTRON__.native, handles=[]; let refused=0;
    for(let i=0;i<40;i++) { try { handles.push((await bridge.open()).handle); } catch { refused++; } }
    window.idleHandles=handles; return {opened:handles.length,refused};
  })()`);
  assert.deepEqual(admission, {opened:31,refused:9});
  assert.deepEqual(broker.counts, {connections:32,opening:0});
  assert.equal(owner.counts.streams, 32); assert.equal(budget.counts.workers, 1);
  assert.equal((await evaluate("smokeClient.request('mux.status')")).status, 200, 'existing client remains live at the handle ceiling');
  await evaluate('idleHandles.forEach(handle => window.__SPLITTY_ELECTRON__.native.close(handle)); true');
  await until(() => owner.counts.streams === 1 && broker.counts.connections === 1, 'idle handle cleanup');
  console.log(JSON.stringify({explicitUploadCancel:true,disconnectedUploadCleanup:true,byteExactTerminal:true,
    actualWindowHandleCeiling:32,excessOpensRejected:admission.refused,healthyAtCeiling:true}));
  owner.close(); await owner.helper.exited; await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(broker.counts, { connections: 0, opening: 0 });
  assert.deepEqual(budget.counts, { data: 0, controls: 0, workers: 0 });
  console.log(JSON.stringify({ sandboxedPreload: true, privateMessagePort: true, nativeApi: true, reloadRevocation: true,
    pendingNavigationDenied: true, queuedOldDocumentDenied: true, abortedNavigationFreshChallenge: true, frameReused,
    unchangedService: true, queryHasNoAuthority: true, unassignedDenied: true, counters: budget.counts }));
  await finish(0);
}).catch(async error => { console.error(error); await finish(1); });
