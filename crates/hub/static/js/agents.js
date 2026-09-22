async function agentAction(a, act) {
  const id = encodeURIComponent(a.id);
  const after = async () => { await loadProfiles(); if (S.route.startsWith('#/agent/')) render(); else drawAgents(); };
  try {
    if (act === 'dm') { pickWorkspaceFor(a); return; }
    if (act === 'duplicate') { navigate(`#/agents/new?template=${id}`); return; }
    if (act === 'cancel') {
      const n = a.running || 0;
      if (!await ask(`取消"${a.name}"的全部运行？\n${n ? `将取消运行：${n} 个进行中。进行中的运行最多需要 5 秒才能完全停止。已取消的运行无法恢复。` : '没有可取消的活动运行。'}`,
        { ok: '取消全部运行', cancel: '保留', danger: true })) return;
      const r = await post(`/api/agent-profiles/${id}/cancel-runs`);
      toast(r.cancelled ? `已取消 ${r.cancelled} 次运行` : '没有可取消的活动运行');
    } else if (act === 'archive') {
      if (!await ask(`归档"${a.name}"？\n归档后这个智能体将无法被选择，进行中的运行会被取消。所有历史会被保留，可以稍后恢复。`, { ok: '归档', danger: true })) return;
      await post(`/api/agent-profiles/${id}/archive`); toast('已归档智能体');
    } else if (act === 'restore') {
      await post(`/api/agent-profiles/${id}/restore`); toast('已恢复智能体');
    } else if (act === 'delete') {
      if (!await ask(`彻底删除"${a.name}"？\n这一步不可撤销（对话记录仍保留）。`, { ok: '删除', danger: true })) return;
      await api(`/api/agent-profiles/${id}`, { method: 'DELETE' }); toast('已删除智能体');
      if (S.route.startsWith('#/agent/')) { await loadProfiles(); navigate('#/agents'); return; }
    }
    await after();
  } catch (e) { toast(e.message || '操作失败'); }
}

function pickWorkspaceFor(a) {
  if (agentPresence(a).availability === 'unbound') { toast('请先绑定运行时，再运行该智能体。'); return; }
  const list = S.workspaces || [];
  if (!list.length) { toast('还没有工作区，先建一个'); navigate('#/workspaces'); return; }
  openDlg(`<h3>在哪个工作区和"${esc(a.name)}"对话</h3>
    <div style="max-height:50vh;overflow:auto">${list.map(w => `<button class="cp-row" data-w="${esc(w.id)}"><span class="cp-t"><b>${esc(w.name)}</b><span>${esc(w.node)}:${esc(w.path)}</span></span></button>`).join('')}</div>
    <div class="dfoot"><button class="btn btn-outline" onclick="closeDlg()">取消</button></div>`);
  $$('#dlgBody [data-w]').forEach(b => {
    b.onclick = () => {
      try { localStorage.setItem('blazar.agent.' + b.dataset.w, 'p:' + a.id); localStorage.setItem('blazar.chats.' + b.dataset.w + '.newchat', '1'); } catch (_) {}
      closeDlg(); navigate(`#/workspaces/${b.dataset.w}`);
    };
  });
}

function openThreadIn(ws, thread) {
  try {
    const key = 'blazar.chats.' + ws;
    const saved = JSON.parse(localStorage.getItem(key) || 'null') || { tabs: [] };
    if (!saved.tabs.includes(thread)) saved.tabs.push(thread);
    saved.active = thread;
    localStorage.setItem(key, JSON.stringify(saved));
  } catch (_) {}
  navigate(`#/workspaces/${ws}`);
}
function agentMenuItems(a, inDetail) {
  const archived = !!a.archived_at;
  return [
    !inDetail && { label: '打开', run: () => navigate(`#/agent/${encodeURIComponent(a.id)}`) },
    !inDetail && '-',
    !archived && a.running > 0 && { label: '取消全部运行', run: () => agentAction(a, 'cancel') },
    !archived && { label: '复制', run: () => agentAction(a, 'duplicate') },
    archived && { label: '恢复', run: () => agentAction(a, 'restore') },
    !archived && '-',
    !archived && { label: inDetail ? '归档智能体' : '归档', run: () => agentAction(a, 'archive'), danger: true },
    archived && '-',
    archived && { label: '删除', run: () => agentAction(a, 'delete'), danger: true },
  ];
}

