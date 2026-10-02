import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const listeners = new Map();
let confirmations = 0;
let accepted = false;
globalThis.location = new URL('http://hub.test/w/example');
globalThis.window = {
  addEventListener: (name, callback) => listeners.set(name, callback),
  dispatchEvent: (event) => { listeners.get(event.type)?.(event); return !event.defaultPrevented; },
  confirm: () => { confirmations++; return accepted; },
};
const pushed = [];
globalThis.history = { pushState: (_, __, path) => pushed.push(path) };
const source = await readFile(new URL('../js/files.js', import.meta.url), 'utf8');
const bridge = await import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);

function click(path, modifiers = {}) {
  const event = {
    button: 0, defaultPrevented: false, prevented: false, stopped: false,
    target: { closest: () => ({ href: new URL(path, location).href, hasAttribute: () => false, target: '' }) },
    preventDefault() { this.prevented = true; },
    stopImmediatePropagation() { this.stopped = true; },
    ...modifiers,
  };
  listeners.get('click')(event);
  return event;
}

test('unsaved edits survive cancelled links, shortcuts and browser history', () => {
  let dirty = 2;
  bridge.setNavigationGuard('example', () => dirty);
  assert.equal(click('/tasks').prevented, true);
  assert.equal(click('/tasks').stopped, true);
  assert.equal(bridge.confirmNavigation(), false);
  const before = confirmations;
  assert.equal(click('/w/example?thread=second').prevented, false);
  assert.equal(click('/workspaces/example').prevented, false);
  assert.equal(click('/tasks', { ctrlKey: true }).prevented, false);
  assert.equal(confirmations, before);

  location = new URL('http://hub.test/inbox');
  let stopped = false;
  listeners.get('popstate')({ stopImmediatePropagation() { stopped = true; } });
  assert.equal(stopped, true);
  assert.deepEqual(pushed, ['/w/example']);

  accepted = true;
  assert.equal(click('/tasks').prevented, false);
  accepted = false;
  dirty = 0;
  assert.equal(click('/tasks').prevented, false);
  bridge.setNavigationGuard('new-workspace', () => 1);
  bridge.clearNavigationGuard('example');
  assert.equal(bridge.confirmNavigation(), false);
  bridge.clearNavigationGuard('new-workspace');
  assert.equal(bridge.confirmNavigation(), true);
});


test('programmatic navigation respects unsaved editors outside workspaces', () => {
  listeners.set('blazar:before-navigate', event => event.preventDefault());
  assert.equal(bridge.confirmNavigation(), false);
  listeners.delete('blazar:before-navigate');
  assert.equal(bridge.confirmNavigation(), true);
});
