// Exercise the production entry point, packaged frontend and real Rust helper.
// A private package layout selects the debug executable without a production
// environment override or test-only branch in the launcher.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { app, BrowserWindow, ipcMain, shell, session } = require('electron');
const revealed = [];
shell.showItemInFolder = filename => { revealed.push(filename); };
const external = [];
shell.openExternal = async url => { external.push(url); };
const children = [];
const processes = require('node:child_process'), spawn = processes.spawn;
processes.spawn = (...args) => { const child = spawn(...args); children.push({ child, args }); return child; };
const fixture = fs.mkdtempSync(path.join(os.tmpdir(), 'splitty-native-app-'));
const compiled = path.join(fixture, 'electron', 'compiled');
fs.mkdirSync(compiled, { recursive: true });
for (const entry of fs.readdirSync(path.resolve(__dirname, '../compiled'), { withFileTypes: true })) {
  const source = path.resolve(__dirname, '../compiled', entry.name), target = path.join(compiled, entry.name);
  if (entry.isDirectory()) fs.symlinkSync(source, target, 'dir'); else fs.copyFileSync(source, target);
}
// Expose only native registration hooks in this private test copy, so real
// isolated remote WebContents can exercise production guards without SSH.
fs.appendFileSync(path.join(compiled, 'main.js'), '\nmodule.exports.review = { policies, broker, guardPermissions };\n');
fs.mkdirSync(path.join(fixture, 'target', 'release'), { recursive: true });
fs.symlinkSync(path.resolve(process.argv[2] || '../target/debug/splitty'), path.join(fixture, 'target', 'release', 'splitty'));
fs.writeFileSync(path.join(fixture, ".zshrc"), "# Isolated smoke shell.\n");
process.env.HOME = fixture;
process.env.XDG_CONFIG_HOME = path.join(fixture, 'config');
process.env.XDG_DATA_HOME = path.join(fixture, 'data');
process.env.XDG_RUNTIME_DIR = path.join(fixture, 'run');
fs.mkdirSync(process.env.XDG_RUNTIME_DIR, { mode: 0o700 });
for (const name of Object.keys(process.env)) if (name.startsWith('SPLITTY_')) delete process.env[name];
app.setPath('userData', path.join(fixture, 'electron-data'));
let finished = false, main, opens = 0;
const errors = [], requests = [];
ipcMain.on('native:open', () => opens++);
app.on('web-contents-created', (_event, contents) => {
  contents.on('console-message', details => { if (details.level === 'error' || details.level === 'warning') errors.push(details.message); });
});
const timeout = setTimeout(() => { console.error('native app smoke timed out', { errors, opens }); fail(Error('timeout')); }, 40_000);
function fail(error) {
  console.error(error); process.exitCode = 1;
  finish(false);
}
function finish(ok) {
  if (finished) return; finished = true; clearTimeout(timeout);
  if (ok) console.log(JSON.stringify({ productionEntry: true, packagedFrontend: true, nativeStartupReads: true,
    terminalSession: true, documentOwnedDownload: true, parentOwnedPopout: true, remoteIsolation: true,
    assignedDocumentNavigation: true, externalHandoff: true, reload: true, noHttpRequests: true, opens }));
  app.quit();
}
app.once('will-quit', () => {
  try {
    assert.ok(children.length >= 1);
    for (const { child } of children) assert.ok(child.exitCode !== null || child.signalCode !== null, 'owned helper must exit before the app quits');
    assert.deepEqual(require(path.join(compiled, 'native_budget.js')).nativeProcessBudget.counts, { data: 0, controls: 0, workers: 0 });
  } catch (error) { console.error(error); process.exitCode = 1; }
  fs.rmSync(fixture, { recursive: true, force: true });
  if (process.exitCode) app.exit(process.exitCode);
});
function assertNoHelperListener() {
  // Actual Linux socket inventory; API metadata alone cannot prove no listener.
  if (process.platform !== 'linux') return;
  const listeners = new Set();
  for (const protocol of ['tcp', 'tcp6']) {
    for (const line of fs.readFileSync(`/proc/net/${protocol}`, 'utf8').trim().split('\n').slice(1)) {
      const fields = line.trim().split(/\s+/); if (fields[3] === '0A') listeners.add(fields[9]);
    }
  }
  for (const { child, args } of children) {
    assert.equal(args[1][0], 'desktop-backend');
    if (!child.pid || child.exitCode !== null || child.signalCode !== null) continue;
    for (const fd of fs.readdirSync(`/proc/${child.pid}/fd`)) {
      let link; try { link = fs.readlinkSync(`/proc/${child.pid}/fd/${fd}`); } catch { continue; }
      const socket = /^socket:\[(\d+)\]$/.exec(link);
      assert.ok(!socket || !listeners.has(socket[1]), 'native helper must have no TCP listener');
    }
  }
}
app.on('browser-window-created', (_event, win) => {
  if (main) return;
  main = win; win.hide();
  win.webContents.session.webRequest.onBeforeRequest((details, callback) => {
    requests.push(details.url); callback({});
  });
  void (async () => {
    const waitFor = async predicate => {
      for (let i = 0; i < 200; i++) {
        if (win.isDestroyed()) throw Error('Window closed during startup');
        if (await win.webContents.executeJavaScript(predicate)) return;
        await new Promise(resolve => setTimeout(resolve, 50));
      }
      throw Error(`UI did not become ready: ${await win.webContents.executeJavaScript('document.body.innerText')}\n${errors.join('\n')}`);
    };
    await new Promise(resolve => win.webContents.once('did-finish-load', resolve));
    await waitFor('!!document.querySelector(".xterm-helper-textarea")');
    assert.equal(win.webContents.getURL(), 'splitty://app/index.html');
    await win.webContents.executeJavaScript('document.querySelector(".xterm-helper-textarea").focus(); true');
    await win.webContents.insertText("printf 'SPLITTY_%s_OK\\n' NATIVE_APP");
    win.webContents.sendInputEvent({ type: 'keyDown', keyCode: 'Return' });
    win.webContents.sendInputEvent({ type: 'keyUp', keyCode: 'Return' });
    await waitFor('document.body.innerText.includes("SPLITTY_NATIVE_APP_OK")');
    const beforePopout = opens;
    const created = new Promise(resolve => win.webContents.once('did-create-window', popup => { popup.hide(); resolve(popup); }));
    assert.equal(await win.webContents.executeJavaScript(`!!window.open('/popout.html', 'native-smoke-popout')`, true), true);
    const popup = await created;
    if (popup.webContents.isLoading()) await new Promise(resolve => popup.webContents.once('did-finish-load', resolve));
    const popout = await popup.webContents.executeJavaScript(`({ origin: location.origin, parent: !!opener, bridge: typeof window.__SPLITTY_ELECTRON__, body: document.body.innerText })`);
    assert.equal(popout.origin, 'splitty://app'); assert.equal(popout.parent, true);
    assert.equal(popout.bridge, 'undefined'); assert.equal(popout.body, '');
    assert.equal(opens, beforePopout, 'parent-owned view must not open an independent service client');
    const mediated = await popup.webContents.executeJavaScript(`opener.__SPLITTY_ELECTRON__.invoke('window:is-maximized').then(value=>typeof value)`);
    assert.equal(mediated, 'boolean'); // Explicit shared-parent trust, no containment claim.
    await checkRemoteWindows(win, popup);
    popup.destroy();
    const downloadPath = path.join(fixture, 'download.txt');
    const saved = new Promise((resolve, reject) => win.webContents.session.once('will-download', (_event, item) => {
      item.setSavePath(downloadPath);
      item.once('done', (_event, state) => state === 'completed' ? resolve() : reject(Error(`download ${state}`)));
    }));
    const blobUrl = await win.webContents.executeJavaScript(`(() => {
      const url = URL.createObjectURL(new Blob(['native download']));
      const link = document.createElement('a'); link.href = url; link.download = 'download.txt';
      document.body.appendChild(link); link.click(); link.remove(); return url;
    })()`, true);
    await saved; assert.equal(fs.readFileSync(downloadPath, 'utf8'), 'native download');
    await win.webContents.executeJavaScript(`window.__SPLITTY_ELECTRON__.invoke('download:reveal', ${JSON.stringify(blobUrl)})`);
    assert.deepEqual(revealed, [downloadPath]);
    const before = opens;
    await win.loadURL('splitty://app/index.html?host=forged&desktop=1');
    await waitFor('!!document.querySelector(".xterm-helper-textarea")');
    const staleDownload = await win.webContents.executeJavaScript(`window.__SPLITTY_ELECTRON__.invoke('download:reveal', ${JSON.stringify(blobUrl)}).then(()=>true,()=>false)`);
    assert.equal(staleDownload, false); assert.equal(revealed.length, 1);
    assert.ok(opens >= before + 2, 'new document gets fresh startup and application clients');
    review.broker.revoke(win.webContents);
    await waitFor('Array.from(document.querySelectorAll("[role=alert] button")).some(button => button.textContent === "Reload")');
    const reloaded = new Promise(resolve => win.webContents.once('did-finish-load', resolve));
    await win.webContents.executeJavaScript('Array.from(document.querySelectorAll("[role=alert] button")).find(button => button.textContent === "Reload").click(); true');
    await reloaded; await waitFor('!!document.querySelector(".xterm-helper-textarea")');
    assert.equal(requests.some(url => /^(https?|wss?):/.test(url)), false, JSON.stringify(requests));
    assertNoHelperListener();
    finish(true);
  })().catch(fail);
});
const { review } = require(path.join(compiled, 'main.js'));

