// 对话里的图片（md.rs 的 figure 生成 .cc-img）：点击看大图、复制图片 / 路径、
// 加载失败换成一行占位、超过上限给下载按钮。全部挂在 document 上委托，装一次即可。

const MAX_BYTES = 20 * 1024 * 1024;
let hooks = { toast: () => {}, download: () => {} };

function fig(el) {
  return el && el.closest ? el.closest('.cc-img') : null;
}

function imgOf(f) {
  return f.querySelector('img');
}

function srcOf(f) {
  const img = imgOf(f);
  return img ? img.currentSrc || img.src : '';
}

function line(f, text, withDownload) {
  const alt = f.querySelector('.cc-imgalt');
  if (!alt) return;
  alt.textContent = '';
  alt.append(text, ' · ', Object.assign(document.createElement('b'), { textContent: f.dataset.name || 'image' }));
  if (withDownload) {
    const b = Object.assign(document.createElement('button'), { type: 'button', textContent: 'Download' });
    b.dataset.img = 'download';
    alt.append(' ', b);
  }
}

function canDownload(f) {
  return !!(f.dataset.ws && f.dataset.path && f.dataset.path.indexOf('://') < 0);
}

async function failed(f) {
  if (f.dataset.state === 'failed' || f.dataset.state === 'big') return;
  f.dataset.state = 'failed';
  const src = srcOf(f);
  imgOf(f)?.remove();
  line(f, 'Image failed to load', false);
  let url;
  try { url = new URL(src, location.href); } catch (_) { return; }
  if (url.origin !== location.origin || !url.pathname.startsWith('/api/')) return;
  try {
    const r = await fetch(src, { method: 'GET', credentials: 'same-origin' });
    if (r.status === 413) {
      f.dataset.state = 'big';
      line(f, 'Image is larger than 20 MB', canDownload(f));
    }
    r.body?.cancel();
  } catch (_) { /* 已经显示了占位 */ }
}

function viewer(src, name) {
  const layer = document.createElement('div');
  layer.className = 'cc-imgview';
  layer.tabIndex = -1;
  const img = Object.assign(document.createElement('img'), { src, alt: name || '' });
  layer.append(img);
  const close = () => {
    layer.remove();
    document.removeEventListener('keydown', onKey, true);
  };
  const onKey = (e) => {
    if (e.key === 'Escape') {
      e.stopPropagation();
      e.preventDefault();
      close();
    }
  };
  layer.addEventListener('click', close);
  document.addEventListener('keydown', onKey, true);
  document.body.append(layer);
  layer.focus();
}

async function pngBlob(blob) {
  if (blob.type === 'image/png') return blob;
  const bmp = await createImageBitmap(blob);
  const c = document.createElement('canvas');
  c.width = bmp.width;
  c.height = bmp.height;
  c.getContext('2d').drawImage(bmp, 0, 0);
  return new Promise((ok, bad) => c.toBlob((b) => (b ? ok(b) : bad(new Error('encode'))), 'image/png'));
}

async function fetchBlob(src) {
  const r = await fetch(src, { credentials: 'same-origin' });
  if (!r.ok) throw new Error(`HTTP ${r.status}`);
  const b = await r.blob();
  if (b.size > MAX_BYTES) throw new Error('too large');
  return b;
}

function fileName(f, type) {
  let n = f.dataset.name || 'image';
  if (!/\.[a-z0-9]{2,5}$/i.test(n)) n += '.' + ((type || 'image/png').split('/')[1] || 'png').replace('jpeg', 'jpg');
  return n;
}

async function copyImage(f) {
  const src = srcOf(f);
  if (!src) return hooks.toast('Copy failed');
  const tauri = window.__TAURI__ && window.__TAURI__.core;
  try {
    if (tauri) {
      const blob = await fetchBlob(src);
      const bytes = new Uint8Array(await blob.arrayBuffer());
      await tauri.invoke('copy_image', bytes, {
        headers: { 'x-image-name': encodeURIComponent(fileName(f, blob.type)) },
      });
    } else {
      // Safari 要求在点击里同步创建 ClipboardItem，数据用 Promise 晚点给。
      const item = new ClipboardItem({ 'image/png': fetchBlob(src).then(pngBlob) });
      await navigator.clipboard.write([item]);
    }
    hooks.toast('Image copied');
  } catch (e) {
    console.warn('copy image', e);
    hooks.toast('Copy failed');
  }
}

async function copyPath(f) {
  try {
    await navigator.clipboard.writeText(f.dataset.path || '');
    hooks.toast('Path copied');
  } catch (_) {
    hooks.toast('Copy failed');
  }
}

function onClick(e) {
  const f = fig(e.target);
  if (!f) return;
  const btn = e.target.closest('button[data-img]');
  if (btn) {
    e.preventDefault();
    e.stopPropagation();
    const what = btn.dataset.img;
    if (what === 'copy') copyImage(f);
    else if (what === 'path') copyPath(f);
    else if (what === 'download') {
      const url = `/api/workspaces/${f.dataset.ws}/download?path=${encodeURIComponent(f.dataset.path)}`;
      hooks.download(url, f.dataset.name || 'image');
    }
    return;
  }
  const img = e.target.closest('img');
  if (img && img.parentElement === f) {
    e.preventDefault();
    e.stopPropagation();
    viewer(srcOf(f), f.dataset.name);
  }
}

function onError(e) {
  const t = e.target;
  if (t && t.tagName === 'IMG') {
    const f = fig(t);
    if (f) failed(f);
  }
}

function settle(root) {
  for (const f of root.querySelectorAll('.cc-img[data-state="failed"]:not([data-told])')) {
    f.dataset.told = '';
    line(f, 'Image failed to load', false);
  }
  for (const f of root.querySelectorAll('.cc-img[data-state="big"]:not([data-told])')) {
    f.dataset.told = '';
    line(f, 'Image is larger than 20 MB', canDownload(f));
  }
}

export function installImages(toast, download) {
  hooks = { toast, download };
  document.addEventListener('click', onClick, true);
  document.addEventListener('error', onError, true);
}

export function settleImages(root) {
  if (root) settle(root);
}
