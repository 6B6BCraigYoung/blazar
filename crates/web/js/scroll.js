const offset = (el, n) => n.getBoundingClientRect().top - el.getBoundingClientRect().top;

export function keepScroll(el) {
  let width = el.clientWidth;
  let bottom = true;
  let anchor = null;
  let queued = false;
  const sticky = n => getComputedStyle(n).position === 'sticky';
  const firstVisible = (nodes, top) => {
    for (const n of nodes) {
      const r = n.getBoundingClientRect();
      if (r.height > 0 && r.bottom > top && !sticky(n)) return [n, r.top - top];
    }
    return null;
  };
  const record = () => {
    queued = false;
    bottom = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
    anchor = null;
    if (bottom) return;
    const top = el.getBoundingClientRect().top;
    const rows = el.querySelector(':scope > .cc-flow')?.children ?? el.children;
    const row = firstVisible(rows, top);
    if (!row) return;
    anchor = firstVisible(row[0].querySelectorAll('.cc-md > *'), top) || row;
  };
  el.addEventListener('scroll', () => {
    if (el.clientWidth !== width || queued) return;
    queued = true;
    requestAnimationFrame(record);
  }, { passive: true });
  new ResizeObserver(() => {
    if (el.clientWidth === width) return;
    width = el.clientWidth;
    if (bottom) el.scrollTop = el.scrollHeight;
    else if (anchor && anchor[0].isConnected) el.scrollTop += offset(el, anchor[0]) - anchor[1];
  }).observe(el);
}

export function holdTail(el) {
  const flow = el.querySelector(':scope > .cc-flow') ?? el;
  const tail = el.scrollHeight - el.scrollTop;
  let expected = el.scrollTop;
  const watch = new ResizeObserver(() => {
    if (Math.abs(el.scrollTop - expected) > 4) return watch.disconnect();
    el.scrollTop = el.scrollHeight - tail;
    expected = el.scrollTop;
  });
  watch.observe(flow);
  setTimeout(() => watch.disconnect(), 1000);
}

const bottoms = new WeakMap();

export function holdBottom(el) {
  bottoms.get(el)?.();
  const flow = el.querySelector(':scope > .cc-flow') ?? el;
  let pinned = -Infinity;
  const watch = new ResizeObserver(() => {
    if (el.scrollTop < pinned - 4) return stop();
    el.scrollTop = el.scrollHeight;
    pinned = el.scrollTop;
  });
  const timer = setTimeout(() => stop(), 1000);
  const stop = () => {
    watch.disconnect();
    clearTimeout(timer);
    bottoms.delete(el);
  };
  bottoms.set(el, stop);
  watch.observe(flow);
}
