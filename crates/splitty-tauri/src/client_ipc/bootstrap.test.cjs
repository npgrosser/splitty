const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const { test } = require('node:test');
const template = fs.readFileSync(__dirname + '/bootstrap.js', 'utf8');
const secret = Array(32).fill(7), challenge = Array(32).fill(8), proof = Array(32).fill(9);
function capsule({ child = false, href = 'tauri://localhost/index.html', entry = 'tauri://localhost/index.html', confirm, stateReport, openReport, consumedReport, closeReport } = {}) {
  let next = 0;
  const calls = [], callbacks = new Map(), events = new Map();
  const window = { location: { href } }; window.top = child ? {} : window;
  window.__TAURI_INTERNALS__ = {
    invoke: async (command, args) => {
      calls.push({ command, args });
      if (command === 'native_bootstrap_confirm') return confirm ? confirm(args) : { auth: { document: 12, secret: proof }, role: 'main', target: null, forbidden: proof };
      if (command === 'native_connection_state') return stateReport?.(args);
      if (command === 'native_consumed') return consumedReport?.(args);
      if (command === 'native_close') return closeReport?.(args);
      if (command === 'native_open' && openReport) return openReport(args);
      if (command === 'native_open') return { handle: 'a'.repeat(32), document: '12', generation: 1, instance: 'b'.repeat(32), role: 'main', auth: proof };
    },
    transformCallback(callback) { callbacks.set(++next, callback); return next; },
    unregisterCallback(id) { callbacks.delete(id); },
    runCallback(id, value) { callbacks.get(id)?.(value); },
  };
  const scope = vm.createContext({ window, URL, Uint8Array, ArrayBuffer, setTimeout, clearTimeout, crypto: { getRandomValues: bytes => bytes.fill(4) },
    addEventListener: (event, callback) => events.set(event, callback) });
  vm.runInContext(template.replace('__SPLITTY_BOOTSTRAP_CONFIG__', JSON.stringify({ bootstrap: secret, receiver: 'receive', entry })), scope);
  const begin = calls.find(call => call.command === 'native_bootstrap_begin');
  return { window, calls, callbacks, events, begin, bridge: window.__SPLITTY_TAURI__,
    authenticate: () => window.receive({ nonce: begin.args.nonce, challenge }) };
}
test('capsule requires assigned packaged document and main frame', () => {
  for (const options of [{ child: true }, { href: 'https://example.com/index.html' }, { href: 'tauri://localhost/popout.html' }]) {
    const c = capsule(options); assert.equal(c.bridge, undefined); assert.equal(c.calls.length, 0);
  }
});

