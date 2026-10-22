const INTERACTIVE = 'a[href],button,input,textarea,select,summary,[role="button"],[role="menuitem"],[role="tab"],[role="option"],[role="checkbox"],[role="switch"],[contenteditable="true"],[tabindex]:not([tabindex="-1"])';

const GUARDED = '.xterm, [data-ui-guard]';

const state = { refs: new Map(), next: 1, errors: [], installed: false };

function guarded(el) {
  return !!(el && el.closest && el.closest(GUARDED));
}

function guard(el) {
  if (guarded(el)) throw new Error('受保护的区域：界面驱动不能操作终端、审批和权限控件');
  if (el.closest('[inert]')) throw new Error('被上层弹窗挡住了，先处理最上面的弹窗');
  return el;
}

function install() {
  if (state.installed) return;
  state.installed = true;
  window.addEventListener('error', e => state.errors.push(`error: ${e.message}`));
  window.addEventListener('unhandledrejection', e => {
    if (!e.defaultPrevented) state.errors.push(`rejection: ${e.reason}`);
  });
  const original = console.error.bind(console);
  console.error = (...args) => {
    state.errors.push(args.map(String).join(' '));
    original(...args);
  };
}

function collapsed(el) {
  for (let d = el.closest('details'); d; d = d.parentElement?.closest('details')) {
    const summary = d.querySelector(':scope > summary');
    if (!d.open && !(summary && summary.contains(el))) return true;
  }
  return false;
}

function shown(el) {
  if (el.closest('[inert]') || collapsed(el)) return false;
  const r = el.getBoundingClientRect();
  if (r.width < 1 || r.height < 1) return false;
  const s = getComputedStyle(el);
  return s.visibility !== 'hidden' && s.display !== 'none' && Number(s.opacity) > 0;
}

function visible(el) {
  if (!shown(el)) return false;
  const r = el.getBoundingClientRect();
  return !(r.bottom < 0 || r.right < 0 || r.top > innerHeight || r.left > innerWidth);
}

const clean = text => (text || '').trim().replace(/\s+/g, ' ').slice(0, 100);

