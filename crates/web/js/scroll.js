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
    const row = firstVisible(el.children, top);
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
    else if (anchor && anchor[0].isConnected) {
      el.scrollTop += anchor[0].getBoundingClientRect().top - el.getBoundingClientRect().top - anchor[1];
    }
  }).observe(el);
}
