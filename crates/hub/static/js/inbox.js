const IN_KIND = {
  run_done: ['跑完了', 'ok', '<path d="M5 12.5l4.5 4.5L19 7.5"/>'],
  run_failed: ['没跑成', 'bad', '<path d="M12 8v5M12 16.5v.5"/><circle cx="12" cy="12" r="9"/>'],
  approval: ['等你裁决', 'warn', '<path d="M12 9v4M12 17h.01"/><path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z"/>'],
  question: ['在问你', 'info', '<path d="M4 5h16v11H9l-5 4z"/><path d="M10 9a2 2 0 1 1 3 1.7c-.6.4-1 .8-1 1.5M12 14.5v.2"/>'],
  autopilot_paused: ['自动化暂停', 'warn', '<circle cx="12" cy="12" r="8.5"/><path d="M10 9v6M14 9v6"/>'],
  rate_limit: ['额度告警', 'warn', '<path d="M4 18a8 8 0 1 1 16 0"/><path d="M12 18l4-6"/>'],
};
async function loadInbox(filter, kind) {
  const qs = new URLSearchParams(); if (filter) qs.set('filter', filter); if (kind) qs.set('kind', kind);
  const r = await api(`/api/inbox?${qs}`);
  S.inboxUnread = r.unread;
  return r;
}
async function refreshInboxCount() {
  try { S.inboxUnread = (await api('/api/inbox/count')).unread; } catch (_) {}
  const c = $('#cnt-inbox'); if (c) { c.textContent = S.inboxUnread || ''; c.dataset.hot = String(!!S.inboxUnread); }
}
let inboxTimer;
function refreshInboxSoon() {
  clearTimeout(inboxTimer);
  inboxTimer = setTimeout(async () => {
    const before = S.inboxUnread || 0;
    await refreshInboxCount();
    if (S.route.split('?')[0] === '#/inbox') drawInbox();

    if (S.ws && S.viewSession && document.visibilityState === 'visible' && (S.inboxUnread || 0) > before) seenThread(S.viewSession);
  }, 300);
}
async function seenThread(thread) {
  try { const r = await post(`/api/inbox/seen/${encodeURIComponent(thread)}`); S.inboxUnread = r.unread; const c = $('#cnt-inbox'); if (c) { c.textContent = r.unread || ''; c.dataset.hot = String(!!r.unread); } } catch (_) {}
}
const inboxView = () => { try { return { filter: 'all', kind: '', ...JSON.parse(localStorage.getItem('blazar.inboxview') || '{}') }; } catch (_) { return { filter: 'all', kind: '' }; } };
const setInboxView = p => { try { localStorage.setItem('blazar.inboxview', JSON.stringify({ ...inboxView(), ...p })); } catch (_) {} };
async function pageInbox() {
  $('#page').innerHTML = `
    <div class="phead">
      <button class="btn btn-ghost btn-icon menu-btn" style="display:none" aria-label="菜单"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M4 7h16M4 12h16M4 17h16"/></svg></button>
      <h1>收件箱</h1><span class="badge badge-muted num" id="inCount"></span>
      <div class="seg" id="inFilter" style="margin-left:10px"><button data-f="unread">未读</button><button data-f="all">全部</button><button data-f="archived">已归档</button></div>
      <div class="grow"></div>
      <button class="btn btn-outline btn-xs" id="inKind">类型</button>
      <button class="btn btn-outline btn-xs" id="inReadAll">全部已读</button>
      <button class="btn btn-outline btn-xs" id="inArchive" title="把已读的都归档">归档已读</button>
    </div>
    <div class="scroll" style="padding:14px var(--gutter)"><div id="inBody" class="in-list"></div></div>`;
  wireHeader();
  $$('#inFilter [data-f]').forEach(b => { b.onclick = () => { setInboxView({ filter: b.dataset.f }); drawInbox(); }; });
  $('#inKind').onclick = e => openMenu(e.currentTarget, [['', '全部类型'], ...Object.entries(IN_KIND).map(([k, v]) => [k, v[0]])]
    .map(([k, t]) => ({ label: `${inboxView().kind === k ? '✓ ' : '　'}${t}`, run: () => { setInboxView({ kind: k }); drawInbox(); } })));
  $('#inReadAll').onclick = async () => { await post('/api/inbox', { action: 'read' }); drawInbox(); };
  $('#inArchive').onclick = async () => { await post('/api/inbox', { action: 'archive' }); drawInbox(); };
  await drawInbox();
}
async function drawInbox() {
  const host = $('#inBody'); if (!host) return;
  const v = inboxView();
  $$('#inFilter [data-f]').forEach(b => { b.dataset.on = String(b.dataset.f === v.filter); });
  $('#inKind').textContent = v.kind ? `类型：${IN_KIND[v.kind][0]}` : '类型';
  let r;
  try { r = await loadInbox(v.filter, v.kind); } catch (e) { host.innerHTML = `<div class="empty">${esc(e.message)}</div>`; return; }
  $('#inCount').textContent = r.unread ? `${r.unread} 未读` : '';
  const c = $('#cnt-inbox'); if (c) { c.textContent = r.unread || ''; c.dataset.hot = String(!!r.unread); }
  if (!r.items.length) {
    host.innerHTML = `<div class="empty" style="padding:60px 20px">${v.filter === 'unread' ? '没有未读的' : v.filter === 'archived' ? '还没有归档的' : '收件箱是空的'}
      <div class="t-caption faint" style="margin-top:8px">一轮跑完或失败、等你裁决、agent 在问你、自动化被暂停、额度快用完 —— 这些会出现在这里。</div></div>`;
    return;
  }
  host.innerHTML = r.items.map(it => {
    const [label, tone, path] = IN_KIND[it.kind] || [it.kind, '', ''];
    return `<div class="in-row" data-id="${esc(it.id)}" data-read="${it.read}" tabindex="0">
      <span class="in-ic ${tone}"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${path}</svg></span>
      <span class="in-main"><span class="in-t">${esc(it.title)}</span>${it.body ? `<span class="in-b">${esc(it.body)}</span>` : ''}</span>
      <span class="in-meta"><span class="gchip ${tone}">${label}</span><span class="faint t-micro num">${ago(it.created_at)}</span></span>
      <span class="in-acts"><button class="linkbtn" data-a="${it.read ? 'unread' : 'read'}">${it.read ? '标为未读' : '标为已读'}</button>
        <button class="linkbtn" data-a="${it.archived ? 'unarchive' : 'archive'}">${it.archived ? '移回' : '归档'}</button></span></div>`;
  }).join('');
  host.querySelectorAll('.in-row').forEach(row => {
    const it = r.items.find(x => x.id === row.dataset.id);
    const open = async () => {
      if (!it.read) await post('/api/inbox', { action: 'read', ids: [it.id] }).catch(() => {});
      if (it.kind === 'autopilot_paused') navigate('#/autopilots?id=' + it.ref_id);
      else if (it.kind === 'rate_limit') navigate('#/usage');
      else if (it.workspace_id && it.thread_id) openThreadIn(it.workspace_id, it.thread_id);
      else if (it.workspace_id) navigate('#/workspaces/' + it.workspace_id);
      else drawInbox();
    };
    row.onclick = async e => {
      const a = e.target.closest('[data-a]');
      if (a) { e.stopPropagation(); await post('/api/inbox', { action: a.dataset.a, ids: [it.id] }); drawInbox(); return; }
      open();
    };
    row.onkeydown = e => { if (e.key === 'Enter') open(); };
  });
}

