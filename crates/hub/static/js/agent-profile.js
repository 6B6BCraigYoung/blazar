const AG_SUB = { capabilities: [['instructions', '指令'], ['skills', '技能'], ['mcp', 'MCP']], settings: [['general', '通用'], ['env', '环境变量'], ['args', '自定义参数']] };

async function agLeaveOk() {
  if (!S.agDirty) return true;
  if (!await ask('放弃未保存的修改？\n当前 tab 有未保存的修改，离开会丢弃这些修改。', { ok: '放弃修改', cancel: '继续编辑', danger: true })) return false;
  S.agDirty = false; return true;
}
function setAgDirty(d) {
  S.agDirty = d;
  const el = $('#agUnsaved'); if (el) el.hidden = !d;
  const btn = $('#agSaveBtn'); if (btn) btn.disabled = !d;
}
async function putAgent(a, patch) {
  const body = { ...a, ...patch };
  for (const k of ['runtime_label', 'status', 'running', 'runs_30d', 'succeeded_30d', 'failed_30d', 'cancelled_30d', 'cost_30d', 'last_used_at', 'avg_secs_30d', 'activity_7d', 'failed_7d', 'runs']) delete body[k];
  const r = await api(`/api/agent-profiles/${encodeURIComponent(a.id)}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
  Object.assign(a, r);
  return r;
}
async function pageAgentProfile(id) {
  S.agDirty = false;
  let a;
  try { a = await api(`/api/agent-profiles/${encodeURIComponent(id)}`); }
  catch (e) {
    $('#page').innerHTML = `<div class="ag-blank" style="margin-top:80px"><b>未找到该智能体</b><span>该智能体可能已被归档或删除。</span>
      <a class="btn btn-outline btn-sm" href="#/agents" style="margin-top:10px">返回智能体列表</a></div>`; return;
  }
  if (!S.runtimes) { try { S.runtimes = (await api('/api/runtimes')).runtimes; } catch (_) { S.runtimes = []; } }
  const qs = new URLSearchParams(location.hash.split('?')[1] || '');
  const view = ['overview', 'work', 'capabilities', 'settings'].includes(qs.get('view')) ? qs.get('view') : 'overview';
  const subs = AG_SUB[view] || [];
  const sub = subs.some(x => x[0] === qs.get('tab')) ? qs.get('tab') : subs[0]?.[0];
  const p = agentPresence(a);
  $('#page').innerHTML = `
    <div class="phead">
      <button class="btn btn-ghost btn-icon menu-btn" style="display:none" aria-label="菜单"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M4 7h16M4 12h16M4 17h16"/></svg></button>
      <a href="#/agents" class="muted t-label" data-leave>智能体</a><span class="faint">/</span><h1 class="truncate" id="agCrumb">${esc(a.name)}</h1>
      <div class="grow"></div>
    </div>
    <div class="scroll">
      ${a.archived_at ? `<div class="notice">该智能体已归档，无法被选择。<button class="btn btn-outline btn-xs" id="agRestore" style="margin-left:8px">恢复</button></div>` : ''}
      ${!a.archived_at && p.availability === 'unbound' ? `<div class="notice warn">该智能体的配置和历史记录已保留，但需要绑定运行时后才能运行。<a class="btn btn-outline btn-xs" style="margin-left:8px" href="#/agent/${encodeURIComponent(a.id)}?view=settings&tab=general" data-leave>绑定运行时</a></div>` : ''}
      <div class="ag-head" id="agHead"></div>
      <div class="dtabs" role="tablist" aria-label="智能体页面">${[['overview', '概览'], ['work', '工作'], ['capabilities', '能力'], ['settings', '设置']].map(([k, t]) =>
        `<button class="rtab" role="tab" data-view="${k}" data-active="${k === view}">${t}</button>`).join('')}</div>
      ${subs.length > 1 ? `<div class="subtabs" role="tablist" aria-label="智能体分区">${subs.map(([k, t]) => `<button class="subtab" data-sub="${k}" data-on="${k === sub}">${t}</button>`).join('')}</div>` : ''}
      <div id="agPane"></div>
    </div>`;
  wireHeader();
  drawAgentHead(a);
  $('#agRestore')?.addEventListener('click', () => agentAction(a, 'restore'));
  const go = async h => { if (await agLeaveOk()) navigate(h); };
  $$('#page [data-view]').forEach(b => { b.onclick = () => go(`#/agent/${encodeURIComponent(a.id)}?view=${b.dataset.view}`); });
  $$('#page [data-sub]').forEach(b => { b.onclick = () => go(`#/agent/${encodeURIComponent(a.id)}?view=${view}&tab=${b.dataset.sub}`); });
  $$('#page a[data-leave]').forEach(l => { l.onclick = e => { if (S.agDirty) { e.preventDefault(); go(l.getAttribute('href')); } }; });
  const pane = $('#agPane');
  if (view === 'overview') drawAgentOverview(pane, a);
  else if (view === 'work') drawAgentWork(pane, a);
  else if (view === 'capabilities' && (sub === 'skills' || sub === 'mcp')) drawAgentCaps(pane, a, sub);
  else if (view === 'capabilities') drawAgentInstructions(pane, a);
  else if (sub === 'env') drawAgentEnv(pane, a);
  else if (sub === 'args') drawAgentArgs(pane, a);
  else drawAgentGeneral(pane, a);
}
function drawAgentHead(a) {
  const host = $('#agHead'); if (!host) return;
  const crumb = $('#agCrumb'); if (crumb) crumb.textContent = a.name;
  host.innerHTML = `${avatarHtml(a, 'lg')}
    <div style="flex:1;min-width:0"><h1>${esc(a.name)} ${presencePillHtml(a)}</h1>
      <div class="muted ag-desc-x">${esc(a.description || '暂无描述')}</div>
      <div class="meta"><span title="模型">◉ <span class="mono">${esc(a.model || '默认')}</span></span>
        <span title="运行时">${rtIcon(a.runtime)} ${esc(a.runtime_label || '无运行时')}</span>
        ${a.thinking_level ? `<span title="思考">思考 ${esc(THINK_LABEL[a.thinking_level] || a.thinking_level)}</span>` : ''}
        <span>${ago(a.updated_at)}更新</span></div></div>
    <div class="ag-head-act">${a.archived_at ? '' : '<button class="btn btn-outline btn-sm" id="agDm">私信</button>'}
      <button class="kebab" id="agMenu" aria-label="智能体操作">⋯</button></div>`;
  $('#agDm')?.addEventListener('click', () => agentAction(a, 'dm'));
  $('#agMenu').onclick = e => openMenu(e.currentTarget, agentMenuItems(a, true));
}
const RUN_STATUS = { running: ['进行中', 'running'], done: ['成功', 'completed'], failed: ['失败', 'errored'], interrupted: ['已取消', 'idle'] };
const runRowHtml = r => { const [t, pill] = RUN_STATUS[r.status] || [r.status, 'idle']; return `<div class="run-row" data-ws="${esc(r.workspace_id)}" data-thread="${esc(r.thread)}">
    <span class="pill ${pill}"><span class="dot ${pill}"></span>${t}</span>
    <span class="t" title="查看记录">${esc(r.title || '聊天会话')} <span class="faint">· ${esc(r.workspace || '工作区已删除')}</span></span>
    <span class="t-caption faint">${ago(r.created_at)}启动 · ${r.events} 条</span></div>`; };
const wireRunRows = host => host.querySelectorAll('.run-row[data-ws]').forEach(el => { el.onclick = () => openThreadIn(el.dataset.ws, el.dataset.thread); });
function drawAgentOverview(pane, a) {
  const done = a.succeeded_30d + a.failed_30d;
  const runs = a.runs || [], now = runs.filter(r => r.status === 'running'), recent = runs.filter(r => r.status !== 'running').slice(0, 5);
  pane.innerHTML = `<div class="ag-two">
    <div>
      <div class="card"><h3>当前 <span class="t-caption faint" style="font-weight:400">${now.length ? `${now.length} 次运行进行中` : '无进行中的工作'}</span></h3>
        ${now.length ? now.map(runRowHtml).join('') : '<div class="t-caption faint">这个智能体当前没有在运行。</div>'}</div>
      <div class="card"><h3>近 30 天 <span class="t-caption faint" style="font-weight:400">表现</span></h3>
        ${a.runs_30d ? `<div class="stat">
          <div><b>${a.runs_30d}</b><span>次运行</span></div>
          <div title="成功率按已完成和失败的运行计算，不包含已取消的运行。"><b>${done ? Math.round(a.succeeded_30d / done * 100) + '%' : '—'}</b><span>成功率</span></div>
          <div><b>${fmtSecs(a.avg_secs_30d)}</b><span>平均耗时</span></div>
          <div><b>${a.failed_30d}</b><span>失败</span></div>
        </div>${a.cancelled_30d ? `<div class="t-caption faint">${a.cancelled_30d} 次已取消${a.cost_30d ? ` · 费用 $${a.cost_30d.toFixed(2)}` : ''}</div>` : a.cost_30d ? `<div class="t-caption faint">费用 $${a.cost_30d.toFixed(2)}</div>` : ''}`
          : '<div class="t-caption faint">近 30 天没有任何完成记录。</div>'}</div>
      <div class="card"><h3>最近工作 <span class="t-caption faint" style="font-weight:400">${recent.length ? `最近 ${recent.length} 次运行` : '还没有完成的运行'}</span></h3>
        ${recent.length ? recent.map(runRowHtml).join('') + `<a class="t-caption" href="#/agent/${encodeURIComponent(a.id)}?view=work" style="display:block;margin-top:8px">查看更多 →</a>`
          : '<div class="t-caption faint">这个智能体还没有完成过任何运行。</div>'}</div>
    </div>
    <aside class="card"><h3>智能体</h3>
      <div class="kv"><span class="k">运行时</span><span class="v">${rtIcon(a.runtime)} ${esc(a.runtime_label)}</span></div>
      <div class="kv"><span class="k">模型</span><span class="v mono t-caption">${esc(a.model || '默认')}</span></div>
      <div class="kv"><span class="k">思考</span><span class="v">${esc(THINK_LABEL[a.thinking_level] || '跟随 CLI 配置')}</span></div>
      <div class="kv"><span class="k">权限模式</span><span class="v">${esc(PERM_TEXT[a.permission_mode] || '按运行时设置')}</span></div>
      <div class="kv"><span class="k">并发</span><span class="v num">${a.max_concurrent}</span></div>
      <div class="kv"><span class="k">创建时间</span><span class="v">${ago(a.created_at)}</span></div>
      <div class="kv"><span class="k">更新时间</span><span class="v">${ago(a.updated_at)}</span></div>
    </aside></div>`;
  wireRunRows(pane);
}
function drawAgentWork(pane, a) {
  const runs = a.runs || [];
  let f = 'all';
  const draw = () => {
    const list = runs.filter(r => f === 'all' || r.status === f);
    pane.innerHTML = `<div class="card"><h3>运行 <span class="badge badge-muted num">${runs.length}</span><div class="grow"></div>
      <div class="seg">${[['all', '全部'], ['running', '进行中'], ['done', '成功'], ['failed', '失败'], ['interrupted', '已取消']].map(([k, t]) => `<button data-f="${k}" data-on="${k === f}">${t}</button>`).join('')}</div></h3>
      ${list.length ? list.map(runRowHtml).join('') : '<div class="t-caption faint" style="padding:10px 0">没有匹配的运行。</div>'}</div>`;
    pane.querySelectorAll('[data-f]').forEach(b => { b.onclick = () => { f = b.dataset.f; draw(); }; });
    wireRunRows(pane);
  };
  draw();
}

function drawAgentInstructions(pane, a) {
  let starters = (a.starters || []).map(x => ({ ...x }));
  const editable = !a.archived_at;
  const saveBar = `<div class="ag-savebar"><span class="t-caption" id="agUnsaved" hidden>● 未保存的修改</span><span class="grow"></span><button class="btn btn-brand btn-sm" id="agSaveBtn" disabled>保存</button></div>`;
  const drawStarters = () => {
    $('#agStarters').innerHTML = starters.map((x, i) => `<div class="st-item">
        <input class="input input-sm" data-sl="${i}" aria-label="建议 ${i + 1} 的标题" placeholder="审查拉取请求" value="${esc(x.label)}" maxlength="80" ${editable ? '' : 'disabled'}>
        <textarea class="input" rows="2" data-sp="${i}" aria-label="建议 ${i + 1} 的提示词" placeholder="审查当前拉取请求，总结风险并建议下一步。" maxlength="4000" ${editable ? '' : 'disabled'}>${esc(x.prompt)}</textarea>
        ${editable ? `<button class="btn btn-ghost btn-xs" data-rm="${i}" aria-label="移除建议 ${i + 1}">移除</button>` : ''}</div>`).join('')
      + (editable && starters.length < 3 ? '<button class="btn btn-outline btn-xs" id="stAdd">＋ 添加建议</button>' : '');
    $('#stPreview').innerHTML = starters.filter(x => x.label).length
      ? starters.filter(x => x.label).map(x => `<span class="starter" style="cursor:default"><b>${esc(x.label)}</b><span>${esc(x.prompt)}</span></span>`).join('')
      : '<span class="t-caption faint">还没有建议。添加后，新对话的空白页会显示成可点击的卡片。</span>';
    $$('#agStarters [data-sl]').forEach(i => { i.oninput = () => { starters[+i.dataset.sl].label = i.value; setAgDirty(true); drawPreviewOnly(); }; });
    $$('#agStarters [data-sp]').forEach(i => { i.oninput = () => { starters[+i.dataset.sp].prompt = i.value; setAgDirty(true); drawPreviewOnly(); }; });
    $$('#agStarters [data-rm]').forEach(b => { b.onclick = () => { starters.splice(+b.dataset.rm, 1); setAgDirty(true); drawStarters(); }; });
    $('#stAdd')?.addEventListener('click', () => { starters.push({ label: '', prompt: '' }); drawStarters(); $(`[data-sl="${starters.length - 1}"]`)?.focus(); });
  };
  const drawPreviewOnly = () => {
    $('#stPreview').innerHTML = starters.filter(x => x.label).map(x => `<span class="starter" style="cursor:default"><b>${esc(x.label)}</b><span>${esc(x.prompt)}</span></span>`).join('')
      || '<span class="t-caption faint">还没有建议。</span>';
  };
  pane.innerHTML = `<div class="card">
      <div class="desc">设置每次运行都会使用的 System Prompt，支持 Markdown。${TIP('Claude 走 --append-system-prompt，Codex 走 developer_instructions，Grok 走 --rules，其它运行时拼在提示词前面。')}</div>
      <div class="field"><label>System Prompt</label>
        <textarea id="agIns" class="input" rows="12" ${editable ? '' : 'disabled'} placeholder="定义这个智能体的角色、专长和工作风格。\n\n# 示例\n你是一名专注 React 和 TypeScript 的前端工程师。\n\n## 工作风格\n- 写小而聚焦的 PR——每个逻辑变更一个 commit\n- 优先组合而不是继承">${esc(a.instructions || '')}</textarea></div>
      <div class="field"><label>对话开场建议</label><div class="t-caption faint" style="margin-bottom:6px">最多三条。点击后填入输入框，不会发送。</div>
        <div id="agStarters"></div></div>
      <div class="field"><label>新对话预览</label><div class="starters" id="stPreview"></div></div>
      ${editable ? saveBar : ''}</div>`;
  drawStarters();
  if (!editable) return;
  $('#agIns').oninput = () => setAgDirty(true);
  $('#agSaveBtn').onclick = async () => {
    const clean = starters.map(x => ({ label: x.label.trim(), prompt: x.prompt.trim() }));
    if (clean.some(x => !x.label || !x.prompt)) { toast('请填写完整或移除每条建议后再保存。'); return; }
    try { await putAgent(a, { instructions: $('#agIns').value, starters: clean }); setAgDirty(false); toast('已更新智能体'); await loadProfiles(); }
    catch (e) { toast(e.message || '更新智能体失败'); }
  };
}

function drawAgentGeneral(pane, a) {
  const editable = !a.archived_at, dis = editable ? '' : 'disabled';
  const rts = (S.runtimes || []).filter(r => r.installed);
  const rtOpts = rts.map(r => `<option value="${esc(r.id)}" ${r.id === a.runtime ? 'selected' : ''}>${esc(r.label)}${r.authed === false ? '（离线：未登录）' : ''}</option>`).join('')
    + (rts.some(r => r.id === a.runtime) ? '' : `<option value="${esc(a.runtime)}" selected>${esc(a.runtime_label)}（本机没装）</option>`);
  pane.innerHTML = `
    <div class="set-sec"><div class="set-h"><h3>资料</h3><span class="t-caption faint" id="agSaveState"></span></div>
      <div class="card set-card">
        <div class="set-row"><label>头像</label><div><input id="agAv" class="input" style="width:90px;text-align:center;font-size:20px" value="${esc(a.avatar || '')}" placeholder="🤖" maxlength="4" ${dis} aria-label="更换头像"><div class="t-caption faint">一个 emoji；留空用运行时的标识</div></div></div>
        <div class="set-row"><label>名称</label><div><input id="agName" class="input" value="${esc(a.name)}" placeholder="智能体名称" maxlength="40" ${dis}><div class="t-caption" id="agNameErr" style="color:var(--danger,#e5484d)" hidden>名称必填</div></div></div>
        <div class="set-row"><label>描述</label><div><textarea id="agDesc" class="input" rows="2" placeholder="这个智能体做什么？" ${dis}>${esc(a.description || '')}</textarea><div class="t-caption faint num" id="agDescN"></div></div></div>
      </div></div>
    <div class="set-sec"><div class="set-h"><h3>执行配置</h3></div>
      <div class="card set-card">
        <div class="set-row"><label>运行时</label><select id="agRt" class="input" ${dis}>${rtOpts}</select></div>
        <div class="set-row"><label>模型</label><div><input id="agModel" class="input mono" list="agModelList" value="${esc(a.model || '')}" placeholder="默认（提供方）" ${dis}><datalist id="agModelList"></datalist><div class="t-caption faint">搜索或输入模型 ID；清空 = 使用提供方默认</div></div></div>
        <div class="set-row"><label>思考</label><select id="agThink" class="input" ${dis}><option value="">跟随 CLI 配置</option>${Object.entries(THINK_LABEL).map(([v, t]) => `<option value="${v}" ${a.thinking_level === v ? 'selected' : ''}>${t}</option>`).join('')}</select></div>
        <div class="set-row"><label>权限模式</label><select id="agPerm" class="input" ${dis}>${permOpts(a.runtime, a.permission_mode)}</select></div>
        <div class="set-row"><label>并发</label><div><input id="agConc" class="input" type="number" min="1" max="50" style="width:90px" value="${a.max_concurrent || 1}" ${dis}><div class="t-caption faint">最大并行运行数（1–50）</div></div></div>
      </div></div>
    <div class="set-sec"><div class="set-h"><h3>详情</h3><span class="t-caption faint">只读的生命周期信息。</span></div>
      <div class="card set-card">
        <div class="set-row"><label>创建时间</label><span>${esc(new Date(a.created_at).toLocaleString('zh-CN'))}</span></div>
        <div class="set-row"><label>更新时间</label><span id="agUpd">${esc(new Date(a.updated_at).toLocaleString('zh-CN'))}</span></div>
      </div></div>`;
  if (!editable) return;
  const state = $('#agSaveState');
  let timer = 0, pending = {};
  const flush = async () => {
    clearTimeout(timer);
    if (!Object.keys(pending).length) return;
    const patch = pending; pending = {};
    state.textContent = '保存中…';
    try { await putAgent(a, patch); state.textContent = '已保存'; drawAgentHead(a); $('#agUpd').textContent = new Date(a.updated_at).toLocaleString('zh-CN'); loadProfiles(); }
    catch (e) { state.textContent = '保存失败'; toast(e.message || '更新智能体失败'); }
  };
  const later = patch => { Object.assign(pending, patch); clearTimeout(timer); timer = setTimeout(flush, 700); };
  const now = patch => { Object.assign(pending, patch); flush(); };
  const count = () => { const n = $('#agDesc').value.length; $('#agDescN').textContent = `${n} / 255${n > 255 ? ` · 超出 ${n - 255} 字` : ''}`; };
  count();
  $('#agName').oninput = () => { const v = $('#agName').value.trim(); $('#agNameErr').hidden = !!v; if (v) later({ name: v }); else { delete pending.name; } };
  $('#agDesc').oninput = () => { count(); if ($('#agDesc').value.length <= 255) later({ description: $('#agDesc').value.trim() }); };
  $('#agAv').oninput = () => later({ avatar: $('#agAv').value.trim() });
  for (const id of ['#agName', '#agDesc', '#agAv']) $(id).onblur = flush;
  $('#agRt').onchange = () => {

    $('#agPerm').innerHTML = permOpts($('#agRt').value, ''); $('#agModel').value = ''; $('#agThink').value = '';
    now({ runtime: $('#agRt').value, model: null, thinking_level: null, permission_mode: null });
    fillModels();
  };
  $('#agModel').onchange = () => now({ model: $('#agModel').value.trim() || null });
  $('#agThink').onchange = () => now({ thinking_level: $('#agThink').value || null });
  $('#agPerm').onchange = () => now({ permission_mode: $('#agPerm').value || null });
  $('#agConc').onchange = () => { const n = Math.max(1, Math.min(50, +$('#agConc').value || 1)); $('#agConc').value = n; now({ max_concurrent: n }); };
  const fillModels = async () => {
    const list = await modelsFor($('#agRt').value);
    $('#agModelList').innerHTML = list.filter(m => m.id).map(m => `<option value="${esc(m.id)}">${esc(m.label)}</option>`).join('');
  };
  fillModels();
}

function drawAgentEnv(pane, a) {
  let rows = Object.entries(a.env || {}).map(([k, v]) => ({ k, v, show: false }));
  let bulk = false;
  const editable = !a.archived_at;
  const parseBulk = text => {
    const out = [], seen = new Set();
    const lines = text.split('\n');
    for (let i = 0; i < lines.length; i++) {
      const t = lines[i].trim(); if (!t || t.startsWith('#')) continue;
      const at = t.indexOf('=');
      if (at < 1) throw new Error(`第 ${i + 1} 行不是 KEY=value 格式`);
      const k = t.slice(0, at).trim();
      if (seen.has(k)) throw new Error(`第 ${i + 1} 行的 key "${k}" 重复`);
      seen.add(k); out.push({ k, v: t.slice(at + 1), show: false });
    }
    return out;
  };
  const draw = () => {
    pane.innerHTML = `<div class="card">
      <div class="desc">在智能体进程启动时注入（例如 <span class="mono">HTTPS_PROXY</span>、<span class="mono">ANTHROPIC_BASE_URL</span>）。</div>
      <div class="actions" style="justify-content:flex-end;margin-bottom:8px">${editable ? `<button class="btn btn-ghost btn-xs" id="envMode">${bulk ? '逐条编辑' : '批量编辑'}</button>` : ''}</div>
      ${bulk ? `<textarea id="envBulk" class="input mono" rows="8" placeholder="API_KEY=值\nBASE_URL=https://example.com">${esc(rows.map(r => `${r.k}=${r.v}`).join('\n'))}</textarea>
          <div class="t-caption faint" style="margin-top:4px">批量编辑时，值以明文显示。</div>`
        : (rows.length ? rows.map((r, i) => `<div class="env-row">
            <input class="input input-sm mono" data-ek="${i}" placeholder="KEY" value="${esc(r.k)}" ${editable ? '' : 'disabled'}>
            <input class="input input-sm mono" data-ev="${i}" placeholder="值" type="${r.show ? 'text' : 'password'}" value="${esc(r.v)}" ${editable ? '' : 'disabled'}>
            <button class="btn btn-ghost btn-xs" data-eye="${i}" aria-label="${r.show ? '隐藏值' : '显示值'}">${r.show ? '隐藏' : '显示'}</button>
            ${editable ? `<button class="btn btn-ghost btn-xs" data-erm="${i}" aria-label="移除变量">移除</button>` : ''}</div>`).join('')
          : `<div class="t-caption faint" style="padding:6px 0">${editable ? '暂无环境变量。' : '尚未配置环境变量。'}</div>`)
        + (editable ? '<button class="btn btn-outline btn-xs" id="envAdd" style="margin-top:6px">＋ 添加</button>' : '')}
      ${editable ? `<div class="ag-savebar"><span class="t-caption" id="agUnsaved" ${S.agDirty ? '' : 'hidden'}>● 未保存的修改</span><span class="grow"></span><button class="btn btn-brand btn-sm" id="agSaveBtn" ${S.agDirty ? '' : 'disabled'}>保存</button></div>` : ''}</div>`;
    $('#envMode')?.addEventListener('click', () => {
      if (bulk) { try { rows = parseBulk($('#envBulk').value); } catch (e) { toast(e.message); return; } }
      bulk = !bulk; draw();
    });
    $('#envBulk')?.addEventListener('input', () => setAgDirty(true));
    $$('[data-ek]').forEach(i => { i.oninput = () => { rows[+i.dataset.ek].k = i.value; setAgDirty(true); }; });
    $$('[data-ev]').forEach(i => { i.oninput = () => { rows[+i.dataset.ev].v = i.value; setAgDirty(true); }; });
    $$('[data-eye]').forEach(b => { b.onclick = () => { rows[+b.dataset.eye].show = !rows[+b.dataset.eye].show; draw(); }; });
    $$('[data-erm]').forEach(b => { b.onclick = () => { rows.splice(+b.dataset.erm, 1); setAgDirty(true); draw(); }; });
    $('#envAdd')?.addEventListener('click', () => { rows.push({ k: '', v: '', show: true }); draw(); $(`[data-ek="${rows.length - 1}"]`)?.focus(); });
    $('#agSaveBtn')?.addEventListener('click', async () => {
      try {
        const list = bulk ? parseBulk($('#envBulk').value) : rows.filter(r => r.k.trim());
        const env = {};
        for (const r of list) { const k = r.k.trim(); if (k in env) { toast('环境变量 key 重复'); return; } env[k] = r.v; }
        await putAgent(a, { env }); rows = Object.entries(a.env || {}).map(([k, v]) => ({ k, v, show: false }));
        setAgDirty(false); toast('已保存环境变量'); draw();
      } catch (e) { toast(e.message || '保存环境变量失败'); }
    });
  };
  draw();
}

function drawAgentArgs(pane, a) {
  let args = [...(a.custom_args || [])];
  let editing = null;
  const editable = !a.archived_at;
  const fmt = v => (/\s/.test(v) ? JSON.stringify(v) : v);
  const header = { claude: 'claude -p --output-format stream-json', codex: 'codex exec --json', grok: 'grok --output-format plain', dsh: 'dsh --profile headless', deepcode: 'deepcode -x' }[a.runtime] || a.runtime;
  const editor = (val, label) => `<form class="arg-edit" id="argForm"><input class="input input-sm mono" id="argIn" autocomplete="off" spellcheck="false" placeholder="--profile" value="${esc(val)}">
      <div class="actions" style="justify-content:flex-end;margin-top:6px"><button type="button" class="btn btn-ghost btn-xs" id="argCancel">取消</button><button type="submit" class="btn btn-brand btn-xs">${label}</button></div></form>`;
  const draw = () => {
    const dirty = JSON.stringify(args) !== JSON.stringify(a.custom_args || []);
    S.agDirty = dirty;
    pane.innerHTML = `<div class="card">
      <div class="desc">添加智能体启动时传入的 CLI 参数。</div>
      <div class="set-h"><div><h3 style="margin:0">参数</h3><div class="t-caption faint">每一项会作为一个完整 token 传入，并保留列表顺序。</div></div>
        ${editable ? `<button class="btn btn-outline btn-xs" id="argAdd" ${editing !== null ? 'disabled' : ''}>＋ 添加参数</button>` : ''}</div>
      <div class="arg-list">${!args.length && editing !== 'add' ? '<div class="ag-blank" style="padding:18px"><b>还没有参数</b></div>' : ''}
        ${args.map((v, i) => editing === i ? editor(v, '更新') : `<div class="arg-item"><span class="n">${i + 1}</span><code>${esc(v)}</code>
          ${editable ? `<button class="btn btn-ghost btn-xs" data-aed="${i}" ${editing !== null ? 'disabled' : ''} aria-label="编辑参数 ${i + 1}">编辑</button><button class="btn btn-ghost btn-xs" data-arm="${i}" aria-label="移除参数 ${i + 1}">移除</button>` : ''}</div>`).join('')}
        ${editing === 'add' ? editor('', '添加') : ''}</div>
      <div class="field" style="margin-top:12px"><label>命令预览</label><pre class="acmd" style="margin:0">${esc([header, ...args.map(fmt)].join(' '))}</pre></div>
      ${editable ? `<div class="ag-savebar"><span class="t-caption" id="agUnsaved" ${dirty ? '' : 'hidden'}>● 未保存的修改</span><span class="grow"></span><button class="btn btn-brand btn-sm" id="agSaveBtn" ${dirty ? '' : 'disabled'}>保存</button></div>` : ''}</div>`;
    $('#argAdd')?.addEventListener('click', () => { editing = 'add'; draw(); });
    $$('[data-aed]').forEach(b => { b.onclick = () => { editing = +b.dataset.aed; draw(); }; });
    $$('[data-arm]').forEach(b => { b.onclick = () => { args.splice(+b.dataset.arm, 1); editing = null; draw(); }; });
    const form = $('#argForm');
    if (form) {
      const inp = $('#argIn'); inp.focus();
      form.onsubmit = e => { e.preventDefault(); const v = inp.value.trim(); if (!v) return; if (editing === 'add') args.push(v); else args[editing] = v; editing = null; draw(); };
      form.onkeydown = e => { if (e.key === 'Escape') { e.stopPropagation(); editing = null; draw(); } };
      $('#argCancel').onclick = () => { editing = null; draw(); };
    }
    $('#agSaveBtn')?.addEventListener('click', async () => {
      try { await putAgent(a, { custom_args: args }); args = [...(a.custom_args || [])]; toast('已保存自定义参数'); draw(); }
      catch (e) { toast(e.message || '保存自定义参数失败'); }
    });
  };
  draw();
}