test('delayed consumption replies bound acknowledgements without disconnecting a healthy channel', async () => {
  const complete = [], acknowledgements = [];
  const c = capsule({ consumedReport: () => new Promise(resolve => complete.push(resolve)) });
  await c.authenticate();
  const assignment = await c.bridge.native.open(undefined, 1);
  const callback = c.callbacks.values().next().value;
  let ended = false;
  c.bridge.native.listen(assignment.handle, (_bytes, token) => {
    acknowledgements.push(c.bridge.native.consumed(assignment.handle, token));
  }, () => { ended = true; });
  const send = index => callback({ index, message: new Uint8Array([1, ...Array(16).fill(index + 1), 42]).buffer });
  const settle = () => new Promise(resolve => setImmediate(resolve));
  send(0); await settle();
  // The backend already consumed packet 0 and may send packet 1, while
  // WebKit still has not delivered the first command's response.
  send(1); await settle();
  assert.equal(complete.length, 1);
  await assert.rejects(c.bridge.native.consumed(assignment.handle, '02'.repeat(16)), /Duplicate/);
  for (let index = 2; index < 20; index++) {
    complete[index - 2](); await settle();
    send(index); await settle();
    assert.equal(complete.length, index);
    assert.equal(ended, false);
  }
  complete[18](); await settle();
  complete[19](); await Promise.all(acknowledgements);
  assert.equal(c.calls.filter(call => call.command === 'native_consumed').length, 20);
  assert.equal(ended, false);
  callback({ index: 20, message: new Uint8Array([0]).buffer });
  assert.equal(c.callbacks.size, 0);
});
test('nonce mismatch cannot confirm; proof stays private and caller auth is replaced', async () => {
  const c = capsule();
  assert.ok(Object.isFrozen(c.bridge)); assert.ok(Object.isFrozen(c.bridge.native));
  await c.window.receive({ nonce: 'old-document', challenge });
  assert.equal(c.calls.length, 1);
  await c.authenticate();
  const metadata = await c.bridge.ready;
  assert.deepEqual(Object.keys(metadata), ['role']);
  await c.bridge.invoke('remote_hosts', { auth: 'forged' });
  const request = c.calls.at(-1); assert.equal(request.args.auth.document, 12); assert.deepEqual([...request.args.auth.secret], proof);
  await assert.rejects(c.bridge.invoke('native_open', {}), /denied/);
  await assert.rejects(c.bridge.invoke('run_arbitrary_command', {}), /denied/);
});
test('late confirmation cannot authorize a document after pagehide', async () => {
  let resolve;
  const c = capsule({ confirm: () => new Promise(done => { resolve = done; }) });
  const confirming = c.authenticate();
  c.events.get('pagehide')();
  resolve({ auth: { document: 12, secret: proof }, role: 'main' });
  await confirming;
  await assert.rejects(c.bridge.ready, /closed/);
  await assert.rejects(c.bridge.invoke('remote_hosts'), /closed/);
});
test('native Channel packets and receipts preserve bytes without exposing proof', async () => {
  const c = capsule(); await c.authenticate();
  const assignment = await c.bridge.native.open(undefined, 1);
  assert.equal(assignment.auth, undefined); assert.ok(Object.isFrozen(assignment));
  const output = c.calls.find(call => call.command === 'native_open').args.output;
  const callback = c.callbacks.get(output);
  let delivered, ended = false;
  c.bridge.native.listen(assignment.handle, (bytes, token) => { delivered = { bytes, token }; }, () => { ended = true; });
  const packet = new Uint8Array([1, ...Array(16).fill(0xaa), 0, 255, 2]);
  callback({ index: 0, message: packet.buffer });
  assert.deepEqual([...delivered.bytes], [0, 255, 2]);
  assert.equal(delivered.token, 'aa'.repeat(16));
  await c.bridge.native.consumed(assignment.handle, delivered.token);
  assert.deepEqual([...c.calls.at(-1).args.token], Array(16).fill(0xaa));
  await c.bridge.native.write(assignment.handle, new Uint8Array([0, 255]));
  assert.deepEqual([...c.calls.at(-1).args.bytes], [0, 255]);
  callback({ index: 1, message: new Uint8Array([0]).buffer });
  assert.equal(ended, true); assert.equal(c.callbacks.size, 0);
});
test('evaluated output only reaches its original live document and callback', async () => {
  const c = capsule(); await c.authenticate();
  const assignment = await c.bridge.native.open(undefined, 1);
  const callback = c.calls.find(call => call.command === 'native_open').args.output;
  const envelope = { index: 0, message: new Uint8Array([1, ...Array(16).fill(0xaa), 0, 255, 128]).buffer };
  let received;
  c.bridge.native.listen(assignment.handle, bytes => { received = [...bytes]; }, () => {});
  assert.equal(c.window.receive_output(11, callback, envelope), false);
  assert.equal(c.window.receive_output(12, callback + 1, envelope), false);
  assert.equal(received, undefined);
  assert.equal(c.window.receive_output(12, callback, envelope), true);
  assert.deepEqual(received, [0, 255, 128]);
  c.bridge.native.close(assignment.handle);
  assert.equal(c.window.receive_output(12, callback, envelope), false);
  const next = capsule({ confirm: () => ({ auth: { document: 13, secret: proof }, role: 'main' }) });
  await next.authenticate();
  await next.bridge.native.open(undefined, 1);
  assert.equal(next.window.receive_output(12, callback, envelope), false, 'reused callback ID cannot receive prior-document data');
  next.events.get('pagehide')();
  assert.equal(next.window.receive_output(13, callback, envelope), false);
});
test('bad channel ordering closes the connection instead of retaining unbounded packets', async () => {
  const c = capsule(); await c.authenticate();
  const assignment = await c.bridge.native.open(undefined, 1);
  let ended = false;
  c.bridge.native.listen(assignment.handle, () => {}, () => { ended = true; });
  c.callbacks.values().next().value({ index: 9000, message: new Uint8Array([0]).buffer });
  assert.equal(ended, true); assert.equal(c.callbacks.size, 0);
});
test('opening admission includes requests still waiting for authentication', async () => {
  const c = capsule();
  const pending = Array.from({ length: 32 }, (_, index) => c.bridge.native.open(undefined, index + 1).catch(error => error));
  await assert.rejects(c.bridge.native.open(undefined, 33), /capacity/);
  c.events.get('pagehide')();
  await Promise.all(pending);
  assert.equal(c.callbacks.size, 0);
});