const ATT_ORDER = { awaiting_approval: 0, errored: 1, completed: 2, running: 3 };
const ATT_TEXT = { awaiting_approval: '等你', errored: '出错', completed: '跑完', running: '运行中' };
function attentionList() {
  return S.workspaces
    .filter(w => w.activity === 'running' || w.activity === 'awaiting_approval' || ((w.activity === 'completed' || w.activity === 'errored') && unseen(w)))
    .sort((a, b) => (ATT_ORDER[a.activity] - ATT_ORDER[b.activity]) || String(b.last_active_at || '').localeCompare(String(a.last_active_at || '')));
}
function drawAttention() {
  const host = $('#nav-attention'); if (!host) return;
  const list = attentionList();
  host.innerHTML = list.length ? `<div class="grp-label">注意力</div>${list.slice(0, 6).map(w => `
    <button class="nav att" data-route="#/workspaces/${esc(w.id)}" title="${esc(w.name)} · ${esc(w.node)}">
      <span class="att-dot" data-s="${esc(w.activity)}"></span><span class="nm">${esc(w.name)}</span>
      <span class="cnt att-s" data-s="${esc(w.activity)}">${ATT_TEXT[w.activity]}</span></button>`).join('')}${
      list.length > 6 ? `<div class="t-micro faint" style="padding:2px 12px">还有 ${list.length - 6} 个</div>` : ''}` : '';
}