const AG_VIEW_KEY = 'blazar.agents.view';
function agView() {
  let v = {}; try { v = JSON.parse(localStorage.getItem(AG_VIEW_KEY) || '{}'); } catch (_) {}
  return { scope: 'all', sort: 'recent', dir: 'desc', avail: 'all', runtime: '', ...v };
}
function setAgView(patch) {
  const v = { ...agView(), ...patch };
  try { localStorage.setItem(AG_VIEW_KEY, JSON.stringify(v)); } catch (_) {}
  return v;
}
async function pageAgents() {
  S.agSel = new Set();
  $('#page').innerHTML = `
    <div class="phead">
      <button class="btn btn-ghost btn-icon menu-btn" style="display:none" aria-label="菜单"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M4 7h16M4 12h16M4 17h16"/></svg></button>
      <h1>智能体</h1><span class="badge badge-muted num" id="agCount"></span>
      <span class="t-caption faint truncate" style="margin-left:6px">由运行时创建的 AI 队友：各有各的指令、模型和权限。</span>
      <div class="grow"></div>
      <button class="btn btn-brand btn-sm" id="btnNewAgent">新建智能体</button>
    </div>
    <div class="agl-bar">
      <div class="seg" id="agScope"><button data-scope="all">全部</button><button data-scope="archived">已归档</button></div>
      <input id="agq" class="input input-sm" style="width:220px" placeholder="搜索智能体...">
      <div class="grow"></div>
      <span class="t-caption faint num" id="agOf" title="当前结果 / 范围内全部"></span>
      <button class="btn btn-outline btn-xs" id="agFilter">筛选</button>
      <button class="btn btn-outline btn-xs" id="agDisplay">显示</button>
    </div>
    <div class="scroll flush"><div id="agBody"><div class="t-caption faint" style="padding:16px var(--gutter)">读取中…</div></div></div>
    <div class="ag-batch" id="agBatch" hidden></div>`;
  wireHeader();
  $('#btnNewAgent').onclick = () => navigate('#/agents/new');
  $('#agq').oninput = drawAgents;
  $$('#agScope [data-scope]').forEach(b => { b.onclick = async () => { setAgView({ scope: b.dataset.scope }); S.agSel.clear(); await loadProfiles(); drawAgents(); }; });
  $('#agFilter').onclick = e => {
    const v = agView(), rts = [...new Set((S.profiles || []).concat(S.profilesView || []).map(a => a.runtime))];
    openMenu(e.currentTarget, [
      ...[['all', '全部'], ['online', '在线'], ['offline', '离线']].map(([k, t]) => ({ label: `${v.avail === k ? '✓ ' : '　'}可用性：${t}`, run: () => { setAgView({ avail: k }); drawAgents(); } })),
      '-',
      { label: `${!v.runtime ? '✓ ' : '　'}全部运行时`, run: () => { setAgView({ runtime: '' }); drawAgents(); } },
      ...rts.map(r => ({ label: `${v.runtime === r ? '✓ ' : '　'}${AGENT_LABEL[r] || r}`, run: () => { setAgView({ runtime: r }); drawAgents(); } })),
      (v.avail !== 'all' || v.runtime) && '-',
      (v.avail !== 'all' || v.runtime) && { label: '清除筛选', run: () => { setAgView({ avail: 'all', runtime: '' }); drawAgents(); } },
    ]);
  };
  $('#agDisplay').onclick = e => {
    const v = agView();
    openMenu(e.currentTarget, [
      ...[['recent', '最近活跃'], ['name', '名称'], ['runs', '运行最多'], ['created', '最近创建']].map(([k, t]) => ({ label: `${v.sort === k ? '✓ ' : '　'}排序：${t}`, run: () => { setAgView({ sort: k }); drawAgents(); } })),
      '-',
      { label: `${v.dir === 'asc' ? '✓ ' : '　'}升序`, run: () => { setAgView({ dir: 'asc' }); drawAgents(); } },
      { label: `${v.dir === 'desc' ? '✓ ' : '　'}降序`, run: () => { setAgView({ dir: 'desc' }); drawAgents(); } },
    ]);
  };
  if (!S.runtimes) { try { S.runtimes = (await api('/api/runtimes')).runtimes; } catch (_) { S.runtimes = []; } }
  await loadProfiles();
  drawAgents();
}
async function loadProfiles() {

  try { S.profiles = await api('/api/agent-profiles'); } catch (_) { S.profiles = S.profiles || []; }
  if (S.route === '#/agents' && agView().scope === 'archived') {
    try { S.profilesView = await api('/api/agent-profiles?scope=archived'); } catch (_) { S.profilesView = []; }
  }
  const c = $('#cnt-agents'); if (c) c.textContent = (S.profiles || []).length || '';
  return S.profiles;
}

