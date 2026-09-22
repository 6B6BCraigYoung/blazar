function pageNodes() {
  $('#page').innerHTML = `
    <div class="phead">
      <button class="btn btn-ghost btn-icon menu-btn" style="display:none" aria-label="菜单"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M4 7h16M4 12h16M4 17h16"/></svg></button>
      <h1>机器与组网</h1>
      <span class="badge badge-muted num">${S.nodes.filter(n => n.status === 'online').length}/${S.nodes.length}</span>
      <div class="grow"></div>
      <button class="btn btn-outline btn-sm" id="btnJoinToml">导入 EasyTier 配置…</button>
      <button class="btn btn-brand btn-sm" id="btnJoinFile">用邀请文件加入…</button>
    </div>
    <div class="scroll">
      <div class="card" id="meshLocal"><div class="t-caption faint">读取本机状态…</div></div>
      <div class="card">
        <h3>机器 <span class="badge badge-muted num">${S.nodes.length}</span><span id="topoSrc" class="t-caption faint" style="font-weight:400"></span><div class="grow"></div>
          <input id="nq" class="input input-sm" style="width:180px" placeholder="筛选…">
          <button class="btn btn-outline btn-sm" id="btnMesh">从 mesh 发现</button></h3>
        <div class="desc">组网里的机器自动出现；只能 SSH 的机器在 <span class="mono">~/.ssh/config</span> 配好 Host 后直接用 Host 名新建工作区。</div>
        <div id="topoWarn"></div>
        <div id="nlist" class="ws-grid" style="padding:4px 0 0"></div>
      </div>
      <div class="card" id="meshIssue"><div class="t-caption faint">连接签发节点…</div></div>
      <div class="card" id="meshInvites"><div class="t-caption faint">读取邀请记录…</div></div>
    </div>
    <input type="file" id="inviteFile" accept=".blazar" hidden>
    <input type="file" id="tomlFile" accept=".toml,text/plain" hidden>`;
  wireHeader();
  $('#btnJoinFile').onclick = () => $('#inviteFile').click();
  $('#btnJoinToml').onclick = () => $('#tomlFile').click();
  for (const id of ['#inviteFile', '#tomlFile']) {
    $(id).onchange = e => {
      const f = e.target.files[0]; e.target.value = '';
      if (f) openMeshFile(f);
    };
  }
  const draw = () => {
    const q = ($('#nq')?.value || '').toLowerCase();
    const rows = S.nodes.filter(n => !q || n.name.toLowerCase().includes(q));
    const host = $('#nlist'); if (!host) return;
    host.innerHTML = rows.length ? rows.map(n => `
      <div class="card" data-n="${esc(n.name)}">
        <div class="wc-top"><b title="${esc(n.name)}">${esc(n.name)}</b>${statusPill(n.status)}</div>
        <div class="wc-path">${esc(n.name === 'local' ? '本机' : (n.ipv4 || 'SSH'))}${n.cost ? ` · ${esc(n.cost)}` : ''}</div>
        <div class="wc-meta">
          <span>延迟 <span class="num">${latency(n.latency_ms)}</span></span>
          <span>工作区 <span class="num">${n.workspace_count || 0}</span></span>
        </div>
        <div class="wc-act">
          <button class="btn btn-outline btn-xs" data-detail="${esc(n.name)}">属性</button>
          <button class="btn btn-brand btn-xs" data-newws="${esc(n.name)}">新建工作区</button>
        </div>
      </div>`).join('')
      : '<div class="empty" style="grid-column:1/-1">还没有机器 —— 点「从 mesh 发现」</div>';
    $$('#nlist .card').forEach(el => {
      el.onclick = e => {
        if (e.target.closest('[data-newws]')) { dlgNewWorkspace(el.dataset.n); return; }
        navigate(`#/nodes/${encodeURIComponent(el.dataset.n)}`);
      };
    });
  };
  $('#nq').oninput = draw;
  S.onStateChange = draw;
  $('#btnMesh').onclick = async () => {
    try {
      const r = await post('/api/mesh/refresh');
      toast(`发现 ${r.discovered} 个 mesh 节点`);
      await refreshState(); draw();
    } catch (e) { toast('mesh 发现失败: ' + e.message); }
  };
  draw();
  drawTopology();
  drawMeshLocal(); drawIssuer(); drawInvites();
}

