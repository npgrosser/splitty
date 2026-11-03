// Real Chromium document startup/auth/reconnect against an owned browser
// gateway. No desktop preload or user daemon participates in this test.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawn } = require('node:child_process');
const { once } = require('node:events');
const { app, BrowserWindow } = require('electron');

const fixture = fs.mkdtempSync(path.join(os.tmpdir(), 'splitty-browser-assignment-'));
const env = { ...process.env, HOME: fixture, XDG_CONFIG_HOME: path.join(fixture, 'config'),
  XDG_DATA_HOME: path.join(fixture, 'data'), XDG_RUNTIME_DIR: path.join(fixture, 'run'), SHELL: '/bin/sh' };
for (const key of Object.keys(env)) if (key.startsWith('SPLITTY_')) delete env[key];
fs.mkdirSync(env.XDG_RUNTIME_DIR, { mode: 0o700 });
fs.writeFileSync(path.join(fixture, '.zshrc'), '# Isolated smoke shell.\n');
app.setPath('userData', path.join(fixture, 'chromium'));
const binary = path.resolve(process.argv[2] || '../target/debug/splitty');
let server, startup;
const windows = [];
const errors = [];
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const deadline = setTimeout(() => finish(Error('browser assignment smoke timed out')), 50_000);
let finished = false;

async function stopServer() {
  if (!server || server.exitCode !== null || server.signalCode !== null) return;
  const child = server, exited = once(child, 'exit'); child.kill('SIGTERM');
  const force = setTimeout(() => child.kill('SIGKILL'), 5_000);
  try { await exited; } finally { clearTimeout(force); }
}
async function startServer(port = 0) {
  server = spawn(binary, ['serve', '--addr', `127.0.0.1:${port}`], { env, stdio: ['ignore', 'pipe', 'pipe'] });
  server.stderr.on('data', () => {});
  startup = await new Promise((resolve, reject) => {
    let text = '';
    const timer = setTimeout(() => reject(Error('gateway startup timed out')), 10_000);
    server.once('error', reject);
    server.once('exit', () => { clearTimeout(timer); reject(Error('gateway exited before ready')); });
    server.stdout.on('data', bytes => {
      text = (text + bytes).slice(-8192);
      const port = /SPLITTY_PORT=(\d+)/.exec(text)?.[1];
      const bootstrap = /SPLITTY_BOOTSTRAP=([a-f0-9]+)/.exec(text)?.[1];
      if (port && bootstrap) { clearTimeout(timer); resolve({ port, bootstrap }); }
    });
  });
  return `http://127.0.0.1:${startup.port}/`;
}
function window() {
  const win = new BrowserWindow({ show: false, webPreferences: { nodeIntegration: false,
    contextIsolation: true, sandbox: true, backgroundThrottling: false } });
  windows.push(win);
  win.webContents.on('console-message', details => { if (details.level === 'error') errors.push(details.message); });
  return win;
}
async function waitFor(win, expression) {
  for (let i = 0; i < 200; i++) {
    if (await win.webContents.executeJavaScript(expression)) return;
    await delay(50);
  }
  throw Error(`browser UI did not become ready: ${await win.webContents.executeJavaScript('document.body.innerText')}`);
}
async function submit(win, token) {
  await win.webContents.executeJavaScript(`(() => {
    const input = document.querySelector('.dialog-input'); input.value = ${JSON.stringify(token)};
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.form.requestSubmit();
  })()`);
}
async function finish(error) {
  if (finished) return; finished = true; clearTimeout(deadline);
  if (error) console.error(error, errors);
  for (const win of windows) if (!win.isDestroyed()) win.destroy();
  await stopServer(); fs.rmSync(fixture, { recursive: true, force: true });
  app.exit(error ? 1 : 0);
}
app.on('window-all-closed', () => {});
app.whenReady().then(async () => {
  const origin = await startServer();
  const first = window();
  await first.loadURL(`${origin}?host=forged&role=remote`);
  await waitFor(first, "!!document.querySelector('.dialog-input')");
  assert.equal(await first.webContents.executeJavaScript("!!document.querySelector('.xterm-helper-textarea')"), false);
  await submit(first, 'wrong'); await waitFor(first, "!!document.querySelector('.dialog-error')");
  const token = fs.readFileSync(path.join(env.XDG_CONFIG_HOME, 'splitty', 'auth-token'), 'utf8').trim();
  await submit(first, token); await waitFor(first, "!!document.querySelector('.xterm-helper-textarea')");
  const oldId = await first.webContents.executeJavaScript("fetch('/api/client-assignment').then(r => r.json()).then(a => a.gateway)");

  // Suspend this document's JS so a second tab authenticates the replacement
  // before its reconnect callback runs. The old assignment must still fail.
  first.webContents.debugger.attach('1.3');
  await first.webContents.debugger.sendCommand('Debugger.enable');
  await first.webContents.debugger.sendCommand('Debugger.pause');
  const port = startup.port;
  await stopServer(); await startServer(port);
  const second = window();
  await second.loadURL(`${origin}?bootstrap=${startup.bootstrap}`);
  await waitFor(second, "!!document.querySelector('.xterm-helper-textarea')");
  assert.ok(!second.webContents.getURL().includes('bootstrap='));
  const newId = await second.webContents.executeJavaScript("fetch('/api/client-assignment').then(r => r.json()).then(a => a.gateway)");
  assert.notEqual(oldId, newId);
  let reloaded = false;
  first.webContents.once('did-start-navigation', (_event, _url, _inPlace, mainFrame) => { if (mainFrame) reloaded = true; });
  await first.webContents.debugger.sendCommand('Debugger.resume');
  first.webContents.debugger.detach();
  for (let i = 0; i < 200 && !reloaded; i++) await delay(50);
  assert.ok(reloaded, 'the old tab must reload instead of retaining its gateway assignment');
  await waitFor(first, "!!document.querySelector('.xterm-helper-textarea')");
  console.log(JSON.stringify({ manualLogin: true, invalidToken: true, bootstrapLogin: true,
    workspaceAfterAssignment: true, otherTabReauthRevokesOldDocument: true, nativePreload: false }));
  await finish();
}).catch(finish);
