// Portable fixture ownership checks, not native WebView runtime evidence.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const { test } = require('node:test');
const typescript = require('../../../frontend/node_modules/typescript');
const policy = import('./document.mjs');
const source = fs.readFileSync(path.join(__dirname, 'entry.ts'), 'utf8');
const compiled = typescript.transpileModule(source, {
  compilerOptions: { module: typescript.ModuleKind.CommonJS, target: typescript.ScriptTarget.ES2022 },
}).outputText;
function view(href, child = false) {
  const window = { location: { href }, opener: null, name: 'fixture' };
  window.top = child ? {} : window;
  return window;
}

test('probe admits only the top packaged workspace, independently of capsule presence', async () => {
  const { isProbeDocument } = await policy;
  for (const href of ['tauri://localhost', 'tauri://localhost/', 'tauri://localhost/index.html']) {
    const window = view(href);
    assert.equal(window.__SPLITTY_TAURI__, undefined);
    assert.equal(isProbeDocument(window), true, href);
    assert.equal(isProbeDocument(view(href, true)), false, 'an inherited iframe cannot own the probe');
  }
  for (const href of ['tauri://localhost/popout.html', 'tauri://localhost/setup.html', 'about:blank',
      'data:text/html,foreign', 'https://localhost/index.html', 'tauri://foreign/index.html',
      'tauri://localhost:1420/index.html', 'tauri://user@localhost/index.html']) {
    assert.equal(isProbeDocument(view(href)), false, href);
  }
});

test('inherited popout/frame probe has no reporting or instrumentation side effects', async () => {
  const rules = await policy;
  for (const window of [view('tauri://localhost/popout.html'), view('tauri://localhost/index.html', true)]) {
    const context = { window, exports: {}, require: name => name.endsWith('document.mjs') ? rules : {} };
    // No DOM/timers/IPC are provided: any probe activity in this document fails.
    vm.runInNewContext(compiled, context);
  }
});

test('actual main entry still reports missing capsule with its location/frame metadata', async () => {
  const rules = await policy, reports = [];
  const window = view('tauri://localhost/index.html');
  window.__TAURI_INTERNALS__ = { invoke: async (command, args) => reports.push({ command, ...args }) };
  window.fetch = async () => {};
  window.addEventListener = () => {};
  window.WebSocket = function() {};
  const context = { window, exports: {}, location: window.location, URL,
    require: name => name.endsWith('document.mjs') ? rules : {},
    document: { body: { innerText: '' }, querySelector: () => null },
    sessionStorage: { getItem: () => null },
    MutationObserver: class { observe() {} },
    XMLHttpRequest: class { open() {} },
    setTimeout: () => 1, clearTimeout: () => {},
    __SPLITTY_FULL_UI__: true, __SPLITTY_SMOKE_RELOADS__: 36,
  };
  vm.runInNewContext(compiled, context);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(reports.length, 1);
  assert.equal(reports[0].command, 'smoke_report');
  assert.match(reports[0].value.error, /production initialization capsule is missing/);
  assert.equal(reports[0].value.stage, 'capsule');
  assert.equal(reports[0].value.probeDocument.href, window.location.href);
  assert.equal(reports[0].value.probeDocument.topLevel, true);
  assert.equal(reports[0].value.probeDocument.hasOpener, false);
});