function modesFor(rt) {
  if (rt === 'codex') return MODES_CODEX;
  if (rt === 'claude' || rt === 'grok') return MODES_CLAUDE;
  return [];
}
const permOpts = (rt, cur) => `<option value="">按运行时设置</option>` + modesFor(rt).map(([v, t, d]) =>
  `<option value="${v}" title="${esc(d)}" ${cur === v ? 'selected' : ''}>${esc(t)}</option>`).join('');
function drawAgents() {
  const host = $('#agBody'); if (!host) return;
  const v = agView();
  const all = v.scope === 'archived' ? (S.profilesView || []) : (S.profiles || []);
  $$('#agScope [data-scope]').forEach(b => { b.dataset.on = String(b.dataset.scope === v.scope); });
  const q = ($('#agq')?.value || '').trim().toLowerCase();
  let list = all.filter(a => (!q || `${a.name} ${a.description}`.toLowerCase().includes(q))
    && (v.avail === 'all' || agentPresence(a).availability === v.avail)
    && (!v.runtime || a.runtime === v.runtime));
  const key = { recent: a => Date.parse(a.last_used_at || 0) || 0, name: a => a.name.toLowerCase(), runs: a => a.runs_30d, created: a => Date.parse(a.created_at) || 0 }[v.sort];
  list = [...list].sort((x, y) => { const a = key(x), b = key(y); return (a < b ? -1 : a > b ? 1 : 0) * (v.dir === 'asc' ? 1 : -1); });
  $('#agCount').textContent = (S.profiles || []).length || '';
  $('#agOf').textContent = all.length ? `${list.length} / ${all.length}` : '';
  $('#agFilter').dataset.on = String(v.avail !== 'all' || !!v.runtime);
  if (!all.length) {
    host.innerHTML = v.scope === 'archived'
      ? `<div class="ag-blank"><b>无匹配项</b><span>还没有已归档智能体。</span></div>`
      : `<div class="ag-blank"><b>还没有智能体</b><span>创建智能体后，就可以在工作区里选它来对话。</span>
          <button class="btn btn-brand btn-sm" id="agFirst" style="margin-top:10px">新建智能体</button></div>`;
    $('#agFirst')?.addEventListener('click', () => navigate('#/agents/new'));
    drawAgBatch(); return;
  }
  if (!list.length) {
    host.innerHTML = `<div class="ag-blank"><b>无匹配项</b><span>${q ? `没有${v.scope === 'archived' ? '已归档' : ''}智能体匹配"${esc(q)}"。` : '该筛选下没有匹配的智能体。'}</span></div>`;
    drawAgBatch(); return;
  }
  host.innerHTML = `<div class="agl">
    <div class="agl-row agl-head"><span></span><span>智能体</span><span>状态</span><span>工作负载</span><span>运行时</span><span>活动（7 天）</span><span class="r">运行次数</span><span>最近活跃</span><span>模型</span><span></span></div>
    ${list.map(a => { const p = agentPresence(a); return `
    <div class="agl-row" data-id="${esc(a.id)}" data-sel="${S.agSel.has(a.id)}">
      <span class="ck"><input type="checkbox" data-ck ${S.agSel.has(a.id) ? 'checked' : ''} aria-label="选择"></span>
      <span class="who">${avatarHtml(a)}<span class="nm"><b class="${a.archived_at ? 'muted' : ''}">${esc(a.name)}</b><span>${esc(a.description || '暂无描述')}</span></span></span>
      <span>${statusCellHtml(a)}</span>
      <span class="t-caption ${p.workload === 'working' ? '' : 'muted'}">${a.archived_at ? '—' : WORK_TEXT[p.workload]}</span>
      <span class="rtc">${rtIcon(a.runtime)}<span class="truncate">${esc(a.runtime_label)}</span></span>
      <span title="近 7 天 ${a.activity_7d.reduce((x, y) => x + y, 0)} 次运行${a.failed_7d ? ` · ${a.failed_7d} 次失败` : ''}">${sparkline(a.activity_7d)}</span>
      <span class="r num">${a.runs_30d || '<span class="faint">0</span>'}</span>
      <span class="t-caption muted">${lastActiveText(a.last_used_at)}</span>
      <span class="mono t-caption truncate muted">${esc(a.model || '默认')}</span>
      <span class="r"><button class="kebab" data-menu aria-label="行操作">⋯</button></span>
    </div>`; }).join('')}</div>`;
  $$('#agBody .agl-row[data-id]').forEach(row => {
    const a = all.find(x => x.id === row.dataset.id);
    row.onclick = e => {
      if (e.target.closest('[data-ck]')) { e.stopPropagation(); const on = e.target.checked; on ? S.agSel.add(a.id) : S.agSel.delete(a.id); row.dataset.sel = String(on); drawAgBatch(); return; }
      const k = e.target.closest('[data-menu]');
      if (k) { e.stopPropagation(); openMenu(k, agentMenuItems(a, false)); return; }
      navigate(`#/agent/${encodeURIComponent(a.id)}`);
    };
  });
  drawAgBatch();
}