test('decoder failure releases fetched framework delivery before connection close', async () => {
  const c = capsule(); await c.authenticate();
  const assignment = await c.bridge.native.open(undefined, 1);
  c.bridge.native.listen(assignment.handle, () => { throw Error('bad native frame'); }, () => {});
  const callback = c.callbacks.values().next().value;
  callback({ index: 0, message: new Uint8Array([1, ...Array(16).fill(0xab), 1]).buffer });
  await new Promise(resolve => setImmediate(resolve));
  const cleanup = c.calls.filter(call => ['native_consumed', 'native_close'].includes(call.command));
  assert.deepEqual(cleanup.map(call => call.command), ['native_consumed', 'native_close']);
  assert.deepEqual([...cleanup[0].args.token], Array(16).fill(0xab));
  assert.equal(c.callbacks.size, 0);
});

test('setup capsule admits native upgrade entry while query parameters grant no role', async () => {
  const c = capsule({ entry: 'tauri://localhost/setup.html', href: 'tauri://localhost/index.html?role=remote&target=forged' });
  assert.ok(c.bridge);
  await c.authenticate();
  assert.equal((await c.bridge.ready).role, 'main');
  assert.equal((await c.bridge.ready).target, undefined);
  const forbidden = capsule({ href: 'tauri://localhost/setup.html' });
  assert.equal(forbidden.bridge, undefined);
});

test('recovery state reports keep proof private and coalesce behind one pending native call', async () => {
  let complete;
  const c = capsule({ stateReport: () => new Promise(resolve => { complete = resolve; }) });
  await c.authenticate();
  c.bridge.native.state('reconnecting');
  await new Promise(resolve => setImmediate(resolve));
  c.bridge.native.state('connected'); c.bridge.native.state('disconnected');
  let reports = c.calls.filter(call => call.command === 'native_connection_state');
  assert.equal(reports.length, 1);
  assert.equal(reports[0].args.state, 'reconnecting');
  assert.deepEqual([...reports[0].args.auth.secret], proof);
  complete(); await new Promise(resolve => setImmediate(resolve));
  reports = c.calls.filter(call => call.command === 'native_connection_state');
  assert.equal(reports.length, 2);
  assert.equal(reports[1].args.state, 'disconnected');
  assert.throws(() => c.bridge.native.state('forged'), /Invalid/);
  complete(); await new Promise(resolve => setImmediate(resolve));
  c.events.get('pagehide')(); c.bridge.native.state('connected');
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(c.calls.filter(call => call.command === 'native_connection_state').length, 2);
});

test('pagehide sends fetched receipt cleanup with captured auth while normal commands stay closed', async () => {
  const c = capsule(); await c.authenticate();
  const assignment = await c.bridge.native.open(undefined, 1);
  const callback = c.callbacks.values().next().value;
  // No subscriber yet: the private capsule retains this already-fetched packet.
  callback({ index: 0, message: new Uint8Array([1, ...Array(16).fill(0xcd), 1]).buffer });
  c.events.get('pagehide')();
  await new Promise(resolve => setImmediate(resolve));
  const receipts = c.calls.filter(call => call.command === 'native_consumed');
  assert.equal(receipts.length, 1);
  assert.equal(receipts[0].args.handle, assignment.handle);
  assert.deepEqual([...receipts[0].args.token], Array(16).fill(0xcd));
  assert.deepEqual([...receipts[0].args.auth.secret], proof);
  await assert.rejects(c.bridge.invoke('remote_hosts'), /closed/);
  await assert.rejects(c.bridge.native.open(undefined, 2), /closed|capacity/);
  assert.equal(c.callbacks.size, 0);
  assert.throws(() => c.bridge.native.write(assignment.handle, new Uint8Array([1])), /closed/);
  assert.equal(c.calls.filter(call => call.command === 'native_write').length, 0);
});

