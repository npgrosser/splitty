// Real executable + compiled Node adapter: documents share one owned service,
// close independently, and can reconnect without adopting a local daemon.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { randomBytes } = require('node:crypto');
const { NativeOwner } = require('../compiled/native_owner');
const { NativeProcessBudget } = require('../compiled/native_budget');
const { NativeKind: K, application, dataFrame } = require('../compiled/native_wire');

function drain(pipe, ready) {
  pipe.activate(frame => {
    if (frame.kind === K.credit) return;
    const { delivery } = application(frame);
    pipe.send({ kind: K.credit, payload: Buffer.from(JSON.stringify({ generation: ready.generation,
      receipt: Buffer.from(delivery.receipt).toString('hex') })) });
  });
}

async function saturatedClose(binary, env, budget) {
  let ownerClosed = 0;
  const owner = new NativeOwner(binary, env, { closed() { ownerClosed++; } }, budget);
  const timeout = setTimeout(() => owner.close(), 10_000);
  try {
    await owner.ready;
    owner.helper.child.stdin.cork();
    const pipes = Array.from({ length: 32 }, () => owner.open({ closed() {} }));
    const ready = pipes.map(pipe => pipe.ready.then(identity => { drain(pipe, identity); return pipe; }));
    const settled = Promise.allSettled(ready);
    assert.doesNotThrow(() => pipes[0].close(), 'cancellation has reserved capacity beyond 64 pending data frames');
    owner.helper.child.stdin.uncork();
    const results = await settled;
    assert(results.some(result => result.status === 'fulfilled'));
    assert.equal(ownerClosed, 0, 'one close must not end the app owner');
    for (const pipe of pipes) pipe.close();
    const deadline = Date.now() + 3000;
    while (owner.counts.streams && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 5));
    assert.deepEqual(owner.counts, { streams: 0, chunks: 0 });
    const active = await client(owner);
    assert.equal((await active.request('mux.status')).status, 200);
    let notified = 0;
    const throwing = owner.open({ closed() { throw Error('document notification unavailable'); } });
    drain(throwing, await throwing.ready);
    const another = owner.open({ closed() { notified++; } }); drain(another, await another.ready);
    const original = console.error; let reports = 0;
    console.error = () => { reports++; };
    try { assert.doesNotThrow(() => owner.close()); } finally { console.error = original; }
    await active.released;
    assert.notEqual(owner.helper.child.exitCode, null, 'fatal owner cleanup is not confirmed before helper exit');
    await owner.helper.exited;
    assert.equal(reports, 1); assert.equal(notified, 1); assert.equal(ownerClosed, 1);
    assert.deepEqual(owner.counts, { streams: 0, chunks: 0 });
    assert.deepEqual(budget.counts, { data: 0, controls: 0, workers: 0 });
  } finally { clearTimeout(timeout); owner.close(); await owner.helper.exited; }
}

async function client(owner) {
  let closed;
  const waiting = new Map();
  const connection = owner.connect({ closed(reason) {
    closed = reason; for (const waiter of waiting.values()) waiter.reject(Error(reason.message)); waiting.clear();
  } });
  const pipe = connection.pipe;
  const ready = await pipe.ready;
  let next = 0, welcomed = false;
  pipe.activate(frame => {
    if (frame.kind === K.credit) return;
    const { delivery, bytes } = application(frame);
    const value = frame.kind === K.binary ? null : JSON.parse(Buffer.from(bytes).toString('utf8'));
    if (!welcomed) { assert.equal(value.type, 'mux.welcome'); welcomed = true; }
    pipe.send({ kind: K.credit, payload: Buffer.from(JSON.stringify({ generation: ready.generation,
      receipt: Buffer.from(delivery.receipt).toString('hex') })) });
    if (frame.kind === K.reply) {
      const waiter = waiting.get(value.id); assert(waiter, 'reply belongs to this client');
      waiting.delete(value.id); waiter.resolve(value);
    }
  });
  return { pipe, ready, released: connection.released, request(operation, args = {}) {
    if (closed) return Promise.reject(Error(closed.message));
    const id = ++next;
    return new Promise((resolve, reject) => {
      waiting.set(id, { resolve, reject });
      pipe.send(dataFrame(K.request, { generation: ready.generation, receipt: randomBytes(16) },
        Buffer.from(JSON.stringify({ id, operation, args }))));
    });
  } };
}

