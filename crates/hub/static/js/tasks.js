const TASK_STATUS = [['backlog', '待规划'], ['todo', '待办'], ['in_progress', '进行中'], ['in_review', '待审阅'], ['done', '完成'], ['cancelled', '已取消']];
const TASK_STATUS_TEXT = Object.fromEntries(TASK_STATUS);
const TASK_PRIO = { urgent: ['紧急', '#e5484d', '‼'], high: ['高', '#f97316', '▲'], medium: ['中', '#eab308', '■'], low: ['低', '#64748b', '▽'], none: ['无', 'var(--faint-fg)', '·'] };
const TASK_VIEW_KEY = 'blazar.tasks.view';
function taskView() {
  let v = {}; try { v = JSON.parse(localStorage.getItem(TASK_VIEW_KEY) || '{}'); } catch (_) {}
  return { mode: 'board', showHidden: false, prio: '', ws: '', agent: '', ...v };
}
function setTaskView(p) { const v = { ...taskView(), ...p }; try { localStorage.setItem(TASK_VIEW_KEY, JSON.stringify(v)); } catch (_) {} return v; }
async function loadTasks() {
  try { S.tasks = await api('/api/tasks'); } catch (_) { S.tasks = S.tasks || []; }
  const c = $('#cnt-tasks'); if (c) c.textContent = S.tasks.filter(t => ['todo', 'in_progress', 'in_review'].includes(t.status)).length || '';
  return S.tasks;
}
let tasksTimer = 0;
function refreshTasksSoon() {
  if (tasksTimer) return;
  tasksTimer = setTimeout(async () => {
    tasksTimer = 0; await loadTasks();
    if (S.route.split('?')[0] === '#/tasks') { drawTasks(); if (S.taskOpen && !$('#taskPanel :focus')) openTask(S.taskOpen, true); }
  }, 600);
}
const prioHtml = p => { const [t, c, g] = TASK_PRIO[p] || TASK_PRIO.none; return `<span class="tk-prio" style="color:${c}" title="优先级：${t}">${g}</span>`; };
const taskAssignee = t => t.agent_name ? `${t.agent_avatar ? esc(t.agent_avatar) + ' ' : ''}${esc(t.agent_name)}` : t.runtime ? esc(AGENT_LABEL[t.runtime] || t.runtime) : '';

async function taskToLark(id) {
  if (!await ask('把这条任务（描述和每一轮回复）存成一篇飞书文档？会用你本机 lark-cli 的登录身份创建。', { ok: '创建文档' })) return;
  try {
    const r = await post(`/api/tasks/${id}/lark-doc`, {});
    if (r.ok && r.url) toastAction('飞书文档已创建', '打开', () => window.open(r.url, '_blank', 'noopener'));
    else toast(r.ok ? '已创建（没拿到链接，去飞书云文档里找）' : '没创建成：' + shortStr(r.detail || '', 160));
  } catch (e) { toast(e.message); }
}

async function taskToObsidian(id) {
  const go = async overwrite => {
    const r = await post(`/api/tasks/${id}/obsidian-note`, { overwrite });
    toastAction(`已存到 ${r.path}`, '在 Obsidian 里打开', () => { location.href = r.open; });
  };
  try { await go(false); }
  catch (e) {
    if (/同名/.test(e.message)) { if (await ask('库里已经有这篇笔记了，用现在的内容覆盖？', { ok: '覆盖', danger: true })) { try { await go(true); } catch (e2) { toast(e2.message); } } }
    else toast(e.message);
  }
}

