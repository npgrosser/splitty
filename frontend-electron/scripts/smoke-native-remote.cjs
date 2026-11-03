// Invoked only by the Rust remote_connect SSH lab. Uses production main/preload
// and compiled UI with native registration hooks in a private test copy.
const assert = require('node:assert/strict');
const fs = require('node:fs'), os = require('node:os'), path = require('node:path');
const { execFile } = require('node:child_process');
const { promisify } = require('node:util');
const { app, BrowserWindow } = require('electron');
const [binary, target, lab, newKey] = process.argv.slice(2);
assert.ok(binary && target && lab && newKey, 'run through the isolated Rust SSH lab');
const fixture = fs.mkdtempSync(path.join(os.tmpdir(), 'splitty-native-remote-'));
const compiled = path.join(fixture, 'electron', 'compiled'); fs.mkdirSync(compiled, { recursive: true });
for (const entry of fs.readdirSync(path.resolve(__dirname, '../compiled'), { withFileTypes: true })) {
  const from = path.resolve(__dirname, '../compiled', entry.name), to = path.join(compiled, entry.name);
  if (entry.isDirectory()) fs.symlinkSync(from, to, 'dir'); else fs.copyFileSync(from, to);
}
fs.appendFileSync(path.join(compiled, 'main.js'), '\nmodule.exports.review = { remoteWindows, remoteHelpers, broker };\n');
fs.mkdirSync(path.join(fixture, 'target', 'release'), { recursive: true });
fs.symlinkSync(binary, path.join(fixture, 'target', 'release', 'splitty'));
fs.writeFileSync(path.join(fixture, '.zshrc'), '# Isolated smoke shell.\n');
process.env.HOME = fixture; process.env.XDG_CONFIG_HOME = path.join(fixture, 'config');
process.env.XDG_DATA_HOME = path.join(fixture, 'data'); process.env.XDG_RUNTIME_DIR = path.join(fixture, 'run');
fs.mkdirSync(process.env.XDG_RUNTIME_DIR, { mode: 0o700 });
for (const key of Object.keys(process.env)) if (key.startsWith('SPLITTY_')) delete process.env[key];
app.setPath('userData', path.join(fixture, 'electron-data'));
const execute = promisify(execFile), delay = ms => new Promise(resolve => setTimeout(resolve, ms));
let main, finished = false;
const timeout = setTimeout(() => finish(Error('native remote smoke timed out')), 100_000);
const errors = [], networkRequests = [];
const observedSessions = new Set();
const observedNativeProcesses = new Map();
app.on('web-contents-created', (_event, contents) => {
  if (!observedSessions.has(contents.session)) {
    observedSessions.add(contents.session);
    contents.session.webRequest.onBeforeRequest({urls: ['http://*/*', 'https://*/*', 'ws://*/*', 'wss://*/*']}, (details, callback) => {
      networkRequests.push(details.url); callback({cancel: true});
    });
  }
  contents.on('console-message', details => { if (details.level === 'error') errors.push(details.message); });
});
function finish(error) {
  if (finished) return; finished = true; clearTimeout(timeout);
  if (error) { console.error(error, errors); process.exitCode = 1; }
  app.quit();
}
app.once('will-quit', () => {
  try {
    assert.deepEqual(require(path.join(compiled, 'native_budget.js')).nativeProcessBudget.counts, { data: 0, controls: 0, workers: 0 });
    assert.ok(observedNativeProcesses.size >= 4, 'inventory includes replacements as well as the first helper/SSH');
    for (const {pid, born} of observedNativeProcesses.values()) {
      const current = processIdentity(pid);
      assert.ok(!current || current.born !== born || ['Z', 'X'].includes(current.state), `observed native process ${pid} survived app shutdown`);
    }
  } catch (error) { console.error(error); process.exitCode = 1; }
  fs.rmSync(fixture, { recursive: true, force: true });
  if (process.exitCode) app.exit(process.exitCode);
});
async function until(check, label) {
  for (let i = 0; i < 600; i++) { if (await check()) return; await delay(25); }
  throw Error(`timed out: ${label}`);
}
const ui = (win, expression) => win.webContents.executeJavaScript(expression);
const metadata = () => JSON.parse(fs.readFileSync(path.join(lab, 'home/.config/splitty/daemon.json'), 'utf8'));
const keepers = () => fs.readdirSync(path.join(lab, 'home/.local/share/splitty/run/keepers'))
  .filter(name => name.endsWith('.sock')).map(name => fs.statSync(path.join(lab, 'home/.local/share/splitty/run/keepers', name)).ino).sort();
