#!/usr/bin/env node
// Browser/transport adapter for remote_connect's private lab. No server/SSH fixture here.
import { constants } from 'node:fs';
import { createHash } from 'node:crypto';
import { request as httpRequest } from 'node:http';
import { request as httpsRequest } from 'node:https';
import { open, mkdtemp, mkdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { isAbsolute, join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { setTimeout as sleep } from 'node:timers/promises';

const ACTIVE = '/api/remote/forwards';
const SAVED = '/api/remote/forward-preferences';
const HOLD_MS = 6500;
const TIMEOUT = 15000;
const digest = bytes => createHash('sha256').update(bytes).digest('hex');

class CheckError extends Error {
  constructor(code, blocked = false) { super(code); this.code = code; this.blocked = blocked; }
}
function check(ok, code, blocked = false) {
  if (!ok) throw new CheckError(code, blocked);
}
function deferred() {
  let resolve;
  const promise = new Promise(r => { resolve = r; });
  return { promise, resolve };
}
async function within(promise, code, ms = TIMEOUT) {
  let timer;
  try {
    return await Promise.race([promise, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new CheckError(code)), ms);
    })]);
  } finally { clearTimeout(timer); }
}

export function validateConfig(config) {
  const keys = ['hubUrl', 'cookie', 'target', 'remote', 'local', 'marker', 'playwrightModule', 'executablePath', 'apiService'];
  check(config && typeof config === 'object' && !Array.isArray(config)
    && Object.keys(config).length === keys.length && keys.every(key => Object.hasOwn(config, key)), 'CONFIG_FIELDS', true);
  check(typeof config.hubUrl === 'string' && /^http:\/\/127\.0\.0\.1:[1-9][0-9]{0,4}\/?$/.test(config.hubUrl), 'LOOPBACK_HUB_REQUIRED', true);
  let hub;
  try { hub = new URL(config.hubUrl); } catch { throw new CheckError('LOOPBACK_HUB_REQUIRED', true); }
  check(hub.port && Number(hub.port) !== 44877, 'PRIVATE_LAB_PORT_REQUIRED', true);
  for (const key of ['remote', 'local']) {
    check(Number.isInteger(config[key]) && config[key] >= 1024 && config[key] <= 65535, 'UNPRIVILEGED_PORT_REQUIRED', true);
  }
  check(config.local !== Number(hub.port) && config.local !== 44877, 'DISTINCT_LAB_PORT_REQUIRED', true);
  check(typeof config.target === 'string' && /^[A-Za-z][A-Za-z0-9_-]{0,62}$/.test(config.target), 'LAB_SSH_ALIAS_REQUIRED', true);
  check(typeof config.cookie === 'string' && /^splitty-session=[A-Za-z0-9_-]{16,512}$/.test(config.cookie), 'SESSION_COOKIE_REQUIRED', true);
  check(typeof config.marker === 'string' && /^[\x20-\x7e]{16,200}$/.test(config.marker), 'UNIQUE_SERVICE_MARKER_REQUIRED', true);
  for (const key of ['playwrightModule', 'executablePath']) {
    check(typeof config[key] === 'string' && isAbsolute(config[key]), 'ABSOLUTE_TOOL_PATHS_REQUIRED', true);
  }
  check(!/headless[-_]shell/i.test(config.executablePath), 'FULL_CHROME_REQUIRED', true);
  {
    const service = config.apiService;
    check(service && Object.keys(service).length === 4
      && ['remote', 'local', 'marker', 'tlsCa'].every(key => Object.hasOwn(service, key)), 'API_SERVICE_FIELDS', true);
    for (const key of ['remote', 'local']) {
      check(Number.isInteger(service[key]) && service[key] >= 1024 && service[key] <= 65535
        && service[key] !== 44877, 'API_SERVICE_PORT', true);
    }
    check(service.remote !== config.remote && service.local !== config.local
      && service.local !== Number(hub.port), 'DISTINCT_API_SERVICE_REQUIRED', true);
    check(typeof service.marker === 'string' && /^[A-Za-z0-9-]{16,200}$/.test(service.marker)
      && service.marker !== config.marker, 'UNIQUE_API_MARKER_REQUIRED', true);
    check(service.tlsCa === null || (typeof service.tlsCa === 'string' && service.tlsCa.length <= 4096
      && /^-----BEGIN CERTIFICATE-----\n[A-Za-z0-9+/=\n]+-----END CERTIFICATE-----\n$/.test(service.tlsCa)), 'FIXTURE_TLS_CA_REQUIRED', true);
  }
  return { ...config, hubUrl: hub.origin };
}