async function pageTasks() {
  $('#page').innerHTML = `
    <div class="phead">
      <button class="btn btn-ghost btn-icon menu-btn" style="display:none" aria-label="菜单"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M4 7h16M4 12h16M4 17h16"/></svg></button>
      <h1>任务</h1><span class="badge badge-muted num" id="tkCount"></span>
      <div class="seg" id="tkMode" style="margin-left:10px"><button data-m="board">看板</button><button data-m="list">列表</button></div>
      <div class="grow"></div>
      <input id="tkq" class="input input-sm" style="width:200px" placeholder="搜索任务…">
      <button class="btn btn-outline btn-xs" id="tkFilter">筛选</button>
      <button class="btn btn-brand btn-sm" id="tkNew">新建任务</button>
    </div>
    <div class="tk-wrap"><div id="tkBody" class="tk-body"></div><aside id="taskPanel" class="tk-panel" hidden></aside></div>`;
  wireHeader();
  $('#tkNew').onclick = () => dlgTask();
  $('#tkq').oninput = drawTasks;
  $$('#tkMode [data-m]').forEach(b => { b.onclick = () => { setTaskView({ mode: b.dataset.m }); drawTasks(); }; });
  $('#tkFilter').onclick = e => {
    const v = taskView();
    const wss = [...new Map((S.tasks || []).filter(t => t.workspace_id).map(t => [t.workspace_id, t.workspace_name])).entries()];
    openMenu(e.currentTarget, [
      ...[['', '全部优先级'], ...Object.entries(TASK_PRIO).map(([k, x]) => [k, `优先级：${x[0]}`])].map(([k, t]) => ({ label: `${v.prio === k ? '✓ ' : '　'}${t}`, run: () => { setTaskView({ prio: k }); drawTasks(); } })),
      '-',
      { label: `${!v.ws ? '✓ ' : '　'}全部工作区`, run: () => { setTaskView({ ws: '' }); drawTasks(); } },
      ...wss.map(([id, n]) => ({ label: `${v.ws === id ? '✓ ' : '　'}${n || '（已删除）'}`, run: () => { setTaskView({ ws: id }); drawTasks(); } })),
      '-',
      { label: `${v.showHidden ? '✓ ' : '　'}显示「待规划 / 已取消」`, run: () => { setTaskView({ showHidden: !v.showHidden }); drawTasks(); } },
      (v.prio || v.ws) && { label: '清除筛选', run: () => { setTaskView({ prio: '', ws: '' }); drawTasks(); } },
    ]);
  };
  if (!S.profiles) await loadProfiles();
  await loadTasks();
  drawTasks();
  const open = new URLSearchParams(location.hash.split('?')[1] || '').get('task');
  if (open) openTask(open);
}
function filteredTasks() {
  const v = taskView(), q = ($('#tkq')?.value || '').trim().toLowerCase();
  return (S.tasks || []).filter(t => !t.parent_id
    && (!q || `${t.key} ${t.title} ${t.description}`.toLowerCase().includes(q))
    && (!v.prio || t.priority === v.prio) && (!v.ws || t.workspace_id === v.ws));
}
const taskCardHtml = t => `<div class="tk-card" draggable="true" data-id="${esc(t.id)}" data-open="${S.taskOpen === t.id}">
    <div class="tk-c1"><span class="tk-key">${esc(t.key)}</span>${prioHtml(t.priority)}<span class="grow"></span>${t.running ? '<span class="tk-live" title="正在运行"></span>' : ''}</div>
    <div class="tk-title">${esc(t.title)}</div>
    ${(t.labels || []).length ? `<div class="tk-labels">${t.labels.map(l => `<span class="tk-label">${esc(l)}</span>`).join('')}</div>` : ''}
    <div class="tk-c3">${t.workspace_name ? `<span class="truncate" title="工作区">▣ ${esc(t.workspace_name)}</span>` : '<span class="faint">未选工作区</span>'}
      <span class="grow"></span>${t.children ? `<span title="子任务">${t.children_done}/${t.children}</span>` : ''}${t.comments ? `<span title="评论">✎ ${t.comments}</span>` : ''}
      ${taskAssignee(t) ? `<span class="tk-who truncate">${taskAssignee(t)}</span>` : ''}</div></div>`;
