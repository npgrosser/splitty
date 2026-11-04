export function isWindowsFixtureDocument(view, includeSetup = false) {
  if (view.top !== view) return false;
  const url = new URL(view.location.href);
  return url.protocol === 'http:' && url.host === 'tauri.localhost'
    && ['', '/', '/index.html', ...(includeSetup ? ['/setup.html'] : [])].includes(url.pathname);
}
export function isWindowsClipboardDocument(view) {
  const url = new URL(view.location.href);
  return url.protocol === 'http:' && url.host === 'tauri.localhost'
    && ['', '/', '/index.html', '/setup.html', '/popout.html'].includes(url.pathname);
}
// Production App and separately bundled probe use disjoint private open IDs.
export function probeBridge(native) {
  const request = value => {
    if (!Number.isInteger(value) || value < 1 || value >= 0x40000000) throw Error('probe request namespace exhausted');
    return value + 0x40000000;
  };
  return {
    state: native.state.bind(native), open: (options, id) => native.open(options, request(id)),
    cancelOpen: id => native.cancelOpen(request(id)), listen: native.listen.bind(native),
    start: native.start.bind(native), write: native.write.bind(native),
    consumed: native.consumed.bind(native), close: native.close.bind(native),
  };
}