async function readConfig(source) {
  let raw = '';
  if (source === '-') {
    check(!process.stdin.isTTY, 'PRIVATE_STDIN_REQUIRED', true);
    try {
      await within((async () => {
        for await (const chunk of process.stdin) {
          raw += chunk;
          check(Buffer.byteLength(raw) <= 16384, 'CONFIG_TOO_LARGE', true);
        }
      })(), 'CONFIG_STDIN_TIMEOUT', 5000);
    } finally { process.stdin.destroy(); }
  } else {
    const file = await open(source, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
    try {
      const stat = await file.stat();
      check(stat.isFile() && stat.size <= 16384 && (stat.mode & 0o077) === 0
        && typeof process.getuid === 'function' && stat.uid === process.getuid(), 'PRIVATE_CONFIG_FILE_REQUIRED', true);
      raw = await file.readFile('utf8');
    } finally { await file.close(); }
  }
  let parsed;
  try { parsed = JSON.parse(raw); } catch { throw new CheckError('INVALID_CONFIG_JSON', true); }
  return validateConfig(parsed);
}

async function playwright(modulePath) {
  check(isAbsolute(modulePath), 'ABSOLUTE_MODULE_PATH_REQUIRED', true);
  // Debug logging can include network headers. Refuse it before importing Playwright.
  check(!process.env.DEBUG && !process.env.PWDEBUG && !process.env.DEBUG_FILE, 'DEBUG_LOGGING_MUST_BE_OFF', true);
  const module = await import(pathToFileURL(modulePath).href);
  check(typeof module.chromium?.launch === 'function', 'PLAYWRIGHT_CHROMIUM_REQUIRED', true);
  return module.chromium;
}

function matchingForward(value, config) {
  return value?.target === config.target && value.remote === config.remote
    && value.local === config.local && value.url === `http://localhost:${config.local}` && value.state === 'up';
}

// Observe the native call during a trusted click's dispatch. No fake return value,
// evaluated window.open, forced click, or user-activation/popup policy override.
function observeOpen() {
  let click;
  const observations = [];
  Object.defineProperty(window, '__splittyBrowserOpenObservations', { value: observations });
  window.addEventListener('click', event => { click = event; }, true);
  const nativeOpen = window.open;
  window.open = function (...args) {
    const record = {
      trustedClick: click?.isTrusted === true && click.eventPhase !== Event.NONE,
      active: navigator.userActivation.isActive,
      blankArgument: args[0] === '' || args[0] === 'about:blank',
      at: performance.now(),
    };
    const result = Reflect.apply(nativeOpen, this, args);
    observations.push({ ...record, nativeReturnedWindow: result !== null });
    return result;
  };
}

// Called inside the real service page, so Chrome performs the raw upgrade and
// owns masking, protocol negotiation and close. Playwright never routes this WS.
async function serviceWebSocket() {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(`ws://${location.host}/hmr?mode=raw%2Fupdate`, 'splitty-hmr');
    socket.binaryType = 'arraybuffer';
    const messages = [];
    const timer = setTimeout(() => { socket.close(); reject(new Error('WS_TIMEOUT')); }, 10000);
    socket.onerror = () => { clearTimeout(timer); reject(new Error('WS_FAILED')); };
    socket.onmessage = event => {
      if (typeof event.data === 'string') {
        messages.push(JSON.parse(event.data));
        if (messages.length === 1) socket.send('change:src/App.tsx');
        else socket.send(Uint8Array.from({ length: 256 }, (_, i) => i));
      } else {
        messages.push(Array.from(new Uint8Array(event.data)));
        socket.close(1000, 'checked');
      }
    };
    socket.onclose = event => {
      clearTimeout(timer);
      resolve({ protocol: socket.protocol, messages, code: event.code, reason: event.reason, clean: event.wasClean });
    };
  });
}