function drawTasks() {
  const host = $('#tkBody'); if (!host) return;
  const v = taskView(), list = filteredTasks();
  $$('#tkMode [data-m]').forEach(b => { b.dataset.on = String(b.dataset.m === v.mode); });
  $('#tkFilter').dataset.on = String(!!(v.prio || v.ws));
  $('#tkCount').textContent = (S.tasks || []).filter(t => !t.parent_id).length || '';
  const cols = TASK_STATUS.filter(([k]) => v.showHidden || !['backlog', 'cancelled'].includes(k));
  if (!(S.tasks || []).length) {
    host.innerHTML = `<div class="ag-blank" style="margin:60px auto"><b>还没有任务</b><span>把要做的事写成任务，指给某个智能体在某个工作区里做；做完回到这里看结果。</span>
      <button class="btn btn-brand btn-sm" style="margin-top:10px" id="tkFirst">新建任务</button></div>`;
    $('#tkFirst').onclick = () => dlgTask(); return;
  }
  if (v.mode === 'list') {
    host.innerHTML = `<div class="tk-list">${cols.map(([k, name]) => { const rows = list.filter(t => t.status === k); return rows.length ? `
      <div class="tk-lh">${name} <span class="faint num">${rows.length}</span></div>
      ${rows.map(t => `<div class="tk-lrow" data-id="${esc(t.id)}" data-open="${S.taskOpen === t.id}"><span class="tk-key">${esc(t.key)}</span>${prioHtml(t.priority)}
        <span class="t truncate">${esc(t.title)}</span>${t.running ? '<span class="tk-live"></span>' : ''}
        <span class="muted t-caption truncate" style="max-width:160px">${esc(t.workspace_name || '')}</span><span class="muted t-caption truncate" style="max-width:140px">${taskAssignee(t)}</span>
        <span class="faint t-caption" style="width:70px;text-align:right">${ago(t.updated_at)}</span></div>`).join('')}` : ''; }).join('') || '<div class="ag-blank"><b>无匹配项</b></div>'}</div>`;
    $$('#tkBody .tk-lrow').forEach(r => { r.onclick = () => openTask(r.dataset.id); });
    return;
  }
  host.innerHTML = `<div class="tk-board">${cols.map(([k, name]) => { const rows = list.filter(t => t.status === k); return `
    <section class="tk-col" data-status="${k}"><header><b>${name}</b><span class="faint num">${rows.length}</span><span class="grow"></span>
      <button class="kebab" data-add="${k}" title="在这一列新建">＋</button></header>
      <div class="tk-cards">${rows.map(taskCardHtml).join('')}</div></section>`; }).join('')}</div>`;
  $$('#tkBody [data-add]').forEach(b => { b.onclick = () => dlgTask({ status: b.dataset.add }); });
  $$('#tkBody .tk-card').forEach(c => {
    c.onclick = () => openTask(c.dataset.id);
    c.ondragstart = e => { e.dataTransfer.setData('text/plain', c.dataset.id); e.dataTransfer.effectAllowed = 'move'; c.dataset.drag = 'true'; };
    c.ondragend = () => { delete c.dataset.drag; $$('.tk-col').forEach(x => delete x.dataset.over); };
  });
  $$('#tkBody .tk-col').forEach(col => {
    col.ondragover = e => { e.preventDefault(); col.dataset.over = 'true'; };
    col.ondragleave = e => { if (!col.contains(e.relatedTarget)) delete col.dataset.over; };
    col.ondrop = async e => {
      e.preventDefault(); delete col.dataset.over;
      const id = e.dataTransfer.getData('text/plain'); const t = (S.tasks || []).find(x => x.id === id); if (!t) return;

      const cards = [...col.querySelectorAll('.tk-card')].filter(x => x.dataset.id !== id);
      const before = cards.find(x => { const r = x.getBoundingClientRect(); return e.clientY < r.top + r.height / 2; });
      const pos = x => (S.tasks.find(y => y.id === x.dataset.id)?.position ?? 0);
      const i = before ? cards.indexOf(before) : cards.length;
      const lo = i > 0 ? pos(cards[i - 1]) : (cards.length ? pos(cards[0]) - 2 : 0), hi = before ? pos(before) : lo + 2;
      const position = (lo + hi) / 2;
      t.status = col.dataset.status; t.position = position; drawTasks();
      try { await api(`/api/tasks/${id}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ status: t.status, position }) }); }
      catch (err) { toast('移动失败: ' + err.message); await loadTasks(); drawTasks(); }
    };
  });
}

const assigneeOpts = cur => `<option value="">未指派</option>`
  + ((S.profiles || []).length ? `<optgroup label="智能体">${S.profiles.map(p => `<option value="p:${esc(p.id)}" ${cur === 'p:' + p.id ? 'selected' : ''}>${p.avatar ? esc(p.avatar) + ' ' : ''}${esc(p.name)}</option>`).join('')}</optgroup>` : '')
  + `<optgroup label="运行时">${(S.agents || []).filter(a => !S.runtimes || S.runtimes.some(r => r.id === a.id && r.installed)).map(a => `<option value="r:${esc(a.id)}" ${cur === 'r:' + a.id ? 'selected' : ''}>${esc(a.label)}</option>`).join('')}</optgroup>`;
const wsOpts = cur => `<option value="">未选择</option>` + (S.workspaces || []).map(w => `<option value="${esc(w.id)}" ${cur === w.id ? 'selected' : ''}>${esc(w.name)} · ${esc(w.node)}</option>`).join('');
const assigneeOf = t => t.agent_profile ? 'p:' + t.agent_profile : t.runtime ? 'r:' + t.runtime : '';
const assigneePatch = v => v.startsWith('p:') ? { agent_profile: v.slice(2), runtime: null } : v.startsWith('r:') ? { agent_profile: null, runtime: v.slice(2) } : { agent_profile: null, runtime: null };

function dlgTask(init = {}) {
  openDlg(`<h3>${init.parent_id ? '新建子任务' : '新建任务'}</h3>
    <div class="field"><label>标题</label><input id="ntTitle" class="input" placeholder="要做什么"></div>
    <div class="field"><label>描述（开始时会和标题一起发给智能体）</label><textarea id="ntDesc" class="input" rows="5" placeholder="越具体越好：涉及哪些文件、验收标准、不要动什么…"></textarea></div>
    <div class="grid2">
      <div class="field"><label>工作区</label><select id="ntWs" class="input">${wsOpts(init.workspace_id || '')}</select></div>
      <div class="field"><label>指派给</label><select id="ntWho" class="input">${assigneeOpts(init.assignee || '')}</select></div>
      <div class="field"><label>优先级</label><select id="ntPrio" class="input">${Object.entries(TASK_PRIO).map(([k, x]) => `<option value="${k}" ${k === 'none' ? 'selected' : ''}>${x[0]}</option>`).join('')}</select></div>
      <div class="field"><label>标签（逗号分隔）</label><input id="ntLabels" class="input" placeholder="bug, 前端"></div>
    </div>
    <div class="dfoot"><button class="btn btn-outline" onclick="closeDlg()">取消</button>
      <button class="btn btn-outline" id="ntCreate">创建</button><button class="btn btn-brand" id="ntStart">创建并开始</button></div>`);
  $('#ntTitle').focus();
  const submit = async startNow => {
    const title = $('#ntTitle').value.trim(); if (!title) { toast('标题必填'); return; }
    const who = $('#ntWho').value;
    if (startNow && (!$('#ntWs').value || !who)) { toast('要开始的话，先选工作区和指派对象'); return; }
    const body = { title, description: $('#ntDesc').value, status: init.status || 'todo', priority: $('#ntPrio').value,
      labels: $('#ntLabels').value.split(/[,，]/).map(x => x.trim()).filter(Boolean), workspace_id: $('#ntWs').value || null,
      parent_id: init.parent_id || null, ...assigneePatch(who) };
    try {
      const t = await api('/api/tasks', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
      closeDlg(); toast(`已创建 ${t.key}`);
      if (startNow) await startTask(t.id);
      await loadTasks(); drawTasks();
      if (S.route.split('?')[0] === '#/tasks') openTask(init.parent_id || t.id);
    } catch (e) { toast(e.message); }
  };
  $('#ntCreate').onclick = () => submit(false);
  $('#ntStart').onclick = () => submit(true);
}
async function startTask(id, text) {
  try {
    const r = await post(`/api/tasks/${id}/start`, text ? { text } : {});
    if (r.activity && r.activity.started === false) toast(r.activity.reason || '智能体没能启动'); else toast('已开始');
    return r;
  } catch (e) { toast(e.message); return null; }
}
const RUN_TRIGGER = { initial: '首次', follow_up: '评论 / 重试' };
async function openTask(id, quiet) {
  if (!$('#taskPanel')) return;
  let t;
  try { t = await api(`/api/tasks/${id}`); } catch (e) { if (!quiet) toast(e.message); return; }

  const panel = $('#taskPanel'); if (!panel) return;
  S.taskOpen = id;
  $$('#tkBody [data-id]').forEach(x => { x.dataset.open = String(x.dataset.id === id); });
  const runs = t.runs || [], canRun = !!t.workspace_id && !!(t.agent_profile || t.runtime);
  const dur = r => r.first_at && r.last_at ? fmtSecs((Date.parse(r.last_at) - Date.parse(r.first_at)) / 1000) : '—';
  panel.hidden = false;
  panel.innerHTML = `
    <header class="tk-ph"><span class="tk-key">${esc(t.key)}</span>${t.running ? '<span class="pill running"><span class="dot running"></span>运行中</span>' : ''}<span class="grow"></span>
      <button class="kebab" id="tpMenu" aria-label="更多">⋯</button><button class="kebab" id="tpClose" aria-label="关闭">×</button></header>
    <div class="tk-pb">
      ${t.parent_id ? `<a class="t-caption" href="#/tasks?task=${esc(t.parent_id)}" id="tpParent">↳ 父任务</a>` : ''}
      <input id="tpTitle" class="tk-tin" value="${esc(t.title)}" aria-label="标题">
      <textarea id="tpDesc" class="input" rows="5" placeholder="描述：要做什么、验收标准、不要动什么…">${esc(t.description || '')}</textarea>
      <div class="t-caption faint" id="tpSaved" style="height:16px"></div>
      <div class="tk-props">
        <label>状态</label><select id="tpStatus" class="input input-sm">${TASK_STATUS.map(([k, n]) => `<option value="${k}" ${t.status === k ? 'selected' : ''}>${n}</option>`).join('')}</select>
        <label>优先级</label><select id="tpPrio" class="input input-sm">${Object.entries(TASK_PRIO).map(([k, x]) => `<option value="${k}" ${t.priority === k ? 'selected' : ''}>${x[0]}</option>`).join('')}</select>
        <label>工作区</label><select id="tpWs" class="input input-sm" ${t.thread_id ? 'disabled title="已经开始过，工作区不能再换"' : ''}>${wsOpts(t.workspace_id || '')}</select>
        <label>指派给</label><select id="tpWho" class="input input-sm">${assigneeOpts(assigneeOf(t))}</select>
        <label>标签</label><input id="tpLabels" class="input input-sm" value="${esc((t.labels || []).join(', '))}" placeholder="逗号分隔">
      </div>
      <div class="actions" style="margin:12px 0">
        <button class="btn btn-brand btn-sm" id="tpStart" ${canRun && !t.running ? '' : 'disabled'} title="${canRun ? '' : '先选工作区和指派对象'}">${t.thread_id ? '继续' : '开始'}</button>
        ${t.running ? '<button class="btn btn-outline btn-sm" id="tpStop">停止</button>' : ''}
        ${t.thread_id ? '<button class="btn btn-outline btn-sm" id="tpOpen">打开对话</button>' : ''}
      </div>
      ${t.parent_id ? '' : `<div class="tk-sec"><b>子任务</b>${t.children ? `<span class="faint num">${t.children_done}/${t.children}</span>` : ''}<span class="grow"></span><button class="btn btn-ghost btn-xs" id="tpAddChild">＋ 添加</button></div>
        ${(t.child_list || []).map(c => `<div class="tk-lrow" data-child="${esc(c.id)}"><span class="tk-key">${esc(c.key)}</span><span class="t truncate">${esc(c.title)}</span><span class="t-caption muted">${TASK_STATUS_TEXT[c.status] || c.status}</span></div>`).join('')}`}
      <div class="tk-sec"><b>执行日志</b><span class="faint num">${runs.length || ''}</span></div>
      ${runs.length ? runs.map((r, i) => { const [txt, pill] = RUN_STATUS[r.status] || [r.status, 'idle']; return `<div class="run-row" data-run="${esc(r.id)}">
        <span class="pill ${pill}"><span class="dot ${pill}"></span>${txt}</span>
        <span class="t">第 ${i + 1} 次 · ${RUN_TRIGGER[r.trigger] || r.trigger} <span class="faint">· ${esc(AGENT_LABEL[r.runtime] || r.runtime)}</span></span>
        <span class="t-caption faint">${dur(r)} · ${r.events} 条${r.cost_usd ? ` · $${(+r.cost_usd).toFixed(2)}` : ''} · ${ago(r.created_at)}</span></div>`; }).join('')
        : '<div class="t-caption faint" style="padding:4px 0 8px">还没有运行过。选好工作区和指派对象后点「开始」。</div>'}
      <div class="tk-sec"><b>评论</b><span class="faint num">${(t.comment_list || []).length || ''}</span></div>
      <div id="tpComments">${(t.comment_list || []).map(c => `<div class="tk-cm" data-a="${esc(c.author)}"><div class="h"><b>${c.author === 'user' ? '你' : c.author === 'agent' ? (t.agent_name || AGENT_LABEL[t.runtime] || '智能体') : '系统'}</b>${c.note ? '<span class="badge badge-muted">仅备注</span>' : ''}<span class="faint t-caption">${ago(c.created_at)}</span></div>
        <div class="cc-md">${md(c.body)}</div></div>`).join('')}</div>
      <div class="tk-compose"><textarea id="tpCm" class="input" rows="3" placeholder="${canRun ? '写评论 —— 默认会作为后续指令发给智能体' : '写评论（还没指派，只会记下来）'}"></textarea>
        <div class="actions"><label class="t-caption" style="display:flex;gap:6px;align-items:center"><input type="checkbox" id="tpNote" ${canRun ? '' : 'checked disabled'}> 仅备注，不触发智能体</label><span class="grow"></span>
          <button class="btn btn-brand btn-sm" id="tpSend">发送</button></div></div>
    </div>`;
  const put = async patch => {
    try { const r = await api(`/api/tasks/${id}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(patch) });
      const i = (S.tasks || []).findIndex(x => x.id === id); if (i >= 0) S.tasks[i] = { ...S.tasks[i], ...r }; drawTasks(); return r; }
    catch (e) { toast(e.message); return null; }
  };

  let timer = 0; const saved = $('#tpSaved');
  const later = patch => { clearTimeout(timer); saved.textContent = '…'; timer = setTimeout(async () => { saved.textContent = (await put(patch)) ? '已保存' : '保存失败'; }, 600); };
  $('#tpTitle').oninput = () => { const v = $('#tpTitle').value.trim(); if (v) later({ title: v }); };
  $('#tpDesc').oninput = () => later({ description: $('#tpDesc').value });
  $('#tpStatus').onchange = () => put({ status: $('#tpStatus').value });
  $('#tpPrio').onchange = () => put({ priority: $('#tpPrio').value });
  $('#tpWs').onchange = async () => { await put({ workspace_id: $('#tpWs').value || null }); openTask(id, true); };
  $('#tpWho').onchange = async () => { await put(assigneePatch($('#tpWho').value)); openTask(id, true); };
  $('#tpLabels').onchange = () => put({ labels: $('#tpLabels').value.split(/[,，]/).map(x => x.trim()).filter(Boolean) });
  $('#tpClose').onclick = () => { S.taskOpen = null; panel.hidden = true; panel.innerHTML = ''; $$('#tkBody [data-id]').forEach(x => { x.dataset.open = 'false'; }); };
  $('#tpMenu').onclick = e => openMenu(e.currentTarget, [
    { label: '复制编号', run: () => { navigator.clipboard?.writeText(t.key); toast('已复制 ' + t.key); } },
    { label: '存为飞书文档…', run: () => taskToLark(id) },
    { label: '存到 Obsidian', run: () => taskToObsidian(id) },
    t.parent_id && { label: '变成独立任务', run: async () => { await put({ parent_id: null }); openTask(id, true); } },
    '-',
    { label: '删除任务', danger: true, run: async () => {
      if (!await ask(`删除 ${t.key}「${t.title}」？\n对话记录不受影响；子任务会变成独立任务。`, { ok: '删除', danger: true })) return;
      try { await api(`/api/tasks/${id}`, { method: 'DELETE' }); toast('已删除'); $('#tpClose').click(); await loadTasks(); drawTasks(); } catch (err) { toast(err.message); } } },
  ]);
  $('#tpStart')?.addEventListener('click', async () => { $('#tpStart').disabled = true; await startTask(id); await loadTasks(); drawTasks(); openTask(id, true); });
  $('#tpStop')?.addEventListener('click', async () => { const r = runs.find(x => x.status === 'running'); if (r) { try { await post(`/api/sessions/${r.id}/interrupt`); toast('已请求停止'); } catch (e) { toast(e.message); } } });
  $('#tpOpen')?.addEventListener('click', () => openThreadIn(t.workspace_id, t.thread_id));
  $('#tpAddChild')?.addEventListener('click', () => dlgTask({ parent_id: id, workspace_id: t.workspace_id, assignee: assigneeOf(t) }));
  $$('#taskPanel [data-child]').forEach(r => { r.onclick = () => openTask(r.dataset.child); });
  $$('#taskPanel [data-run]').forEach(r => { r.onclick = () => openThreadIn(t.workspace_id, t.thread_id); r.title = '查看记录'; });
  $('#tpParent')?.addEventListener('click', e => { e.preventDefault(); openTask(t.parent_id); });
  $('#tpSend').onclick = async () => {
    const body = $('#tpCm').value.trim(); if (!body) return;
    const btn = $('#tpSend'); btn.disabled = true;
    try {
      const r = await post(`/api/tasks/${id}/comments`, { body, note: $('#tpNote').checked });
      toast(r.triggered ? '已发给智能体' : '已记下');
      await loadTasks(); drawTasks(); openTask(id, true);
    } catch (e) { toast(e.message); btn.disabled = false; }
  };
  $('#tpCm').onkeydown = e => { if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) { e.preventDefault(); $('#tpSend').click(); } };
}

