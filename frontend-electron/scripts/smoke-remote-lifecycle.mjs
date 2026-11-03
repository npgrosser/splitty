// Production remote-window orchestration with real child pipes and delayed
// exit. Electron window/asset operations are stubbed; smoke:app covers actual
// Electron documents. Process tests are needed to observe reaper-held quotas.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { EventEmitter } from 'node:events';
const worktree = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const original = process.cwd();
const require = createRequire(path.join(worktree, 'frontend-electron/package.json'));
const ts = require('typescript'), cache = new Map(), helpers = [], windows = [];
let overrides = {};
class Contents extends EventEmitter {
  mainFrame = { parent: null, url: 'splitty://app/index.html', origin: 'splitty://app', send: (...args) => this.messages.push(args), postMessage: (...args) => this.messages.push(args) };
  destroyed = false; messages = [];
  isDestroyed() { return this.destroyed; }
  isLoadingMainFrame() { return false; }
  send(...args) { this.messages.push(args); }
}
class Window extends EventEmitter {
  webContents = new Contents(); destroyed = false;
  constructor() { super(); windows.push(this); }
  isDestroyed() { return this.destroyed; }
  isMinimized() { return false; }
  focus() {} restore() {} setTitle() {}
  loadURL() { return Promise.resolve(); }
  destroy() {
    if (this.destroyed) return;
    this.destroyed = true; this.webContents.destroyed = true;
    this.webContents.emit('destroyed'); this.emit('closed');
  }
}
class Port extends EventEmitter { start() {} postMessage() {} close() { if (this.closed) return; this.closed = true; this.emit('close'); } }
const electron = {
  app: { isPackaged: false, requestSingleInstanceLock: () => false, quit() {} },
  protocol: { registerSchemesAsPrivileged() {} },
  ipcMain: { on() {}, handle() {} }, BrowserWindow: Window,
  MessageChannelMain: class { port1 = new Port(); port2 = new Port(); }, Menu: {}, shell: {}, webContents: {},
  session: { fromPartition: () => ({ setPermissionRequestHandler() {}, setPermissionCheckHandler() {}, on() {} }) },
};
function load(file) {
  file = path.resolve(file); if (!path.extname(file)) file += '.ts';
  if (cache.has(file)) return cache.get(file).exports;
  const module = { exports: {} }; cache.set(file, module);
  let source = fs.readFileSync(file, 'utf8');
  if (file.endsWith('/main.ts')) source += '\nexport const review = {openRemote, cancelRemote, pendingOpens, remoteWindows, remoteHelpers, broker, policies};';
  const output = ts.transpileModule(source, { compilerOptions: {
    target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS, esModuleInterop: true,
  } }).outputText;
  new Function('require', 'module', 'exports', '__dirname', output)(name => {
    if (name === 'electron') return electron;
    if (file.endsWith('/main.ts') && name in overrides) return overrides[name];
    return name.startsWith('.') ? load(path.resolve(path.dirname(file), name)) : require(name);
  }, module, module.exports, path.dirname(file));
  return module.exports;
}
const source = path.join(worktree, 'frontend-electron/src');
const { NativeProcessBudget } = load(path.join(source, 'native_budget.ts'));
const { startNativeHelper } = load(path.join(source, 'native_helper.ts'));
const budget = new NativeProcessBudget();
const fixture = fs.mkdtempSync(path.join(os.tmpdir(), 'splitty-main-open-review-'));
fs.writeFileSync(path.join(fixture, 'desktop-backend'), `
process.stdout.on('error', () => {}); process.stdin.resume();
function frame(kind, value) { const body = Buffer.from(JSON.stringify(value)), head = Buffer.alloc(5); head[0] = kind; head.writeUInt32BE(body.length, 1); process.stdout.write(Buffer.concat([head, body])); }
process.stdout.write(Buffer.from([83,80,76,73,84,84,89,1])); frame(3, {transportVersion:1});
setTimeout(() => frame(4, {generation:1,instance:'a'.repeat(32),muxProtocol:1,purpose:{kind:'host',target:process.argv[process.argv.indexOf('--target')+1]},limits:{frameBytes:16*1024*1024,apiBytes:1024*1024,pendingRequests:64,connectionBytes:32*1024*1024}}), Number(process.env.REVIEW_DELAY));
// Deliberately retain the child after stdin EOF, exercising the real reaper.
setInterval(() => {}, 1000);
`);
overrides = {
  './asset_protocol': { installAssets() {} },
  './native_helper': { startNativeHelper(_binary, _env, events, target) {
    const helper = startNativeHelper(process.execPath, { ...process.env, REVIEW_DELAY: target === 'late' ? '200' : '10000' }, events, target, budget);
    helpers.push({ helper, events }); return helper;
  } },
};
const { review } = load(path.join(source, 'main.ts'));
const event = sender => ({ sender, senderFrame: sender.mainFrame });
function caller() {
  const contents = new Contents();
  review.policies.set(contents, { origin: 'splitty://app', role: 'main' });
  review.broker.grant(contents, { role: 'main', open() { throw Error('unused direct grant'); } });
  return contents;
}
const tick = async () => { for (let i = 0; i < 6; i++) await Promise.resolve(); };
process.chdir(fixture);
try {
  const a = caller();
  const first = Array.from({ length: 4 }, (_, i) => review.openRemote('vm' + i, event(a)));
  for (let i = 0; i < 4; i++) review.cancelRemote('vm' + i, a);
  await tick();
  assert.equal(review.pendingOpens.size, 4);
  assert.equal(budget.counts.workers, 4);
  const refused = await review.openRemote('excess', event(a));
  assert.equal(refused.ok, false); assert.equal(helpers.length, 4);
  console.log('PASS pending limit remains held after cancellation while four real helpers are alive');
  await Promise.all(first);
  assert.equal(review.pendingOpens.size, 0); assert.equal(budget.counts.workers, 0);

  const old = caller(), late = review.openRemote('late', event(old));
  const before = old.messages.length;
  old.emit('did-start-navigation', { isMainFrame: true, isSameDocument: false, url: 'splitty://app/setup.html' });
  assert.equal(review.pendingOpens.get('late').requesters.size, 1, 'native guard rejects this navigation without cancelling the caller');
  old.emit('did-start-navigation', { isMainFrame: true, isSameDocument: false, url: 'splitty://app/index.html' });
  old.emit('did-navigate');
  const last = helpers.at(-1);
  assert.equal(last.helper.pipe.stage, 'closed');
  last.events.progress?.({ phase: 'bridge', message: 'old document progress' });
  assert.equal(old.messages.length, before);
  assert.equal((await late).ok, false); assert.equal(windows.length, 0);
  console.log('PASS document navigation cancels real helper; no stale progress or late remote window');

  const one = caller(), two = caller();
  const shared = review.openRemote('shared', event(one));
  assert.equal(review.openRemote('shared', event(two)), shared);
  const active = helpers.at(-1).helper;
  review.cancelRemote('shared', one); assert.notEqual(active.pipe.stage, 'closed');
  review.cancelRemote('shared', two); assert.equal(active.pipe.stage, 'closed');
  await shared;
  console.log('PASS coalesced attempt survives first requester cancellation and ends after last requester');
  {
  const baseHelpers = helpers.length;
  const a = caller(), opening = review.openRemote('late', event(a));
  const until = async test => { for(let i=0;i<200;i++) { if(test()) return; await new Promise(r=>setTimeout(r,10)); } throw Error('wait timed out'); };
  await until(() => review.remoteWindows.has('late'));
  const entry = review.remoteWindows.get('late'), helper = helpers.at(-1).helper;
  helper.child.kill('SIGKILL'); await helper.exited; await tick();
  review.broker.document(event(entry.win.webContents));
  const document = entry.win.webContents.messages.at(-1)[1].document;
  entry.win.webContents.messages.length = 0;
  review.broker.open(event(entry.win.webContents), { request: 'initial-after-child-exit', document });
  await tick();
  console.log(JSON.stringify({ pipe: helper.pipe.stage, observedExit: helper.child.exitCode !== null || helper.child.signalCode !== null,
    pendingOpen: review.pendingOpens.has('late'), childWorkers: budget.counts.workers, brokerCounts: review.broker.counts,
    uiReplies: entry.win.webContents.messages }));
  assert.ok(entry.win.webContents.messages.some(message => message[0] === 'native:failed'), 'already exited helper reports failure to the first UI open');
  review.cancelRemote('late', a); assert.equal((await opening).ok, false);

  console.log('PASS child exit before first UI handoff reports a terminal failure');

  entry.win.destroy();
  }
  {
  const baseHelpers = helpers.length;
  const a = caller(), opening = review.openRemote('late', event(a));
  const until = async test => { for(let i=0;i<200;i++) { if(test()) return; await new Promise(r=>setTimeout(r,10)); } throw Error('wait timed out'); };
  await until(() => review.remoteWindows.has('late'));
  const entry = review.remoteWindows.get('late'), helper = helpers.at(-1).helper;
  review.broker.document(event(entry.win.webContents));
  const document = entry.win.webContents.messages.at(-1)[1].document;
  review.broker.open(event(entry.win.webContents), { request: 'startup-read', document });
  await tick();
  review.broker.open(event(entry.win.webContents), { request: 'root-after-startup', document });
  await tick();
  assert.equal(helpers.length - baseHelpers, 2);
  assert.equal(review.openRemote('late', event(a)), opening);
  assert.equal(helpers.length - baseHelpers, 2, 'open during startup remains coalesced');
  let settled = false; void opening.then(() => { settled = true; });
  review.cancelRemote('late', a); await tick();
  console.log(JSON.stringify({ cancelled: true, liveHelpers: budget.counts.workers, pendingRetained: review.pendingOpens.has('late'), settled,
    windowDestroyed: entry.win.isDestroyed() }));
  assert.equal(review.pendingOpens.has('late'), true);
  assert.equal(budget.counts.workers, 2); assert.equal(settled, false);
  assert.equal((await opening).ok, false);
  assert.equal(review.pendingOpens.has('late'), false); assert.equal(budget.counts.workers, 0);
  console.log('PASS startup coalescing and cancellation wait for both real helper exits');

  entry.win.destroy();
  }
  {
  const baseHelpers = helpers.length;
  const a = caller(), opening = review.openRemote('late', event(a));
  const until = async test => { for(let i=0;i<200;i++) { if(test()) return; await new Promise(r=>setTimeout(r,10)); } throw Error('wait timed out'); };
  await until(() => review.remoteWindows.has('late'));
  const entry = review.remoteWindows.get('late'), helper = helpers.at(-1).helper;
  review.broker.document(event(entry.win.webContents));
  const document = entry.win.webContents.messages.at(-1)[1].document;
  review.broker.open(event(entry.win.webContents), { request: 'startup-read', document });
  await tick();
  entry.state = 'connected'; entry.connected = true; entry.ready();
  assert.equal((await opening).ok, true);
  helper.close(); await helper.exited; await tick();
  entry.state = 'disconnected';
  entry.win.webContents.messages.length = 0;
  const reopening = review.openRemote('late', event(a));
  const queuedRetry = entry.win.webContents.messages.find(message => message[0] === 'native:retry');
  assert.ok(queuedRetry, 'native retry was queued for the actual remote document');
  review.cancelRemote('late', a);
  assert.equal((await reopening).ok, false);
  const before = helpers.length;
  assert.equal(review.pendingOpens.has('late'), false);
  // Deliver the already queued retry as the private preload/root connection would.
  review.broker.open(event(entry.win.webContents), { request: 'late-queued-retry', document: queuedRetry[1].document });
  await tick();
  console.log(JSON.stringify({ cancelled: true, pendingAfterCancellation: review.pendingOpens.has('late'),
    helpersStartedAfterCancellation: helpers.length - before, liveWorkersAfterCancellation: budget.counts.workers,
    remoteDocumentStillAssigned: review.broker.remoteState(event(entry.win.webContents), {document, state:'reconnecting'}) === 'reconnecting' }));
  assert.equal(helpers.length, before, 'cancelled queued retry must not start a helper');
  const reopeningAgain = review.openRemote('late', event(a));
  const freshRetry = entry.win.webContents.messages.filter(message => message[0] === 'native:retry').at(-1);
  review.broker.open(event(entry.win.webContents), { request: 'fresh-explicit-retry', document: freshRetry[1].document });
  await tick();
  assert.equal(helpers.length, before + 1, 'explicit reopen enables a fresh helper');
  const progressBefore = a.messages.length;
  helpers.at(-1).events.progress?.({phase: 'bridge', message: 'retry is preparing'});
  assert.equal(a.messages.length, progressBefore + 1);
  assert.equal(a.messages.at(-1)[0], 'remote:progress');
  assert.equal(review.pendingOpens.has('late'), true);
  review.cancelRemote('late', a);
  assert.equal((await reopeningAgain).ok, false);
  assert.equal(budget.counts.workers, 0);
  console.log('PASS native cancel gate rejects queued retry and explicit reopen re-enables it');


  entry.win.destroy();
  }
  // Worker admission is measured with actual OS children, not synthetic
  // reservations. The fixture intentionally holds stdin EOF until escalation.
  {
    assert.equal(budget.counts.workers, 0);
    const workers = Array.from({length:32}, () => {
      const helper = startNativeHelper(process.execPath, {...process.env, REVIEW_DELAY:'10000'}, {closed() {}}, 'worker-limit', budget);
      void helper.pipe.ready.catch(() => {}); helpers.push({helper,events:{}}); return helper;
    });
    assert.equal(budget.counts.workers, 32);
    assert.ok(workers.every(helper => Number.isInteger(helper.child.pid)));
    assert.throws(() => startNativeHelper(process.execPath, process.env, {closed() {}}, 'excess', budget), /worker capacity/);
    for (const helper of workers) helper.close();
    assert.equal(budget.counts.workers, 32, 'closing pipes cannot return worker slots before exit');
    await Promise.all(workers.map(helper => helper.exited));
    assert.equal(budget.counts.workers, 0);
    console.log('PASS 32 real children fill the worker ceiling; excess spawn refused and slots released only after exit');
  }
} finally {
  for (const win of windows) win.destroy();
  for (const { helper } of helpers) helper.close();
  await Promise.all(helpers.map(({ helper }) => helper.exited));
  assert.deepEqual(budget.counts, { data: 0, controls: 0, workers: 0 });
  console.log('PASS all children observed exited; all counters zero');
  process.chdir(original); fs.rmSync(fixture, { recursive: true, force: true });
}