async function drawTopology() {
  let v; try { v = await api('/api/mesh/topology'); } catch (_) { return; }
  const src = $('#topoSrc'), warn = $('#topoWarn');
  if (!src || !warn) return;
  if (v.source) { src.textContent = `来自 ${v.source}`; warn.innerHTML = ''; return; }
  src.textContent = '';
  warn.innerHTML = `<div class="notice warn">读不到组网拓扑，只显示本机 ${TIP('填一台能 SSH 登录、跑着 EasyTier 的节点（通常是枢纽），节点列表从它读取。')}
    <div class="row" style="margin-top:8px;gap:6px">
      <input id="topoVia" class="input input-sm mono" style="max-width:160px" placeholder="SSH Host，如 hub-host">
      <input id="topoCt" class="input input-sm mono" style="max-width:160px" placeholder="容器名（可选）">
      <button class="btn btn-brand btn-sm" id="topoSave">读取节点</button></div></div>`;
  $('#topoSave').onclick = async () => {
    const via = $('#topoVia').value.trim();
    if (!via) { toast('填一个 SSH Host'); return; }
    try {
      await api('/api/mesh/issuer', { method: 'PUT', headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ via, container: $('#topoCt').value.trim() || null }) });
      toast('已设置，正在发现节点…');
      setTimeout(async () => { await refreshState(); render(); }, 4000);
    } catch (e) { toast('设置失败：' + e.message); }
  };
}

