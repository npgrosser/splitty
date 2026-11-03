// DOM actions against frontend/dist. No Svelte module or renderer is mocked.
const wait = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));
async function until<T>(description: string, read: () => T): Promise<NonNullable<T>> {
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    const value = read();
    if (value) return value as NonNullable<T>;
    const failure = document.querySelector('#app > p')?.textContent;
    if (failure?.includes('could not open')) throw Error(failure);
    await wait(25);
  }
  throw Error(`${description} timed out; DOM: ${document.body.innerText.slice(0, 1800)}`);
}
export async function exerciseFullUi(request: (operation: string, args?: object) => Promise<any>, checkPopout: boolean) {
  await until('production App mount', () => document.querySelector('#app .dockview-theme-abyss, #app .dv-dockview, #app .dockview-container, #app .workspace-root'));
  const initialTerminal = await until('production terminal mount', () => document.querySelector('#app textarea'));
  const style = getComputedStyle(document.body);
  if (!document.styleSheets.length || style.margin !== '0px') throw Error('production CSS did not apply');
  await until('packaged terminal font', () => Array.from(document.fonts).some(face => face.family === 'SplittyMono' && face.status === 'loaded'));
  const settingsButton = await until('settings navigation', () => Array.from(document.querySelectorAll<HTMLButtonElement>('button')).find(button => Array.from(button.querySelectorAll('span')).some(span => span.textContent === 'Settings')));
  settingsButton.click();
  const renderer = await until('renderer setting', () => Array.from(document.querySelectorAll<HTMLSelectElement>('.settings-panel select')).find(select => select.querySelector('option[value="ghostty"]')));
  const alreadyGhostty = renderer.value === 'ghostty';
  renderer.value = 'ghostty';
  renderer.dispatchEvent(new Event('change', { bubbles: true }));
  await until('native persisted UI setting', () => {
    // Keep API polling sequential below rather than accumulating requests.
    return document.querySelector<HTMLSelectElement>('.settings-panel select')?.value === 'ghostty';
  });
  let saved: any;
  for (let tries = 0; tries < 100; tries++) {
    saved = await request('home.get');
    if (JSON.stringify(saved).includes('"renderer":"ghostty"')) break;
    await wait(25);
  }
  if (!JSON.stringify(saved).includes('"renderer":"ghostty"')) throw Error('UI renderer setting did not persist through native API');
  // Renderer selection applies to newly mounted terminals. Create one through
  // the ordinary UI instead of replacing an existing emulator in test code.
  if (!alreadyGhostty) {
    const newScreen = await until('new screen action', () => document.querySelector<HTMLButtonElement>('.drawer-new-main'));
    newScreen.click();
  }
  const terminal = await until('Ghostty textarea and canvas', () => {
    return Array.from(document.querySelectorAll<HTMLTextAreaElement>('textarea[aria-label="Terminal input"]'))
      .find(input => input.parentElement?.querySelector('canvas'));
  });
  const wasm = await until('packaged Ghostty WASM resource', () => ((window as any).__SPLITTY_SMOKE_ASSETS__ as {name:string; ok:boolean; contentType:string}[]).find(entry => /ghostty.*\.wasm/.test(entry.name) && entry.ok));
  const wasmUrl = new URL(wasm.name);
  if (wasmUrl.protocol !== location.protocol || wasmUrl.hostname !== location.hostname || !wasmUrl.pathname.startsWith('/assets/')) throw Error(`Ghostty WASM escaped packaged assets: ${wasm.name}`);
  // Exercise real terminal input through the production emulator, then inspect
  // the canvas. Binary byte integrity is checked independently by entry.ts.
  terminal.focus();
  terminal.dispatchEvent(new KeyboardEvent('keydown', { key: 'a', code: 'KeyA', bubbles: true }));
  const canvas = terminal.parentElement!.querySelector('canvas')!;
  if (!canvas.width || !canvas.height) throw Error('Ghostty canvas has no backing pixels');
  await until('Ghostty rendered glyph pixels', () => {
    const pixels = canvas.getContext('2d')?.getImageData(0, 0, canvas.width, canvas.height).data;
    return pixels?.some((byte, index) => index % 4 !== 3 && byte !== pixels[index % 4]);
  });
  const foreign = window.open('data:text/html,<title>foreign smoke document</title>', 'splitty-smoke-foreign');
  await wait(100);
  if (foreign && !foreign.closed) { foreign.close(); throw Error('native new-window guard admitted a data document'); }
  const mainStillUsable = await (window as any).__SPLITTY_TAURI__.invoke('remote_hosts');
  if (!Array.isArray(mainStillUsable)) throw Error('denied foreign navigation revoked the main document');
  const inventory = () => (window as any).__TAURI_INTERNALS__.invoke('smoke_windows');
  const baseline = await inventory();
  if (baseline.managed.some((label: string) => label.startsWith('popout-')) || baseline.nativeVisible !== 1) {
    throw Error(`parent reload left native child windows: ${JSON.stringify(baseline)}`);
  }
  let popout: object = { skipped: 'only first document opens a child' };
  if (checkPopout) {
    let child: Window | null = null;
    try {
      child = window.open('/popout.html', 'splitty-smoke-popout', 'width=400,height=300');
      if (!child) throw Error('runtime did not provide the parent-owned popout');
      await until('parent-owned popout load', () => child!.document.readyState === 'complete' && child!.location.pathname === '/popout.html');
      if ((child as any).__SPLITTY_TAURI__) throw Error('popout received independent native capsule');
      let directDenied = false;
      try { await (child as any).__TAURI_INTERNALS__.invoke('remote_hosts', { auth: { document: 0, secret: Array(32).fill(0) } }); }
      catch { directDenied = true; }
      if (!directDenied) throw Error('direct popout native command was admitted');
      const hosts = await (child.opener as any).__SPLITTY_TAURI__.invoke('remote_hosts');
      if (!Array.isArray(hosts)) throw Error('parent-mediated popout request failed');
      const opened = await inventory();
      if (opened.managed.filter((label: string) => label.startsWith('popout-')).length !== 1 || opened.nativeVisible !== 2) throw Error(`native popout not owned: ${JSON.stringify(opened)}`);
      const sharedOpener = child.opener === window;
      child.close();
      await until('child.closed after production window.close()', () => child!.closed);
      let closed: any;
      for (let tries = 0; tries < 100; tries++) {
        closed = await inventory();
        if (closed.nativeVisible === baseline.nativeVisible && closed.managed.length === baseline.managed.length) break;
        await wait(20);
      }
      if (closed.nativeVisible !== 1 || closed.managed.length !== 1) throw Error(`window.close left native window shells: ${JSON.stringify(closed)}`);
      popout = { opened: true, directDenied, parentMediated: true, sharedOpener, closed: true, nativeInventoryClosed: true };
    } finally { child?.close(); }
    for (const label of ['main', 'remote-foreign', 'unknown-popout']) {
      let denied = false;
      try { await (window as any).__SPLITTY_TAURI__.invoke('close_popout', { label }); }
      catch { denied = true; }
      if (!denied) throw Error(`parent close command accepted foreign label ${label}`);
    }
    const commanded = window.open('/popout.html', 'splitty-smoke-command-close', 'width=400,height=300');
    if (!commanded) throw Error('command-close popout was refused');
    try {
      await until('command-close child load', () => commanded.document.readyState === 'complete' && commanded.location.pathname === '/popout.html');
      const owned = await inventory();
      const label = owned.managed.find((label: string) => label.startsWith('popout-'));
      if (!label) throw Error('command-close native label missing');
      await (window as any).__SPLITTY_TAURI__.invoke('close_popout', { label });
      await until('command-close child.closed', () => commanded.closed);
      let closed: any;
      for (let tries=0; tries<100; tries++) { closed = await inventory(); if(closed.nativeVisible===1 && closed.managed.length===1) break; await wait(20); }
      if (closed.nativeVisible!==1 || closed.managed.length!==1) throw Error('parent command left native child window');
      popout = { ...popout, parentCloseCommand: true, foreignCloseDenied: true };
    } finally { commanded.close(); }
    // This final child deliberately survives until the parent's navigation;
    // the next document checks actual native teardown before continuing.
    const reloadChild = window.open('/popout.html', 'splitty-smoke-parent-reload', 'width=400,height=300');
    if (!reloadChild) throw Error('parent-reload popout was refused');
    await until('parent-reload popout load', () => reloadChild.document.readyState === 'complete' && reloadChild.location.pathname === '/popout.html');
  }
  return { appMounted: true, styles: true, font: true, initialTerminal: initialTerminal.className,
    rendererSetting: 'ghostty', foreignWindowDenied: true, mainUsableAfterDenial: true, ghosttyWasm: wasm.name, canvas: [canvas.width, canvas.height], popout };
}
