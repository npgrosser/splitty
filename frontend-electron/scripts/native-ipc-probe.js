// Test-only client loaded into a real sandboxed WebContents. It uses the public
// preload surface and wire contract; no Node access or private transport hooks.
(async () => {
  const bridge = window.__SPLITTY_ELECTRON__.native;
  const assignment = await bridge.open();
  const encoder = new TextEncoder(), decoder = new TextDecoder('utf-8', {fatal: true});
  const waits = new Set(), events = [], replies = new Map();
  let buffered = new Uint8Array(), nextId = 0, closed;
  const binary = {bytes: 0, tail: [], sessions: {}};
  function encode(kind, payload) {
    const frame = new Uint8Array(5 + payload.length); frame[0] = kind;
    new DataView(frame.buffer).setUint32(1, payload.length, false); frame.set(payload, 5); return frame;
  }
  const json = value => encoder.encode(JSON.stringify(value));
  function send(value, kind = 1) {
    if (closed) throw Error(closed.message);
    const body = json(value), payload = new Uint8Array(body.length + 20);
    new DataView(payload.buffer).setUint32(0, assignment.generation, false);
    payload.set(body, 4); payload.set(crypto.getRandomValues(new Uint8Array(16)), payload.length - 16);
    bridge.write(assignment.handle, encode(kind, payload));
  }
  function next(type, field, expected) {
    const match = event => event.type === type && (field === undefined || event[field] === expected);
    const index = events.findIndex(match);
    if (index !== -1) return Promise.resolve(events.splice(index, 1)[0]);
    return new Promise((resolve, reject) => {
      if (closed) { reject(Error(closed.message)); return; }
      const wait = {match, resolve(value) { clearTimeout(timer); waits.delete(wait); resolve(value); },
        reject(error) { clearTimeout(timer); waits.delete(wait); reject(error); }};
      const timer = setTimeout(() => wait.reject(Error(`No ${type} event`)), 10_000);
      waits.add(wait);
    });
  }
  bridge.listen(assignment.handle, chunk => {
    if (buffered.length + chunk.length > 16 * 1024 * 1024) throw Error('oversized probe input');
    const joined = new Uint8Array(buffered.length + chunk.length);
    joined.set(buffered); joined.set(chunk, buffered.length); buffered = joined;
    while (buffered.length >= 5) {
      const length = new DataView(buffered.buffer).getUint32(1, false);
      if (buffered.length < length + 5) break;
      const kind = buffered[0], payload = buffered.slice(5, length + 5);
      buffered = buffered.slice(length + 5);
      if (kind === 10) continue;
      const body = payload.subarray(4, payload.length - 16);
      const receipt = Array.from(payload.subarray(payload.length - 16), b => b.toString(16).padStart(2, '0')).join('');
      bridge.write(assignment.handle, encode(10, json({generation: assignment.generation, receipt})));
      if (kind === 2) {
        const id = decoder.decode(body.subarray(1, body[0] + 1)), bytes = body.subarray(body[0] + 1);
        binary.bytes += bytes.length; binary.sessions[id] = (binary.sessions[id] || 0) + bytes.length;
        binary.tail = [...binary.tail, ...bytes.subarray(Math.max(0, bytes.length - 64))].slice(-64);
      } else {
        const value = JSON.parse(decoder.decode(body));
        if (kind === 6) { const reply = replies.get(value.id); replies.delete(value.id); reply?.resolve(value); }
        else {
          const wait = [...waits].find(wait => wait.match(value));
          if (wait) wait.resolve(value);
          else { events.push(value); if (events.length > 64) events.shift(); }
        }
      }
    }
  }, reason => {
    closed = reason;
    for (const wait of [...waits]) wait.reject(Error(reason.message));
    for (const reply of replies.values()) reply.reject(Error(reason.message)); replies.clear();
  });
  window.smokeClient = {assignment, binary, send, next,
    get closed() { return closed; },
    request(operation, args = {}) {
      const id = ++nextId;
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => { replies.delete(id); reject(Error(`No ${operation} reply`)); }, 10_000);
        replies.set(id, {resolve(value) { clearTimeout(timer); resolve(value); }, reject(error) { clearTimeout(timer); reject(error); }});
        try { send({id, operation, args}, 5); } catch (error) { clearTimeout(timer); replies.delete(id); reject(error); }
      });
    },
    close() { bridge.close(assignment.handle); },
  };
  const welcome = next('mux.welcome'); bridge.start(assignment.handle); await welcome;
  return assignment;
})()
