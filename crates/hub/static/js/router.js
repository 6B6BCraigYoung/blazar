const ROUTES = [
  [/^#\/workspaces\/(.+)$/, (m) => pageWorkspace(m[1])],
  [/^#\/workspaces$/,       () => pageWorkspaceList('all')],
  [/^#\/running$/,          () => pageWorkspaceList('running')],
  [/^#\/waiting$/,          () => pageWorkspaceList('awaiting_approval')],
  [/^#\/inbox$/,            () => pageInbox()],
  [/^#\/apps$/,             () => pageApps()],
  [/^#\/apps\/(.+)$/,       (m) => pageApp(m[1])],
  [/^#\/skills$/,           () => pageLibrary()],
  [/^#\/library$/,          () => pageLibrary()],
  [/^#\/tasks$/,            () => pageTasks()],
  [/^#\/autopilots$/,       () => pageAutopilots()],
  [/^#\/project\/(.+)$/,    (m) => pageWorkspaceList('project', decodeURIComponent(m[1]))],
  [/^#\/runtimes\/(.+)$/,   (m) => pageAgentDetail(decodeURIComponent(m[1]))],

  [/^#\/agents\/new$/,      () => pageAgentCreate()],
  [/^#\/agent\/(.+)$/,      (m) => pageAgentProfile(decodeURIComponent(m[1].split('?')[0]))],
  [/^#\/agents\/(.+)$/,     (m) => navigate('#/runtimes/' + m[1])],
  [/^#\/agents$/,           () => pageAgents()],
  [/^#\/runtimes$/,         () => pageRuntimes()],
  [/^#\/nodes\/(.+)$/,      (m) => pageNodeDetail(decodeURIComponent(m[1]))],
  [/^#\/nodes$/,            () => pageNodes()],
  [/^#\/mesh$/,             () => navigate('#/nodes')],
  [/^#\/usage$/,            () => pageUsage()],
  [/^#\/settings$/,         () => pageSettings()],
];
function navigate(hash) { location.hash = hash; }
async function render() {
  const hash = location.hash || '#/workspaces';

  const path = hash.split('?')[0];
  if (S.ws && S.route && path !== S.route.split('?')[0] && dirtyFiles().length) {
    const n = dirtyFiles().length;
    if (!await ask(`有 ${n} 个文件还没保存，离开的话改动就丢了。`, { ok: '不保存，离开', danger: true })) {
      history.replaceState(null, '', S.route); markNav(); return;
    }
  }
  S.route = hash;
  S.onStateChange = null;
  resetWorkspace();
  for (const [re, fn] of ROUTES) {
    const m = path.match(re);
    if (m) { try { await fn(m); } catch (e) { $('#page').innerHTML = errPage(e); } markNav(); return; }
  }
  navigate('#/workspaces');
}
const errPage = e => `<div class="empty">加载失败<br><span class="t-caption">${esc(e.message)}</span></div>`;
function markNav() {

  const path = S.route.split('?')[0];
  const exact = $$('#side [data-route]').some(b => b.dataset.route === path);
  $$('#side [data-route]').forEach(b => {
    b.dataset.active = String(exact ? b.dataset.route === path
      : (b.dataset.route !== '#/workspaces' && S.route.startsWith(b.dataset.route)));
  });
}
addEventListener('hashchange', render);
