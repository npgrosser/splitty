// Adapter input guards only. These tests never launch Chrome, contact a hub,
// create a fake service, or assert a D8 browser/SSH pass.
import assert from 'node:assert/strict';
import { mkdtemp, chmod, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { main, validateConfig } from './remote-forwards-browser.mjs';

const fixture = {
  hubUrl: 'http://127.0.0.1:30001', cookie: 'splitty-session=private-test-only-value',
  target: 'splittyhost', remote: 30002, local: 30003, marker: 'unique-lab-service-marker',
  playwrightModule: '/not-installed/playwright/index.mjs', executablePath: '/not-installed/chrome',
  apiService: { remote: 30004, local: 30005, marker: 'unique-api-service-marker', tlsCa: null },
};
// Call the CLI entry point in-process: child-process stdio uses denied socketpair
// operations in the restricted verification environment. No networking is mocked.
async function invoke(args) {
  const lines = [];
  const log = console.log;
  console.log = line => lines.push(line);
  try { return { status: await main(args), stdout: lines.join('\n') }; }
  finally { console.log = log; }
}

test('requires an explicit literal loopback origin and distinct unprivileged lab port', () => {
  assert.equal(validateConfig(fixture).hubUrl, fixture.hubUrl);
  for (const hubUrl of ['https://example.com', 'http://localhost:30001', 'http://127.0.0.1:44877',
    'http://127.1:30001', 'http://2130706433:30001', 'http://127.0.0.1:30001/?bootstrap=secret',
    'http://user@127.0.0.1:30001', 'http://127.0.0.1:99999', 'http://127.0.0.1:80']) {
    assert.throws(() => validateConfig({ ...fixture, hubUrl }));
  }
  for (const local of [44877, 30001, 0, 80, 65536, 30003.5, '30003']) {
    assert.throws(() => validateConfig({ ...fixture, local }));
  }
});

test('rejects ambiguous targets, credentials, extra config, and headless-shell', () => {
  for (const change of [
    { target: 'user@public.example' }, { target: '127.0.0.1' }, { target: '-Fconfig' },
    { cookie: fixture.cookie + '; other=value' }, { cookie: fixture.cookie + '\r\nHost: example.com' },
    { marker: 'ok' }, { playwrightModule: './index.mjs' },
    { executablePath: '/cache/chrome-headless-shell' }, { ignoreHTTPSErrors: true },
  ]) assert.throws(() => validateConfig({ ...fixture, ...change }));
});

test('requires a second distinct service and bounded explicit TLS trust', () => {
  for (const apiService of [undefined, null, {}, [],
    { ...fixture.apiService, remote: fixture.remote },
    { ...fixture.apiService, local: fixture.local },
    { ...fixture.apiService, local: 30001 },
    { ...fixture.apiService, local: 44877 },
    { ...fixture.apiService, local: '30005' },
    { ...fixture.apiService, remote: 65536 },
    { ...fixture.apiService, marker: fixture.marker },
    { ...fixture.apiService, marker: 'service\r\nsecret-header' },
    { ...fixture.apiService, tlsCa: false },
    { ...fixture.apiService, tlsCa: '/path/to/cert.pem' },
    { ...fixture.apiService, tlsCa: '-----BEGIN PRIVATE KEY-----\nsecret\n-----END PRIVATE KEY-----\n' },
    { ...fixture.apiService, tlsCa: 'x'.repeat(4097) },
    { ...fixture.apiService, rejectUnauthorized: false },
  ]) assert.throws(() => validateConfig({ ...fixture, apiService }));
  const missing = { ...fixture };
  delete missing.apiService;
  assert.throws(() => validateConfig(missing));
  assert.equal(validateConfig(fixture).apiService.tlsCa, null);
});

test('CLI refuses missing acknowledgment before loading the module or connecting', async () => {
  const result = await invoke(['--config', '-']);
  assert.equal(result.status, 2);
  const report = JSON.parse(result.stdout);
  assert.equal(report.code, 'EXPLICIT_LAB_ONLY_ACK_REQUIRED');
  assert.deepEqual(report.checks, []);
  assert.ok(!result.stdout.includes(fixture.cookie));
});

test('invalid private input never leaks its secret in errors', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'splitty-browser-input-'));
  try {
    const config = join(dir, 'fixture.json');
    for (const input of [JSON.stringify({ ...fixture, hubUrl: 'http://127.0.0.1:44877' }),
      `{"cookie":"${fixture.cookie}", malformed`]) {
      await writeFile(config, input, { mode: 0o600 });
      const result = await invoke(['--lab-only', '--config', config]);
      assert.equal(result.status, 2);
      assert.equal(JSON.parse(result.stdout).stage, 'config');
      assert.ok(!result.stdout.includes(fixture.cookie));
    }
  } finally { await rm(dir, { recursive: true, force: true }); }
});

test('file input rejects group/world access before reading a credential', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'splitty-browser-input-'));
  try {
    const config = join(dir, 'fixture.json');
    await writeFile(config, JSON.stringify(fixture), { mode: 0o600 });
    await chmod(config, 0o644);
    const result = await invoke(['--lab-only', '--config', config]);
    assert.equal(result.status, 2);
    assert.equal(JSON.parse(result.stdout).code, 'PRIVATE_CONFIG_FILE_REQUIRED');
    assert.ok(!result.stdout.includes(fixture.cookie));
  } finally { await rm(dir, { recursive: true, force: true }); }
});