function processIdentity(pid) {
  try {
    const stat = fs.readFileSync(`/proc/${pid}/stat`, 'utf8');
    const fields = stat.slice(stat.lastIndexOf(') ') + 2).trim().split(/\s+/);
    return { state: fields[0], born: fields[19] };
  } catch (error) { if (error.code === 'ENOENT') return undefined; throw error; }
}
function inspectNativeProcess(pid) {
  const identity = processIdentity(pid); assert.ok(identity && identity.state !== 'Z', 'live native process');
  observedNativeProcesses.set(`${pid}:${identity.born}`, {pid, born: identity.born});
  const sockets = new Set();
  for (const fd of fs.readdirSync(`/proc/${pid}/fd`)) {
    let link;
    try { link = fs.readlinkSync(`/proc/${pid}/fd/${fd}`); }
    catch (error) { if (error.code === 'ENOENT') continue; throw error; }
    const socket = /^socket:\[(\d+)\]$/.exec(link); if (socket) sockets.add(socket[1]);
  }
  for (const table of ['tcp', 'tcp6']) {
    for (const line of fs.readFileSync(`/proc/${pid}/net/${table}`, 'utf8').trim().split('\n').slice(1)) {
      const fields = line.trim().split(/\s+/);
      assert.ok(fields[3] !== '0A' || !sockets.has(fields[9]), `native process ${pid} owns a TCP listener`);
    }
  }
}
function inspectActiveRemote() {
  const helper = activeHelper(); inspectNativeProcess(helper.child.pid);
  const children = new Set();
  for (const thread of fs.readdirSync(`/proc/${helper.child.pid}/task`)) {
    for (const pid of fs.readFileSync(`/proc/${helper.child.pid}/task/${thread}/children`, 'utf8').trim().split(/\s+/).filter(Boolean)) children.add(Number(pid));
  }
  const ssh = [...children].filter(pid => path.basename(fs.readFileSync(`/proc/${pid}/cmdline`, 'utf8').split('\0')[0]) === 'ssh');
  assert.equal(ssh.length, 1, 'one actual SSH bridge per active root helper');
  for (const pid of ssh) inspectNativeProcess(pid);
  return ssh[0];
}
function activeHelper() {
  const active = [...review.remoteHelpers].filter(helper => helper.pipe.stage === 'active');
  assert.equal(active.length, 1, 'one active application stream after startup reads');
  return active[0];
}
app.on('browser-window-created', (_event, win) => {
  if (main) { win.hide(); return; }
  main = win; win.hide();
  void (async () => {
    await new Promise(resolve => win.webContents.once('did-finish-load', resolve));
    await until(() => ui(main, '!!document.querySelector(".xterm-helper-textarea")'), 'local workspace');
    const openHost = () => ui(main, `window.__SPLITTY_ELECTRON__.invoke('remote:open', ${JSON.stringify(target)})`);
    const unknownKey = await ui(main, `window.__SPLITTY_ELECTRON__.invoke('remote:open', ${JSON.stringify(newKey)})`);
    assert.equal(unknownKey.ok, false);
    assert.ok(unknownKey.error.hint.includes(`ssh ${newKey}`));
    assert.ok(unknownKey.error.hint.includes('fingerprint'));
    assert.equal(review.remoteWindows.size, 0, 'failed host admission creates no window');
    const results = await Promise.all([openHost(), openHost()]);
    assert.deepEqual(results, [{ ok: true }, { ok: true }]);
    assert.equal(review.remoteWindows.size, 1);
    const localConfig = path.join(process.env.XDG_CONFIG_HOME, 'splitty');
    const recent = () => JSON.parse(fs.readFileSync(path.join(localConfig, 'recent-hosts.json'), 'utf8'));
    assert.ok(recent().hosts.some(host => host.target === target), 'explicit open records the target');
    assert.ok(!recent().hosts.some(host => host.target === newKey), 'failed admission is not recorded');
    // Simulate another local window forgetting the host while this one remains
    // connected, without depending on the sidebar layout.
    fs.writeFileSync(path.join(localConfig, 'remote-host-preferences.json'), JSON.stringify({version: 1, hosts: [{target, hidden: true}]}), {mode: 0o600});
    const prefs = () => JSON.parse(fs.readFileSync(path.join(localConfig, 'remote-host-preferences.json'), 'utf8'));
    const entry = review.remoteWindows.get(target), remote = entry.win;
    await until(() => ui(remote, '!!document.querySelector(".xterm-helper-textarea") || Array.from(document.querySelectorAll("button")).some(button => button.textContent.includes("new terminal"))'), 'remote workspace');
    if (!await ui(remote, '!!document.querySelector(".xterm-helper-textarea")')) {
      await ui(remote, 'Array.from(document.querySelectorAll("button")).find(button => button.textContent.includes("new terminal")).click(); true');
    }
    try { await until(() => ui(remote, '!!document.querySelector(".xterm-helper-textarea")'), 'remote terminal'); }
    catch (error) { throw Error(`${error.message}: ${await ui(remote, 'document.body.innerText')}`); }
    assert.equal(await ui(remote, 'Array.from(document.querySelectorAll("button")).some(button => button.textContent.trim() === "Hosts" || button.getAttribute("aria-label") === "New screen on a host…")'), false, 'assigned remote UI offers no other-host entry points');
    const typeCommand = async command => {
      await ui(remote, 'document.querySelector(".xterm-helper-textarea").focus(); true');
      await remote.webContents.insertText(command);
      remote.webContents.sendInputEvent({type: 'keyDown', keyCode: 'Return'});
      remote.webContents.sendInputEvent({type: 'keyUp', keyCode: 'Return'});
    };
    await until(() => ui(remote, '!!document.querySelector(".splitty-scratchpad")'), 'initial session attach');
    await typeCommand('touch "$HOME/.splitty-initial-input"');
    try { await until(() => fs.existsSync(path.join(lab, 'home/.splitty-initial-input')), 'initial terminal roundtrip'); }
    catch (error) { throw Error(`${error.message}: ${await ui(remote, 'document.body.innerText')}`); }
    const identity = await ui(remote, 'window.smokeDocument = crypto.randomUUID()');
    let navigations = 0; remote.webContents.on('did-start-navigation', details => { if (details.isMainFrame && !details.isSameDocument) navigations++; });
    const before = metadata(), preserved = keepers();
    assert.ok(preserved.length > 0);
    await until(() => [...review.remoteHelpers].filter(helper => helper.pipe.stage === 'active').length === 1, 'single root helper');
    await ui(remote, 'window.smokeTerminal = document.querySelector(".xterm-helper-textarea"); true');
    const helper = activeHelper();
    const ssh = inspectActiveRemote(); process.kill(ssh, 'SIGKILL');
    await until(() => entry.state === 'reconnecting', 'SSH loss presentation');
    assert.ok(remote.getTitle().includes('(reconnecting…)'));
    await typeCommand('touch "$HOME/.splitty-offline-input"');
    await until(() => entry.state === 'connected', 'SSH recovery');
    assert.equal(metadata().instance, before.instance); assert.deepEqual(keepers(), preserved);
    assert.notEqual(activeHelper(), helper);
    inspectActiveRemote();
    await until(() => ui(remote, '!!document.querySelector(".splitty-scratchpad") && document.querySelector(".xterm-helper-textarea") !== window.smokeTerminal'), 'restored session attach');
    await typeCommand('touch "$HOME/.splitty-online-input"');
    await until(() => fs.existsSync(path.join(lab, 'home/.splitty-online-input')), 'connected terminal input');
    assert.equal(fs.existsSync(path.join(lab, 'home/.splitty-offline-input')), false, 'offline input is never replayed');
    assert.equal(prefs().hosts.find(host => host.target === target).hidden, true, 'automatic recovery preserves hidden hosts');

    await ui(remote, 'window.smokeTerminal = document.querySelector(".xterm-helper-textarea"); true');
    await execute('ssh', ['-o', 'BatchMode=yes', target, '--', 'splitty', 'daemon', 'restart'], { timeout: 30_000 });
    await until(() => metadata().instance !== before.instance && entry.state === 'connected', 'new daemon instance');
    await until(() => ui(remote, '!!document.querySelector(".splitty-scratchpad") && document.querySelector(".xterm-helper-textarea") !== window.smokeTerminal'), 'new daemon session attach');
    inspectActiveRemote();
    await typeCommand('touch "$HOME/.splitty-restarted-input"');
    await until(() => fs.existsSync(path.join(lab, 'home/.splitty-restarted-input')), 'terminal input after daemon restart');
    assert.deepEqual(keepers(), preserved);
    // A permanent transport error must stay disconnected until an explicit
    // main-window open requests a retry through the current private preload.
    activeHelper().pipe.close({ code: 'compatibility', retryable: false, message: 'Fixture permanent connection failure' });
    await until(() => entry.state === 'disconnected', 'permanent loss');
    assert.ok(remote.getTitle().includes('(disconnected)'));
    assert.deepEqual(await Promise.all([openHost(), openHost()]), [{ok: true}, {ok: true}]);
    assert.equal(entry.state, 'connected', 'open resolves only after the host is connected');
    inspectActiveRemote();
    assert.notEqual(prefs().hosts.find(host => host.target === target).hidden, true, 'deliberate retry reveals its target');
    assert.equal(review.remoteWindows.get(target).win, remote);
    assert.equal(await ui(remote, 'window.smokeDocument'), identity); assert.equal(navigations, 0);
    assert.deepEqual(keepers(), preserved);
    remote.destroy();
    await until(() => review.remoteHelpers.size === 0, 'remote helpers observed exited');
    assert.equal(review.remoteWindows.size, 0); assert.deepEqual(keepers(), preserved);
    assert.ok(metadata().instance);
    assert.deepEqual(networkRequests, [], 'desktop transport never makes HTTP or WebSocket requests');
    console.log(JSON.stringify({ sshReconnect: true, daemonRestart: true, fixedDocument: true,
      coalescedOpen: true, noInputReplay: true, noHttp: true, nativeListenerInventory: true, hostKeyHint: true, recentHosts: true, nativeTitles: true, explicitSameWindowRetry: true, keepersSurviveClosure: true }));
    finish();
  })().catch(finish);
});
const { review } = require(path.join(compiled, 'main.js'));
