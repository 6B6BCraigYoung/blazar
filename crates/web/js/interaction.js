const modals = [];
const focusableSelector = 'a[href], button, input:not([type="hidden"]), select, textarea, details > summary, [tabindex], [contenteditable="true"]';

function visible(element) {
  if (!(element instanceof HTMLElement) || !element.isConnected || element.closest('[hidden], [inert]') || element.getClientRects().length === 0 || getComputedStyle(element).visibility === 'hidden') return false;
  for (let parent = element.parentElement; parent; parent = parent.parentElement) {
    if (parent instanceof HTMLDetailsElement && !parent.open) {
      const summary = [...parent.children].find(child => child.tagName === 'SUMMARY');
      if (!summary?.contains(element)) return false;
    }
  }
  return true;
}

function enabled(element) {
  return visible(element) && !element.matches(':disabled, [aria-disabled="true"]');
}

function focus(element) {
  if (!enabled(element)) return false;
  element.focus({ preventScroll: true });
  return document.activeElement === element;
}

function tabbables(root) {
  const items = [...root.querySelectorAll(focusableSelector)].filter(element => enabled(element) && element.tabIndex >= 0);
  return items.filter(element => {
    if (!(element instanceof HTMLInputElement) || element.type !== 'radio' || !element.name) return true;
    const group = items.filter(item => item instanceof HTMLInputElement && item.type === 'radio' && item.name === element.name && item.form === element.form);
    return element === (group.find(item => item.checked) || group[0]);
  }).sort((a, b) => {
    const ai = a.tabIndex || Number.MAX_SAFE_INTEGER;
    const bi = b.tabIndex || Number.MAX_SAFE_INTEGER;
    return ai - bi;
  });
}

function topModal() {
  return modals.at(-1);
}

const inerted = new Set();

function isolateTop() {
  for (const element of inerted) element.inert = false;
  inerted.clear();
  const top = topModal();
  if (!top || !top.mask.isConnected) return;
  for (let node = top.mask; node.parentElement && node !== document.body; node = node.parentElement) {
    for (const sibling of node.parentElement.children) {
      if (sibling === node || sibling.inert || sibling.matches('script, [aria-live]')) continue;
      sibling.inert = true;
      inerted.add(sibling);
    }
  }
}

function syncModals() {
  for (const [index, modal] of modals.entries()) {
    modal.mask.style.setProperty('--modal-depth', String(index));
    modal.root.setAttribute('aria-modal', String(modal === topModal()));
  }
  isolateTop();
}

function focusWithin(modal) {
  if (modal.last && modal.root.contains(modal.last) && focus(modal.last)) return;
  const initial = modal.root.querySelector('[data-modal-initial-focus]');
  if (initial && focus(initial)) return;
  focus(modal.root);
}

export function captureModalFocus() {
  const element = document.activeElement;
  const parent = topModal();
  return { element, fallback: parent?.root.contains(element) ? parent.opener : null };
}

function restoreFocus(origin, next) {
  for (let current = origin; current; current = current.fallback) {
    if ((!next || next.root.contains(current.element)) && focus(current.element)) return true;
  }
  return false;
}

