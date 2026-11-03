import { fixtureError } from './error.mjs';
// Isolated fixture only: production native adapters, no HTTP backend; production UI/popouts are opt-in.
import { setupIsolation, remoteIsolation, mainIsolation, fixtureState } from './isolation.ts';
import { consumerAdmission } from './consumer.ts';
import { exerciseFullUi } from '../tauri-webkit-smoke/full-ui.ts';
import { isWindowsFixtureDocument, probeBridge } from './document.mjs';
import { NativeSocket } from '../../../frontend/src/lib/protocol/native_socket.ts';
import { openTauriChannel } from '../../../frontend/src/lib/protocol/tauri_channel.ts';
import { SessionClient } from '../../../frontend/src/lib/terminal/session_client.ts';
import { AttachRegistry } from '../../../frontend/src/lib/terminal/attach_registry.ts';
declare const __FIXTURES__: string[];
declare const __WINDOWS_FULL_UI__: boolean;
declare const __ISOLATION_CONSUMER__: boolean;
declare const __ISOLATION_ALIASES__: string[];
if (isWindowsFixtureDocument(window, true)) {
const capsule = (window as any).__SPLITTY_TAURI__;
const report = (value: unknown) => (window as any).__TAURI_INTERNALS__.invoke('windows_smoke_report', { value });
const assert = (condition: unknown, message: string) => { if (!condition) throw Error(message); };
const wait = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));
let stage = 'bootstrap';
const timer = setTimeout(() => { void report({ error: 'isolated WSL deadline', stage }); }, 150_000);
void (async () => {
  const metadata = await capsule.ready;
  if (__ISOLATION_CONSUMER__ && metadata.role === 'remote') {
    try { await remoteIsolation(capsule, metadata, __ISOLATION_ALIASES__); } catch(error) { await fixtureState({kind:'remote',error:fixtureError(error)}); }
    clearTimeout(timer);return;
  }
  if (metadata.role === 'setup') {
    if (__ISOLATION_CONSUMER__) {
      stage = 'Setup remote assignments';
      await setupIsolation(capsule,__ISOLATION_ALIASES__);
      stage = 'local B owner';
      await capsule.invoke('wsl_start_backend',{distro:__FIXTURES__[1]});
      await report({phase:'started-b'});clearTimeout(timer);await capsule.invoke('wsl_open_backend');return;
    }
    const state = await capsule.invoke('wsl_setup_state');
    if (state.pendingDistro === __FIXTURES__[1]) {
      stage = 'start switched WSL owner after restart';
      await capsule.invoke('wsl_start_backend', { distro: __FIXTURES__[1] });
      await report({ phase: 'started-b' });
    } else {
      stage = 'first WSL owner';
      await capsule.invoke('wsl_start_backend', { distro: __FIXTURES__[0] });
      const first = await report({ phase: 'started-a' });
      stage = 'duplicate WSL start';
      await capsule.invoke('wsl_start_backend', { distro: __FIXTURES__[0] });
      const duplicate = await report({ phase: 'duplicate-a' });
      assert(first.wslOwnerPid === duplicate.wslOwnerPid, 'duplicate start replaced its owner');
    }
    clearTimeout(timer);
    await capsule.invoke('wsl_open_backend');
    return;
  }
  assert(metadata.role === 'main', 'main WSL assignment missing');
  stage = 'WSL native channel';
  const socket = new NativeSocket();
  const sessions = new SessionClient(socket, new AttachRegistry(socket));
  const events: any[] = [], bytes: number[] = [];
  let failure: Error | undefined;
  socket.onError = error => { failure = error; };
  socket.onMessage(message => events.push(message));
  socket.onBinaryMessage((_id, data) => bytes.push(...data));
  socket.adopt(await openTauriChannel(__WINDOWS_FULL_UI__ ? probeBridge(capsule.native) : capsule.native));
  const request = async (operation: string, args = {}) => {
    const reply = await socket.request({ operation, args });
    assert(reply.status >= 200 && reply.status < 300, `${operation}: ${reply.status}: ${reply.body}`);
    return JSON.parse(reply.body);
  };
  const build = await request('build.get');
  if (build.wslDistro === __FIXTURES__[0]) {
    await report({ phase: 'main-a' });
    sessions.dispose(); socket.dispose(); clearTimeout(timer);
    setTimeout(() => { void report({ error: 'distribution switch did not restart the app' }); }, 20000);
    // App restart deliberately destroys this document and its invoke callback.
    void capsule.invoke('wsl_switch_distribution', { distro: __FIXTURES__[1] }).catch(() => {});
    return;
  }
  assert(build.wslDistro === __FIXTURES__[1], 'wrong WSL backend identity');
  await request('home.get');
  stage = 'production UI';
  const isolation=__ISOLATION_CONSUMER__ ? await mainIsolation() : undefined;
  if(isolation)await report({phase:'isolation-complete',isolation});
  const pressure=__ISOLATION_CONSUMER__ ? await consumerAdmission(capsule,request,()=>sessions.uploadFile(new File([Uint8Array.from({length:65537},(_,index)=>index%256)],'healthy-pressure.bin'),undefined,30000,{dir:'/home/splitty-fixture'}),()=>failure?undefined:socket.assignment?.handle,(phase,value)=>report({phase,...value})) : undefined;
  const ui = __WINDOWS_FULL_UI__ && !__ISOLATION_CONSUMER__ ? await exerciseFullUi(request, true, { countField: 'nativeCount' }) : undefined;
  stage = 'WSL terminal bytes';
  socket.send({ type: 'create', cols: 80, rows: 24 });
  const deadline = Date.now() + 15000;
  let session: string | undefined;
  while (!session && Date.now() < deadline) { session = events.find(event => event.type === 'created')?.sessionId; if (failure) throw failure; await wait(50); }
  assert(session, 'WSL PTY creation failed');
  await request('session.send', { id: session, body: { data: "printf 'WINDOWS_WSL_NATIVE_OK\\000\\377\\200\\n'; printf 'FIXTURE_SHELL_%s\\n' \"$$\"\n" } });
  const marker = [...new TextEncoder().encode('WINDOWS_WSL_NATIVE_OK'), 0, 255, 128];
  let byteExact = false, pid: RegExpMatchArray | null = null;
  while ((!byteExact || !pid) && Date.now() < deadline) {
    byteExact = bytes.some((_byte, index) => marker.every((byte, offset) => bytes[index + offset] === byte));
    pid = new TextDecoder().decode(new Uint8Array(bytes)).match(/FIXTURE_SHELL_(\d+)/);
    if (failure) throw failure; await wait(50);
  }
  assert(byteExact && pid, 'WSL terminal binary traffic corrupted');
  stage = 'WSL file bytes';
  const fileBytes = Uint8Array.from({ length: 1_048_833 }, (_, index) => index % 256);
  const savedPath = await sessions.uploadFile(new File([fileBytes], 'windows-native-fixture.bin'), undefined, 30000, { dir: '/home/splitty-fixture' });
  await report({ phase: 'native-open' });
  stage = 'WSL heartbeat'; await wait(17000); if (failure) throw failure;
  const observations = __WINDOWS_FULL_UI__ ? (window as any).__SPLITTY_WINDOWS_UI_PROBE__ : undefined;
  if (__WINDOWS_FULL_UI__) {
    assert(observations && observations.network.length === 0, 'production UI attempted network fallback');
    assert(observations.clipboard.length === 0, 'production UI attempted global clipboard access');
    assert(observations.errors.length === 0, 'production UI connection error: ' + observations.errors.join('; '));
  }
  sessions.dispose(); socket.dispose(); clearTimeout(timer);
  await report({ phase: 'final', fullUi: __WINDOWS_FULL_UI__, ui, observations, isolation, pressure, terminalBinary: true, terminalPid: Number(pid![1]), savedPath, fileBytes: fileBytes.length, distro: build.wslDistro });
})().catch(error => { clearTimeout(timer); void report({ error: fixtureError(error), stage, stack: error?.stack ? fixtureError(error.stack) : undefined, pressureChronology:error?.pressureChronology }); });

}
