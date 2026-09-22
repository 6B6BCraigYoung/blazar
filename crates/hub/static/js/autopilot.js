const AP_STATUS = { completed: ['完成', 'ok'], running: ['运行中', 'info'], failed: ['失败', 'bad'], skipped: ['跳过', 'warn'], pending: ['等待中', 'warn'], starting: ['启动中', 'info'] };
const AP_SOURCE = { schedule: '日程', manual: '手动', webhook: 'Webhook' };
const AP_TZ = ['Asia/Shanghai', 'Asia/Tokyo', 'Asia/Singapore', 'Asia/Kolkata', 'Europe/London', 'Europe/Berlin', 'America/New_York', 'America/Chicago', 'America/Los_Angeles', 'Australia/Sydney', 'UTC'];
const browserTz = () => { try { return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'; } catch (_) { return 'UTC'; } };
const WEEK_CN = ['日', '一', '二', '三', '四', '五', '六'];
const pad2 = n => String(n).padStart(2, '0');

function cronToForm(cron) {
  if (!cron) return { kind: 'none' };
  const f = cron.trim().split(/\s+/); if (f.length !== 5) return { kind: 'custom', cron };
  const [mi, h, dom, mon, dow] = f; let m;
  if ((m = /^\*\/(\d+)$/.exec(mi)) && h === '*' && dom === '*' && mon === '*' && dow === '*') return { kind: 'minutes', n: +m[1] };
  if (/^\d+$/.test(mi) && (m = /^\*\/(\d+)$/.exec(h)) && dom === '*' && mon === '*' && dow === '*') return { kind: 'hours', n: +m[1], minute: +mi };
  if (/^\d+$/.test(mi) && /^\d+$/.test(h) && dom === '*' && mon === '*') {
    if (dow === '*') return { kind: 'daily', time: `${pad2(h)}:${pad2(mi)}` };
    if (/^[0-7](,[0-7])*$/.test(dow)) return { kind: 'weekly', time: `${pad2(h)}:${pad2(mi)}`, days: dow.split(',').map(d => +d % 7) };
    if (dow === '1-5') return { kind: 'weekly', time: `${pad2(h)}:${pad2(mi)}`, days: [1, 2, 3, 4, 5] };
  }
  return { kind: 'custom', cron };
}
function formToCron(f) {
  const [h, mi] = (f.time || '09:00').split(':').map(Number);
  switch (f.kind) {
    case 'none': return '';
    case 'minutes': return `*/${Math.max(1, Math.min(59, f.n || 15))} * * * *`;
    case 'hours': return `${Math.max(0, Math.min(59, f.minute || 0))} */${Math.max(1, Math.min(23, f.n || 1))} * * *`;
    case 'daily': return `${mi} ${h} * * *`;
    case 'weekly': return `${mi} ${h} * * ${(f.days?.length ? [...f.days].sort() : [1]).join(',')}`;
    default: return (f.cron || '').trim();
  }
}
function cronText(cron) {
  const f = cronToForm(cron);
  switch (f.kind) {
    case 'none': return '不定时';
    case 'minutes': return `每 ${f.n} 分钟`;
    case 'hours': return `每 ${f.n} 小时（第 ${f.minute} 分）`;
    case 'daily': return `每天 ${f.time}`;
    case 'weekly': return `每周${f.days.map(d => WEEK_CN[d]).join('、')} ${f.time}`;
    default: return cron;
  }
}
const until = iso => {
  if (!iso) return '';
  const s = Math.round((Date.parse(iso) - Date.now()) / 1000);
  if (s <= 0) return '马上';
  return s < 90 ? `${s} 秒后` : s < 5400 ? `${Math.round(s / 60)} 分钟后` : s < 129600 ? `${Math.round(s / 3600)} 小时后` : `${Math.round(s / 86400)} 天后`;
};
const localTime = iso => { try { return new Date(iso).toLocaleString(undefined, { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit', weekday: 'short' }); } catch (_) { return iso; } };

async function loadAutopilots() {
  try { S.autopilots = await api('/api/autopilots'); } catch (_) { S.autopilots = S.autopilots || []; }
  return S.autopilots;
}
let apTimer;
function refreshAutopilotsSoon() {
  clearTimeout(apTimer);
  apTimer = setTimeout(async () => {
    await loadAutopilots();
    if (S.route.split('?')[0] === '#/autopilots') { drawAutopilots(); if (S.apOpen && !$('#apPanel :focus')) openAutopilot(S.apOpen, true); }
  }, 400);
}
async function pageAutopilots() {
  $('#page').innerHTML = `
    <div class="phead">
      <button class="btn btn-ghost btn-icon menu-btn" style="display:none" aria-label="菜单"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M4 7h16M4 12h16M4 17h16"/></svg></button>
      <h1>自动化</h1><span class="badge badge-muted num" id="apCount"></span>
      <div class="grow"></div>
      <button class="btn btn-brand btn-sm" id="apNew">新建自动化</button>
    </div>
    <div class="tk-wrap"><div id="apBody" class="tk-body" style="padding:14px var(--gutter)"></div><aside id="apPanel" class="tk-panel" hidden></aside></div>`;
  wireHeader();
  $('#apNew').onclick = () => dlgAutopilot();
  if (!S.profiles) await loadProfiles();
  await loadAutopilots();
  drawAutopilots();
  const want = new URLSearchParams(S.route.split('?')[1] || '').get('id');
  if (want) openAutopilot(want);

  clearInterval(S.apTick);
  S.apTick = setInterval(() => { if (S.route.split('?')[0] === '#/autopilots') drawAutopilots(); else clearInterval(S.apTick); }, 30000);
}
function apAssignee(a) {
  return a.agent_name ? `${a.agent_avatar ? esc(a.agent_avatar) + ' ' : ''}${esc(a.agent_name)}` : esc(AGENT_LABEL[a.runtime] || a.runtime || 'Claude Code');
}
function drawAutopilots() {
  const host = $('#apBody'); if (!host) return;
  const list = S.autopilots || [];
  $('#apCount').textContent = list.length || '';
  if (!list.length) {
    host.innerHTML = `<div class="empty" style="padding:60px 20px">还没有自动化<div class="t-caption faint" style="margin-top:8px;line-height:1.7">
      把反复要做的事交给智能体：每天早上看一遍依赖更新、每小时跑一次冒烟测试、CI 挂了用 webhook 叫它来看。<br>
      每次触发可以建一条任务沉淀在看板上，也可以直接跑。</div>
      <button class="btn btn-brand btn-sm" style="margin-top:14px" onclick="dlgAutopilot()">新建自动化</button></div>`;
    return;
  }
  host.innerHTML = `<div class="ap-table">
    <div class="ap-row ap-hd"><span>名称</span><span>触发</span><span>工作区 · 智能体</span><span>下一次</span><span>上一次</span><span></span></div>
    ${list.map(a => {
      const [lt, tone] = AP_STATUS[a.last_status] || ['', ''];
      return `<div class="ap-row" data-id="${esc(a.id)}" data-on="${S.apOpen === a.id}" tabindex="0">
        <span class="ap-nm"><span class="ap-dot" data-s="${esc(a.status)}"></span><b>${esc(a.name)}</b>
          <span class="badge badge-muted">${a.mode === 'run' ? '直接运行' : '建任务'}</span></span>
        <span class="ap-tr">${esc(cronText(a.cron))}${a.webhook ? ' <span class="badge badge-muted">Webhook</span>' : ''}</span>
        <span class="muted">${a.workspace_name ? esc(a.workspace_name) : '<span class="bad">工作区已删除</span>'} · ${apAssignee(a)}</span>
        <span class="num">${a.status === 'paused' ? '<span class="warn">已暂停</span>' : a.next_run_at ? `<span title="${esc(localTime(a.next_run_at))}">${until(a.next_run_at)}</span>` : '<span class="faint">—</span>'}</span>
        <span>${lt ? `<span class="gchip ${tone}">${lt}</span> <span class="faint t-micro">${a.last_run_at ? ago(a.last_run_at) : ''}</span>` : '<span class="faint">还没跑过</span>'}</span>
        <span><button class="laybtn" data-more title="更多" aria-label="更多">⋯</button></span></div>`;
    }).join('')}</div>`;
  host.querySelectorAll('.ap-row[data-id]').forEach(r => {
    const a = list.find(x => x.id === r.dataset.id);
    r.onclick = e => {
      if (e.target.closest('[data-more]')) { openMenu(e.target.closest('[data-more]'), apMenu(a)); return; }
      openAutopilot(a.id);
    };
    r.onkeydown = e => { if (e.key === 'Enter') openAutopilot(a.id); };
  });
}
const apMenu = a => [
  { label: '立即运行一次', run: () => apRun(a.id) },
  { label: a.status === 'paused' ? '恢复' : '暂停', run: () => apPatch(a.id, { status: a.status === 'paused' ? 'active' : 'paused' }) },
  { label: '编辑…', run: () => dlgAutopilot(a) },
  { label: '复制一份', run: async () => { const d = await api(`/api/autopilots/${a.id}`); dlgAutopilot({ ...d, id: null, name: d.name + ' 副本' }); } },
  '-',
  { label: '删除', danger: true, run: async () => {
    if (!await ask(`删除自动化「${a.name}」？运行记录一起删掉；它建过的任务和对话不受影响。`, { ok: '删除', danger: true })) return;
    await api(`/api/autopilots/${a.id}`, { method: 'DELETE' }); if (S.apOpen === a.id) closeAutopilot(); await loadAutopilots(); drawAutopilots();
  } },
];
async function apPatch(id, patch) {
  try {
    await api(`/api/autopilots/${id}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(patch) });
    await loadAutopilots(); drawAutopilots(); if (S.apOpen === id) openAutopilot(id, true);
  } catch (e) { toast(e.message); }
}
async function apRun(id) {
  try {
    const r = await post(`/api/autopilots/${id}/run`);
    toast(r.status === 'running' ? '已开始运行' : r.status === 'pending' ? '工作区正忙，空出来就跑' : `${(AP_STATUS[r.status] || [r.status])[0]}：${r.reason || ''}`);
  } catch (e) { toast(e.message); }
  await loadAutopilots(); drawAutopilots(); if (S.apOpen === id) openAutopilot(id, true);
}
function closeAutopilot() { S.apOpen = null; const p = $('#apPanel'); if (p) p.hidden = true; drawAutopilots(); }
async function openAutopilot(id, quiet) {
  S.apOpen = id;
  let p = $('#apPanel'); if (!p) return;
  if (!quiet) { p.hidden = false; p.innerHTML = '<div class="empty">加载中…</div>'; drawAutopilots(); }
  let a;
  try { a = await api(`/api/autopilots/${id}`); } catch (e) { p.innerHTML = `<div class="empty">${esc(e.message)}</div>`; return; }
  p = $('#apPanel'); if (!p || S.apOpen !== id) return;
  p.hidden = false;
  const once = S.apToken?.id === id ? S.apToken : null;
  const hookUrl = once ? `${location.origin}${once.path}` : '';
  p.innerHTML = `
    <header class="tk-ph"><span class="ap-dot" data-s="${esc(a.status)}"></span><b style="flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(a.name)}</b>
      <button class="btn btn-brand btn-xs" data-a="run">立即运行</button>
      <button class="btn btn-outline btn-xs" data-a="toggle">${a.status === 'paused' ? '恢复' : '暂停'}</button>
      <button class="btn btn-outline btn-xs" data-a="edit">编辑</button>
      <button class="kebab" data-a="close" aria-label="关闭">×</button></header>
    <div class="tk-pb">
      ${a.paused_reason ? `<div class="gp-op" style="margin:0 0 12px"><span>${esc(a.paused_reason)}</span></div>` : ''}
      <div class="ap-kv"><span>触发</span><b>${esc(cronText(a.cron))}</b>${a.cron ? `<span class="faint mono t-micro">${esc(a.cron)} · ${esc(a.timezone)}</span>` : ''}</div>
      <div class="ap-kv"><span>在哪做</span><b>${a.workspace_name ? `<a href="#/workspaces/${esc(a.workspace_id)}">${esc(a.workspace_name)}</a>` : '<span class="bad">工作区已删除</span>'}</b><span class="faint">${esc(a.node || '')}</span></div>
      <div class="ap-kv"><span>谁来做</span><b>${apAssignee(a)}</b><span class="faint">${esc([a.model, PERM_TEXT[a.permission_mode || '']].filter(Boolean).join(' · '))}</span></div>
      <div class="ap-kv"><span>方式</span><b>${a.mode === 'run' ? '直接运行（不建任务）' : '每次建一条任务'}</b><span class="faint">工作区正忙时${a.concurrency === 'wait' ? '等它空出来（最多 30 分钟）' : '跳过这一次'}</span></div>
      ${a.upcoming?.length && a.status === 'active' ? `<div class="ap-sec"><h5>接下来</h5>${a.upcoming.map(t => `<div class="ap-up"><span>${esc(localTime(t))}</span><span class="faint">${until(t)}</span></div>`).join('')}</div>` : ''}
      <div class="ap-sec"><h5>指令</h5><pre class="ap-ins">${esc(a.instructions)}</pre></div>
      <div class="ap-sec"><h5>Webhook</h5>
        ${once ? `<div class="ap-hook"><div class="warn t-caption">地址只显示这一次，现在就复制走；丢了只能轮换。</div>
            <input class="input input-sm mono" readonly value="${esc(hookUrl)}" id="apHookUrl">
            <div class="row" style="gap:6px;margin-top:6px"><button class="btn btn-outline btn-xs" data-a="copy">复制地址</button><button class="btn btn-outline btn-xs" data-a="copycurl">复制 curl 示例</button></div></div>`
          : a.webhook ? `<div class="t-caption muted">已开启，地址以 <span class="mono">…${esc(a.webhook_hint || '')}</span> 结尾。POST 一次触发一次，请求体会作为数据附给智能体。</div>`
          : '<div class="t-caption faint">没开。开启后会得到一个带令牌的地址，外部系统（CI、监控、脚本）POST 它就触发一次。</div>'}
        <div class="row" style="gap:6px;margin-top:8px"><button class="btn btn-outline btn-xs" data-a="hook">${a.webhook ? '轮换地址' : '开启 Webhook'}</button>
          ${a.webhook ? '<button class="btn btn-outline btn-xs" data-a="hookoff">关闭</button>' : ''}</div></div>
      <div class="ap-sec"><h5>运行记录 <span class="num faint">${a.runs}</span></h5>
        ${a.run_list.length ? a.run_list.map(r => { const [t, tone] = AP_STATUS[r.status] || [r.status, '']; return `<div class="ap-run">
          <span class="gchip ${tone}">${t}</span><span class="muted">${AP_SOURCE[r.source] || esc(r.source)}</span>
          <span class="ap-run-w">${r.task_id ? `<a href="#/tasks?task=${esc(r.task_id)}">${esc(r.task_key || '任务')} ${esc(shortStr(r.task_title || '', 30))}</a>` : r.thread_id ? `<button class="linkbtn" data-thread="${esc(r.thread_id)}">打开对话</button>` : ''}
            ${r.reason ? `<span class="faint">${esc(r.reason)}</span>` : ''}</span>
          <span class="faint t-micro num" title="${esc(r.triggered_at)}">${ago(r.triggered_at)}</span></div>`; }).join('')
          : '<div class="t-caption faint">还没跑过</div>'}</div>
    </div>`;
  const on = (sel, fn) => p.querySelectorAll(sel).forEach(b => { b.onclick = fn; });
  on('[data-a="close"]', closeAutopilot);
  on('[data-a="run"]', () => apRun(id));
  on('[data-a="toggle"]', () => apPatch(id, { status: a.status === 'paused' ? 'active' : 'paused' }));
  on('[data-a="edit"]', () => dlgAutopilot(a));
  on('[data-a="copy"]', () => copyText(hookUrl, '地址已复制'));
  on('[data-a="copycurl"]', () => copyText(`curl -X POST '${hookUrl}' -H 'content-type: application/json' -d '{"reason":"ci failed","url":"https://…"}'`, 'curl 示例已复制'));
  on('[data-a="hook"]', async () => {
    if (a.webhook && !await ask('轮换地址？旧地址立刻失效，用着它的外部系统要换成新的。', { ok: '轮换', danger: true })) return;
    try { const r = await post(`/api/autopilots/${id}/webhook`); S.apToken = { id, ...r }; await loadAutopilots(); drawAutopilots(); openAutopilot(id, true); } catch (e) { toast(e.message); }
  });
  on('[data-a="hookoff"]', async () => {
    if (!await ask('关闭 Webhook？地址立刻失效。', { ok: '关闭', danger: true })) return;
    await api(`/api/autopilots/${id}/webhook`, { method: 'DELETE' }); S.apToken = null; await loadAutopilots(); drawAutopilots(); openAutopilot(id, true);
  });
  on('[data-thread]', e => openThreadIn(a.workspace_id, e.currentTarget.dataset.thread));
}
async function copyText(text, okMsg) {
  try { await navigator.clipboard.writeText(text); toast(okMsg); }
  catch (_) { const i = $('#apHookUrl'); if (i) { i.select(); document.execCommand('copy'); toast(okMsg); } else toast('复制失败，手动选中复制'); }
}

function dlgAutopilot(init) {
  const a = init || {};
  const editing = !!a.id;
  let f = cronToForm(a.cron || (editing ? '' : '0 9 * * *'));
  const tz = a.timezone || browserTz();
  const tzs = [...new Set([tz, browserTz(), ...AP_TZ])];
  const cur = a.agent_profile ? 'p:' + a.agent_profile : a.runtime ? 'r:' + a.runtime : 'r:claude';
  openDlg(`<h3>${editing ? '编辑自动化' : '新建自动化'}</h3>
    <div class="field"><label>名字</label><input id="apName" class="input" value="${esc(a.name || '')}" placeholder="每日依赖巡检"></div>
    <div class="field"><label>指令（每次触发都原样发给智能体）</label><textarea id="apIns" class="input" rows="6" placeholder="检查依赖有没有安全更新。有的话升级、跑测试，把结果写进任务的回复里；没有就只回复「没有更新」。">${esc(a.instructions || '')}</textarea></div>
    <div class="grid2">
      <div class="field"><label>工作区</label><select id="apWs" class="input">${wsOpts(a.workspace_id || S.ws?.id || '')}</select></div>
      <div class="field"><label>谁来做</label><select id="apWho" class="input">${assigneeOpts(cur).replace('<option value="">未指派</option>', '')}</select></div>
      <div class="field"><label>方式</label><select id="apMode" class="input"><option value="task" ${a.mode !== 'run' ? 'selected' : ''}>每次建一条任务（沉淀在看板上）</option><option value="run" ${a.mode === 'run' ? 'selected' : ''}>直接运行（不建任务）</option></select></div>
      <div class="field"><label>权限模式</label><select id="apPerm" class="input">
        ${[['', '跟随智能体 / 运行时设置'], ['acceptEdits', 'Edit automatically（自动批准改文件）'], ['bypassPermissions', 'Bypass permissions（全部放行）'], ['plan', 'Plan（只规划不动手）'], ['default', 'Manual（每一步都等你批）']].map(([v, t]) => `<option value="${v}" ${(a.permission_mode || '') === v ? 'selected' : ''}>${t}</option>`).join('')}</select></div>
    </div>
    <div class="field" id="apTitleBox"><label>任务标题模板（可用 {{date}} {{time}} {{name}}）</label><input id="apTitle" class="input" value="${esc(a.title_template || '')}" placeholder="{{name}} · {{date}}"></div>
    <div class="field"><label>日程</label>
      <div class="seg" id="apKind">${[['none', '不定时'], ['minutes', '每 N 分钟'], ['hours', '每 N 小时'], ['daily', '每天'], ['weekly', '每周'], ['custom', 'cron']].map(([k, t]) => `<button data-k="${k}">${t}</button>`).join('')}</div>
      <div id="apSched" class="ap-sched"></div>
      <div id="apNext" class="ap-next"></div></div>
    <div class="field"><label>工作区正忙时</label><select id="apConc" class="input"><option value="skip" ${a.concurrency !== 'wait' ? 'selected' : ''}>跳过这一次</option><option value="wait" ${a.concurrency === 'wait' ? 'selected' : ''}>等它空出来再跑（最多等 30 分钟）</option></select></div>
    <div class="t-micro faint">无人值守时没人点「允许」：权限模式选 Manual 的话，运行会停在第一个需要批准的操作上，出现在「等我审批」里。</div>
    <div class="dfoot"><button class="btn btn-outline" onclick="closeDlg()">取消</button><button class="btn btn-brand" id="apSave">${editing ? '保存' : '创建'}</button></div>`);
  const drawSched = () => {
    $$('#apKind [data-k]').forEach(b => { b.dataset.on = String(b.dataset.k === f.kind); });
    const tzSel = `<select id="apTz" class="input input-sm" style="width:auto">${tzs.map(z => `<option ${z === (f.tz || tz) ? 'selected' : ''}>${esc(z)}</option>`).join('')}</select>`;
    const time = `<input id="apTime" type="time" class="input input-sm" style="width:auto" value="${esc(f.time || '09:00')}">`;
    $('#apSched').innerHTML = {
      none: '<span class="faint t-caption">只在手动运行或 webhook 触发时跑。</span>',
      minutes: `每 <input id="apN" type="number" min="1" max="59" class="input input-sm" style="width:70px" value="${f.n || 15}"> 分钟`,
      hours: `每 <input id="apN" type="number" min="1" max="23" class="input input-sm" style="width:70px" value="${f.n || 1}"> 小时，在第 <input id="apMin" type="number" min="0" max="59" class="input input-sm" style="width:70px" value="${f.minute || 0}"> 分 ${tzSel}`,
      daily: `每天 ${time} ${tzSel}`,
      weekly: `<span class="ap-days">${WEEK_CN.map((d, i) => `<button data-d="${i}" aria-pressed="${(f.days || [1]).includes(i)}">${d}</button>`).join('')}</span> ${time} ${tzSel}`,
      custom: `<input id="apCron" class="input input-sm mono" style="width:180px" value="${esc(f.cron || '0 9 * * 1-5')}" placeholder="分 时 日 月 周"> ${tzSel}`,
    }[f.kind];
    $('#apSched').querySelectorAll('input, select').forEach(i => { i.oninput = readSched; });
    $('#apSched').querySelectorAll('[data-d]').forEach(b => { b.onclick = () => { b.setAttribute('aria-pressed', String(b.getAttribute('aria-pressed') !== 'true')); readSched(); }; });
    preview();
  };
  const readSched = () => {
    f = { ...f, n: +($('#apN')?.value || f.n || 0), minute: +($('#apMin')?.value || 0), time: $('#apTime')?.value || f.time, cron: $('#apCron')?.value ?? f.cron,
      tz: $('#apTz')?.value || f.tz, days: $('#apSched [data-d]') ? [...$$('#apSched [data-d][aria-pressed="true"]')].map(b => +b.dataset.d) : f.days };
    preview();
  };
  let pvTimer;
  const preview = () => {
    clearTimeout(pvTimer);
    const cron = formToCron(f), host = $('#apNext');
    if (!cron) { host.innerHTML = ''; return; }
    pvTimer = setTimeout(async () => {
      try {
        const r = await post('/api/autopilots/preview', { cron, timezone: f.tz || tz });
        if (!$('#apNext')) return;
        $('#apNext').innerHTML = r.ok ? `<span class="faint">接下来：</span>${r.upcoming.slice(0, 5).map(t => `<span class="gchip">${esc(localTime(t))}</span>`).join(' ') || '<span class="warn">这条日程永远不会触发</span>'}`
          : `<span class="bad">${esc(r.error)}</span>`;
      } catch (_) {}
    }, 250);
  };
  $$('#apKind [data-k]').forEach(b => { b.onclick = () => { f = { ...f, kind: b.dataset.k }; drawSched(); }; });
  const syncMode = () => { $('#apTitleBox').hidden = $('#apMode').value === 'run'; };
  $('#apMode').onchange = syncMode; syncMode();
  drawSched();
  $('#apName').focus();
  $('#apSave').onclick = async () => {
    const who = $('#apWho').value;
    const body = { name: $('#apName').value.trim(), instructions: $('#apIns').value, workspace_id: $('#apWs').value,
      agent_profile: who.startsWith('p:') ? who.slice(2) : '', runtime: who.startsWith('r:') ? who.slice(2) : '',
      mode: $('#apMode').value, permission_mode: $('#apPerm').value, title_template: $('#apTitle').value,
      cron: formToCron(f), timezone: f.tz || tz, concurrency: $('#apConc').value };
    if (!body.name || !body.instructions.trim() || !body.workspace_id) { toast('名字、指令、工作区都要填'); return; }
    try {
      const r = await api(editing ? `/api/autopilots/${a.id}` : '/api/autopilots', { method: editing ? 'PUT' : 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
      closeDlg(); toast(editing ? '已保存' : '已创建');
      await loadAutopilots();
      if (S.route.split('?')[0] === '#/autopilots') { drawAutopilots(); openAutopilot(r.id); } else navigate('#/autopilots?id=' + r.id);
    } catch (e) { toast(e.message); }
  };
}
