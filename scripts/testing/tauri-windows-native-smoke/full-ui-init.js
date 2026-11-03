// Clipboard-only protection also applies to owned packaged popouts/frames.
// Workload, fetch observation and UI probing remain restricted to the root.
if (isWindowsClipboardDocument(window)) {
  const calls = window.__SPLITTY_WINDOWS_CLIPBOARD_ATTEMPTS__ = [];
  const record = name => {
    calls.push(name);
    try {
      const parent = window.opener?.__SPLITTY_WINDOWS_UI_PROBE__?.clipboard;
      if (parent && parent !== calls) parent.push('popout:' + name);
    } catch { /* No cross-origin parent access is required. */ }
  };
  const originalExec = document.execCommand.bind(document);
  Object.defineProperty(document, 'execCommand', { configurable: true, value(command, ...args) {
    if (['copy', 'cut', 'paste'].includes(String(command).toLowerCase())) { record('execCommand:' + command); return false; }
    return originalExec(command, ...args);
  } });
  if (navigator.clipboard) for (const name of ['read', 'readText', 'write', 'writeText']) {
    Object.defineProperty(navigator.clipboard, name, { configurable: true, value() { record(name); return Promise.reject(Error('global clipboard disabled in fixture')); } });
  }
}
// Prepended document helpers are private to this native initialization closure.
if (isWindowsFixtureDocument(window)) {
  const state = { assets: [], network: [], clipboard: window.__SPLITTY_WINDOWS_CLIPBOARD_ATTEMPTS__ ?? [], errors: [] };
  window.__SPLITTY_WINDOWS_UI_PROBE__ = state;
  window.__SPLITTY_SMOKE_ASSETS__ = state.assets;
  const originalFetch = window.fetch;
  window.fetch = function(input, ...args) {
    const url = new URL(typeof input === 'string' ? input : input instanceof URL ? input.href : input.url, location.href);
    if ((url.origin !== location.origin && url.protocol !== 'ipc:' && url.origin !== 'http://ipc.localhost') || /^\/(api|ws)(\/|$)/.test(url.pathname)) state.network.push(url.href);
    return originalFetch.call(this, input, ...args).then(response => {
      if (url.pathname.endsWith('.wasm')) state.assets.push({ name: url.href, ok: response.ok, contentType: response.headers.get('content-type') });
      return response;
    });
  };
  const originalSocket = window.WebSocket;
  window.WebSocket = new Proxy(originalSocket, { construct(target, args, receiver) { state.network.push(String(args[0])); return Reflect.construct(target, args, receiver); } });
  new MutationObserver(() => {
    const text = document.querySelector('#app [role="alert"]')?.textContent?.trim();
    if (text && state.errors.at(-1) !== text) state.errors.push(text);
  }).observe(document, { subtree: true, childList: true, characterData: true });
}
