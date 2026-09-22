function wsFilter(kind, arg) {
  if (kind === 'running') return S.workspaces.filter(w => w.activity === 'running');
  if (kind === 'awaiting_approval') return S.workspaces.filter(w => w.activity === 'awaiting_approval');
  if (kind === 'project') return S.workspaces.filter(w => w.project === arg);
  return S.workspaces;
}
function pageWorkspaceList(kind, arg) {
  const title = { all: '全部工作区', running: '运行中', awaiting_approval: '等我审批',
                  project: arg }[kind];
  const list = wsFilter(kind, arg);
  $('#page').innerHTML = `
    <div class="phead">
      <button class="btn btn-ghost btn-icon menu-btn" style="display:none" aria-label="菜单"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M4 7h16M4 12h16M4 17h16"/></svg></button>
      <h1>${esc(title)}</h1><span class="badge badge-muted num">${list.length}</span>
      <div class="grow"></div>
      <input id="wsq" class="input input-sm" style="width:220px" placeholder="筛选…">
      <button class="btn btn-brand btn-sm" onclick="dlgNewWorkspace()">新建</button>
    </div>
    ${kind === 'awaiting_approval' ? '<div id="apprAll" style="padding:12px var(--gutter) 0"></div>' : ''}
    <div class="scroll flush">
      <div id="wslist" class="ws-grid"></div>
    </div>`;
  const draw = () => {

    const list = wsFilter(kind, arg);
    const q = ($('#wsq').value || '').toLowerCase();
    const rows = list.filter(w => !q ||
      `${w.name} ${w.node} ${w.project || ''} ${w.path}`.toLowerCase().includes(q));
    $('#wslist').innerHTML = rows.length ? rows.map(w => `
      <div class="card" data-id="${w.id}">
        <div class="wc-top"><b class="${unseen(w) ? 'strong' : ''}" title="${esc(w.name)}">${esc(w.name)}</b>${statusPill(shownActivity(w))}</div>
        <div class="wc-path" title="${esc(w.path)}">${esc(w.node)}:${esc(w.path)}</div>
        <div class="wc-meta">
          ${w.project ? `<span title="项目">${esc(w.project)}</span>` : ''}
          <span class="num">${diffBadge(w.diff, w.diff_at)}</span>
          <span class="faint">${ago(w.last_active_at)}</span>
        </div>
        <div class="wc-act">
          <button class="btn btn-outline btn-xs" data-insp="${w.id}">属性</button>
          <button class="btn btn-brand btn-xs" data-open="${w.id}">打开</button>
        </div>
      </div>`).join('')
      : `<div class="empty" style="grid-column:1/-1">${list.length ? '无匹配' : '还没有工作区 —— 点右上角「新建」'}</div>`;
    $$('#wslist .card').forEach(el => {
      el.onclick = e => {
        if (e.target.closest('[data-insp]')) { openWsInspector(el.dataset.id); return; }
        navigate(`#/workspaces/${el.dataset.id}`);
      };
    });
  };
  $('#wsq').oninput = draw;

  const drawAppr = async () => {
    const host = $('#apprAll'); if (!host) return;
    try {
      const list = await api('/api/approvals');
      const name = id => S.workspaces.find(w => w.id === id);
      host.innerHTML = list.length ? list.map(a => {
        const w = name(a.workspace_id);
        return `<div style="margin-bottom:10px"><div class="t-caption muted" style="margin-bottom:4px">
          <a href="#/workspaces/${esc(a.workspace_id)}">${esc(w?.name || a.workspace_id)}</a>
          · ${esc(w?.node || '')} · ${ago(a.created_at)}</div>
          ${(toolName(a.request?.tool_name) === 'AskUserQuestion' ? askCard : approvalCard)({ id: a.id, request: a.request })}</div>`;
      }).join('') : '<div class="t-caption faint" style="padding-bottom:8px">没有等你裁决的操作</div>';
    } catch (e) { host.innerHTML = `<div class="empty">${esc(e.message)}</div>`; }
  };
  if (kind === 'awaiting_approval') drawAppr();

  S.onStateChange = () => { draw(); if (kind === 'awaiting_approval') drawAppr(); };
  draw();
  wireHeader();
}

async function openWsInspector(id) {
  try {
    const d = await api(`/api/workspaces/${id}/detail`);
    openInspector({ ...d, id });
  } catch (e) { toast('读取失败: ' + e.message); }
}
