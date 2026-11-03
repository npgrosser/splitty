// Scope the test probe, not application authority. WKWebView can inherit the
// parent's initialization scripts into a new top-level popout configuration.
export function isProbeDocument(view) {
  try {
    const url = new URL(view.location.href);
    return view.top === view && url.protocol === 'tauri:' && url.hostname === 'localhost' &&
      !url.port && !url.username && !url.password &&
      ['', '/', '/index.html'].includes(url.pathname);
  } catch {
    return false;
  }
}

export function probeDocumentMetadata(view) {
  return {
    href: view.location.href,
    topLevel: view.top === view,
    hasOpener: view.opener !== null && view.opener !== undefined,
    frameName: String(view.name || '').slice(0, 128),
  };
}
