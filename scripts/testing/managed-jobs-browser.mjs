#!/usr/bin/env node
// Owns a disposable backend, HOME and fresh browser context. Never connects to a user daemon.
import { spawn, execFile } from 'node:child_process';
import { mkdtemp, mkdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { promisify } from 'node:util';
import assert from 'node:assert/strict';

const args = Object.fromEntries(process.argv.slice(2).reduce((pairs, word, i, all) => i % 2 ? pairs : [...pairs, [word, all[i + 1]]], []));
for (const key of ['--binary', '--playwright-module', '--browser']) assert(args[key], `missing ${key}`);
const binary = resolve(args['--binary']);
const { chromium } = await import(pathToFileURL(resolve(args['--playwright-module'])).href);
const home = await mkdtemp(join(tmpdir(), 'splitty-jobs-browser-'));
const env = { ...process.env, HOME: home, XDG_CONFIG_HOME: join(home, 'config'), XDG_DATA_HOME: join(home, 'data'), XDG_RUNTIME_DIR: join(home, 'run'), SHELL: '/bin/sh' };
for (const key of Object.keys(env)) if (key.startsWith('SPLITTY_')) delete env[key];
await mkdir(env.XDG_RUNTIME_DIR, { recursive: true, mode: 0o700 });
const server = spawn(binary, ['serve', '--addr', '127.0.0.1:0'], { env, stdio: ['ignore', 'pipe', 'pipe'] });
let diagnostic = '';
server.stderr.on('data', data => { diagnostic = (diagnostic + data).slice(-4000); });
let browser;
const execute = promisify(execFile);
const ownerEnv = { ...env, SPLITTY_SOCK: join(env.XDG_RUNTIME_DIR, 'splitty', `${server.pid}.sock`) };
async function cli(words) {
  const { stdout } = await execute(binary, words, { env: ownerEnv, timeout: 35000, maxBuffer: 2 * 1024 * 1024 });
  return JSON.parse(stdout);
}
let daemonRunning = false;
async function daemonCli(words) {
  const { stdout } = await execute(binary, words, { env, timeout: 35000, maxBuffer: 2 * 1024 * 1024 });
  return JSON.parse(stdout);
}
try {
  const startup = await new Promise((accept, reject) => {
    let text = ''; const timer = setTimeout(() => reject(new Error('backend startup timeout')), 15000);
    server.once('exit', () => { clearTimeout(timer); reject(new Error('backend exited before startup')); });
    server.stdout.on('data', data => {
      text += data;
      const port = /SPLITTY_PORT=(\d+)/.exec(text)?.[1];
      const bootstrap = /SPLITTY_BOOTSTRAP=([a-f0-9]+)/.exec(text)?.[1];
      if (port && bootstrap) { clearTimeout(timer); accept({ port, bootstrap }); }
    });
  });
  const user = await cli(['run', '--name', 'User', '--cwd', home, '--json', '--', '/bin/sh']);
  const agent = await cli(['agent', 'create', '--name', 'workflow', '--screen', user.screenId, '--pane', user.paneId, '--json']);
  async function agentCli(words, socket = agent.socket) {
    const { stdout } = await execute(binary, words, { env: { ...env, SPLITTY_AGENT_SOCK: socket }, timeout: 35000 });
    return JSON.parse(stdout);
  }
  if (args['--claude']) {
    const grant = await cli(['agent', 'create', '--name', 'claude-check', '--screen', user.screenId, '--pane', user.paneId, '--json']);
    const { stdout } = await execute(binary, ['agent', 'exec', grant.agentId, '--', resolve(args['--claude']), '--version'], { env: ownerEnv, timeout: 15000 });
    assert(stdout.includes('Claude Code'), 'Claude version invocation failed');
    console.log(`PASS: scoped launcher (${stdout.trim()})`);
  }
  browser = await chromium.launch({ executablePath: resolve(args['--browser']), headless: true, chromiumSandbox: true });
  const context = await browser.newContext({ viewport: { width: 1280, height: 850 } });
  const page = await context.newPage();
  const received = new Map();
  page.on('websocket', socket => socket.on('framereceived', ({ payload }) => {
    if (typeof payload === 'string' || !payload.length) return;
    const length = payload[0];
    const id = payload.subarray(1, 1 + length).toString();
    received.set(id, ((received.get(id) ?? '') + payload.subarray(1 + length).toString()).slice(-262144));
  }));
  const failures = [];
  page.on('pageerror', error => failures.push(error.message));
  await page.goto(`http://127.0.0.1:${startup.port}/?bootstrap=${startup.bootstrap}`);
  const userPane = page.locator(`[data-pane-id="${user.paneId}"]`);
  await userPane.locator('.xterm-screen').waitFor();
  await userPane.locator('.xterm-screen').click();
  const jobs = [];
  for (const placement of ['tab', 'left', 'right', 'above', 'below', 'screen']) {
    const placementArgs = ['left', 'right', 'above', 'below'].includes(placement) ? ['--split', placement] : [placement === 'tab' ? '--tab' : '--new-screen'];
    if (placement !== 'screen') placementArgs.push('--screen', user.screenId, '--pane', user.paneId);
    const job = await agentCli(['run', ...placementArgs, '--background', '--name', `Backend-${placement}`, '--cwd', home, '--json', '--', '/bin/sh', '-c', 'printf SHARED_OUTPUT; while read line; do printf "GOT:%s\\n" "$line"; done']);
    jobs.push(job);
    await page.locator(`[data-pane-id="${job.paneId}"]`).waitFor({ state: 'attached' });
    if (['left', 'right', 'above', 'below'].includes(placement)) {
      await page.waitForFunction(({ pane, anchor, direction }) => {
        const a = document.querySelector(`[data-pane-id="${anchor}"]`)?.getBoundingClientRect();
        const b = document.querySelector(`[data-pane-id="${pane}"]`)?.getBoundingClientRect();
        if (!a?.width || !b?.width) return false;
        return direction === 'left' ? b.x + b.width / 2 < a.x + a.width / 2
          : direction === 'right' ? b.x + b.width / 2 > a.x + a.width / 2
          : direction === 'above' ? b.y + b.height / 2 < a.y + a.height / 2
          : b.y + b.height / 2 > a.y + a.height / 2;
      }, { pane: job.paneId, anchor: user.paneId, direction: placement });
    }
    await page.waitForFunction(pane => document.activeElement?.closest('[data-pane-id]')?.getAttribute('data-pane-id') === pane, user.paneId);
    const marker = `USER_${placement}_ONLY`;
    await page.keyboard.type(`printf '${marker}\\n'`);
    await page.keyboard.press('Enter');
    const deadline = Date.now() + 5000;
    let output;
    do { output = await cli(['logs', user.jobId, '--json']); } while (!output.data.includes(marker) && Date.now() < deadline);
    assert(output.data.includes(marker), 'keyboard input did not reach original pane');
    assert(!(await cli(['logs', job.jobId, '--json'])).data.includes(marker), 'keyboard input leaked to new pane');
    assert((await cli(['logs', job.jobId, '--wait', '5s', '--json'])).data.includes('SHARED_OUTPUT'));
  }
  const latest = cli(['run', '--tab', '--screen', user.screenId, '--pane', user.paneId, '--name', 'Focus-race', '--cwd', home, '--json', '--', '/bin/sh', '-c', 'read line']);
  await page.locator(`[data-pane-id="${jobs[1].paneId}"] .xterm-screen`).click();
  const late = await latest;
  await page.locator(`[data-pane-id="${late.paneId}"]`).waitFor({ state: 'attached' });
  await page.waitForFunction(pane => document.activeElement?.closest('[data-pane-id]')?.getAttribute('data-pane-id') === pane, jobs[1].paneId);
  await page.keyboard.type('LATEST_FOCUS'); await page.keyboard.press('Enter');
  const focusDeadline = Date.now() + 5000;
  while (!(await cli(['logs', jobs[1].jobId, '--json'])).data.includes('LATEST_FOCUS')) {
    assert(Date.now() < focusDeadline, 'newer user focus was overridden');
  }
  const resumed = await cli(['agent', 'create', '--name', 'resumed', '--json']);
  await cli(['agent', 'assign', resumed.agentId, '--job', jobs[0].jobId, '--json']);
  await cli(['agent', 'revoke', agent.agentId, '--json']);
  let denied = false;
  try { await agentCli(['inspect', jobs[0].jobId, '--json']); } catch { denied = true; }
  assert(denied, 'revoked client still has access');
  assert.equal((await agentCli(['inspect', jobs[0].jobId, '--json'], resumed.socket)).sessionId, jobs[0].sessionId);
  await page.locator('.dv-tab').filter({ hasText: 'Backend-tab' }).first().click();
  const outputDeadline = Date.now() + 5000;
  while (!(received.get(jobs[0].sessionId) ?? '').includes('SHARED_OUTPUT') && Date.now() < outputDeadline) {
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  assert((received.get(jobs[0].sessionId) ?? '').includes('SHARED_OUTPUT'), 'UI did not receive shared output');
  // xterm renders to a GPU canvas, not DOM text. Keep an optional visual artifact
  // for inspecting the actual rendered marker, in addition to the wire assertion.
  if (args['--evidence-dir']) {
    await mkdir(resolve(args['--evidence-dir']), { recursive: true, mode: 0o700 });
    await page.locator(`[data-pane-id="${jobs[0].paneId}"]`).screenshot({ path: join(resolve(args['--evidence-dir']), 'shared-output.png') });
  }
  await cli(['stop', jobs[0].jobId, '--json']);
  assert.equal(await page.locator(`[data-pane-id="${jobs[0].paneId}"]`).count(), 1, 'ended job disappeared');
  let failed;
  try { await cli(['run', '--name', 'Failed', '--cwd', home, '--json', '--', '/not/a/splitty-test-executable']); }
  catch (error) { failed = JSON.parse(error.stdout); }
  assert.equal(failed.code, 'spawn_failed');
  await page.reload();
  await page.locator(`[data-pane-id="${failed.paneId}"]`).waitFor({ state: 'attached' });
  assert((await page.locator(`[data-pane-id="${failed.paneId}"]`).textContent()).includes('failed'));
  assert.equal((await cli(['inspect', failed.jobId, '--json'])).state, 'failed');
  assert.deepEqual(failures, []);
  await context.close();

  // A second owned backend covers a real interrupted reload, not a mocked state.
  await execute(binary, ['daemon', 'start', '--addr', '127.0.0.1:0'], { env, timeout: 15000 });
  daemonRunning = true;
  const interrupted = await daemonCli(['run', '--name', 'Interrupted', '--cwd', home, '--json', '--', '/bin/sh', '-c', 'printf ONCE; read line']);
  assert((await daemonCli(['logs', interrupted.jobId, '--wait', '5s', '--json'])).data.includes('ONCE'));
  await execute(binary, ['daemon', 'stop'], { env, timeout: 15000 });
  const stopDeadline = Date.now() + 15000;
  while ((await daemonCli(['daemon', 'status', '--json'])).running) {
    assert(Date.now() < stopDeadline, 'daemon did not stop');
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  await execute(binary, ['daemon', 'start', '--addr', '127.0.0.1:0'], { env, timeout: 15000 });
  const status = await daemonCli(['daemon', 'status', '--json']);
  const { stdout: bootstrapLine } = await execute(binary, ['issue-bootstrap'], { env, timeout: 15000 });
  const bootstrap = /SPLITTY_BOOTSTRAP=([a-f0-9]+)/.exec(bootstrapLine)?.[1]; assert(bootstrap);
  const restored = await browser.newContext(); const restoredPage = await restored.newPage();
  const sent = [];
  restoredPage.on('websocket', socket => socket.on('framesent', ({ payload }) => {
    if (typeof payload === 'string') { try { sent.push(JSON.parse(payload)); } catch {} }
  }));
  await restoredPage.goto(`http://127.0.0.1:${status.port}/?bootstrap=${bootstrap}`);
  const outcome = restoredPage.locator(`[data-pane-id="${interrupted.paneId}"]`);
  await outcome.waitFor({ state: 'visible' }); assert((await outcome.textContent()).includes('interrupted'));
  assert(!sent.some(message => message.type === 'create'), 'interrupted reload created a shell');
  assert.equal((await daemonCli(['logs', interrupted.jobId, '--json'])).outputAvailable, false);
  await restored.close();
  console.log('PASS: scoped resume/revoke, background tab/all splits/screen, keyboard and newer-focus routing, shared output, exit retention, failed/interrupted reload');
} finally {
  await browser?.close();
  if (daemonRunning) await execute(binary, ['daemon', 'stop'], { env, timeout: 15000 }).catch(() => {});
  server.kill('SIGTERM');
  await Promise.race([new Promise(resolve => server.once('exit', resolve)), new Promise(resolve => setTimeout(resolve, 12000))]);
  if (server.exitCode === null && server.signalCode === null) server.kill('SIGKILL');
  await rm(home, { recursive: true, force: true });
}
