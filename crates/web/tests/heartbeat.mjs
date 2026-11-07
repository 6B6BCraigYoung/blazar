import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

let clock = 1000;
let tick = null;
const posted = [];
globalThis.location = new URL('http://hub.test/w/demo');
globalThis.document = { visibilityState: 'visible', addEventListener: () => {} };
globalThis.history = { pushState: () => {}, replaceState: () => {} };
Object.defineProperty(globalThis, 'performance', { value: { now: () => clock }, configurable: true, writable: true });
globalThis.setInterval = fn => {
  tick = fn;
  return 1;
};
globalThis.fetch = async (url, init) => {
  if (String(url).endsWith('/api/client/log')) posted.push(...JSON.parse(init.body));
  return { ok: true };
};
globalThis.window = { addEventListener: () => {}, fetch: globalThis.fetch };

const source = await readFile(new URL('../js/client_log.js', import.meta.url), 'utf8');
const log = await import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);
log.install();
const settle = () => new Promise(resolve => setTimeout(resolve, 0));

test('a long gap on a visible page is logged with what it was doing', async () => {
  tick();
  log.doing('rendering conversation history (12000 rows)');
  clock += 500;
  tick();
  clock += 3500;
  log.done();
  tick();
  await settle();
  const stall = posted.find(e => e.kind === 'stall');
  assert.ok(stall, 'a stall is reported');
  assert.match(stall.message, /page blocked 3000 ms/);
  assert.match(stall.message, /rendering conversation history \(12000 rows\)/);
  assert.equal(stall.path, '/w/demo');
});

test('a hidden page is never blamed for the gap timers take while hidden', async () => {
  const before = posted.length;
  document.visibilityState = 'hidden';
  clock += 20000;
  tick();
  document.visibilityState = 'visible';
  clock += 9000;
  tick();
  clock += 500;
  tick();
  await settle();
  assert.equal(posted.length, before);
});

test('requests still waiting are named in the context, without their query', async () => {
  const call = window.fetch('/api/workspaces/w1/history?session=t1');
  assert.match(log.context(), /in flight GET \/api\/workspaces\/w1\/history \d/);
  assert.doesNotMatch(log.context(), /session=t1/);
  await call;
  assert.doesNotMatch(log.context(), /in flight/);
});