function label(el) {
  const by = el.getAttribute('aria-labelledby');
  if (by) {
    const text = by.split(/\s+/).map(id => document.getElementById(id)?.innerText || '').join(' ');
    if (clean(text)) return clean(text);
  }
  if (el.getAttribute('aria-label')) return clean(el.getAttribute('aria-label'));
  if (el.labels && el.labels.length) {
    const text = [...el.labels].map(l => l.innerText).join(' ');
    if (clean(text)) return clean(text);
  }
  const field = el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT';
  return clean((field ? '' : el.innerText) || el.getAttribute('title') || el.getAttribute('placeholder'));
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
  const active = document.activeElement;
  return {
    url: location.pathname + location.search,
    title: document.title,
    focused: active && active !== document.body ? { ref: refOf(active), tag: active.tagName.toLowerCase(), name: label(active) } : null,
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
  if (command.selector && !command.text) {
    const el = document.querySelector(command.selector);
    if (!el) throw new Error(`没有匹配 ${command.selector} 的元素`);
    return el;
  }
  if (command.text) {
    const want = command.text.trim();
    const fits = e => (command.partial ? label(e).includes(want) : label(e) === want);
    const pick = pool => {
      const exact = pool.find(e => label(e) === want);
      if (exact || !command.partial) return exact;
      const partial = pool.filter(fits);
      if (partial.length > 1) {
        const names = partial.slice(0, 5).map(e => `「${label(e)}」`).join('、');
        throw new Error(`「${want}」同时匹配 ${partial.length} 个元素：${names}，请写完整文字或用 ref`);
      }
      return partial[0];
    };
    const all = [...document.querySelectorAll(command.selector || INTERACTIVE)];
    const el = pick(all.filter(shown));
    if (el) return el;
    if (all.some(e => e.closest('[inert]') && fits(e))) throw new Error(`「${want}」被上层弹窗挡住了，先处理最上面的弹窗`);
    const near = all.filter(shown).map(label).filter(n => n && n !== want && n.includes(want)).slice(0, 5);
    const hint = near.length ? `；包含这段文字的有 ${near.map(n => `「${n}」`).join('、')}，要按包含匹配请加 "partial": true` : '';
    throw new Error(`没有文字为「${want}」的${command.selector ? `、匹配 ${command.selector} 的` : '可交互'}元素${hint}`);
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
  if (el.disabled || el.getAttribute('aria-disabled') === 'true') throw new Error(`「${label(el)}」是禁用状态，没有点`);
  el.scrollIntoView({ block: 'center', inline: 'center' });
  for (const t of ['pointerdown', 'mousedown']) pointer(el, t);
  if (typeof el.focus === 'function') el.focus({ preventScroll: true });
  for (const t of ['pointerup', 'mouseup']) pointer(el, t);
  el.click();
}

function setValue(el, text) {
  if (el instanceof HTMLSelectElement && ![...el.options].some(o => o.value === text)) {
    const options = [...el.options].map(o => `「${o.value}」`).join('、');
    throw new Error(`下拉框没有值为「${text}」的选项，可选：${options}`);
  }
  const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype
    : el instanceof HTMLSelectElement ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
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
    const shown = (el && visible(el)) || text;
    if (command.gone ? !shown : shown) return { found: !command.gone };
    await sleep(100);
  }
  return { found: !!command.gone };
}

function scrolledSideways(el) {
  for (let p = el.parentElement; p && p !== document.body; p = p.parentElement) {
    if (!['auto', 'scroll'].includes(getComputedStyle(p).overflowX) || p.scrollHeight > p.clientHeight + 1) continue;
    const r = p.getBoundingClientRect();
    if (r.left >= -1 && r.right <= innerWidth + 1) return true;
  }
  return false;
}

function overflowing() {
  const out = [];
  for (const el of document.querySelectorAll('body *')) {
    if (!visible(el) || el.children.length > 0) continue;
    const s = getComputedStyle(el);
    const clipped = el.scrollWidth > el.clientWidth + 1 && s.overflowX !== 'visible' && s.textOverflow !== 'ellipsis';
    const r = el.getBoundingClientRect();
    const outside = (r.right > innerWidth + 1 || r.left < -1) && !scrolledSideways(el);
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
      const box = el.getBoundingClientRect();
      if (box.width < 1 || box.height < 1) throw new Error(`${label(el) || command.selector || command.text} 没有显示出来，不能点`);
      const clicked = label(el);
      click(el);
      await sleep(settle);
      return { ok: true, clicked };
    }
    case 'file': {
      if (typeof command.value !== 'string') throw new Error('file 需要 value（文件内容），可选 name（文件名）');
      const el = command.ref ? find(command) : document.querySelector(command.selector || 'input[type="file"]');
      if (!(el instanceof HTMLInputElement) || el.type !== 'file') throw new Error('file 只能用在 <input type="file"> 上');
      guard(el);
      const file = new File([command.value], command.name || 'upload.txt', { type: command.type || '' });
      const files = new DataTransfer();
      files.items.add(file);
      el.files = files.files;
      el.dispatchEvent(new Event('change', { bubbles: true }));
      await sleep(settle);
      return { ok: true, name: file.name, bytes: file.size };
    }
    case 'drag': {
      const dx = Number(command.dx || 0);
      const dy = Number(command.dy || 0);
      const el = guard(find(command));
      const r = el.getBoundingClientRect();
      if (r.width < 1 || r.height < 1) throw new Error(`${label(el) || command.selector} 没有显示出来，不能拖`);
      const x = r.x + r.width / 2;
      const y = r.y + r.height / 2;
      const steps = Math.max(1, Math.min(Number(command.steps || 8), 60));
      const at = (type, i) => pointer(el, type, { clientX: x + (dx * i) / steps, clientY: y + (dy * i) / steps, pointerId: 1, isPrimary: true });
      at('pointerdown', 0);
      for (let i = 1; i <= steps; i++) at('pointermove', i);
      at('pointerup', steps);
      await sleep(settle);
      return { ok: true, target: label(el), dx, dy };
    }
    case 'drop': {
      const el = guard(find(command));
      const dt = new DataTransfer();
      for (const [type, value] of Object.entries(command.data || {})) dt.setData(type, String(value));
      const init = { bubbles: true, cancelable: true, composed: true, dataTransfer: dt };
      el.dispatchEvent(new DragEvent('dragenter', init));
      el.dispatchEvent(new DragEvent('dragover', init));
      const accepted = el.dispatchEvent(new DragEvent('drop', init));
      await sleep(settle);
      return { ok: true, target: label(el), types: [...dt.types], default_prevented: !accepted };
    }
    case 'context_menu': {
      const el = guard(find(command));
      el.scrollIntoView({ block: 'center' });
      pointer(el, 'contextmenu', { button: 2 });
      await sleep(settle);
      return { ok: true, target: label(el) };
    }
    case 'scroll': {
      const to = command.top;
      if (to !== 'start' && to !== 'end' && typeof to !== 'number') throw new Error('scroll 需要 top：start、end 或像素值');
      const el = guard(find(command));
      el.scrollTop = to === 'start' ? 0 : to === 'end' ? el.scrollHeight : to;
      el.dispatchEvent(new Event('scroll'));
      await sleep(settle);
      return { ok: true, top: Math.round(el.scrollTop), height: el.scrollHeight, view: el.clientHeight };
    }
    case 'type': {
      if (typeof command.value !== 'string') throw new Error('type 需要 value（要输入的文字）；text 只用来按文字找元素');
      const el = guard(find(command));
      el.focus();
      setValue(el, command.value);
      if (command.submit) key(el, { key: 'Enter', code: 'Enter' });
      await sleep(settle);
      return { ok: true, target: label(el) };
    }
    case 'key': {
      if (command.key === 'Tab' && command.shift) throw new Error('Shift+Tab 会切换权限模式，界面驱动不能用');
      const target = guard(command.ref || command.selector || command.text ? find(command) : (document.activeElement || document.body));
      const accepted = key(target, command);
      await sleep(settle);
      return { ok: true, default_prevented: !accepted };
    }
    case 'navigate': {
      const target = new URL(command.path, location.href);
      const a = Object.assign(document.createElement('a'), { href: target.href });
      document.body.appendChild(a);
      a.click();
      a.remove();
      await sleep(command.settle_ms ?? 400);
      const url = location.pathname + location.search;
      if (url === target.pathname + target.search) return { ok: true, url };
      return { ok: false, url, note: '没有跳到目标地址，可能被未保存修改的确认框拦下了' };
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
    case 'timing': {
      const nav = performance.getEntriesByType('navigation')[0];
      const wasm = performance.getEntriesByType('resource').find(r => r.name.endsWith('.wasm'));
      const round = v => (typeof v === 'number' ? Math.round(v) : null);
      return {
        now: round(performance.now()),
        dom_interactive: round(nav?.domInteractive),
        dom_content_loaded: round(nav?.domContentLoadedEventEnd),
        load: round(nav?.loadEventEnd),
        wasm: wasm ? { start: round(wasm.startTime), duration: round(wasm.duration), bytes: wasm.encodedBodySize || null } : null,
        marks: performance.getEntriesByType('mark').map(m => ({ name: m.name, at: round(m.startTime) })),
      };
    }
    case 'text': {
      const all = [...document.querySelectorAll(command.selector || 'body')];
      return {
        matches: all.length,
        texts: all.slice(0, 40).map(el => ({ text: (el.innerText || el.textContent || '').trim().slice(0, 400), shown: shown(el), rect: rect(el) })),
      };
    }
    case 'style': {
      const props = Array.isArray(command.props) ? command.props.slice(0, 40) : [];
      const all = [...document.querySelectorAll(command.selector || ':root')];
      return {
        matches: all.length,
        styles: all.slice(0, 20).map(el => {
          const s = getComputedStyle(el, command.pseudo || null);
          const out = Object.fromEntries(props.map(p => [p, s.getPropertyValue(p)]));
          if (command.rects) {
            out.rect = rect(el);
            out.scroll = [el.scrollWidth, el.scrollHeight];
          }
          return out;
        }),
      };
    }
    case 'css': {
      if (typeof command.value !== 'string') throw new Error('css 需要 value（要注入的样式），传空字符串清除');
      let sheet = document.getElementById('blazar-driver-css');
      if (!sheet) {
        sheet = Object.assign(document.createElement('style'), { id: 'blazar-driver-css' });
        document.head.appendChild(sheet);
      }
      sheet.textContent = command.value;
      await sleep(settle);
      return { ok: true, length: command.value.length };
    }
    case 'shot': {
      const invoke = window.__TAURI__?.core?.invoke;
      if (typeof invoke !== 'function') throw new Error('shot 只能在桌面端里用：当前页面没有 Tauri（比如在浏览器或无界面 Chrome 里）');
      if (command.name !== undefined && typeof command.name !== 'string') throw new Error('shot 的 name 必须是文件名字符串');
      await sleep(command.settle_ms ?? 0);
      for (const animation of document.getAnimations()) {
        if (Number.isFinite(animation.effect?.getComputedTiming().endTime)) animation.finish();
      }
      const shot = await invoke('snapshot_webview', { name: command.name || null });
      return { ok: true, path: shot.path, width: shot.width, height: shot.height };
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
