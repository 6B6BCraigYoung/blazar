const INTERACTIVE = 'a[href],button,input,textarea,select,summary,[role="button"],[role="menuitem"],[role="tab"],[role="option"],[role="checkbox"],[role="switch"],[contenteditable="true"],[tabindex]:not([tabindex="-1"])';

const GUARDED = '.xterm, [data-ui-guard]';

const state = { refs: new Map(), next: 1, errors: [], installed: false };

function guarded(el) {
  return !!(el && el.closest && el.closest(GUARDED));
}

function guard(el) {
  if (guarded(el)) throw new Error('受保护的区域：界面驱动不能操作终端、审批和权限控件');
  return el;
}

function install() {
  if (state.installed) return;
  state.installed = true;
  window.addEventListener('error', e => state.errors.push(`error: ${e.message}`));
  window.addEventListener('unhandledrejection', e => state.errors.push(`rejection: ${e.reason}`));
  const original = console.error.bind(console);
  console.error = (...args) => {
    state.errors.push(args.map(String).join(' '));
    original(...args);
  };
}

function visible(el) {
  const r = el.getBoundingClientRect();
  if (r.width < 1 || r.height < 1) return false;
  if (r.bottom < 0 || r.right < 0 || r.top > innerHeight || r.left > innerWidth) return false;
  const s = getComputedStyle(el);
  return s.visibility !== 'hidden' && s.display !== 'none' && Number(s.opacity) > 0;
}

function label(el) {
  const text = el.getAttribute('aria-label') || el.getAttribute('title') || el.innerText
    || el.value || el.getAttribute('placeholder') || '';
  return text.trim().replace(/\s+/g, ' ').slice(0, 100);
}

function refOf(el) {
  for (const [id, node] of state.refs) if (node === el) return id;
  const id = `e${state.next++}`;
  state.refs.set(id, el);
  return id;
}

function rect(el) {
  const r = el.getBoundingClientRect();
  return [Math.round(r.x), Math.round(r.y), Math.round(r.width), Math.round(r.height)];
}

function snapshot() {
  for (const [id, el] of state.refs) if (!el.isConnected) state.refs.delete(id);
  const items = [...document.querySelectorAll(INTERACTIVE)].filter(visible).map(el => {
    const item = { ref: refOf(el), tag: el.tagName.toLowerCase(), name: label(el), rect: rect(el) };
    const role = el.getAttribute('role');
    if (role) item.role = role;
    if (guarded(el)) item.guarded = true;
    if (el.disabled || el.getAttribute('aria-disabled') === 'true') item.disabled = true;
    if (el.getAttribute('aria-pressed') === 'true' || el.getAttribute('aria-selected') === 'true' || el.checked) item.on = true;
    if ('value' in el && el.value && el.tagName !== 'BUTTON') item.value = String(el.value).slice(0, 200);
    return item;
  });
  const headings = [...document.querySelectorAll('h1,h2,h3')].filter(visible).map(h => label(h)).filter(Boolean);
  return {
    url: location.pathname + location.search,
    title: document.title,
    viewport: [innerWidth, innerHeight],
    headings,
    items,
    text: (document.body.innerText || '').replace(/\n{3,}/g, '\n\n').slice(0, 4000),
  };
}

function find(command) {
  if (command.ref) {
    const el = state.refs.get(command.ref);
    if (!el || !el.isConnected) throw new Error(`找不到元素 ${command.ref}，请重新 snapshot`);
    return el;
  }
  if (command.selector) {
    const el = document.querySelector(command.selector);
    if (!el) throw new Error(`没有匹配 ${command.selector} 的元素`);
    return el;
  }
  if (command.text) {
    const want = command.text.trim();
    const pool = [...document.querySelectorAll(INTERACTIVE)].filter(visible);
    const el = pool.find(e => label(e) === want) || pool.find(e => label(e).includes(want));
    if (!el) throw new Error(`没有文字为「${want}」的可交互元素`);
    return el;
  }
  throw new Error('需要 ref、selector 或 text');
}

function pointer(el, type, extra = {}) {
  const r = el.getBoundingClientRect();
  const init = { bubbles: true, cancelable: true, composed: true, clientX: r.x + r.width / 2, clientY: r.y + r.height / 2, button: 0, ...extra };
  const Ctor = type.startsWith('pointer') ? PointerEvent : MouseEvent;
  return el.dispatchEvent(new Ctor(type, init));
}