test('an open returning after pagehide cleans only its captured original handle', async () => {
  let complete;
  const c = capsule({ openReport: () => new Promise(resolve => { complete = resolve; }) });
  await c.authenticate();
  const opening = c.bridge.native.open(undefined, 1);
  const rejected = assert.rejects(opening, /closed during open/);
  await new Promise(resolve => setImmediate(resolve));
  c.events.get('pagehide')();
  complete({ handle: 'a'.repeat(32), document: '12', generation: 1, instance: 'b'.repeat(32), role: 'main' });
  await rejected; await new Promise(resolve => setImmediate(resolve));
  const cleanup = c.calls.filter(call => call.command === 'native_close');
  assert.equal(cleanup.length, 1);
  assert.equal(cleanup[0].args.handle, 'a'.repeat(32));
  assert.deepEqual([...cleanup[0].args.auth.secret], proof);
  await assert.rejects(c.bridge.invoke('open_local_editor', {}), /closed/);
  assert.equal(c.calls.filter(call => call.command === 'open_local_editor').length, 0);
});

test('pagehide cleanup remains bounded and releases sibling receipts when a consumer throws', async () => {
  let sequence = 0;
  const c = capsule({ openReport: () => ({ handle: String(++sequence).repeat(32), document: '12', generation: 1, instance: 'b'.repeat(32), role: 'main' }) });
  await c.authenticate();
  const first = await c.bridge.native.open(undefined, 1);
  await c.bridge.native.open(undefined, 2);
  c.bridge.native.listen(first.handle, () => {}, () => { throw Error('consumer failed'); });
  const callbacks = [...c.callbacks.values()];
  callbacks[0]({ index: 0, message: new Uint8Array([1, ...Array(16).fill(7), 1]).buffer });
  callbacks[1]({ index: 0, message: new Uint8Array([1, ...Array(16).fill(9), 1]).buffer });
  assert.doesNotThrow(() => c.events.get('pagehide')());
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(c.calls.filter(call => call.command === 'native_consumed').length, 2);
  assert.equal(c.calls.filter(call => call.command === 'native_close').length, 2);
  assert.equal(c.callbacks.size, 0);
});

test('retired channels retain their slots through delayed ACK and cleanup replies across 64 cycles', async () => {
  let sequence = 0;
  const complete = [];
  const c = capsule({
    openReport: () => ({ handle: (++sequence).toString(16).padStart(32, '0'), document: '12', generation: 1, instance: 'b'.repeat(32), role: 'main' }),
    consumedReport: () => new Promise(resolve => complete.push(resolve)),
  });
  await c.authenticate();
  const settle = () => new Promise(resolve => setImmediate(resolve));
  let last;
  for (let index = 0; index < 32; index++) {
    last = await c.bridge.native.open(undefined, index + 1);
    c.bridge.native.listen(last.handle, (_bytes, token) => { void c.bridge.native.consumed(last.handle, token).catch(() => {}); }, () => {});
    c.callbacks.values().next().value({ index: 0, message: new Uint8Array([1, ...Array(16).fill(index + 1), 1]).buffer });
    await settle();
    c.bridge.native.close(last.handle);
    await settle();
  }
  assert.equal(complete.length, 64, 'one normal ACK and its cleanup ACK per retired record');
  assert.equal(c.callbacks.size, 0, 'retirement unregisters delivery immediately');
  assert.throws(() => c.bridge.native.write(last.handle, new Uint8Array([1])), /closed/);
  await assert.rejects(c.bridge.native.open(undefined, 33), /capacity/);
  // Settling only the original ACKs does not release their cleanup ownership.
  for (let index = 0; index < 64; index += 2) complete[index]();
  await settle();
  await assert.rejects(c.bridge.native.open(undefined, 33), /capacity/);
  for (let index = 1; index < 64; index += 2) complete[index]();
  await settle();
  // Another 32 independent cycles can proceed once actual cleanup replies end.
  for (let index = 32; index < 64; index++) {
    const assignment = await c.bridge.native.open(undefined, index + 1);
    c.bridge.native.close(assignment.handle);
    await settle();
  }
  assert.equal(sequence, 64);
  assert.equal(c.callbacks.size, 0);
});

