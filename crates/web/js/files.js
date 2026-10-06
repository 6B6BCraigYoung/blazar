
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

export function readText(file) {
  return file.text();
}

let navigationGuard = null;
export function setNavigationGuard(workspace, dirtyCount) {
  navigationGuard = { workspace, dirtyCount };
}
export function clearNavigationGuard(workspace) {
  if (navigationGuard?.workspace === workspace) navigationGuard = null;
}
export function confirmNavigation() {
  if (!window.dispatchEvent(new Event('blazar:before-navigate', { cancelable: true }))) return false;
  const count = navigationGuard?.dirtyCount() || 0;
  return !count || window.confirm(`有 ${count} 个文件还没保存。离开会丢失改动，确定离开？`);
}
function sameWorkspace(url) {
  if (!navigationGuard || url.origin !== location.origin) return false;
  const id = encodeURIComponent(navigationGuard.workspace);
  return url.pathname === `/w/${id}` || url.pathname === `/workspaces/${id}`;
}
window.addEventListener('click', e => {
  if (e.defaultPrevented || e.button !== 0 || e.ctrlKey || e.metaKey || e.shiftKey || e.altKey) return;
  const a = e.target.closest?.('a[href]');
  if (!a || a.hasAttribute('download') || (a.target && a.target !== '_self')) return;
  const url = new URL(a.href, location.href);
  if (sameWorkspace(url) || confirmNavigation()) return;
  e.preventDefault();
  e.stopImmediatePropagation();
}, true);
window.addEventListener('popstate', e => {
  if (!navigationGuard || sameWorkspace(new URL(location.href)) || confirmNavigation()) return;
  history.pushState(null, '', `/w/${encodeURIComponent(navigationGuard.workspace)}`);
  e.stopImmediatePropagation();
}, true);
