// Real-process check: the compiled Node adapter must interoperate with Rust's
// codec, own the child's EOF, and release workers only after observed exit.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { randomBytes } = require('node:crypto');
const { startNativeHelper } = require('../compiled/native_helper');
const { NativeProcessBudget } = require('../compiled/native_budget');
const { NativeKind: K, application, dataFrame } = require('../compiled/native_wire');

async function main() {
  const binary = path.resolve(process.argv[2] || '../target/debug/splitty');
  const fixture = fs.mkdtempSync(path.join(os.tmpdir(), 'splitty-node-pipes-'));
  const env = { ...process.env, HOME: fixture, XDG_CONFIG_HOME: path.join(fixture, 'config'),
    XDG_DATA_HOME: path.join(fixture, 'data'), XDG_RUNTIME_DIR: path.join(fixture, 'run') };
  for (const name of Object.keys(env)) if (name.startsWith('SPLITTY_')) delete env[name];
  fs.mkdirSync(env.XDG_RUNTIME_DIR, { mode: 0o700 });
  const budget = new NativeProcessBudget();
  let helper;
  let timeout;
  try {
    const result = new Promise((resolve, reject) => {
      timeout = setTimeout(() => reject(Error('Native pipe smoke timed out')), 15_000);
      helper = startNativeHelper(binary, env, { closed: reason => reject(Error(reason.message)) }, undefined, budget);
      helper.pipe.ready.then(ready => {
        assert.equal(ready.purpose.kind, 'direct');
        assert.equal(budget.counts.workers, 1);
        let welcome = false;
        helper.pipe.activate(frame => {
          if (frame.kind === K.credit) return;
          const { delivery, bytes } = application(frame);
          const value = JSON.parse(new TextDecoder('utf8', { fatal: true }).decode(bytes));
          helper.pipe.send({ kind: K.credit, payload: Buffer.from(JSON.stringify({
            generation: ready.generation, receipt: Buffer.from(delivery.receipt).toString('hex'),
          })) });
          if (frame.kind === K.text) {
            if (!welcome) assert.equal(value.type, 'mux.welcome');
            welcome = true;
          } else {
            assert.equal(frame.kind, K.reply); assert(welcome, 'welcome must precede API replies');
            assert.equal(value.id, 1); assert.equal(value.status, 200);
            const status = JSON.parse(value.body);
            assert.equal(status.instance, ready.instance); assert.equal(status.listenAddr, null);
            resolve({ nativeVersion: 1, welcome, listener: status.listenAddr });
          }
        });
        helper.pipe.send(dataFrame(K.request, { generation: ready.generation, receipt: randomBytes(16) },
          Buffer.from(JSON.stringify({ id: 1, operation: 'mux.status', args: {} }))));
      }).catch(reject);
    });
    const resultValue = await result;
    helper.close();
    assert.equal(budget.counts.workers, 1, 'requesting close is not observed process exit');
    await helper.exited;
    assert.deepEqual(budget.counts, { data: 0, controls: 0, workers: 0 });
    assert.equal(helper.child.exitCode, 0, 'owned Rust helper exits cleanly after EOF');
    console.log(JSON.stringify({ ...resultValue, cleanExit: true, counters: budget.counts }));
  } finally {
    clearTimeout(timeout);
    if (helper) { helper.close(); await helper.exited; }
    fs.rmSync(fixture, { recursive: true, force: true });
  }
}
main().catch(error => { console.error(error); process.exitCode = 1; });