// TLS is checked by a real Node client using only the fixture certificate as
// trust. Hostname verification stays enabled; no browser TLS exception is used.
async function serviceApi(service, authenticated) {
  const payload = Buffer.from(Array.from({ length: 256 }, (_, i) => i));
  return new Promise((resolve, reject) => {
    const tls = service.tlsCa !== null;
    const request = (tls ? httpsRequest : httpRequest)({
      hostname: 'localhost', family: 4, port: service.local, method: 'POST',
      path: '/api/echo?mode=raw%2Fbytes', agent: false, signal: AbortSignal.timeout(TIMEOUT),
      ...(tls ? { ca: service.tlsCa, rejectUnauthorized: true } : {}),
      headers: { 'Content-Type': 'application/octet-stream', 'Content-Length': payload.length,
        ...(authenticated ? { Authorization: `Bearer app-${service.marker}` } : {}) },
    }, response => {
      const authorized = !tls || response.socket.authorized === true;
      const protocol = tls ? response.socket.getProtocol() : null;
      const chunks = [];
      let size = 0;
      response.on('data', chunk => {
        size += chunk.length;
        if (size > 4096) response.destroy(new CheckError('API_RESPONSE_TOO_LARGE'));
        else chunks.push(chunk);
      });
      response.on('error', reject);
      response.on('end', () => resolve({ status: response.statusCode, bytes: Buffer.concat(chunks), authorized, protocol }));
    });
    request.on('error', reject);
    request.end(payload);
  });
}