export function attachModal(root, opener, close, closeOnBackdrop) {
  const mask = root.parentElement;
  const modal = { root, mask, opener, last: null };
  const descendant = modals.findIndex(item => root.contains(item.root));
  if (descendant < 0) modals.push(modal);
  else modals.splice(descendant, 0, modal);
  syncModals();

  const onFocus = event => {
    if (topModal() !== modal) return;
    if (root.contains(event.target)) {
      modal.last = event.target;
      return;
    }
    const entering = event.target.closest?.('[data-modal-root]');
    if (entering && !modals.some(item => item.root === entering)) return;
    focusWithin(modal);
  };
  const onKey = event => {
    const adrift = event.target === document.body || event.target === document.documentElement;
    if (topModal() !== modal || event.defaultPrevented || event.isComposing) return;
    if (!adrift && !root.contains(event.target)) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      close();
      return;
    }
    if (event.key !== 'Tab') return;
    const items = tabbables(root);
    const active = document.activeElement;
    const index = items.indexOf(active);
    if (!items.length) {
      event.preventDefault();
      focus(root);
    } else if (index < 0 || (!event.shiftKey && index === items.length - 1) || (event.shiftKey && index === 0)) {
      event.preventDefault();
      focus(event.shiftKey ? items.at(-1) : items[0]);
    }
  };
  const onBackdrop = event => {
    if (event.target !== mask || topModal() !== modal || !closeOnBackdrop) return;
    close();
  };
  document.addEventListener('focusin', onFocus);
  window.addEventListener('keydown', onKey);
  mask.addEventListener('click', onBackdrop);
  let home = mask.parentNode;
  const frame = requestAnimationFrame(() => {
    if (mask.parentNode) home = mask.parentNode;
    if (topModal() !== modal || !root.isConnected) return;
    if (root.contains(document.activeElement)) modal.last = document.activeElement;
    else focusWithin(modal);
  });
  let disposed = false;
  return {
    dispose() {
      if (disposed) return;
      disposed = true;
      cancelAnimationFrame(frame);
      lingerClosed(mask, root, mask.parentNode || home, mask.parentNode ? mask.nextSibling : null);
      document.removeEventListener('focusin', onFocus);
      window.removeEventListener('keydown', onKey);
      mask.removeEventListener('click', onBackdrop);
      const wasTop = topModal() === modal;
      const index = modals.indexOf(modal);
      if (index >= 0) modals.splice(index, 1);
      syncModals();
      if (!wasTop) return;
      const next = topModal();
      queueMicrotask(() => {
        if (topModal() !== next) return;
        if (restoreFocus(opener, next)) return;
        if (next) {
          focusWithin(next);
          return;
        }
        requestAnimationFrame(() => {
          if (!topModal() && document.activeElement === document.body) restoreFocus(opener, null);
        });
      });
    }
  };
}

export function prefersReducedMotion() {
  return globalThis.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;
}

/** 退出动画要等多久（毫秒）：--motion-exit；“减少动态效果”时为 0。 */
export function exitMs() {
  if (prefersReducedMotion()) return 0;
  const value = getComputedStyle(document.documentElement).getPropertyValue('--motion-exit').trim();
  const ms = value.endsWith('ms') ? parseFloat(value) : parseFloat(value) * 1000;
  return Number.isFinite(ms) ? ms : 120;
}

// Leptos 卸载弹窗是同步删节点的。卸载之后（同一个微任务队列里，赶在绘制之前）把原节点放回原处，
// 标上 data-closing 播退出动画，--motion-exit 之后再删。残影是 inert 的、不接点击、不再自称 dialog，
// 所以焦点归还、Esc、“有没有浮层开着”的判断都当它已经关了。
function lingerClosed(mask, root, home, next) {
  const ms = exitMs();
  if (!ms || !home) return;
  const homes = [];
  for (let node = home; node && node !== document; node = node.parentNode) homes.push(node);
  queueMicrotask(() => {
    if (mask.isConnected) return;
    // 平常放回原父节点；外面一层也被卸掉了，就放进还在页面上的最近一层祖先。
    const place = homes.find(node => node.isConnected);
    if (!place) return;
    mask.setAttribute('data-closing', '');
    mask.setAttribute('aria-hidden', 'true');
    mask.inert = true;
    for (const element of [root, ...mask.querySelectorAll('[role="dialog"]')]) {
      element.removeAttribute('role');
      element.removeAttribute('aria-modal');
    }
    place.insertBefore(mask, next?.parentNode === place ? next : null);
    setTimeout(() => mask.remove(), ms);
  });
}

function menuItems(root) {
  return [...root.querySelectorAll('button, a[href], [role="menuitem"]')].filter(element => enabled(element) && element.closest('[role="menu"], [data-keyboard-list]') === root);
}

export function menuKeydown(event, root) {
  if (event.defaultPrevented || event.isComposing) return false;
  if (event.key === 'Escape') {
    event.preventDefault();
    event.stopPropagation();
    return true;
  }
  if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return false;
  const target = event.target;
  if (target instanceof HTMLElement && (target.isContentEditable || target.matches('input, textarea, select'))) return false;
  const items = menuItems(root);
  if (!items.length) return false;
  const index = items.indexOf(document.activeElement);
  const next = event.key === 'Home' ? 0 : event.key === 'End' ? items.length - 1 : event.key === 'ArrowDown' ? (index + 1) % items.length : (index < 0 ? items.length - 1 : (index + items.length - 1) % items.length);
  event.preventDefault();
  event.stopPropagation();
  if (focus(items[next])) items[next].scrollIntoView({ block: 'nearest' });
  return false;
}