function openMenu(anchor, items) {
  document.querySelector('.menu')?.remove();
  const list = items.filter(Boolean);
  const m = document.createElement('div'); m.className = 'menu';
  m.innerHTML = list.map((it, i) => it === '-' ? '<div class="menu-sep"></div>'
    : `<button class="cp-row ${it.danger ? 'danger' : ''}" data-i="${i}"><span class="cp-t"><b>${esc(it.label)}</b></span></button>`).join('');
  document.body.appendChild(m);
  const r = anchor.getBoundingClientRect();
  m.style.top = Math.min(r.bottom + 4, innerHeight - m.offsetHeight - 8) + 'px';
  m.style.left = Math.max(8, Math.min(r.right - m.offsetWidth, innerWidth - m.offsetWidth - 8)) + 'px';
  const close = () => { m.remove(); document.removeEventListener('mousedown', out, true); };
  const out = e => { if (!m.contains(e.target)) close(); };
  setTimeout(() => document.addEventListener('mousedown', out, true), 0);
  m.querySelectorAll('[data-i]').forEach(b => { b.onclick = () => { close(); list[+b.dataset.i].run(); }; });
}
const avatarHtml = (a, cls = '') => `<span class="ag-av ${cls}" ${a.archived_at ? 'data-archived="true"' : ''}>${a.avatar ? esc(a.avatar) : rtMark(a.runtime)}</span>`;
const THINK_LABEL = { minimal: 'Minimal', low: 'Low', medium: 'Medium', high: 'High', xhigh: 'Extra high', max: 'Max', ultra: 'Ultra' };

