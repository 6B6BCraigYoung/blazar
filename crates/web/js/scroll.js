

export function keepScroll(el) {
  let width = el.clientWidth;
  let bottom = true;
  let anchor = null;
  const record = () => {
    bottom = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
    anchor = null;
    if (bottom) return;
    const top = el.getBoundingClientRect().top;
    for (const n of el.querySelectorAll(':scope > *, .cc-md > *')) {
      const r = n.getBoundingClientRect();
      if (r.height > 0 && r.bottom > top && getComputedStyle(n).position !== 'sticky') { anchor = [n, r.top - top]; break; }
    }
  };
  el.addEventListener('scroll', () => { if (el.clientWidth === width) record(); }, { passive: true });
  new ResizeObserver(() => {
    if (el.clientWidth === width) return;
    width = el.clientWidth;
    if (bottom) el.scrollTop = el.scrollHeight;
    else if (anchor && anchor[0].isConnected) {
      el.scrollTop += anchor[0].getBoundingClientRect().top - el.getBoundingClientRect().top - anchor[1];
    }
  }).observe(el);
}