async function run(config, report) {
  const chromium = await playwright(config.playwrightModule);
  let browser, context, privateDir, ownedMapping = false, gate;
  const serviceOrigin = `http://localhost:${config.local}`;
  const body = { target: config.target, port: config.remote };
  const reportStart = performance.now();
  const stamp = () => Math.round(performance.now() - reportStart);
  let violation;
  let serviceAllowed = false;
  let serviceRequests = 0;
  let apiSuccessAt;
  let lastNavigationAt;
  let serviceStatus;
  let transportMode = false;
  const api = async (method, path, data, statuses = [200]) => {
    const response = await fetch(config.hubUrl + path, {
      method, redirect: 'error', signal: AbortSignal.timeout(TIMEOUT),
      headers: { Cookie: config.cookie, Origin: config.hubUrl, ...(data ? { 'Content-Type': 'application/json' } : {}) },
      ...(data ? { body: JSON.stringify(data) } : {}),
    });
    check(statuses.includes(response.status), 'LAB_API_STATUS');
    return response.status === 204 || response.status === 404 ? null : response.json();
  };
  const cleanState = async () => {
    const [active, saved] = await Promise.all([api('GET', ACTIVE), api('GET', SAVED)]);
    return Array.isArray(active) && active.length === 0 && saved?.version === 1
      && Array.isArray(saved.forwards) && saved.forwards.length === 0;
  };
  const blank = async popup => {
    check(!popup.isClosed(), 'RESERVATION_CLOSED_EARLY');
    const state = await popup.evaluate(() => ({
      url: location.href, openerNull: window.opener === null, title: document.title,
      referrer: document.querySelector('meta[name="referrer"]')?.content,
    }));
    check(state.url === 'about:blank' && state.openerNull && state.title === 'Opening forwarded port…'
      && state.referrer === 'no-referrer', 'BLANK_RESERVATION_CONTRACT');
    check(!violation && serviceRequests === 0, violation || 'EARLY_SERVICE_REQUEST');
  };
  try {
    report.stage = 'fixture-preflight';
    check(await cleanState(), 'FRESH_LAB_FORWARDS_REQUIRED', true);
    privateDir = await mkdtemp(join(tmpdir(), 'splitty-browser-'));
    const env = { ...process.env };
    for (const key of ['XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_STATE_HOME', 'XDG_CACHE_HOME', 'XDG_RUNTIME_DIR']) {
      env[key] = join(privateDir, key);
      await mkdir(env[key], { mode: 0o700 });
    }
    report.stage = 'browser-launch';
    try {
      // `--enable-automation` lets CDP report the actual switches below; newer
      // Playwright releases no longer pass it by default.
      browser = await chromium.launch({ executablePath: config.executablePath,
        headless: true, chromiumSandbox: true, args: ['--enable-automation'],
        ignoreDefaultArgs: ['--disable-popup-blocking'], env, timeout: TIMEOUT });
    } catch { throw new CheckError('SANDBOXED_CHROME_UNAVAILABLE', true); }
    report.browser = browser.version();
    context = await browser.newContext({ serviceWorkers: 'block', viewport: { width: 1100, height: 800 } });
    context.setDefaultTimeout(TIMEOUT);
    context.setDefaultNavigationTimeout(TIMEOUT);
    // Inspect actual launch switches, rather than merely assuming the requested policy.
    const cdp = await browser.newBrowserCDPSession();
    const { arguments: args } = await cdp.send('Browser.getBrowserCommandLine');
    check(!args.some(arg => /^--(?:disable-popup-blocking|no-sandbox|disable-setuid-sandbox)(?:=|$)/.test(arg)), 'UNSAFE_BROWSER_FLAGS');
    await cdp.detach();
    report.popupBlocking = 'enabled';
    report.sandbox = 'enabled';
    check(typeof context.routeWebSocket === 'function', 'PLAYWRIGHT_WEBSOCKET_ROUTING_REQUIRED', true);
    await context.routeWebSocket(url => !(transportMode
      && url.origin === serviceOrigin.replace('http:', 'ws:')
      && url.pathname === '/hmr' && url.search === '?mode=raw%2Fupdate'), socket => {
      const url = new URL(socket.url());
      // The window's own socket, and the hub's browse of the lab host, which
      // unfolding its entry in the Hosts view opens to show its ports.
      if (url.origin === config.hubUrl.replace('http:', 'ws:') && (url.pathname === '/ws'
        || (url.pathname === '/ws/remote' && url.searchParams.get('target') === config.target))) socket.connectToServer();
      else { violation = 'UNEXPECTED_WEBSOCKET'; socket.close(); }
    });
    await context.route('**/*', async route => {
      try {
        const request = route.request();
        const url = new URL(request.url());
        if (url.origin === serviceOrigin) {
          serviceRequests++;
          if (request.isNavigationRequest()) lastNavigationAt = stamp();
          if (!transportMode && (!serviceAllowed || apiSuccessAt === undefined)) violation = report.stage === 'cross-window-stale-response'
            ? 'STALE_NAVIGATION_AFTER_STOP' : 'NAVIGATION_BEFORE_VERIFIED_RESPONSE';
          const headers = await request.allHeaders();
          if (headers.cookie || headers.authorization || headers.referer) violation = 'SERVICE_RECEIVED_CREDENTIAL_OR_REFERRER';
          if (url.username || url.password || url.search || url.hash) violation = 'UNEXPECTED_SERVICE_URL';
          if (violation) return await route.abort();
          return await route.continue();
        }
        if (url.origin !== config.hubUrl) { violation = 'NON_LAB_REQUEST'; return await route.abort(); }
        if ([ACTIVE, SAVED].includes(url.pathname) && request.method() !== 'GET') {
          let input;
          try { input = request.postDataJSON(); } catch { /* rejected below */ }
          const mapping = transportMode && input?.port === config.apiService?.remote ? config.apiService : config;
          if (input?.target !== config.target || input.port !== mapping.remote || input.automatic
            || (input.local !== undefined && input.local !== mapping.local)) {
            violation = 'UNEXPECTED_FORWARD_MUTATION'; return await route.abort();
          }
          if (request.method() === 'POST' && url.pathname === ACTIVE && gate) {
            if (gate.request) { violation = 'DUPLICATE_OPEN_POST'; return await route.abort(); }
            const current = gate;
            current.request = request;
            try {
              // Complete the real backend POST before another window can Stop.
              // Replay only this response, unchanged; never manufacture success.
              current.backend = await route.fetch({ maxRedirects: 0, maxRetries: 0, timeout: TIMEOUT });
              const bytes = await current.backend.body();
              check(current.backend.status() === 200 && matchingForward(JSON.parse(bytes.toString('utf8')), config), 'REAL_BACKEND_RESPONSE_MISMATCH');
              current.backendDigest = digest(bytes);
              current.heldAt = stamp();
              current.arrived.resolve();
              const proceed = await current.release.promise;
              current.releasedAt = stamp();
              if (proceed) await route.fulfill({ response: current.backend });
              else await route.abort();
            } catch (error) {
              current.error = error instanceof CheckError ? error.code : 'REAL_RESPONSE_GATE_FAILED';
              current.arrived.resolve();
              await route.abort().catch(() => {});
            } finally {
              if (current.backend) await current.backend.dispose().catch(() => {});
              current.settled.resolve();
            }
            return;
          }
        }
        return await route.continue();
      } catch { violation = 'REQUEST_ROUTING_FAILED'; await route.abort().catch(() => {}); }
    });
    await context.addCookies([{ name: 'splitty-session', value: config.cookie.slice('splitty-session='.length),
      url: config.hubUrl, httpOnly: true, sameSite: 'Strict', secure: false }]);
    const page = await context.newPage();
    await page.addInitScript(observeOpen);
    context.on('response', response => {
      if (new URL(response.url()).origin === serviceOrigin && response.request().isNavigationRequest()) serviceStatus = response.status();
    });
    page.on('response', response => {
      if (gate?.request === response.request()) {
        gate.responseAt = stamp();
        if (response.status() === 200) apiSuccessAt = gate.responseAt;
        gate.response.resolve(response);
      }
    });
    report.stage = 'setup-mapping';
    ownedMapping = true; // Also clean a POST whose response is lost.
    check(matchingForward(await api('POST', ACTIVE, { ...body, local: config.local }), config), 'SETUP_MAPPING_MISMATCH');
    report.stage = 'load-ui';
    // The host's entry in the drawer's Hosts view, unfolded to its ports.
    const hostEntry = async tab => {
      const host = tab.getByRole('region', { name: `Host ${config.target}`, exact: true });
      const head = host.locator('button.host-head');
      await head.waitFor({ state: 'visible' });
      if (await head.getAttribute('aria-expanded') !== 'true') await head.click();
      await host.getByRole('region', { name: `Ports on ${config.target}`, exact: true }).waitFor({ state: 'visible' });
      return host;
    };
    const loadRow = async tab => {
      const loaded = await tab.goto(config.hubUrl);
      check(loaded?.ok(), 'UI_UNAVAILABLE', true);
      const drawer = tab.locator('.drawer');
      await drawer.waitFor({ state: 'attached' });
      if (!await drawer.evaluate(node => node.classList.contains('open') || node.classList.contains('pinned'))) {
        const toggle = tab.getByTitle('Toggle drawer (Ctrl+B)', { exact: true });
        if (await toggle.isVisible()) await toggle.click();
        else {
          // Ordinary web pages have no desktop titlebar, and the terminal may
          // take Ctrl+B; a pane's own menu shows the drawer through real clicks.
          await tab.locator('[data-pane-id]').first().click({ button: 'right' });
          await tab.getByRole('menuitem', { name: /^Show Drawer/ }).click();
        }
      }
      await tab.locator('.drawer.open, .drawer.pinned').waitFor({ state: 'visible' });
      await tab.locator('.drawer-nav').getByRole('button', { name: 'Hosts', exact: true }).click();
      const row = (await hostEntry(tab)).getByRole('group', { name: `Port ${config.remote} on ${config.target}`, exact: true });
      await row.waitFor({ state: 'visible' });
      await row.getByRole('button', { name: 'Open', exact: true }).waitFor();
      return row;
    };
    const row = await loadRow(page);

    for (const scenario of ['verified-open', 'cross-window-stale-response']) {
      report.stage = scenario;
      const stopper = scenario === 'cross-window-stale-response' ? await context.newPage() : null;
      const stopRow = stopper ? await loadRow(stopper) : null;
      await page.bringToFront();
      serviceAllowed = false;
      serviceRequests = 0;
      apiSuccessAt = undefined;
      lastNavigationAt = undefined;
      serviceStatus = undefined;
      gate = { arrived: deferred(), release: deferred(), settled: deferred(), response: deferred() };
      const before = await page.evaluate(() => window.__splittyBrowserOpenObservations.length);
      const popupPromise = context.waitForEvent('page');
      // Attach the rejection handler now so a failed click cannot leave an unhandled promise.
      popupPromise.catch(() => {});
      const clickedAt = stamp();
      await row.getByRole('button', { name: 'Open', exact: true }).click();
      const popup = await within(popupPromise, 'NO_NATIVE_POPUP', 3000);
      await within(gate.arrived.promise, 'NO_VERIFY_POST');
      check(!gate.error, gate.error);
      const observed = await page.evaluate(() => window.__splittyBrowserOpenObservations);
      check(observed.length === before + 1 && observed.at(-1).trustedClick && observed.at(-1).active
        && observed.at(-1).blankArgument && observed.at(-1).nativeReturnedWindow, 'SYNCHRONOUS_NATIVE_RESERVATION_REQUIRED');
      await blank(popup);
      const event = { scenario, clickedAt, backendSuccessAt: gate.heldAt,
        synchronousTrustedClick: true, openerNull: true };
      if (stopper) {
        // The real POST has already succeeded; this DELETE is ordered after it.
        const stopped = stopper.waitForResponse(response => response.url() === config.hubUrl + ACTIVE
          && response.request().method() === 'DELETE');
        stopped.catch(() => {});
        await stopRow.getByRole('button', { name: `More for port ${config.remote}`, exact: true }).click();
        // Anchored: Forget's description mentions Stop too.
        await stopper.getByRole('menuitem', { name: /^Stop\b/ }).click();
        check((await stopped).status() === 204 && await cleanState(), 'CROSS_WINDOW_STOP_NOT_ACKNOWLEDGED');
        event.stopAcknowledgedAt = stamp();
        // Focus and a full polling interval give the first window a real chance
        // to converge. Never inject controller calls or fabricate server state.
        await page.bringToFront();
      }
      const holdUntil = Math.max(gate.heldAt + HOLD_MS, (event.stopAcknowledgedAt ?? 0) + HOLD_MS);
      while (stamp() < holdUntil) {
        await sleep(100);
        if (!stopper || !popup.isClosed()) await blank(popup);
      }
      if (stopper) {
        // Diagnostic only: closure may legitimately follow validation of the
        // delivered response, as long as the pending tab never navigates.
        event.closedBeforeRelease = popup.isClosed();
        event.convergedBeforeRelease = await row.count() === 0;
      } else serviceAllowed = true;
      check(!violation && serviceRequests === 0, violation || 'EARLY_SERVICE_REQUEST');
      gate.release.resolve(true);
      const response = await within(gate.response.promise, 'VERIFY_RESPONSE_TIMEOUT');
      check(response.status() === 200 && matchingForward(await response.json(), config), 'REAL_VERIFY_RESPONSE_MISMATCH');
      check(await within(response.finished(), 'VERIFY_RESPONSE_UNFINISHED') === null, 'VERIFY_RESPONSE_UNFINISHED');
      check(digest(await response.body()) === gate.backendDigest, 'RESPONSE_BYTES_CHANGED');
      check(gate.releasedAt - gate.heldAt >= HOLD_MS, 'INSUFFICIENT_POST_DELAY');
      Object.assign(event, { releasedAt: gate.releasedAt, responseAt: gate.responseAt,
        responseHeldMs: gate.releasedAt - gate.heldAt, realMatchingSuccess: true, byteIdenticalResponse: true });
      if (scenario === 'verified-open') {
        await popup.waitForURL(serviceOrigin + '/', { waitUntil: 'load' });
        check(serviceStatus >= 200 && serviceStatus < 300, 'REAL_SERVICE_HTTP_STATUS');
        check((await popup.locator('body').innerText()).includes(config.marker), 'REAL_SERVICE_MARKER_MISSING');
        check(await popup.evaluate(() => window.opener === null), 'NAVIGATED_OPENER_NOT_NULL');
        check(lastNavigationAt >= gate.responseAt && serviceRequests > 0 && !violation, violation || 'NAVIGATION_ORDER');
        Object.assign(event, { navigationAt: lastNavigationAt, realServiceMarker: true });
        await popup.close();
      } else {
        // Verification may await a passive GET after receiving the held POST.
        // Row idleness alone does not prove that navigation validation finished.
        if (!popup.isClosed()) await popup.waitForEvent('close', { timeout: TIMEOUT }).catch(() => {});
        await page.waitForFunction(() => !document.querySelector('[aria-busy="true"]'));
        // Include a post-completion observation window for queued continuations.
        await sleep(1000);
        Object.assign(event, { backendStillStopped: await cleanState(), serviceRequestAttempts: serviceRequests,
          closedByCompletion: popup.isClosed(),
          noStaleNavigation: popup.isClosed() && context.pages().length === 2 && serviceRequests === 0 && !violation });
        report.checks.push(event);
        check(event.backendSuccessAt <= event.stopAcknowledgedAt && event.stopAcknowledgedAt < event.releasedAt, 'INVALID_STOP_ORDERING');
        check(event.backendStillStopped, 'STALE_RESPONSE_REOPENED_BACKEND');
        check(event.noStaleNavigation, violation || 'CROSS_WINDOW_STOP_DID_NOT_CANCEL_RESERVATION');
      }
      if (!stopper) report.checks.push(event);
      gate = undefined;
      await page.bringToFront();
      await page.waitForFunction(() => !document.querySelector('[aria-busy="true"]'));
    }
    check(!violation, violation);
    {
      report.stage = 'manual-services-transport';
      transportMode = true;
      for (const tab of context.pages()) if (tab !== page) await tab.close();
      check(await cleanState(), 'MANUAL_SERVICES_REQUIRE_EMPTY_STATE');
      const mappings = [{ ...config, label: 'Frontend' },
        { ...config.apiService, target: config.target, label: 'API' }];
      for (const mapping of mappings) {
        await page.getByRole('button', { name: 'Forward a port on another host…', exact: true }).click();
        const form = page.getByRole('form', { name: 'Add port', exact: true });
        await form.getByLabel('SSH target', { exact: true }).fill(config.target);
        await form.getByLabel('Remote port', { exact: true }).fill(String(mapping.remote));
        await form.getByLabel('Local port', { exact: false }).fill(String(mapping.local));
        const remember = form.getByRole('checkbox', { name: 'Remember', exact: true });
        check(!await remember.isChecked(), 'REMEMBER_MUST_BE_OPT_IN');
        await remember.check();
        await form.getByLabel('Label', { exact: false }).fill(mapping.label);
        await form.getByRole('button', { name: 'Forward', exact: true }).click();
        await form.waitFor({ state: 'hidden' });
        const manualRow = (await hostEntry(page)).getByRole('group', { name: `Port ${mapping.remote} on ${config.target}`, exact: true });
        await manualRow.getByRole('button', { name: 'Open', exact: true }).waitFor();
        // Only a remembered mapping has a label to show as its name.
        await manualRow.getByText(mapping.label, { exact: true }).waitFor();
      }
      const [active, saved] = await Promise.all([api('GET', ACTIVE), api('GET', SAVED)]);
      check(active.length === 2 && saved.forwards.length === 2 && mappings.every(mapping =>
        active.some(value => matchingForward(value, mapping)) && saved.forwards.some(value =>
          value.target === config.target && value.remote === mapping.remote && value.local === mapping.local
          && value.label === mapping.label && value.enabled === true)), 'MANUAL_SERVICES_NOT_INDEPENDENTLY_SAVED');

      const frontend = await context.newPage();
      const response = await frontend.goto(serviceOrigin + '/');
      check(response?.status() === 200 && (await frontend.locator('body').innerText()) === config.marker, 'FRONTEND_HTTP_BYTES');
      const ws = await frontend.evaluate(serviceWebSocket);
      check(ws.protocol === 'splitty-hmr' && ws.code === 1000 && ws.reason === 'checked' && ws.clean
        && ws.messages.length === 3 && ws.messages[0].type === 'connected' && ws.messages[0].marker === config.marker
        && ws.messages[1].type === 'update' && ws.messages[1].path === '/src/App.tsx' && ws.messages[1].marker === config.marker
        && Array.isArray(ws.messages[2]) && ws.messages[2].length === 256
        && ws.messages[2].every((byte, index) => byte === index), 'RAW_WEBSOCKET_HMR_EXCHANGE');
      const denied = await serviceApi(config.apiService, false);
      check(denied.status === 401 && denied.bytes.toString() === 'app-auth-required', 'API_AUTH_MUST_REACH_SERVICE');
      const allowed = await serviceApi(config.apiService, true);
      const expected = Buffer.concat([Buffer.from(config.apiService.marker + '\n'), Buffer.from(Array.from({ length: 256 }, (_, i) => i))]);
      check(allowed.status === 200 && allowed.bytes.equals(expected), 'API_RAW_BYTES_OR_APPLICATION_AUTH_CHANGED');
      const tls = config.apiService.tlsCa === null
        ? { status: 'unrun', reason: 'openssl absent; API checked over HTTP' }
        : { status: 'pass', client: 'Node HTTPS with fixture CA and localhost verification', protocol: allowed.protocol };
      if (tls.status === 'pass') check(denied.authorized && allowed.authorized
        && ['TLSv1.2', 'TLSv1.3'].includes(allowed.protocol), 'TLS_PASSTHROUGH_NOT_VERIFIED');
      check(!violation, violation);
      report.checks.push({ scenario: 'manual-services-transport', manualSavedServices: 2,
        frontendHttp: true, rawWebSocket: true, hmrStyleUpdate: true,
        binaryBytes: 256, cleanClose: true, apiAuthAndBytes: true, tls });
    }
  } finally {
    const failures = [];
    const cleanup = async (name, work) => { try { await work(); } catch { failures.push(name); } };
    if (gate) {
      gate.release.resolve(false);
      if (gate.request) await cleanup('pending-route', () => within(gate.settled.promise, 'ROUTE_CLEANUP_TIMEOUT', 3000));
    }
    if (browser) await cleanup('browser', () => within(browser.close(), 'BROWSER_CLOSE_TIMEOUT', 5000));
    // Close browser first, so no delayed frontend work can recreate this mapping.
    if (ownedMapping) {
      const keys = [body];
      keys.push({ target: config.target, port: config.apiService.remote });
      for (const key of keys) {
        await cleanup('forward-stop', () => api('DELETE', ACTIVE, key, [204, 404]));
        await cleanup('preference-forget', () => api('DELETE', SAVED, key, [204]));
      }
      await cleanup('empty-state', async () => check(await cleanState(), 'CLEANUP_STATE_NOT_EMPTY'));
    }
    if (privateDir) await cleanup('private-browser-files', () => rm(privateDir, { recursive: true, force: true }));
    report.cleanup = failures.length ? { status: 'failed', stages: failures } : { status: 'pass', mappingOwned: ownedMapping };
    check(failures.length === 0, 'CLEANUP_FAILED');
  }
}