test('retired records keep their own slot until the actual native close reply settles', async () => {
  let sequence = 0;
  const complete = [];
  const c = capsule({
    openReport: () => ({ handle: (++sequence).toString(16).padStart(32, '0'), document: '12', generation: 1, instance: 'b'.repeat(32), role: 'main' }),
    closeReport: () => new Promise(resolve => complete.push(resolve)),
  });
  await c.authenticate();
  const settle = () => new Promise(resolve => setImmediate(resolve));
  for (let index = 0; index < 32; index++) {
    const assignment = await c.bridge.native.open(undefined, index + 1);
    c.bridge.native.close(assignment.handle);
  }
  assert.equal(complete.length, 32);
  assert.equal(c.callbacks.size, 0);
  await assert.rejects(c.bridge.native.open(undefined, 33), /capacity/);
  complete[0](); await settle();
  const next = await c.bridge.native.open(undefined, 33);
  c.bridge.native.close(next.handle);
  await assert.rejects(c.bridge.native.open(undefined, 34), /capacity/);
  for (let index = 1; index < complete.length; index++) complete[index]();
  await settle();
  const final = await c.bridge.native.open(undefined, 34);
  c.bridge.native.close(final.handle);
  complete.at(-1)(); await settle();
});

test('pre-Ready diagnostic delivery preserves structured failure while invoke carries only a marker', async () => {
  let rejectOpen, output;
  const c = capsule({ openReport: args => { output = args.output; return new Promise((_, reject) => { rejectOpen = reject; }); } });
  await c.authenticate();
  const opening = c.bridge.native.open(undefined, 1);
  await new Promise(resolve => setImmediate(resolve));
  const failure = { message: 'backend "failed"\n' + 'x'.repeat(65536), hint: 'retry here', code: 'backendLost', retryable: true };
  assert.equal(c.window.receive_output(12, output, { failure }), true);
  assert.equal(c.window.receive_output(12, output, { failure }), false, 'one diagnostic per opening');
  rejectOpen('SplittyNativeFailureDelivered');
  await assert.rejects(opening, error => error === `SplittyNativeFailure:${JSON.stringify(failure)}`);
  assert.equal(c.callbacks.size, 0);
  assert.equal(c.calls.filter(call => call.command === 'native_start').length, 0);
});

test('late diagnostic delivery cannot reach a retired or replacement document', async () => {
  let rejectOpen, output;
  const c = capsule({ openReport: args => { output = args.output; return new Promise((_, reject) => { rejectOpen = reject; }); } });
  await c.authenticate();
  const opening = c.bridge.native.open(undefined, 1);
  await new Promise(resolve => setImmediate(resolve));
  const failure = { message: 'private old failure', hint: null, code: 'backendLost', retryable: false };
  assert.equal(c.window.receive_output(11, output, { failure }), false);
  c.events.get('pagehide')();
  assert.equal(c.window.receive_output(12, output, { failure }), false);
  rejectOpen('SplittyNativeFailureDelivered');
  await assert.rejects(opening, error => error === 'Native backend failed; diagnostic unavailable');
  assert.equal(c.callbacks.size, 0);
});

test('pending diagnostic opens retain the existing 32 slots until actual invoke completion', async () => {
  const rejected = [], outputs = [];
  const c = capsule({ openReport: args => { outputs.push(args.output); return new Promise((_, reject) => rejected.push(reject)); } });
  await c.authenticate();
  const pending = [];
  for (let request = 1; request <= 64; request++) {
    const opening = c.bridge.native.open(undefined, request);
    if (request <= 32) pending.push(opening.catch(error => error));
    else await assert.rejects(opening, /capacity/);
  }
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(rejected.length, 32);
  const failure = { message: 'failed', hint: null, code: 'backendLost', retryable: false };
  for (const output of outputs) assert.equal(c.window.receive_output(12, output, { failure }), true);
  await assert.rejects(c.bridge.native.open(undefined, 65), /capacity/);
  for (const reject of rejected) reject('SplittyNativeFailureDelivered');
  for (const error of await Promise.all(pending)) assert.equal(error, `SplittyNativeFailure:${JSON.stringify(failure)}`);
  assert.equal(c.callbacks.size, 0);
});
