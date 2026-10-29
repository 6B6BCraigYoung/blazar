export function readDataUrl(file) {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result));
    r.onerror = () => reject(r.error);
    r.readAsDataURL(file);
  });
}

export function downloadText(name, text, mime) {
  const url = URL.createObjectURL(new Blob([text], { type: mime || 'text/plain' }));
  const a = Object.assign(document.createElement('a'), { href: url, download: name });
  document.body.appendChild(a); a.click(); a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 2000);
}

export function downloadUrl(url, name) {
  const a = Object.assign(document.createElement('a'), { href: url, download: name });
  document.body.appendChild(a); a.click(); a.remove();
}

export function canPickSaveFile() {
  return typeof window.showSaveFilePicker === 'function';
}

// 桌面端：原生「存储为」面板 + 桌面进程流式写盘，进度经 Channel 推回来。
// 用户取消时 resolve(null)。
export function desktopSaveDownload(url, name, onEvent) {
  const core = window.__TAURI__.core;
  const channel = new core.Channel();
  channel.onmessage = onEvent;
  return core.invoke('save_download', { url, suggestedName: name, onEvent: channel });
}

export async function pickSaveFile(name) {
  try {
    return await window.showSaveFilePicker({ suggestedName: name, startIn: 'downloads', id: 'blazar-download' });
  } catch (e) {
    if (e && e.name === 'AbortError') return null;
    throw e;
  }
}

export function savedName(handle) {
  return handle.name;
}

function cut(e, total) {
  const why = e && e.message ? e.message : String(e);
  return new Error(total === null ? `连接断了，或节点上打包失败：${why}` : `连接断了：${why}`);
}

// 浏览器：fetch 流式读，边读边写进 showSaveFilePicker 选中的文件。
// createWritable 写的是临时副本，abort 后原文件不受影响。
export async function saveToFile(handle, url, signal, onProgress) {
  const res = await fetch(url, { signal, credentials: 'same-origin', cache: 'no-store' });
  if (!res.ok) {
    const body = (await res.text()).trim();
    if (res.status === 413) throw new Error(`文件太大：${body}`);
    throw new Error(body || `hub 返回 HTTP ${res.status}`);
  }
  const header = res.headers.get('content-length');
  const total = header === null ? null : Number(header);
  const out = await handle.createWritable();
  let received = 0;
  let last = 0;
  onProgress(0, total);
  try {
    const reader = res.body.getReader();
    for (;;) {
      let step;
      try {
        step = await reader.read();
      } catch (e) {
        throw signal.aborted ? e : cut(e, total);
      }
      if (step.done) break;
      await out.write(step.value);
      received += step.value.byteLength;
      const now = performance.now();
      if (now - last >= 150) {
        last = now;
        onProgress(received, total);
      }
    }
    if (total !== null && received !== total) throw new Error(`连接断了：只收到 ${received} / ${total} 字节`);
    await out.close();
    return received;
  } catch (e) {
    await out.abort().catch(() => {});
    throw e;
  }
}

export function readText(file) {
  return file.text();
}

let navigationGuard = null;
let leaveConfirm = null;
let passing = null;
let returning = false;
export function setNavigationGuard(workspace, dirtyCount) {
  navigationGuard = { workspace, dirtyCount };
}
export function clearNavigationGuard(workspace) {
  if (navigationGuard?.workspace === workspace) navigationGuard = null;
}
export function setLeaveConfirm(confirm) {
  leaveConfirm = confirm;
}
export function leaveWarning() {
  const count = navigationGuard?.dirtyCount() || 0;
  if (count) return `有 ${count} 个文件还没保存，离开会丢失这些改动。`;
  if (!window.dispatchEvent(new Event('blazar:before-navigate', { cancelable: true }))) {
    return '有尚未保存的修改，离开会丢失这些修改。';
  }
  return '';
}
function askToLeave(message) {
  return leaveConfirm ? Promise.resolve(leaveConfirm(message)).then(Boolean) : Promise.resolve(false);
}
function sameWorkspace(url) {
  if (!navigationGuard || url.origin !== location.origin) return false;
  const id = encodeURIComponent(navigationGuard.workspace);
  return url.pathname === `/w/${id}` || url.pathname === `/workspaces/${id}`;
}
function follow(a) {
  const link = a.isConnected ? a : Object.assign(document.createElement('a'), { href: a.href });
  if (!link.isConnected) document.body.appendChild(link);
  passing = link;
  link.click();
  if (link !== a) link.remove();
}
window.addEventListener('click', e => {
  if (e.defaultPrevented || e.button !== 0 || e.ctrlKey || e.metaKey || e.shiftKey || e.altKey) return;
  const a = e.target.closest?.('a[href]');
  if (!a || a.hasAttribute('download') || (a.target && a.target !== '_self')) return;
  if (passing === a) {
    passing = null;
    return;
  }
  const url = new URL(a.href, location.href);
  const message = sameWorkspace(url) ? '' : leaveWarning();
  if (!message) return;
  e.preventDefault();
  e.stopImmediatePropagation();
  askToLeave(message).then(ok => ok && follow(a));
}, true);
window.addEventListener('popstate', e => {
  if (returning) {
    returning = false;
    return;
  }
  if (!navigationGuard || sameWorkspace(new URL(location.href))) return;
  const message = leaveWarning();
  if (!message) return;
  history.pushState(null, '', `/w/${encodeURIComponent(navigationGuard.workspace)}`);
  e.stopImmediatePropagation();
  askToLeave(message).then(ok => {
    if (!ok) return;
    returning = true;
    history.back();
  });
}, true);
