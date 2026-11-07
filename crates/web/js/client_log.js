// Front-end errors and the recent route trail, sent to the hub's logs/web.log.
// Only event kinds, short messages and paths are recorded, never conversation text.

const PER_MINUTE = 30;
const TRAIL = 12;
const REPEAT_MS = 10000;
const BEAT_MS = 500;
const STALL_MS = 2000;
const SLOW_FETCH_MS = 1000;
const state = { installed: false, queue: [], trail: [], seen: new Map(), window: 0, count: 0, dropped: 0, timer: 0 };
const live = { last: 0, seq: 0, inflight: new Map(), recent: [], doing: null, finished: null, input: null };

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

function secs(ms) {
  return `${(ms / 1000).toFixed(1)}s`;
}

function shortPath(url) {
  try {
    return new URL(url, location.href).pathname;
  } catch {
    return clip(url, 80).split('?')[0];
  }
}

function describe(target) {
  const el = target instanceof Element ? target.closest('[aria-label], button, a, [role], input, textarea') : null;
  if (!el) return 'page';
  const label = el.getAttribute('aria-label') || el.getAttribute('title') || el.getAttribute('role') || '';
  const cls = (el.getAttribute('class') || '').split(/\s+/)[0];
  return clip(`${el.tagName.toLowerCase()}${cls ? '.' + cls : ''}${label ? ` "${label}"` : ''}`, 60);
}

export function context(now = Date.now()) {
  const parts = [];
  if (live.doing) parts.push(`doing ${live.doing.label} for ${secs(now - live.doing.at)}`);
  if (live.finished && now - live.finished.at < STALL_MS * 4) {
    parts.push(`just did ${live.finished.label} in ${secs(live.finished.took)}`);
  }
  const flying = [...live.inflight.values()].slice(0, 4).map(r => `${r.method} ${r.path} ${secs(now - r.at)}`);
  if (flying.length) parts.push(`in flight ${flying.join(', ')}`);
  if (live.recent.length) parts.push(`slow ${live.recent.slice(-3).join(', ')}`);
  if (live.input && now - live.input.at < 30000) parts.push(`last input ${live.input.what} ${secs(now - live.input.at)} ago`);
  return parts.join('; ') || 'nothing tracked';
}

export function doing(label) {
  live.doing = { label: clip(label, 80), at: Date.now() };
}

export function done() {
  if (!live.doing) return;
  const now = Date.now();
  live.finished = { label: live.doing.label, at: now, took: now - live.doing.at };
  live.doing = null;
}

function track(input, init) {
  const url = typeof input === 'string' ? input : input?.url ?? String(input ?? '');
  const path = shortPath(url);
  if (path === '/api/client/log') return null;
  const method = String(init?.method || input?.method || 'GET').toUpperCase();
  const id = ++live.seq;
  live.inflight.set(id, { method, path, at: Date.now() });
  return id;
}

function untrack(id) {
  if (id === null) return;
  const r = live.inflight.get(id);
  live.inflight.delete(id);
  if (!r) return;
  const took = Date.now() - r.at;
  if (took < SLOW_FETCH_MS) return;
  live.recent.push(`${r.method} ${r.path} ${secs(took)}`);
  if (live.recent.length > 6) live.recent.shift();
}

function beat() {
  const now = performance.now();
  if (document.visibilityState !== 'visible') {
    live.last = 0;
    return;
  }
  const gap = live.last ? now - live.last : 0;
  live.last = now;
  if (gap > STALL_MS) {
    note('stall', `page blocked ${Math.round(gap - BEAT_MS)} ms; ${context()}`, 700);
    flush(false);
  }
}

function watchPage() {
  const realFetch = window.fetch.bind(window);
  window.fetch = (input, init) => {
    const id = track(input, init);
    return realFetch(input, init).then(
      res => {
        untrack(id);
        return res;
      },
      err => {
        untrack(id);
        throw err;
      },
    );
  };
  const onInput = what => e => {
    live.input = { what: what(e), at: Date.now() };
  };
  window.addEventListener('pointerdown', onInput(e => `click ${describe(e.target)}`), { capture: true, passive: true });
  window.addEventListener('keydown', onInput(e => `key ${e.key.length === 1 ? 'typing' : e.key}`), { capture: true, passive: true });
  document.addEventListener('visibilitychange', () => {
    live.last = 0;
  });
  setInterval(beat, BEAT_MS);
  if (typeof PerformanceObserver !== 'undefined' && PerformanceObserver.supportedEntryTypes?.includes('longtask')) {
    new PerformanceObserver(list => {
      for (const e of list.getEntries()) {
        if (e.duration > STALL_MS) note('stall', `long task ${Math.round(e.duration)} ms; ${context()}`, 700);
      }
    }).observe({ type: 'longtask' });
  }
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
  window.addEventListener('unhandledrejection', e => {
    if (e.reason?.name !== 'Canceled') note('rejection', e.reason?.message ?? e.reason);
  });
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
  watchPage();
}