function agentPresence(a) {
  if (a.archived_at) return { availability: 'archived', workload: 'idle', running: 0 };
  const rt = (S.runtimes || []).find(r => r.id === a.runtime);
  const availability = !rt || !rt.installed ? 'unbound' : rt.authed === false ? 'offline' : 'online';
  return { availability, workload: a.running > 0 ? 'working' : 'idle', running: a.running || 0 };
}
const AVAIL_TEXT = { online: '在线', offline: '离线', unbound: '需绑定运行时', archived: '已归档' };
const WORK_TEXT = { working: '处理中', idle: '空闲' };
const statusCellHtml = a => {
  const p = agentPresence(a);
  if (p.availability === 'archived') return '<span class="t-caption muted">已归档</span>';
  if (p.availability === 'unbound') return '<span class="ag-need">⚠ 需绑定运行时</span>';
  return `<span class="ag-pres" data-s="${p.availability}"><span class="dot"></span>${AVAIL_TEXT[p.availability]}${
    p.running ? `<span class="muted"> · ${p.running} 次运行</span>` : ''}</span>`;
};
const presencePillHtml = a => {
  const p = agentPresence(a);
  return `<span class="ag-pill"><span class="ag-pres" data-s="${p.availability}"><span class="dot"></span>${AVAIL_TEXT[p.availability]}</span>${
    p.availability === 'archived' ? '' : `<span class="ag-work" data-w="${p.workload}">${WORK_TEXT[p.workload]}</span>`}</span>`;
};
function lastActiveText(t) {
  if (!t) return '30 天内无活动';
  const days = Math.floor((Date.now() - Date.parse(t)) / 86400e3);
  return days <= 0 ? '今天' : days > 30 ? '30 天内无活动' : `${days} 天前`;
}
function sparkline(vals) {
  const max = Math.max(1, ...vals), w = 70, h = 20, n = vals.length;
  if (!vals.some(Boolean)) return '<span class="faint t-caption">无活动</span>';
  const pts = vals.map((v, i) => `${(i / (n - 1) * w).toFixed(1)},${(h - 2 - v / max * (h - 4)).toFixed(1)}`).join(' ');
  return `<svg class="spark" viewBox="0 0 ${w} ${h}" width="${w}" height="${h}" aria-hidden="true"><polyline points="${pts}" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round" stroke-linecap="round"/></svg>`;
}
const fmtSecs = s => s == null ? '—' : s < 60 ? `${Math.round(s)} 秒` : s < 3600 ? `${Math.round(s / 60)} 分钟` : `${(s / 3600).toFixed(1)} 小时`;
