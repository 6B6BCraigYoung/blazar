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