const SOUND_KEY = 'blazar.sound';
const soundPrefs = () => { try { return { on: false, tone: 'soft', volume: 0.5, ...JSON.parse(localStorage.getItem(SOUND_KEY) || '{}') }; } catch (_) { return { on: false, tone: 'soft', volume: 0.5 }; } };
const setSoundPrefs = p => { try { localStorage.setItem(SOUND_KEY, JSON.stringify({ ...soundPrefs(), ...p })); } catch (_) {} };
const TONES = { soft: ['柔和', 'sine', 1], bright: ['清脆', 'triangle', 1.5], wood: ['木质', 'square', 0.5] };
let audioCtx;
function playSound(kind, force) {
  const p = soundPrefs(); if (!p.on && !force) return;
  try {
    audioCtx = audioCtx || new (window.AudioContext || window.webkitAudioContext)();
    if (audioCtx.state === 'suspended') audioCtx.resume();
    const [, wave, mult] = TONES[p.tone] || TONES.soft;
    const notes = kind === 'attention' ? [[880, 0, .09], [880, .13, .09], [1175, .26, .16]] : [[659, 0, .14], [988, .15, .26]];
    const t0 = audioCtx.currentTime + 0.02;
    for (const [f, at, dur] of notes) {
      const o = audioCtx.createOscillator(), g = audioCtx.createGain();
      o.type = wave; o.frequency.value = f * mult;
      const peak = Math.max(0.0001, p.volume * (wave === 'square' ? 0.12 : 0.3));
      g.gain.setValueAtTime(0.0001, t0 + at);
      g.gain.exponentialRampToValueAtTime(peak, t0 + at + 0.012);
      g.gain.exponentialRampToValueAtTime(0.0001, t0 + at + dur);
      o.connect(g).connect(audioCtx.destination);
      o.start(t0 + at); o.stop(t0 + at + dur + 0.03);
    }
  } catch (_) {  }
}
function openNotifyPop(anchor) {
  const p = soundPrefs();
  openDlg(`<h3>提醒方式</h3>
    <label class="row nt-row"><input type="checkbox" id="ntSys" ${notifyOn() ? 'checked' : ''}><span><b>系统通知</b><span class="faint t-caption">跑完、出错、等你裁决时弹一条（窗口在后台也能看到）</span></span></label>
    <label class="row nt-row"><input type="checkbox" id="ntSnd" ${p.on ? 'checked' : ''}><span><b>提示音</b><span class="faint t-caption">跑完一种声音，需要你处理另一种</span></span></label>
    <div class="grid2" style="margin-top:10px">
      <div class="field"><label>音色</label><select id="ntTone" class="input">${Object.entries(TONES).map(([k, v]) => `<option value="${k}" ${p.tone === k ? 'selected' : ''}>${v[0]}</option>`).join('')}</select></div>
      <div class="field"><label>音量</label><input id="ntVol" type="range" min="0.05" max="1" step="0.05" value="${p.volume}" style="width:100%"></div>
    </div>
    <div class="row" style="gap:8px"><button class="btn btn-outline btn-xs" id="ntTry1">试听：跑完了</button><button class="btn btn-outline btn-xs" id="ntTry2">试听：需要处理</button></div>
    <div class="dfoot"><button class="btn btn-brand" onclick="closeDlg()">好</button></div>`);
  $('#ntSys').onchange = async e => { if (e.target.checked !== notifyOn()) await toggleNotify(); e.target.checked = notifyOn(); };
  $('#ntSnd').onchange = e => { setSoundPrefs({ on: e.target.checked }); if (e.target.checked) playSound('done', true); renderSidebar(); };
  $('#ntTone').onchange = e => { setSoundPrefs({ tone: e.target.value }); playSound('done', true); };
  $('#ntVol').onchange = e => { setSoundPrefs({ volume: +e.target.value }); playSound('done', true); };
  $('#ntTry1').onclick = () => playSound('done', true);
  $('#ntTry2').onclick = () => playSound('attention', true);
}

