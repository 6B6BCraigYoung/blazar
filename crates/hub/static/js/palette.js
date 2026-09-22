let palItems = [], palSel = 0;
function openPalette() {
  palItems = [
    { t: '全部工作区', k: '导航', go: () => navigate('#/workspaces') },
    { t: '运行中', k: '导航', go: () => navigate('#/running') },
    { t: '运行时', k: '导航', go: () => navigate('#/runtimes') },
    { t: 'Agent', k: '导航', go: () => navigate('#/agents') },
    { t: '新建 Agent', k: '命令', go: () => dlgAgent() },
    { t: '机器与组网', k: '导航', go: () => navigate('#/nodes') },
    { t: '用量', k: '导航', go: () => navigate('#/usage') },
    { t: '设置', k: '导航', go: () => navigate('#/settings') },
    { t: '新建工作区', k: '命令', go: dlgNewWorkspace },
    ...(S.ws ? [pickedEditor(), ...Object.keys(EDITORS).filter(k => k !== pickedEditor())].map(k => ({
      t: `在 ${EDITORS[k].label} 中打开`, k: '命令 · 当前工作区', go: () => openInEditor(k) })) : []),
    { t: '从 mesh 发现节点', k: '命令', go: async () => {
        const r = await post('/api/mesh/refresh'); toast(`发现 ${r.discovered} 个节点`);
        await refreshState(); render(); } },
    ...S.workspaces.map(w => ({ t: w.name, k: `工作区 · ${w.node}`, go: () => navigate(`#/workspaces/${w.id}`) })),
    ...S.nodes.map(n => ({ t: n.name, k: '机器', go: () => navigate(`#/nodes/${encodeURIComponent(n.name)}`) })),
  ];
  $('#palette').dataset.open = 'true';
  $('#palInput').value = ''; palSel = 0;
  drawPalette(); $('#palInput').focus();
}
function drawPalette() {
  const q = ($('#palInput').value || '').toLowerCase();
  const hits = palItems.filter(i => !q || (i.t + i.k).toLowerCase().includes(q)).slice(0, 40);
  palSel = Math.min(palSel, Math.max(0, hits.length - 1));
  $('#palList').innerHTML = hits.length ? hits.map((i, idx) => `
    <div class="palrow" data-i="${idx}" data-sel="${idx === palSel}">
      <span class="grow truncate">${esc(i.t)}</span><span class="k">${esc(i.k)}</span></div>`).join('')
    : '<div class="empty" style="padding:24px">无匹配</div>';
  $$('#palList .palrow').forEach(el => {
    el.onclick = () => { $('#palette').dataset.open = 'false'; hits[+el.dataset.i].go(); };
  });
  $('#palList').dataset.hits = hits.length;
  palHits = hits;
}
let palHits = [];
