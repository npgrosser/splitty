// Runs in an actual packaged WebKit document, using the production adapters.
import { NativeSocket } from '../../../frontend/src/lib/protocol/native_socket.ts';
import { exerciseFullUi } from './full-ui.ts';
declare const __SPLITTY_FULL_UI__: boolean;
declare const __SPLITTY_SMOKE_RELOADS__: number;
import { openTauriChannel } from '../../../frontend/src/lib/protocol/tauri_channel.ts';

import { isProbeDocument, probeDocumentMetadata } from './document.mjs';

// A popout may inherit this initialization script, but only the actual top
// packaged workspace document owns the smoke. Keep its capsule assertion hard.
if (isProbeDocument(window)) {
const state = window as any;
const report = (value: object) => state.__TAURI_INTERNALS__.invoke('smoke_report', { value: { ...value, probeDocument: probeDocumentMetadata(window) } });
const assert = (condition: unknown, message: string) => { if (!condition) throw Error(message); };
let stage = 'initialization';
let nativeInstance: string | undefined;
let currentEpoch = 0;
const network: string[] = [];
const securityViolations: string[] = [];
const uiConnectionErrors: string[] = [];
const observer = new MutationObserver(() => {
  const alert = document.querySelector('#app [role="alert"]');
  const message = alert?.textContent?.trim();
  if (message && uiConnectionErrors.at(-1) !== message) uiConnectionErrors.push(message);
});
observer.observe(document, { subtree: true, childList: true, characterData: true });
const assetFetches: { name: string; ok: boolean; contentType: string | null }[] = [];
state.__SPLITTY_SMOKE_ASSETS__ = assetFetches;
window.addEventListener('securitypolicyviolation', event => securityViolations.push(`${event.violatedDirective}: ${event.blockedURI}`));
const fetch = window.fetch;
window.fetch = function(input, ...args) {
  const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
  const parsed = new URL(url, location.href);
  if ((/^(https?|wss?):/.test(url) && !url.startsWith('http://ipc.localhost/'))
      || /^\/(api(?:\/|$)|ws(?:\/|$))/.test(parsed.pathname)) network.push(url);
  return fetch.call(this, input, ...args).then(response => {
    if (parsed.pathname.endsWith('.wasm')) assetFetches.push({ name: parsed.href, ok: response.ok, contentType: response.headers.get('content-type') });
    return response;
  });
};
const originalXhrOpen = XMLHttpRequest.prototype.open;
XMLHttpRequest.prototype.open = function(method: string, url: string | URL, ...args: any[]) {
  network.push(`XHR ${method} ${url}`);
  return (originalXhrOpen as any).call(this, method, url, ...args);
};
const OriginalWebSocket = window.WebSocket;
(window as any).WebSocket = new Proxy(OriginalWebSocket, { construct(target, args, receiver) {
  network.push(String(args[0])); return Reflect.construct(target, args, receiver);
} });
const timeout = setTimeout(() => { void report({ error: 'WebKit smoke deadline', stage }); }, __SPLITTY_FULL_UI__ ? 60000 : 20000);
const wait = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));
void (async () => {
  const epoch = Number(sessionStorage.getItem('smoke-epoch') || '0');
  currentEpoch = epoch;
  const first = epoch === 0, last = epoch === __SPLITTY_SMOKE_RELOADS__;
  stage = 'capsule';
  const capsule = state.__SPLITTY_TAURI__;
  assert(capsule, 'production initialization capsule is missing');
  const role = await capsule.ready;
  assert(role.role === 'main', 'main native role missing');
  stage = 'channel';
  // The production bundle and this independently bundled probe have separate
  // request counters. Reserve a disjoint probe range without changing the app.
  const probe = __SPLITTY_FULL_UI__ ? { ...capsule.native,
    open: (options: unknown, request: number) => capsule.native.open(options, request + 0x40000000),
    cancelOpen: (request: number) => capsule.native.cancelOpen(request + 0x40000000),
  } : capsule.native;
  const channel = await openTauriChannel(probe);
  const assignment = channel.assignment;
  nativeInstance = assignment.instance;
  const socket = new NativeSocket();
  let failure: Error | undefined;
  socket.onError = error => { failure = error; };
  const events: any[] = [];
  const binary: number[] = [];
  socket.onMessage(message => events.push(message));
  socket.onBinaryMessage((_id, bytes) => binary.push(...bytes));
  socket.adopt(channel);
  const request = async (operation: string, args: object = {}) => {
    const reply = await socket.request({ operation, args } as any);
    assert(reply.status >= 200 && reply.status < 300, `${operation}: ${reply.status}: ${reply.body}`);
    return JSON.parse(reply.body);
  };
  stage = 'full UI';
  const ui = __SPLITTY_FULL_UI__ ? await exerciseFullUi(request, first) : undefined;
  if (__SPLITTY_FULL_UI__ && (first || last)) {
    stage = 'native heartbeat while full UI is mounted';
    await wait(17000);
    if (failure) throw failure;
    assert(uiConnectionErrors.length === 0, `production App connection errors: ${uiConnectionErrors}`);
  }
  stage = 'settings';
  const settings = await request('home.get');
  assert(settings && typeof settings === 'object', 'native settings reply missing');
  if (first) {
    stage = 'create shared screen';
    await request('mux.op', { body: { screenId: 'webkit-smoke-screen', op: {
      type: 'create_screen', screenId: 'webkit-smoke-screen', name: 'survives actual WebKit reload',
    } } });
  }
  const screen = await request('mux.screen', { id: 'webkit-smoke-screen' });
  assert(JSON.stringify(screen).includes('survives actual WebKit reload'), 'workspace lost on reload');
  let terminalPid: number | undefined, binaryBytes = 0;
  if (first || last) {
  stage = 'terminal';
  socket.send({ type: 'create', shell: '/bin/sh', cols: 80, rows: 24 });
  const deadline = Date.now() + 8000;
  let session: string | undefined;
  while (!session && Date.now() < deadline) {
    session = events.find(event => event.type === 'created')?.sessionId;
    if (failure) throw failure;
    await wait(10);
  }
  assert(session, 'real terminal creation did not reply');
  await request('session.send', { id: session, body: { data: "printf 'WEBKIT_NATIVE_OK\\000\\377\\200\\n'; printf 'WEBKIT_PID_%s\\n' \"$$\"\n" } });
  const marker = Array.from(new TextEncoder().encode('WEBKIT_NATIVE_OK'));
  const expected = [...marker, 0, 255, 128];
  let byteExact = false;
  let pidMatch: RegExpMatchArray | null = null;
  while ((!byteExact || !pidMatch) && Date.now() < deadline) {
    byteExact = binary.some((_byte, index) => expected.every((byte, offset) => binary[index + offset] === byte));
    pidMatch = new TextDecoder().decode(new Uint8Array(binary)).match(/WEBKIT_PID_(\d+)/);
    if (failure) throw failure;
    await wait(10);
  }
  assert(pidMatch, 'terminal shell PID evidence missing');
  terminalPid = Number(pidMatch![1]); binaryBytes = binary.length;
  assert(byteExact, 'NUL and non-UTF-8 terminal output did not survive actual IPC');
  }
  await wait(200);
  assert(uiConnectionErrors.length === 0, `production App connection errors: ${uiConnectionErrors}`);
  assert(securityViolations.length === 0, `CSP violations: ${securityViolations}`);
  assert(network.length === 0, `unexpected network calls: ${network}`);
  const result = { phase: first ? 'first' : last ? 'reloaded' : 'intermediate', epoch, expectedReloads: __SPLITTY_SMOKE_RELOADS__, instance: assignment.instance,
    document: assignment.document, settings: true, workspace: true, terminalBinary: first || last,
    binaryBytes, terminalPid, network, assetFetches, uiConnectionErrors, fullUi: __SPLITTY_FULL_UI__, ui, securityViolations };
  if (!last) {
    sessionStorage.setItem('smoke-epoch', String(epoch + 1));
    stage = 'reload';
    await report(result);
    location.reload();
  } else {
    stage = 'cleanup';
    socket.revoke();
    await wait(200);
    clearTimeout(timeout);
    await report(result);
  }
})().catch(error => { clearTimeout(timeout); void report({ error: String(error), stage, epoch: currentEpoch, instance: nativeInstance, stack: error?.stack, network, assetFetches, uiConnectionErrors, securityViolations, dom: document.body?.innerText.slice(0, 2000) }); });

}
