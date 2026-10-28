// Front-end errors and the recent route trail, sent to the hub's logs/web.log.
// Only event kinds, short messages and paths are recorded, never conversation text.

const PER_MINUTE = 30;
const TRAIL = 12;
const REPEAT_MS = 10000;
const state = { installed: false, queue: [], trail: [], seen: new Map(), window: 0, count: 0, dropped: 0, timer: 0 };

function here() {
  return location.pathname;
}

function step(to) {
  if (state.trail[state.trail.length - 1] === to) return;
  state.trail.push(to);
  if (state.trail.length > TRAIL) state.trail.shift();
}

function clip(text, max) {
  return String(text ?? '').replace(/\s+/g, ' ').trim().slice(0, max);
}

export function note(kind, message, max = 300) {
  const now = Date.now();
  if (now - state.window > 60000) {
    state.window = now;
    state.count = 0;
  }
  const text = clip(message, max);
  const key = `${kind}\n${text}`;
  if (now - (state.seen.get(key) ?? 0) < REPEAT_MS) return;
  if (state.count >= PER_MINUTE) {
    state.dropped += 1;
    return;
  }
  if (state.seen.size > 200) state.seen.clear();
  state.seen.set(key, now);
  state.count += 1;
  state.queue.push({ kind, message: text, path: here(), trail: state.trail.slice() });
  if (!state.timer) state.timer = setTimeout(() => flush(false), 2000);
}

export function flush(beacon) {
  clearTimeout(state.timer);
  state.timer = 0;
  if (state.dropped) {
    state.queue.push({ kind: 'dropped', message: `${state.dropped} more events over the limit`, path: here(), trail: [] });
    state.dropped = 0;
  }
  if (!state.queue.length) return;
  const body = JSON.stringify(state.queue.splice(0));
  if (beacon && navigator.sendBeacon) {
    navigator.sendBeacon('/api/client/log', new Blob([body], { type: 'application/json' }));
    return;
  }
  fetch('/api/client/log', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body, keepalive: true }).catch(() => {});
}

export function panicked(message) {
  note('panic', message, 2000);
  flush(true);
}

export function install() {
  if (state.installed) return;
  state.installed = true;
  window.addEventListener('error', e => {
    const at = e.filename ? ` (${e.filename.split('/').pop()}:${e.lineno})` : '';
    note('error', `${e.message}${at}`);
  });
  window.addEventListener('unhandledrejection', e => note('rejection', e.reason?.message ?? e.reason));
  const original = console.error.bind(console);
  console.error = (...args) => {
    const text = args.map(a => a?.message ?? String(a)).join(' ');
    if (!text.startsWith('panicked at')) note('console', text);
    original(...args);
  };
  for (const name of ['pushState', 'replaceState']) {
    const original = history[name].bind(history);
    history[name] = (...args) => {
      const out = original(...args);
      step(here());
      return out;
    };
  }
  window.addEventListener('popstate', () => step(here()));
  window.addEventListener('pagehide', () => flush(true));
  step(here());
}
