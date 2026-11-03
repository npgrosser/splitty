(() => {
  'use strict';
  const config = __SPLITTY_BOOTSTRAP_CONFIG__;
  // Initialization scripts are restricted to the main frame by the builder.
  // This second check also refuses an accidentally inherited child-frame script.
  if (window.top !== window) return;
  const here = new URL(window.location.href), entry = new URL(config.entry);
  const path = value => value === '' || value === '/' ? '/index.html' : value;
  if (here.protocol !== entry.protocol || here.hostname !== entry.hostname ||
      here.port !== entry.port || here.username || here.password ||
      !(path(here.pathname) === path(entry.pathname) ||
        (entry.pathname === '/setup.html' && path(here.pathname) === '/index.html'))) return;
  const internals = window.__TAURI_INTERNALS__;
  if (!internals || typeof internals.invoke !== 'function' ||
      typeof internals.transformCallback !== 'function') return;
  const rawInvoke = internals.invoke.bind(internals);
  const register = internals.transformCallback.bind(internals);
  const unregister = internals.unregisterCallback.bind(internals);
  const nonce = Array.from(crypto.getRandomValues(new Uint8Array(32)), byte => byte.toString(16).padStart(2, '0')).join('');
  let auth, dead = false, confirmation = 0;
  let resolveAuth, rejectAuth;
  const authenticated = new Promise((resolve, reject) => { resolveAuth = resolve; rejectAuth = reject; });
  // Avoid an unhandled rejection before the packaged entry starts waiting.
  authenticated.catch(() => {});
  let bootstrapFailed = false;
  const bootstrapTimer = setTimeout(() => {
    bootstrapFailed = true;
    rejectAuth(Error('Native document bootstrap timed out'));
  }, 15000);
  authenticated.then(() => clearTimeout(bootstrapTimer), () => { bootstrapFailed = true; clearTimeout(bootstrapTimer); });
  const secretBytes = value => Array.isArray(value) && value.length === 32 && value.every(byte => Number.isInteger(byte) && byte >= 0 && byte < 256);
  const ready = authenticated.then(result => Object.freeze({ role: result.role, ...(result.target === undefined ? {} : { target: result.target }) }));
  ready.catch(() => {});
  const records = new Map();
  const outputs = new Map();
  const channels = new Set();
  const reject = message => Promise.reject(Error(message));
  const authorized = async (command, args = {}) => {
    await authenticated;
    if (dead) throw Error('Native document closed');
    return rawInvoke(command, { ...args, auth });
  };
  // Closing stops delivery immediately, but an unresolved native invocation
  // still owns its callbacks/arguments. Retired records keep the same slot
  // until opening and every normal/cleanup invocation have actually settled.
  const releaseRecord = record => {
    if (record.ended && !record.opening && record.pendingCalls === 0) channels.delete(record);
  };
  const tracked = (record, invoke) => {
    record.pendingCalls++;
    let result;
    try { result = invoke(); } catch (error) { result = Promise.reject(error); }
    return Promise.resolve(result).finally(() => {
      record.pendingCalls--;
      releaseRecord(record);
    });
  };
  const cleanup = record => {
    if (!record.handle || record.cleanupSent) return;
    const cleanupAuth = record.auth;
    if (!cleanupAuth) return;
    record.cleanupSent = true;
    record.auth = undefined;
    // This private cleanup path can only acknowledge fetched packets and
    // close their original handle. It never waits on or bypasses general
    // document authorization for opening, data or shell actions.
    // These packets were fetched from the framework. Discard their JS buffer
    // references before acknowledging delivery, including a decoder failure.
    for (const token of record.receipts) {
      void tracked(record, () => rawInvoke('native_consumed', { auth: cleanupAuth, handle: record.handle, token: token.match(/../g).map(byte => parseInt(byte, 16)) })).catch(() => {});
    }
    record.receipts.clear();
    void tracked(record, () => rawInvoke('native_close', { auth: cleanupAuth, handle: record.handle })).catch(() => {});
  };
  const closeRecord = (record, reason) => {
    if (record.ended) return;
    record.ended = true;
    record.pending = undefined;
    record.failure = undefined;
    record.reorder.clear();
    unregister(record.callback);
    outputs.delete(record.callback);
    const closed = record.closed;
    record.data = record.closed = undefined;
    if (record.handle) {
      records.delete(record.handle);
      cleanup(record);
    }
    releaseRecord(record);
    // A consumer error cannot prevent cleanup of sibling channels on pagehide.
    try { if (closed) closed(reason || Error('Native connection closed')); } catch {}
  };
  const packet = (record, value) => {
    const bytes = value instanceof ArrayBuffer ? new Uint8Array(value) :
      ArrayBuffer.isView(value) ? new Uint8Array(value.buffer, value.byteOffset, value.byteLength) : undefined;
    if (!bytes || !bytes.length || bytes.length > 65553) throw Error('Invalid native Channel packet');
    if (bytes[0] === 0 && bytes.length === 1) { closeRecord(record); return; }
    if (bytes[0] !== 1 || bytes.length < 18) throw Error('Invalid native Channel packet');
    const token = Array.from(bytes.subarray(1, 17), byte => byte.toString(16).padStart(2, '0')).join('');
    const payload = bytes.subarray(17);
    if (record.data) record.data(payload, token);
    else if (!record.pending) record.pending = { payload, token };
    else throw Error('Native Channel delivery capacity exhausted');
  };
  const channel = record => {
    record.callback = register(envelope => {
      if (record.ended || dead) return;
      try {
        // Native evaluation owns its source until this synchronous delivery
        // returns; receipt credit is independent from evaluation completion.
        if (envelope && !('end' in envelope)) {
          const value = envelope.message;
          const bytes = value instanceof ArrayBuffer ? new Uint8Array(value) :
            ArrayBuffer.isView(value) ? new Uint8Array(value.buffer, value.byteOffset, value.byteLength) : undefined;
          if (bytes && bytes.length >= 18 && bytes.length <= 65553 && bytes[0] === 1) {
            record.receipts.add(Array.from(bytes.subarray(1, 17), byte => byte.toString(16).padStart(2, '0')).join(''));
            if (record.receipts.size > 2) throw Error('Native delivery receipt capacity exhausted');
          }
        }
        const index = envelope.index;
        if (!Number.isSafeInteger(index) || index < record.next || index > record.next + 1 || record.reorder.has(index)) throw Error('Invalid native Channel order');
        if (index !== record.next) {
          if (record.reorder.size >= 1) throw Error('Native Channel ordering capacity exhausted');
          // Bound storage before retaining a framework-delivered packet.
          if (!('end' in envelope)) {
            const value = envelope.message;
            if (!(value instanceof ArrayBuffer) && !ArrayBuffer.isView(value)) throw Error('Invalid native Channel packet');
            if (value.byteLength > 65553) throw Error('Invalid native Channel packet');
          }
          record.reorder.set(index, envelope);
          return;
        }
        while (envelope) {
          record.next++;
          if ('end' in envelope) { closeRecord(record); return; }
          packet(record, envelope.message);
          if (record.ended) return;
          envelope = record.reorder.get(record.next);
          record.reorder.delete(record.next);
        }
      } catch (error) { closeRecord(record, error); }
    });
    outputs.set(record.callback, record);
    return record.callback;
  };
  Object.defineProperty(window, `${config.receiver}_output`, { value: (document, callback, envelope) => {
    if (dead || !auth || document !== auth.document) return false;
    const record = outputs.get(callback);
    if (!record || record.ended) return false;
    if (envelope && 'failure' in envelope) {
      if (!record.opening || record.failure !== undefined) return false;
      const failure = envelope.failure;
      if (!failure || typeof failure.message !== 'string' ||
          !(failure.hint === null || typeof failure.hint === 'string') ||
          failure.code !== 'backendLost' || typeof failure.retryable !== 'boolean') return false;
      record.failure = failure;
      return true;
    }
    internals.runCallback(callback, envelope);
    return !record.ended;
  } });
  const handleRecord = handle => {
    const record = records.get(handle);
    if (!record || record.ended || dead) throw Error('Native connection closed');
    return record;
  };
  let pendingState, reportingState = false;
  const reportState = async () => {
    if (reportingState) return;
    reportingState = true;
    try {
      while (pendingState !== undefined && !dead) {
        const state = pendingState; pendingState = undefined;
        try { await authorized('native_connection_state', { state }); } catch { /* Revoked documents cannot update a replacement. */ }
      }
    } finally { reportingState = false; }
  };
  const native = Object.freeze({
    state(state) {
      if (!['connected', 'reconnecting', 'disconnected'].includes(state)) throw Error('Invalid native connection state');
      pendingState = state;
      void reportState();
    },
    async open(options, request) {
      if (!Number.isInteger(request) || request < 1 || request > 0xffffffff) throw Error('Invalid native request');
      if (channels.size >= 32 || dead) throw Error('Native channel capacity exhausted');
      const record = { next: 0, reorder: new Map(), receipts: new Set(), acknowledging: new Set(), ended: false, opening: true, pendingCalls: 0 };
      channels.add(record);
      const output = channel(record);
      try {
        await authenticated;
        if (dead || record.ended) throw Error('Native document closed');
        record.auth = auth;
        const result = await tracked(record, () => rawInvoke('native_open', { auth: record.auth, request, target: options?.target, update: options?.update, output }));
        if (typeof result.handle !== 'string' || typeof result.document !== 'string' ||
            !Number.isInteger(result.generation) || typeof result.instance !== 'string' ||
            !['main', 'remote'].includes(result.role)) throw Error('Invalid native assignment');
        record.handle = result.handle;
        if (dead || record.ended) {
          cleanup(record);
          throw Error('Native connection closed during open');
        }
        records.set(result.handle, record);
        return Object.freeze({ handle: result.handle, document: result.document, generation: result.generation,
          instance: result.instance, role: result.role, ...(result.target === undefined ? {} : { target: result.target }) });
      } catch (error) {
        if (error === 'SplittyNativeFailureDelivered' && record.failure !== undefined && !dead && !record.ended) {
          error = `SplittyNativeFailure:${JSON.stringify(record.failure)}`;
        } else if (error === 'SplittyNativeFailureDelivered' || error === 'SplittyNativeFailureUnavailable') {
          error = 'Native backend failed; diagnostic unavailable';
        }
        record.failure = undefined;
        closeRecord(record, error); throw error;
      }
      finally { record.opening = false; releaseRecord(record); }
    },
    cancelOpen(request) {
      if (Number.isInteger(request) && request >= 1 && request <= 0xffffffff) {
        void authorized('native_cancel_open', { request }).catch(() => {});
      }
    },
    listen(handle, data, closed) {
      const record = handleRecord(handle);
      if (record.data || typeof data !== 'function' || typeof closed !== 'function') throw Error('Invalid native listener');
      record.data = data; record.closed = closed;
      if (record.pending) {
        const pending = record.pending; record.pending = undefined;
        try { data(pending.payload, pending.token); } catch (error) { closeRecord(record, error); }
      }
      return () => { record.data = record.closed = undefined; };
    },
    start(handle) { const record = handleRecord(handle); return tracked(record, () => authorized('native_start', { handle })); },
    write(handle, bytes) {
      const record = handleRecord(handle);
      if (!(bytes instanceof Uint8Array) || !bytes.length || bytes.length > 65536) return reject('Invalid native write');
      return tracked(record, () => authorized('native_write', { handle, bytes: Array.from(bytes) }));
    },
    consumed(handle, token) {
      const record = handleRecord(handle);
      if (typeof token !== 'string' || !/^[0-9a-f]{32}$/.test(token)) return reject('Invalid native receipt');
      if (!record.receipts.has(token)) return reject('Unknown native receipt');
      if (record.acknowledging.has(token)) return reject('Duplicate native receipt');
      record.acknowledging.add(token);
      // Native consumption releases the next packet before its invoke reply
      // reaches JavaScript. Serialize acknowledgements so delayed replies
      // cannot accumulate tokens/promises: one call and its next packet only.
      const previous = record.acknowledged || Promise.resolve();
      const acknowledged = tracked(record, () => previous.then(() => {
        if (record.ended) throw Error('Native connection closed');
        return authorized('native_consumed', { handle, token: token.match(/../g).map(byte => parseInt(byte, 16)) });
      }).then(() => { record.receipts.delete(token); })
        .finally(() => { record.acknowledging.delete(token); }));
      record.acknowledged = acknowledged;
      return acknowledged;
    },
    close(handle) { const record = records.get(handle); if (record) closeRecord(record); },
  });
  const shell = new Set(['remote_hosts', 'remote_open', 'remote_cancel', 'remote_progress',
    'open_local_editor', 'open_remote_editor', 'reveal_download', 'set_title_bar_color', 'close_popout',
    'wsl_setup_state', 'wsl_list_distributions', 'wsl_start_backend', 'wsl_open_backend', 'wsl_switch_distribution',
    'plugin:opener|open_url']);
  const invoke = (command, args) => shell.has(command) ? authorized(command, args) : reject('Native shell command denied');
  Object.defineProperty(window, '__SPLITTY_TAURI__', { value: Object.freeze({ native, ready, invoke }) });
  Object.defineProperty(window, config.receiver, { value: async message => {
    if (dead || bootstrapFailed || auth || !message || message.nonce !== nonce || !secretBytes(message.challenge)) return;
    const attempt = ++confirmation;
    try {
      const result = await rawInvoke('native_bootstrap_confirm', { bootstrap: config.bootstrap, nonce, challenge: message.challenge });
      if (dead || bootstrapFailed || attempt !== confirmation || auth) return;
      if (!result || !result.auth || !Number.isSafeInteger(result.auth.document) || !secretBytes(result.auth.secret) ||
          !['main', 'remote', 'setup'].includes(result.role) || (result.target != null && typeof result.target !== 'string')) throw Error('Invalid native document assignment');
      auth = Object.freeze({ document: result.auth.document, secret: Object.freeze(result.auth.secret.slice()) });
      resolveAuth({ role: result.role, ...(result.target == null ? {} : { target: result.target }) });
    } catch (error) { if (!dead && attempt === confirmation) rejectAuth(error); }
  } });
  addEventListener('pagehide', () => {
    dead = true; confirmation++;
    rejectAuth(Error('Native document closed'));
    for (const record of channels) closeRecord(record);
    records.clear(); auth = undefined;
  }, { once: true });
  // Invoke readiness does not wait for the UI module (which itself awaits ready).
  void rawInvoke('native_bootstrap_begin', { bootstrap: config.bootstrap, nonce }).catch(rejectAuth);
})();
