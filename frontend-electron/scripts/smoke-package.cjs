// Linux distribution-package launch: real ASAR assets and bundled helper must
// create a PTY without a TCP listener. App termination tests pipe ownership.
const assert = require('node:assert/strict');
const fs = require('node:fs'), os = require('node:os'), path = require('node:path');
const { spawn, execFileSync } = require('node:child_process');
if (process.platform !== 'linux') throw Error('This package/process inventory smoke requires Linux');
const binary = path.resolve(process.argv[2] || 'dist/linux-unpacked/splitty-electron');
const fixture = fs.mkdtempSync(path.join(os.tmpdir(), 'splitty-package-smoke-'));
const env = { ...process.env, HOME: fixture, XDG_CONFIG_HOME: path.join(fixture, 'config'),
  XDG_DATA_HOME: path.join(fixture, 'data'), XDG_RUNTIME_DIR: path.join(fixture, 'run') };
for (const name of Object.keys(env)) if (name.startsWith('SPLITTY_')) delete env[name];
fs.mkdirSync(env.XDG_RUNTIME_DIR, { mode: 0o700 });
fs.writeFileSync(path.join(fixture, '.zshrc'), '# Isolated package smoke shell.\n');
const daemon = path.join(env.XDG_CONFIG_HOME, 'splitty', 'daemon.json');
const backend = path.join(path.dirname(binary), 'resources', 'splitty');
const cli = (args, input) => execFileSync(backend, args, {env,input,encoding:'utf8',timeout:15_000,stdio:[input === undefined ? 'ignore' : 'pipe','pipe','pipe']});
fs.mkdirSync(path.dirname(daemon), {recursive:true,mode:0o700});
const processInfo = pid => {
  try {
    const stat = fs.readFileSync(`/proc/${pid}/stat`, 'utf8'), fields = stat.slice(stat.lastIndexOf(')') + 2).split(' ');
    return { pid, state: fields[0], parent: Number(fields[1]), birth: fields[19],
      args: fs.readFileSync(`/proc/${pid}/cmdline`, 'utf8').split('\0') };
  } catch { return undefined; }
};
const alive = identity => { const current = identity && processInfo(identity.pid); return current && current.birth === identity.birth && !['Z', 'X'].includes(current.state); };
const pause = () => new Promise(resolve => setTimeout(resolve, 50));
async function until(check, message) { for (let i = 0; i < 400; i++) { const found = check(); if (found) return found; await pause(); } throw Error(message); }
async function bounded(promise, milliseconds, message) {
  let timer;
  try { return await Promise.race([promise, new Promise((_, reject) => { timer = setTimeout(() => reject(Error(message)), milliseconds); })]); }
  finally { clearTimeout(timer); }
}
let child, exited = Promise.resolve(), helper, terminal, daemonIdentity, tail = '', daemonStarted = false;
(async () => {
  daemonStarted = true; cli(['daemon', 'start']);
  const job = JSON.parse(cli(['run', '--json', '--', '/bin/sh']));
  assert.equal(JSON.parse(cli(['inspect', job.jobId, '--json'])).state, 'running');
  const savedDiscovery = fs.readFileSync(daemon);
  const independent = JSON.parse(savedDiscovery);
  const keepersPath = path.join(env.XDG_DATA_HOME, 'splitty/run/keepers');
  const keeperInodes = () => fs.readdirSync(keepersPath).filter(name => name.endsWith('.sock'))
    .map(name => fs.statSync(path.join(keepersPath,name)).ino).sort();
  const savedKeepers = keeperInodes(); assert.ok(savedKeepers.length);
  child = spawn(binary, [`--user-data-dir=${path.join(fixture, 'electron-data')}`], { env, stdio: ['ignore', 'ignore', 'pipe'] });
  child.stderr.on('data', bytes => { tail = (tail + bytes.toString()).slice(-8192); });
  exited = new Promise(resolve => { child.once('exit', resolve); child.once('error', resolve); });
  daemonIdentity = processInfo(independent.pid); assert.ok(alive(daemonIdentity));
  helper = await until(() => {
    const processes = fs.readdirSync('/proc').filter(name => /^\d+$/.test(name)).map(name => processInfo(Number(name))).filter(Boolean);
    return processes.find(info => info.parent === child.pid && info.args.includes('desktop-backend') && info.args.includes('--owner'));
  }, 'Packaged application did not start its owned native helper');
  terminal = await until(() => {
    for (const name of fs.readdirSync('/proc').filter(name => /^\d+$/.test(name))) {
      const info = processInfo(Number(name)); if (!info || info.parent !== helper.pid) continue;
      try { if (fs.readlinkSync(`/proc/${info.pid}/fd/0`).startsWith('/dev/pts/')) return info; } catch {}
    }
  }, 'Packaged frontend did not create a terminal');
  const listening = new Set();
  for (const file of ['tcp', 'tcp6']) for (const line of fs.readFileSync(`/proc/net/${file}`, 'utf8').trim().split('\n').slice(1)) {
    const fields = line.trim().split(/\s+/); if (fields[3] === '0A') listening.add(fields[9]);
  }
  for (const fd of fs.readdirSync(`/proc/${helper.pid}/fd`)) {
    let link; try { link = fs.readlinkSync(`/proc/${helper.pid}/fd/${fd}`); } catch { continue; }
    const socket = /^socket:\[(\d+)\]$/.exec(link);
    assert.ok(!socket || !listening.has(socket[1]), 'Native helper opened a TCP listener');
  }
  child.kill('SIGTERM'); await bounded(exited, 10_000, 'Packaged app did not exit after SIGTERM');
  await until(() => !alive(helper) && !alive(terminal), 'Owned helper or terminal survived packaged app exit');
  assert.deepEqual(fs.readFileSync(daemon), savedDiscovery);
  assert.deepEqual(keeperInodes(), savedKeepers);
  assert.ok(alive(daemonIdentity));
  const status = JSON.parse(cli(['daemon','status','--json']));
  assert.equal(status.instance, independent.instance);
  assert.equal(JSON.parse(cli(['inspect',job.jobId,'--json'])).state,'running');
  const keeperMarker = path.join(fixture, 'keeper-still-works');
  cli(['input',job.jobId,'--json'], `touch '${keeperMarker}'\n`);
  await until(() => fs.existsSync(keeperMarker), 'Independent keeper terminal no longer accepts input after app exit');
  console.log(JSON.stringify({ packagedAsar: true, bundledNativeHelper: true, terminalCreated: true,
    noHelperTcpListener: true, appExitEndsHelperAndTerminal: true, independentDaemonAndKeepersUntouched: true, independentTerminalIo: true }));
})().catch(error => { console.error(error, tail); process.exitCode = 1; }).finally(async () => {
  let clean = true;
  if (child && child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
  try { await bounded(exited, 5000, 'Package child exit could not be observed'); }
  catch (error) { console.error(error.message); clean = false; }
  for (const identity of [terminal, helper]) if (alive(identity)) { try { process.kill(identity.pid, 'SIGKILL'); } catch {} }
  try { await until(() => !alive(helper) && !alive(terminal), 'Owned helper/PTY cleanup could not be observed'); }
  catch (error) { console.error(error.message); clean = false; }
  child?.stderr.destroy();
  try {
    if (daemonStarted) cli(['daemon','stop','--end-sessions']);
    await until(() => !alive(daemonIdentity), 'Fixture daemon exit could not be observed');
  } catch (error) { console.error('fixture daemon cleanup failed', error.message); clean = false; }
  if (clean) fs.rmSync(fixture, {recursive:true,force:true});
  else { console.error(`Cleanup incomplete; fixture retained at ${fixture}`); process.exitCode = 1; }
});