function click(el) {
  el.scrollIntoView({ block: 'center', inline: 'center' });
  for (const t of ['pointerdown', 'mousedown']) pointer(el, t);
  if (typeof el.focus === 'function') el.focus();
  for (const t of ['pointerup', 'mouseup']) pointer(el, t);
  el.click();
}

function setValue(el, text) {
  const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(proto, 'value')?.set;
  if (setter) setter.call(el, text); else el.value = text;
  el.dispatchEvent(new InputEvent('input', { bubbles: true, data: text, inputType: 'insertText' }));
  el.dispatchEvent(new Event('change', { bubbles: true }));
}

function key(target, command) {
  const init = {
    key: command.key, code: command.code || '', bubbles: true, cancelable: true,
    metaKey: !!command.meta, ctrlKey: !!command.ctrl, altKey: !!command.alt, shiftKey: !!command.shift,
  };
  const down = target.dispatchEvent(new KeyboardEvent('keydown', init));
  target.dispatchEvent(new KeyboardEvent('keyup', init));
  return down;
}

const sleep = ms => new Promise(r => setTimeout(r, ms));

async function waitFor(command) {
  const until = Date.now() + Math.min(command.timeout_ms || 10000, 60000);
  while (Date.now() < until) {
    const el = command.selector ? document.querySelector(command.selector) : null;
    const text = command.text ? (document.body.innerText || '').includes(command.text) : false;
    if ((el && visible(el)) || text) return { found: true };
    await sleep(100);
  }
  return { found: false };
}

function overflowing() {
  const out = [];
  for (const el of document.querySelectorAll('body *')) {
    if (!visible(el) || el.children.length > 0) continue;
    const s = getComputedStyle(el);
    const clipped = el.scrollWidth > el.clientWidth + 1 && s.overflowX !== 'visible' && s.textOverflow !== 'ellipsis';
    const r = el.getBoundingClientRect();
    const outside = r.right > innerWidth + 1 || r.left < -1;
    if (clipped || outside) out.push({ ref: refOf(el), name: label(el), rect: rect(el), clipped, outside });
    if (out.length >= 50) break;
  }
  return out;
}

async function run(command) {
  install();
  const settle = command.settle_ms ?? 150;
  switch (command.action) {
    case 'init':
      return { ok: true, url: location.pathname };
    case 'snapshot':
      return snapshot();
    case 'click': {
      const el = guard(find(command));
      click(el);
      await sleep(settle);
      return { ok: true, clicked: label(el) };
    }
    case 'context_menu': {
      const el = guard(find(command));
      el.scrollIntoView({ block: 'center' });
      pointer(el, 'contextmenu', { button: 2 });
      await sleep(settle);
      return { ok: true };
    }
    case 'type': {
      const el = guard(find(command));
      el.focus();
      setValue(el, command.value ?? '');
      if (command.submit) key(el, { key: 'Enter', code: 'Enter' });
      await sleep(settle);
      return { ok: true };
    }
    case 'key': {
      if (command.key === 'Tab' && command.shift) throw new Error('Shift+Tab 会切换权限模式，界面驱动不能用');
      const target = guard(command.ref || command.selector || command.text ? find(command) : (document.activeElement || document.body));
      const accepted = key(target, command);
      await sleep(settle);
      return { ok: true, default_prevented: !accepted };
    }
    case 'navigate': {
      const a = Object.assign(document.createElement('a'), { href: command.path });
      document.body.appendChild(a);
      a.click();
      a.remove();
      await sleep(command.settle_ms ?? 400);
      return { ok: true, url: location.pathname + location.search };
    }
    case 'wait':
      if (command.ms) {
        await sleep(Math.min(command.ms, 60000));
        return { ok: true };
      }
      return waitFor(command);
    case 'errors': {
      const errors = state.errors.splice(0);
      return { errors };
    }
    case 'layout':
      return { overflowing: overflowing(), viewport: [innerWidth, innerHeight] };
    case 'style': {
      const props = Array.isArray(command.props) ? command.props.slice(0, 40) : [];
      const nodes = [...document.querySelectorAll(command.selector || ':root')].slice(0, 20);
      return {
        matches: nodes.length,
        styles: nodes.map(el => {
          const s = getComputedStyle(el, command.pseudo || null);
          return Object.fromEntries(props.map(p => [p, s.getPropertyValue(p)]));
        }),
      };
    }
    default:
      throw new Error(`不认识的 action：${command.action}`);
  }
}

export async function runUiCommand(raw) {
  try {
    const result = await run(JSON.parse(raw));
    return JSON.stringify(result);
  } catch (e) {
    return JSON.stringify({ error: String(e && e.message || e) });
  }
}