async function dlgRules(prefill) {
  let rules = [];
  try { rules = await api('/api/approval-rules'); } catch (e) { toast(e.message); }
  openDlg(`<h3>自动批准</h3>
    <div class="t-caption muted" style="margin-bottom:10px;line-height:1.6">命中规则的操作不再弹卡，直接放行，并在对话里注明「自动批准」。默认一条都没有。<br>
      带 <span class="mono">; & | \` $( > <</span> 的命令永远不会被自动批准 —— <span class="mono">cargo test; rm -rf ~</span> 也是以 <span class="mono">cargo test</span> 开头的。</div>
    <div class="snip-list" style="max-height:220px">${rules.map(r => `<div class="snip-row">
      <label class="row" style="gap:8px;flex:1;min-width:0"><input type="checkbox" data-en="${esc(r.id)}" ${r.enabled ? 'checked' : ''}>
        <span style="min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap"><b class="mono">${esc(r.tool)}</b>${r.pattern ? ` <span class="mono muted">${esc(r.pattern)}</span>` : ''}
        <span class="faint t-micro"> · ${r.workspace_id ? esc(r.workspace_name || '（工作区已删除）') : '全局'} · 命中 ${r.hits} 次</span></span></label>
      <button class="linkbtn danger" data-del="${esc(r.id)}">删除</button></div>`).join('') || '<div class="t-caption faint" style="padding:6px">还没有规则</div>'}</div>
    <div class="grid2" style="margin-top:12px">
      <div class="field"><label>工具（如 Read、Edit、Bash、mcp__blazar__*）</label><input id="rlTool" class="input mono" value="${esc(prefill?.tool || '')}" placeholder="Read"></div>
      <div class="field"><label>路径 / 命令前缀（可空）</label><input id="rlPat" class="input mono" value="${esc(prefill?.pattern || '')}" placeholder="src/  或  cargo test"></div>
    </div>
    <div class="field"><label>范围</label><select id="rlWs" class="input"><option value="">全局（所有工作区）</option>${(S.workspaces || []).map(w => `<option value="${esc(w.id)}" ${prefill?.workspace_id === w.id ? 'selected' : ''}>只在 ${esc(w.name)} · ${esc(w.node)}</option>`).join('')}</select></div>
    <div class="dfoot"><span class="grow"></span><button class="btn btn-outline" onclick="closeDlg()">关闭</button><button class="btn btn-brand" id="rlAdd">添加规则</button></div>`);
  $$('#dlgBody [data-en]').forEach(c => { c.onchange = () => api(`/api/approval-rules/${c.dataset.en}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ enabled: c.checked }) }).catch(e => toast(e.message)); });
  $$('#dlgBody [data-del]').forEach(b => { b.onclick = async () => { await api(`/api/approval-rules/${b.dataset.del}`, { method: 'DELETE' }).catch(e => toast(e.message)); dlgRules(); }; });
  $('#rlAdd').onclick = async () => {
    try {
      await post('/api/approval-rules', { tool: $('#rlTool').value.trim(), pattern: $('#rlPat').value.trim(), workspace_id: $('#rlWs').value || null });
      toast('已添加'); dlgRules();
    } catch (e) { toast(e.message); }
  };
}

async function allowAlways(card) {
  const req = CC.apprReq.get(card.dataset.appr); if (!req) return;
  const tn = toolName(req.tool_name), inp = req.input || {};
  let pattern = '';
  if (typeof inp.command === 'string') {

    const w = inp.command.trim().split(/\s+/);
    pattern = w[1] && !w[1].startsWith('-') ? `${w[0]} ${w[1]}` : w[0];
  }
  const what = pattern ? `${tn} ${pattern}…` : tn;
  if (!await ask(`以后在这个工作区里，「${what}」都自动允许，不再问你？\n\n可以随时在「设置 → 自动批准」里关掉或删除。`, { ok: '以后都允许' })) return;
  try {
    await post('/api/approval-rules', { tool: tn, pattern, workspace_id: S.ws?.id || null });
    decide(card.dataset.appr, true);
  } catch (e) { toast(e.message); }
}

const jput = (u, b, method = 'PUT') => api(u, { method, headers: { 'content-type': 'application/json' }, body: JSON.stringify(b) });