async function main() {
  const binary = path.resolve(process.argv[2] || '../target/debug/splitty');
  const fixture = fs.mkdtempSync(path.join(os.tmpdir(), 'splitty-native-owner-'));
  const env = { ...process.env, HOME: fixture, XDG_CONFIG_HOME: path.join(fixture, 'config'),
    XDG_DATA_HOME: path.join(fixture, 'data'), XDG_RUNTIME_DIR: path.join(fixture, 'run') };
  for (const name of Object.keys(env)) if (name.startsWith('SPLITTY_')) delete env[name];
  fs.mkdirSync(env.XDG_RUNTIME_DIR, { mode: 0o700 });
  const config = path.join(env.XDG_CONFIG_HOME, 'splitty'); fs.mkdirSync(config, { recursive: true });
  const discovery = path.join(config, 'daemon.json'); fs.writeFileSync(discovery, 'deliberately corrupt independent discovery');
  const budget = new NativeProcessBudget();
  const owner = new NativeOwner(binary, env, { closed() {} }, budget);
  const deadline = setTimeout(() => owner.close(), 20_000);
  try {
    const instance = await owner.ready;
    const [first, second] = await Promise.all([client(owner), client(owner)]);
    assert.equal(first.ready.instance, instance); assert.equal(second.ready.instance, instance);
    assert.equal(budget.counts.workers, 1);
    const created = await first.request('mux.op', { body: { screenId: 'owner-screen', op: {
      type: 'create_screen', screenId: 'owner-screen', name: 'survives document closure',
    } } });
    assert.equal(created.status, 200, created.body);
    const fromSecond = await second.request('mux.screen', { id: 'owner-screen' });
    assert.equal(fromSecond.status, 200, fromSecond.body);
    assert(fromSecond.body.includes('survives document closure'));
    let invalidHost;
    const invalid = owner.open({ closed(reason) { invalidHost = reason; } }, 'host:abc');
    await assert.rejects(invalid.ready, /invalid owner host target/);
    assert.equal(invalidHost.retryable, false);
    assert.equal((await second.request('mux.status')).status, 200, 'invalid host opens leave local work intact');
    assert.throws(() => owner.open({ closed() {} }, 'a'.repeat(4096)), /command limit/);
    first.pipe.close();
    const third = await client(owner);
    assert.equal(third.ready.instance, instance);
    assert.equal((await third.request('mux.screen', { id: 'owner-screen' })).status, 200);
    assert.equal((await second.request('mux.status')).status, 200, 'the other document remains connected');
    second.pipe.close(); third.pipe.close();
    // Repeated real cleanup must return stream slots, not consume a fixed
    // lifetime allocation of pipes that becomes exhausted after 32 reloads.
    for (let i = 0; i < 40; i++) {
      const reloaded = await client(owner);
      const status = JSON.parse((await reloaded.request('mux.status')).body);
      assert.equal(status.instance, instance); assert.equal(status.listenAddr, null);
      assert.equal((await reloaded.request('mux.screen', { id: 'owner-screen' })).status, 200);
      reloaded.pipe.close();
    }
    assert.equal(fs.readFileSync(discovery, 'utf8'), 'deliberately corrupt independent discovery');
    owner.close(); await owner.helper.exited;
    assert.equal(owner.helper.child.exitCode, 0);
    assert.deepEqual(budget.counts, { data: 0, controls: 0, workers: 0 });
    await saturatedClose(binary, env, budget);
    console.log(JSON.stringify({ sharedService: true, independentClose: true, reloads: 40, untouchedDiscovery: true,
      invalidHostIsolated: true, saturatedCancellation: true, callbackFailureCleanup: true,
      listener: null, cleanExit: true, counters: budget.counts }));
  } finally {
    clearTimeout(deadline); owner.close(); await owner.helper.exited;
    fs.rmSync(fixture, { recursive: true, force: true });
  }
}
main().catch(error => { console.error(error); process.exitCode = 1; });
