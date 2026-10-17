import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const listeners = new Map();
const asked = [];
let answer = false;
globalThis.location = new URL('http://hub.test/w/example');
globalThis.window = {
  addEventListener: (name, callback) => listeners.set(name, callback),
  dispatchEvent: (event) => { listeners.get(event.type)?.(event); return !event.defaultPrevented; },
};
const pushed = [];
let backs = 0;
globalThis.history = { pushState: (_, __, path) => pushed.push(path), back: () => { backs++; } };
const source = await readFile(new URL('../js/files.js', import.meta.url), 'utf8');
const bridge = await import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);
bridge.setLeaveConfirm(message => {
  asked.push(message);
  return Promise.resolve(answer);
});

const followed = [];
const settle = () => new Promise(resolve => setTimeout(resolve, 0));

function fire(anchor, modifiers = {}) {
  const event = {
    button: 0, defaultPrevented: false, prevented: false, stopped: false,
    target: { closest: () => anchor },
    preventDefault() { this.prevented = true; },
    stopImmediatePropagation() { this.stopped = true; },
    ...modifiers,
  };
  listeners.get('click')(event);
  return event;
}

function link(path) {
  const anchor = { href: new URL(path, location).href, isConnected: true, hasAttribute: () => false, target: '' };
  anchor.click = () => {
    if (!fire(anchor).prevented) followed.push(path);
  };
  return anchor;
}

const click = (path, modifiers) => fire(link(path), modifiers);

test('unsaved edits ask inside the app and only leave once the user agrees', async () => {
  let dirty = 2;
  bridge.setNavigationGuard('example', () => dirty);
  answer = false;
  const tasks = link('/tasks');
  const first = fire(tasks);
  assert.equal(first.prevented, true);
  assert.equal(first.stopped, true);
  await settle();
  assert.match(asked.at(-1), /2 个文件/);
  assert.deepEqual(followed, []);

  answer = true;
  fire(tasks);
  await settle();
  assert.deepEqual(followed, ['/tasks']);

  const before = asked.length;
  assert.equal(click('/w/example?thread=second').prevented, false);
  assert.equal(click('/workspaces/example').prevented, false);
  assert.equal(click('/tasks', { ctrlKey: true }).prevented, false);
  dirty = 0;
  assert.equal(click('/tasks').prevented, false);
  assert.equal(asked.length, before);
});

test('back and forward stay on the workspace until the user agrees', async () => {
  bridge.setNavigationGuard('example', () => 1);
  answer = false;
  location = new URL('http://hub.test/inbox');
  let stopped = false;
  listeners.get('popstate')({ stopImmediatePropagation() { stopped = true; } });
  assert.equal(stopped, true);
  assert.equal(pushed.at(-1), '/w/example');
  await settle();
  assert.equal(backs, 0);

  answer = true;
  listeners.get('popstate')({ stopImmediatePropagation() {} });
  await settle();
  assert.equal(backs, 1);
  let passed = true;
  listeners.get('popstate')({ stopImmediatePropagation() { passed = false; } });
  assert.equal(passed, true);
  location = new URL('http://hub.test/w/example');
});

test('pages and guards report unsaved work without asking by themselves', () => {
  bridge.setNavigationGuard('new-workspace', () => 1);
  bridge.clearNavigationGuard('example');
  assert.notEqual(bridge.leaveWarning(), '');
  bridge.clearNavigationGuard('new-workspace');
  assert.equal(bridge.leaveWarning(), '');

  listeners.set('blazar:before-navigate', event => event.preventDefault());
  assert.notEqual(bridge.leaveWarning(), '');
  listeners.delete('blazar:before-navigate');
  assert.equal(bridge.leaveWarning(), '');
});
