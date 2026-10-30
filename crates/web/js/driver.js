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

function pumpFrames() {
  const raf = window.requestAnimationFrame.bind(window);
  const caf = window.cancelAnimationFrame.bind(window);
  window.requestAnimationFrame = cb => (document.hidden ? -setTimeout(() => cb(performance.now()), 16) : raf(cb));
  window.cancelAnimationFrame = id => (id < 0 ? clearTimeout(-id) : caf(id));
}

function install() {
  if (state.installed) return;
  state.installed = true;
  pumpFrames();
  const trace = (text, cause) => (cause?.stack && !String(cause.stack).startsWith(text) ? `${text}\n${cause.stack}` : text);
  window.addEventListener('error', e => state.errors.push(trace(`error: ${e.message}`, e.error)));
  window.addEventListener('unhandledrejection', e => {
    if (!e.defaultPrevented) state.errors.push(trace(`rejection: ${e.reason}`, e.reason));
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

function signature(el, pseudo) {
  const classes = [...el.classList].slice(0, 3).join('.');
  return `${el.tagName.toLowerCase()}${classes ? `.${classes}` : ''}${pseudo || ''}`;
}

function isDot(width, height, radius) {
  return width > 0 && width <= 12 && height > 0 && height <= 12 && radius >= Math.min(width, height) / 2 - 0.5;
}

function colours(command) {
  const skip = command.skip || '.monaco-editor, .xterm, svg, script, style';
  const text = new Map();
  const dots = new Map();
  const note = (map, color, el, pseudo) => {
    if (!color || color === 'rgba(0, 0, 0, 0)') return;
    let entry = map.get(color);
    if (!entry) map.set(color, (entry = { color, count: 0, where: new Map() }));
    entry.count++;
    const sig = signature(el, pseudo);
    if (!entry.where.has(sig) && entry.where.size < 60) entry.where.set(sig, clean(el.innerText || el.textContent).slice(0, 40));
  };
  for (const el of document.querySelectorAll('body *')) {
    if (el.closest(skip) || !shown(el)) continue;
    const s = getComputedStyle(el);
    if ([...el.childNodes].some(n => n.nodeType === Node.TEXT_NODE && n.textContent.trim())) note(text, s.color, el);
    const r = el.getBoundingClientRect();
    if (isDot(r.width, r.height, parseFloat(s.borderRadius))) note(dots, s.backgroundColor, el);
    for (const pseudo of ['::before', '::after']) {
      const ps = getComputedStyle(el, pseudo);
      if (ps.content === 'none' || ps.display === 'none') continue;
      if (isDot(parseFloat(ps.width), parseFloat(ps.height), parseFloat(ps.borderRadius))) note(dots, ps.backgroundColor, el, pseudo);
      if (ps.content !== '""' && ps.content !== 'normal') note(text, ps.color, el, pseudo);
    }
  }
  const list = map => [...map.values()].sort((a, b) => b.count - a.count).map(e => ({
    color: e.color,
    count: e.count,
    where: [...e.where].map(([sig, sample]) => (sample ? `${sig} “${sample}”` : sig)),
  }));
  return { url: location.pathname + location.search, text: list(text), dots: list(dots) };
}

function metrics(command) {
  const skip = command.skip || '.monaco-editor, .xterm, svg, script, style';
  const tally = () => new Map();
  const out = { fontSize: tally(), fontWeight: tally(), color: tally(), background: tally(), surface: tally(), radius: tally(), shadow: tally(), border: tally() };
  const bump = (map, key, el) => {
    if (!key) return;
    let entry = map.get(key);
    if (!entry) map.set(key, (entry = { value: key, count: 0, sample: signature(el) }));
    entry.count++;
  };
  const vw = innerWidth, vh = innerHeight;
  for (const el of document.querySelectorAll('body *')) {
    if (el.closest(skip) || !shown(el)) continue;
    const r = el.getBoundingClientRect();
    if (r.bottom < 0 || r.top > vh || r.right < 0 || r.left > vw) continue;
    const s = getComputedStyle(el);
    if ([...el.childNodes].some(n => n.nodeType === Node.TEXT_NODE && n.textContent.trim())) {
      bump(out.fontSize, s.fontSize, el);
      bump(out.fontWeight, s.fontWeight, el);
      bump(out.color, s.color, el);
    }
    const dot = isDot(r.width, r.height, parseFloat(s.borderTopLeftRadius));
    if (s.backgroundColor !== 'rgba(0, 0, 0, 0)') {
      bump(out.background, s.backgroundColor, el);
      // A surface is a block you read things on: at least 40×24, not a control, a dot or a badge.
      // Controls, selection highlights, tracks and meters, and inline code don't count.
      if (r.width >= 40 && r.height >= 24 && !el.closest('a, button, input, select, textarea, kbd, code, progress, [role="button"], [role="tab"], [role="progressbar"], [aria-current], [aria-selected="true"], [data-sel="true"], .seg')) bump(out.surface, s.backgroundColor, el);
    }
    if (parseFloat(s.borderTopLeftRadius) > 0 && !dot) bump(out.radius, s.borderTopLeftRadius, el);
    if (s.boxShadow !== 'none' && !dot) bump(out.shadow, s.boxShadow, el);
    const bw = ['Top', 'Right', 'Bottom', 'Left'].filter(side => parseFloat(s[`border${side}Width`]) > 0 && s[`border${side}Style`] !== 'none');
    if (bw.length) bump(out.border, `${bw.length === 4 ? 'all' : bw.join('+').toLowerCase()} ${s[`border${bw[0]}Width`]} ${s[`border${bw[0]}Color`]}`, el);
  }
  const list = map => [...map.values()].sort((a, b) => b.count - a.count);
  const res = { url: location.pathname + location.search };
  for (const [k, v] of Object.entries(out)) res[k] = { distinct: v.size, top: list(v).slice(0, command.top ?? 12) };
  return res;
}

// rgb()/rgba() and color(srgb …) as computed styles give them, as [r, g, b, a] with r, g, b in 0–255.
function parseColor(value) {
  let m = value.match(/^rgba?\(([\d.]+),\s*([\d.]+),\s*([\d.]+)(?:,\s*([\d.]+))?\)$/);
  if (m) return [+m[1], +m[2], +m[3], m[4] === undefined ? 1 : +m[4]];
  m = value.match(/^color\(srgb ([\d.e-]+) ([\d.e-]+) ([\d.e-]+)(?: \/ ([\d.]+))?\)$/);
  if (m) return [m[1] * 255, m[2] * 255, m[3] * 255, m[4] === undefined ? 1 : +m[4]];
  return null;
}

const over = (top, under) => {
  const a = top[3];
  return [0, 1, 2].map(i => top[i] * a + under[i] * (1 - a)).concat(1);
};

function luminance([r, g, b]) {
  const lin = c => ((c /= 255) <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

// The colour actually behind an element: its own background and every ancestor's, composited
// down to the first opaque one (the page canvas at the latest).
function backdrop(el) {
  const layers = [];
  for (let p = el; p; p = p.parentElement) {
    const c = parseColor(getComputedStyle(p).backgroundColor);
    if (c && c[3] > 0) {
      layers.push(c);
      if (c[3] >= 1) break;
    }
  }
  let colour = parseColor(getComputedStyle(document.body).backgroundColor) || [255, 255, 255, 1];
  for (const c of layers.reverse()) colour = over(c, colour);
  return colour;
}

// Every visible run of text against what it sits on. Large text (≥ 18px, or ≥ 14px bold) needs
// 3:1, the rest 4.5:1. Disabled controls and placeholders don't count.
function contrast(command) {
  const skip = command.skip || '.monaco-editor, .xterm, svg, script, style, [disabled], [aria-disabled="true"]';
  const fails = new Map();
  let checked = 0;
  for (const el of document.querySelectorAll('body *')) {
    if (el.closest(skip) || !shown(el)) continue;
    if (![...el.childNodes].some(n => n.nodeType === Node.TEXT_NODE && n.textContent.trim())) continue;
    const s = getComputedStyle(el);
    const fg = parseColor(s.color);
    if (!fg) continue;
    let alpha = fg[3];
    for (let p = el; p; p = p.parentElement) alpha *= parseFloat(getComputedStyle(p).opacity);
    // Faded all the way out (closing, or not on screen yet): not text anyone reads.
    if (alpha < 0.1) continue;
    const bg = backdrop(el);
    const ink = over([fg[0], fg[1], fg[2], alpha], bg);
    const [hi, lo] = [luminance(ink), luminance(bg)].sort((a, b) => b - a);
    const ratio = (hi + 0.05) / (lo + 0.05);
    const size = parseFloat(s.fontSize);
    const need = size >= 18 || (size >= 14 && +s.fontWeight >= 700) ? 3 : 4.5;
    checked++;
    if (ratio >= need) continue;
    const sig = signature(el);
    const entry = fails.get(sig) || { where: sig, sample: clean(el.innerText || el.textContent).slice(0, 40), ratio, need, color: s.color, count: 0 };
    entry.count++;
    entry.ratio = Math.min(entry.ratio, ratio);
    fails.set(sig, entry);
  }
  const list = [...fails.values()].sort((a, b) => a.ratio - b.ratio).map(f => ({ ...f, ratio: Math.round(f.ratio * 100) / 100 }));
  return { url: location.pathname + location.search, checked, failing: list.length, fails: list.slice(0, command.top ?? 40) };
}

function watchFrames() {
  const rec = { gaps: [], mutations: 0, lastMutation: performance.now(), stopped: false };
  const observer = new MutationObserver(list => {
    rec.mutations += list.length;
    rec.lastMutation = performance.now();
  });
  observer.observe(document.body, { subtree: true, childList: true, characterData: true, attributes: true });
  let last = 0;
  const loop = now => {
    if (rec.stopped) return;
    if (last) rec.gaps.push(now - last);
    last = now;
    requestAnimationFrame(loop);
  };
  requestAnimationFrame(loop);
  rec.done = () => {
    rec.stopped = true;
    observer.disconnect();
    const gaps = rec.gaps;
    const sorted = [...gaps].sort((a, b) => a - b);
    const budget = Math.min(Math.max(sorted[Math.floor(sorted.length / 2)] || 16.7, 8), 17.5);
    const dropped = gaps.reduce((n, g) => n + Math.max(0, Math.round(g / budget) - 1), 0);
    return {
      frames: gaps.length,
      dropped,
      long_frames: gaps.filter(g => g > 50).length,
      max_gap: Math.round(sorted[sorted.length - 1] || 0),
      mutations: rec.mutations,
    };
  };
  return rec;
}

async function run(command) {
  install();
  const settle = command.settle_ms ?? 150;
  switch (command.action) {
    case 'init':
      return { ok: true, url: location.pathname };
    case 'snapshot':
      return snapshot();
    case 'colors':
      return colours(command);
    case 'metrics':
      return metrics(command);
    case 'contrast':
      return contrast(command);
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
      const shot = await invoke('snapshot_webview', { name: command.name || null, width: innerWidth, height: innerHeight });
      return { ok: true, path: shot.path, width: shot.width, height: shot.height, viewport: [innerWidth, innerHeight] };
    }
    case 'frames': {
      const rec = watchFrames();
      await sleep(Math.min(command.ms ?? 5000, 60000));
      return { ok: true, ...rec.done() };
    }
    case 'measure': {
      if (!command.then || typeof command.then !== 'object' || command.then.action === 'measure') throw new Error('measure 需要 then（要计时的命令）');
      const rec = watchFrames();
      const t0 = performance.now();
      const limit = t0 + Math.min(command.timeout_ms ?? 10000, 60000);
      const result = await run({ settle_ms: 0, ...command.then });
      if (command.until) {
        while (performance.now() < limit && ![...document.querySelectorAll(command.until)].some(shown)) await sleep(16);
      }
      const ready = performance.now() - t0;
      const quiet = command.quiet_ms ?? 300;
      while (performance.now() < limit && performance.now() - rec.lastMutation < quiet) await sleep(25);
      const stats = rec.done();
      return { ok: true, result, ready_ms: Math.round(ready), settle_ms: Math.round(Math.max(rec.lastMutation, t0 + ready) - t0), ...stats };
    }
    case 'sweep': {
      const el = guard(find(command));
      const ms = Math.min(command.ms ?? 2000, 30000);
      const from = el.scrollTop;
      const target = () => (command.to === 'start' ? 0 : el.scrollHeight - el.clientHeight);
      const rec = watchFrames();
      const t0 = performance.now();
      await new Promise(done => {
        const step = now => {
          const k = Math.min(1, (now - t0) / ms);
          el.scrollTop = from + (target() - from) * k;
          if (k < 1) requestAnimationFrame(step);
          else done();
        };
        requestAnimationFrame(step);
      });
      await sleep(command.settle_ms ?? 150);
      return { ok: true, from: Math.round(from), top: Math.round(el.scrollTop), height: el.scrollHeight, ...rec.done() };
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