async function pageNodeDetail(name) {
  const n = S.nodes.find(x => x.name === name) || { name, status: 'offline' };
  $('#page').innerHTML = `
    <div class="phead">
      <button class="btn btn-ghost btn-icon menu-btn" style="display:none" aria-label="菜单"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M4 7h16M4 12h16M4 17h16"/></svg></button>
      <a href="#/nodes" class="muted t-label">机器</a><span class="faint">/</span>
      <h1>${esc(name)}</h1><span class="dot ${esc(n.status)}"></span>
      <span class="t-caption muted">${n.status === 'online' ? '在线' : '离线'}</span>
      <div class="grow"></div>
      <button class="btn btn-outline btn-sm" id="btnProbe">体检</button>
      <button class="btn btn-outline btn-sm" id="btnScan">扫描 agent</button>
    </div>
    <div class="scroll">
      <div class="card"><h3>硬件与出口</h3><div id="hw"><div class="empty">点「体检」采集</div></div></div>
      <div class="card"><h3>已安装的 agent CLI</h3><div id="ag"><div class="empty">点「扫描 agent」</div></div></div>
      <div class="card"><h3>该机器上的工作区</h3><div id="nws"></div></div>
      <div class="card">
        <div class="row" style="margin-bottom:10px"><h3 style="margin:0">残留</h3>
          <span class="t-caption faint" style="margin-left:8px">磁盘上有、库里没记着的 worktree 与私有目录</span>
          <div class="grow"></div>
          <button class="btn btn-outline btn-sm" id="btnLeft">清点</button></div>
        <div id="left"><div class="t-caption faint">工作区删记录、hub 重装、手工实验都会留下这种东西。
          按时间的 gc 找不到它们，只能以磁盘为准反查。</div></div>
      </div>
    </div>`;

  const list = S.workspaces.filter(w => w.node === name);
  $('#nws').innerHTML = list.length ? `<table class="tb"><tbody>${list.map(w => `
      <tr><td><span class="dot ${esc(w.activity)}" style="display:inline-block;margin-right:6px"></span>
        <a href="#/workspaces/${w.id}">${esc(w.name)}</a></td>
      <td class="m">${esc(w.path)}</td>
      <td class="m" style="text-align:right">${ACT[w.activity] || w.activity}</td></tr>`).join('')}</tbody></table>`
    : '<div class="t-caption faint">还没有工作区</div>';

  const drawLeft = async () => {
    const host = $('#left');
    host.innerHTML = '<div class="empty">清点中…</div>';
    try {
      const r = await api(`/api/nodes/${encodeURIComponent(name)}/leftovers`);
      if (!r.orphans.length) {
        host.innerHTML = `<div class="t-caption muted">没有残留。库里记着的 ${r.owned_count} 份都对得上。</div>`;
        return;
      }
      const kb = n => n > 1048576 ? (n / 1048576).toFixed(1) + ' GB' : n > 1024 ? (n / 1024).toFixed(0) + ' MB' : n + ' KB';
      host.innerHTML = `
        <div class="t-caption muted" style="margin-bottom:8px">${r.orphans.length} 份，共 ${kb(r.orphan_kb)}。
          有未提交改动的默认不清 —— 那可能是人的活。</div>
        <table class="tb"><thead><tr><th style="width:24px"></th><th>名字</th><th>分支</th>
          <th>大小</th><th>闲置</th><th>状态</th></tr></thead><tbody>
        ${r.orphans.map(l => `<tr>
          <td><input type="checkbox" class="lchk" value="${esc(l.leaf)}" ${l.dirty ? '' : 'checked'}></td>
          <td class="m" title="${esc(l.worktree || '')}">${esc(l.leaf)}</td>
          <td class="m">${esc(l.branch || '—')}</td>
          <td class="m num">${kb(l.size_kb)}</td>
          <td class="m num">${l.idle_days} 天</td>
          <td>${l.dirty ? `<span class="badge badge-warn">${l.dirty} 处未提交</span>`
              : l.broken ? '<span class="badge badge-muted">目录已损坏</span>'
              : !l.worktree ? '<span class="badge badge-muted">只剩私有目录</span>'
              : '<span class="badge badge-muted">干净</span>'}</td></tr>`).join('')}
        </tbody></table>
        <div class="actions" style="margin-top:10px">
          <button class="btn btn-danger btn-sm" id="btnSweep">清掉勾选的</button>
          <label class="t-caption muted row" style="gap:4px"><input type="checkbox" id="sweepForce">
            连有未提交改动的也清</label></div>`;
      $('#btnSweep').onclick = async () => {
        const leaves = $$('.lchk').filter(c => c.checked).map(c => c.value);
        if (!leaves.length) { toast('没有勾选'); return; }
        const force = $('#sweepForce').checked;
        if (force && !await ask('连有未提交改动的也清掉？这些改动会永久丢失。', { ok: '清掉', danger: true })) return;
        try {
          const s = await post(`/api/nodes/${encodeURIComponent(name)}/sweep`, { leaves, force });
          toast(`清掉 ${s.removed.length} 份` + (s.skipped.length
            ? `；跳过 ${s.skipped.length} 份：${s.skipped.map(([l, w]) => `${l}（${w}）`).join('；')}` : ''));
          drawLeft();
        } catch (e) { toast('清扫失败: ' + e.message); }
      };
    } catch (e) { host.innerHTML = `<div class="empty">${esc(e.message)}</div>`; }
  };
  $('#btnLeft').onclick = drawLeft;

  $('#btnProbe').onclick = async () => {
    $('#hw').innerHTML = '<div class="empty">体检中…</div>';
    try {
      const c = await post(`/api/nodes/${encodeURIComponent(name)}/probe`);
      const eg = Object.entries(c.egress || {});
      $('#hw').innerHTML = `
        <div class="kpi" style="margin-bottom:12px">
          <div class="k"><div class="lbl">CPU</div><div class="val num">${c.cpus}</div></div>
          <div class="k"><div class="lbl">内存</div><div class="val num">${c.mem_gb}G</div></div>
          <div class="k"><div class="lbl">可用磁盘</div><div class="val num">${c.disk_free_gb}G</div></div>
          <div class="k"><div class="lbl">GPU</div><div class="val num">${(c.gpus || []).length}</div></div>
        </div>
        <div class="t-caption muted" style="margin-bottom:6px">${esc(c.os)} ${esc(c.arch)} · load ${c.load1}</div>
        ${(c.gpus || []).length ? `<div class="t-caption muted" style="margin-bottom:8px">
          ${esc([...new Set(c.gpus)].join(' / '))}</div>` : ''}
        <div class="row" style="flex-wrap:wrap;gap:6px">${eg.map(([k, v]) => {
          const ok = (v >= 200 && v < 300) || v === 401;
          return `<span class="badge ${ok ? 'badge-ok' : 'badge-danger'}">${esc(k)} ${v}</span>`;
        }).join('')}</div>
        ${c.has_ai_egress ? '' : `<div class="t-caption" style="color:var(--warning);margin-top:8px">
          ⚠ 无法直连 AI API —— 该机器上的 agent 需要在「Agent → 环境变量」里配出口代理</div>`}`;
      await refreshState();
    } catch (e) { $('#hw').innerHTML = `<div class="empty">${esc(e.message)}</div>`; }
  };
  $('#btnScan').onclick = async () => {
    $('#ag').innerHTML = '<div class="empty">扫描中…</div>';
    try {
      const found = await api(`/api/nodes/${encodeURIComponent(name)}/agents`);
      S.scan = S.scan || {}; S.scan[name] = found;
      const inst = found.filter(a => a.path);
      $('#ag').innerHTML = inst.length ? `<table class="tb"><thead><tr>
          <th>Agent</th><th>版本</th><th>登录</th><th>路径</th></tr></thead><tbody>
        ${inst.map(a => `<tr>
          <td>${esc(a.label)}</td><td class="m">${esc(a.version || '—')}</td>
          <td>${a.authed === true ? '<span class="badge badge-ok">已登录</span>'
              : a.authed === false ? '<span class="badge badge-danger">未登录</span>'
              : `<span class="badge badge-warn" title="${esc(a.auth_hint || '')}">无法判断</span>`}</td>
          <td class="m">${esc(a.path)}</td></tr>`).join('')}</tbody></table>
        <div class="t-caption faint" style="margin-top:8px">
          </div>`
        : '<div class="empty">这台机器上没有发现任何 agent CLI</div>';
    } catch (e) { $('#ag').innerHTML = `<div class="empty">${esc(e.message)}</div>`; }
  };
  wireHeader();
}