function drawAgBatch() {
  const bar = $('#agBatch'); if (!bar) return;
  const n = S.agSel.size;
  bar.hidden = !n; if (!n) return;
  const archived = agView().scope === 'archived';
  bar.innerHTML = `<span>已选 ${n} 项</span><button class="btn btn-ghost btn-xs" data-b="clear">清除选择</button><span class="grow"></span>
    ${archived ? '<button class="btn btn-outline btn-xs" data-b="restore">恢复</button>' : '<button class="btn btn-danger btn-xs" data-b="archive">归档</button>'}`;
  bar.querySelectorAll('[data-b]').forEach(b => {
    b.onclick = async () => {
      if (b.dataset.b === 'clear') { S.agSel.clear(); drawAgents(); return; }
      const act = b.dataset.b;
      if (act === 'archive' && !await ask(`归档这 ${n} 个智能体？\n归档后无法被选择，进行中的运行会被取消。所有历史会被保留，可以稍后恢复。`, { ok: '归档', danger: true })) return;
      let okN = 0, bad = 0;
      for (const id of S.agSel) { try { await post(`/api/agent-profiles/${encodeURIComponent(id)}/${act}`); okN++; } catch (_) { bad++; } }
      toast(bad ? `已应用 ${okN} 个；${bad} 个失败（保持当前状态）。` : act === 'archive' ? '已归档智能体' : '已恢复智能体');
      S.agSel.clear(); await loadProfiles(); drawAgents();
    };
  });
}
