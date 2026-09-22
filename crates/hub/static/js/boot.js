const closeSide = () => { $('#side').dataset.open = 'false'; $('#scrim-side').dataset.open = 'false'; };
function wireHeader() {
  const b = $('.menu-btn');
  if (b && innerWidth <= 900) {
    b.style.display = '';
    b.onclick = () => {
      const open = $('#side').dataset.open !== 'true';
      $('#side').dataset.open = String(open); $('#scrim-side').dataset.open = String(open);
    };
  }
}
$('#scrim-side').onclick = closeSide;
$('#dlg').onclick = e => { if (e.target.id === 'dlg') closeDlg(); };
$('#palette').onclick = e => { if (e.target.id === 'palette') $('#palette').dataset.open = 'false'; };
$('#btnPalette').onclick = openPalette;
$('#btnNew').onclick = dlgNewWorkspace;
$('#palInput').oninput = () => { palSel = 0; drawPalette(); };
$('#palInput').onkeydown = e => {
  if (e.key === 'ArrowDown') { e.preventDefault(); palSel++; drawPalette(); }
  else if (e.key === 'ArrowUp') { e.preventDefault(); palSel = Math.max(0, palSel - 1); drawPalette(); }
  else if (e.key === 'Enter') {
    e.preventDefault();
    const hit = palHits[palSel];
    if (hit) { $('#palette').dataset.open = 'false'; hit.go(); }
  }
};

let layRaf = 0;
addEventListener('resize', () => {
  if (layRaf) return;
  layRaf = requestAnimationFrame(() => { layRaf = 0; if (S.lay) applyLayout(); });
});

const SIDE_KEY = 'blazar.side.collapsed';
function setSide(collapsed) {
  document.body.dataset.side = collapsed ? 'off' : 'on';
  try { localStorage.setItem(SIDE_KEY, collapsed ? '1' : '0'); } catch (_) {}

  setTimeout(() => { if (S.lay) applyLayout(); S.termResize?.(); S.editor?.layout?.(); }, 220);
}
try { document.body.dataset.side = localStorage.getItem(SIDE_KEY) === '1' ? 'off' : 'on'; } catch (_) {}
$('#btnSideHide').onclick = () => setSide(true);
$('#btnSideShow').onclick = () => setSide(false);
addEventListener('keydown', e => {

  if ((e.metaKey || e.ctrlKey) && !e.altKey && e.code === 'KeyS' && S.ws) { e.preventDefault(); saveFile(); return; }
  if (handleShortcut(e)) return;
  if (e.key === 'Escape') { closeDlg(); $('#palette').dataset.open = 'false'; closeSide(); }
});

addEventListener('unhandledrejection', e => { if (e.reason?.name === 'Canceled' || e.reason?.message === 'Canceled') e.preventDefault(); });

Object.assign(window, { dlgNewWorkspace, dlgAddMachine, closeDlg, navigate, dlgRules, openNotifyPop, dlgShortcuts, dlgAutopilot, taskToLark, taskToObsidian });
window.navigate = navigate;

applyUiPrefs();
matchMedia('(prefers-color-scheme: dark)').addEventListener?.('change', () => { if (uiPrefs().theme === 'system') applyUiPrefs(); });
(async () => {
  try { S.agents = await api('/api/agents'); } catch (_) { S.agents = []; }
  loadTasks().then(renderSidebar).catch(() => {});
  loadAutopilots().then(renderSidebar).catch(() => {});
  refreshInboxCount();
  loadServerPrefs();
  loadOffice().catch(() => {});
  loadProfiles().then(() => { if (S.ws) fillAgentSel(); });
  api('/api/runtimes').then(v => { S.runtimes = v.runtimes; if (S.ws) fillAgentSel();
    const c = $('#cnt-rt'); if (c) c.textContent = v.runtimes.filter(r => r.installed && r.authed !== false).length || ''; })
    .catch(() => {});
  try { await refreshState(); } catch (e) { toast('加载失败: ' + e.message); }
  await render();
  connectWs();
  checkPendingInvite();
  addEventListener('beforeunload', e => { if (S.ws && dirtyFiles().length) { e.preventDefault(); e.returnValue = ''; } });
})();