export async function main(argv = process.argv.slice(2)) {
  const report = { scope: 'D8 Open/Stop and opt-in manual service transport', status: 'not-run', checks: [] };
  try {
    if (argv.length === 1 && argv[0] === '--help') {
      console.log('Usage: node remote-forwards-browser.mjs --lab-only --config <private.json|->\n'
        + '       node remote-forwards-browser.mjs --check-module /absolute/playwright/index.mjs\n'
        + 'See scripts/testing/README.md for the private remote_connect lab contract.');
      return 0;
    }
    if (argv.length === 2 && argv[0] === '--check-module') {
      await playwright(argv[1]);
      console.log(JSON.stringify({ status: 'module-loaded', browserLaunched: false, featureTestsRun: false }));
      return 0;
    }
    check(argv.length === 3 && argv[0] === '--lab-only' && argv[1] === '--config' && argv[2], 'EXPLICIT_LAB_ONLY_ACK_REQUIRED', true);
    report.stage = 'config';
    const config = await readConfig(argv[2]);
    await run(config, report);
    report.status = 'pass';
    report.stage = 'complete';
  } catch (error) {
    report.status = error instanceof CheckError && error.blocked ? 'blocked' : 'fail';
    // Never emit raw Playwright/fetch errors, response bodies, headers or config.
    report.code = error instanceof CheckError ? error.code : 'UNEXPECTED_ERROR';
  }
  console.log(JSON.stringify(report));
  return report.status === 'pass' ? 0 : report.status === 'blocked' ? 2 : 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = await main();
}