async function checkRemoteWindows(main, popout) {
  const { installAssets } = require(path.join(compiled, 'asset_protocol.js'));
  const remotes = [];
  try {
    await main.webContents.executeJavaScript("window.name = 'named-main'; localStorage.setItem('isolation-probe', 'main'); true");
    for (let i = 0; i < 2; i++) {
      const isolated = session.fromPartition(`window-policy-smoke-${i}`);
      assert.equal(isolated.isPersistent(), false);
      installAssets(isolated, path.join(compiled, 'ui'));
      review.guardPermissions(isolated, () => 'splitty://app');
      // Only this policy probe suppresses application scripts; the parent
      // smoke already runs production startup/settings/terminal code.
      isolated.webRequest.onBeforeRequest((details, callback) => callback({ cancel: details.resourceType === 'script' }));
      const win = new BrowserWindow({ show: false, webPreferences: { session: isolated,
        preload: path.join(compiled, 'preload.js'), sandbox: true, contextIsolation: true, nodeIntegration: false } });
      remotes.push(win);
      review.policies.set(win.webContents, { role: 'remote', origin: 'splitty://app' });
      review.broker.grant(win.webContents, { role: 'remote', target: `192.0.2.${i + 1}`, open() { throw Error('This probe must not start a helper'); } });
      await win.loadURL('splitty://app/index.html');
      assert.deepEqual(await win.webContents.executeJavaScript(`({ opener: opener === null, value: localStorage.getItem('isolation-probe'), node: typeof require })`),
        { opener: true, value: null, node: 'undefined' });
      await win.webContents.executeJavaScript(`localStorage.setItem('isolation-probe', 'remote-${i}'); window.name = 'named-remote-${i}'; true`);
      const allowed = await win.webContents.executeJavaScript(`Promise.all([
        ['window:is-maximized'], ['remote:hosts'], ['editor:open-local', { editor: 'vscode', cwd: '/tmp' }],
        ['editor:open-remote', { editor: 'zed', cwd: '/tmp', target: '192.0.2.9' }], ['download:reveal', 'blob:splitty://app/forged']
      ].map(args => window.__SPLITTY_ELECTRON__.invoke(...args).then(() => true, () => false)))`);
      assert.deepEqual(allowed, [false, false, false, false, false]);
      for (const name of ['named-main', `named-remote-${1 - i}`, 'native-smoke-popout']) {
        assert.equal(await win.webContents.executeJavaScript(`!!window.open('splitty://app/index.html', ${JSON.stringify(name)})`), false);
      }
      assert.equal(await win.webContents.executeJavaScript("!!window.open('/popout.html', '_blank')"), false);
      assert.equal(await win.webContents.executeJavaScript("navigator.clipboard.readText().then(() => true, () => false)"), false);
      // No native window is created, and only HTTP(S) reaches the OS opener.
      const before = BrowserWindow.getAllWindows().length, handed = external.length;
      await win.webContents.executeJavaScript(`window.open('https://example.com/docs', '_blank', 'noopener,noreferrer');
        window.open('vscode://file/tmp/private'); window.open('file:///tmp/private'); true`);
      assert.equal(BrowserWindow.getAllWindows().length, before);
      assert.deepEqual(external.slice(handed), ['https://example.com/docs']);
      for (const url of ['splitty://app/setup.html', 'splitty://app/popout.html', 'splitty://app/assets/main.js']) {
        await win.webContents.executeJavaScript(`location.href = ${JSON.stringify(url)}; true`);
        await new Promise(resolve => setTimeout(resolve, 30));
        assert.equal(win.webContents.getURL(), 'splitty://app/index.html');
      }
    }
    assert.equal(await main.webContents.executeJavaScript("localStorage.getItem('isolation-probe')"), 'main');
    assert.equal(await remotes[0].webContents.executeJavaScript("localStorage.getItem('isolation-probe')"), 'remote-0');
    assert.equal(await main.webContents.executeJavaScript("!!window.open('splitty://app/index.html', 'named-remote-0')"), false);
    await main.webContents.executeJavaScript("location.href = 'splitty://app/setup.html'; true");
    await new Promise(resolve => setTimeout(resolve, 30));
    assert.equal(main.webContents.getURL(), 'splitty://app/index.html');
    assert.equal(await main.webContents.executeJavaScript("window.__SPLITTY_ELECTRON__.invoke('window:is-maximized').then(value => typeof value)"), 'boolean',
      'a denied navigation must leave the current document usable');
    await popout.webContents.executeJavaScript("location.href = 'splitty://app/index.html'; true");
    await new Promise(resolve => setTimeout(resolve, 30));
    assert.equal(popout.webContents.getURL(), 'splitty://app/popout.html');
  } finally { for (const win of remotes) win.destroy(); }
}
