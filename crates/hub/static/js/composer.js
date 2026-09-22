function fillAgentSel() {
  const sel = $('#agentSel'); if (!sel || !S.ws) return;
  const installed = S.runtimes ? S.runtimes.filter(r => r.installed && r.authed !== false).map(r => r.id) : null;

  const rts = S.agents.filter(a => (!installed || installed.includes(a.id)) && (S.ws.node === 'local' || a.remote_hands));
  const profs = (S.profiles || []);
  if (!profs.length && !rts.length) {
    sel.innerHTML = '<option value="">No runtime available</option>';
    sel.disabled = true;
    $('#btnSend').disabled = true;
    $('#prompt').placeholder = S.ws.node === 'local' ? '本机还没有可用的运行时 —— 去「运行时」页安装或登录'
      : '远端工作区需要本机登录 Claude Code 或 Codex —— 去「运行时」页登录';
    return;
  }
  sel.disabled = false;
  sel.innerHTML = (profs.length ? `<optgroup label="My Agents">${profs.map(p =>
      `<option value="p:${esc(p.id)}">${esc(p.name)} · ${esc(p.runtime_label)}</option>`).join('')}</optgroup>` : '')
    + `<optgroup label="Runtimes">${rts.map(a =>
      `<option value="r:${esc(a.id)}">${esc(a.label)}</option>`).join('')}</optgroup>`
    + '<option value="new">＋ New Agent…</option>';
  const key = 'blazar.agent.' + S.ws.id;
  let saved = null; try { saved = localStorage.getItem(key); } catch (_) {}
  if (saved && [...sel.options].some(o => o.value === saved)) sel.value = saved;
  sel.onchange = () => {
    if (sel.value === 'new') { sel.value = saved || sel.options[0].value; dlgAgent(); return; }
    saved = sel.value;
    try { localStorage.setItem(key, sel.value); } catch (_) {}
    syncModeForRuntime();
    drawModelChip();
    if (!S.viewSession && $('#log .cc-empty')) $('#log').innerHTML = emptyChatHtml();
  };
  drawAgentChip();
}

const EFF_LABEL = { minimal: 'Minimal', low: 'Low', medium: 'Medium', high: 'High', xhigh: 'Extra high', max: 'Max', ultra: 'Ultra' };
S.modelCat = {};
function currentRuntime() {
  const v = $('#agentSel')?.value || '';
  if (v.startsWith('p:')) return (S.profiles || []).find(p => p.id === v.slice(2))?.runtime || 'claude';
  return v.startsWith('r:') ? v.slice(2) : 'claude';
}
function modelSel(rt = currentRuntime()) {
  try { return JSON.parse(localStorage.getItem('blazar.model.' + rt) || '{}'); } catch (_) { return {}; }
}
function setModelSel(v, rt = currentRuntime()) {
  const prev = modelSel(rt);
  try { localStorage.setItem('blazar.model.' + rt, JSON.stringify(v)); } catch (_) {}
  drawModelChip();

  if (wsRunning() && S.session && (v.model || '') !== (prev.model || '')) liveControl({ model: v.model || '' }, '模型');
}
async function liveControl(body, what, sid = S.session) {
  if (!sid) return;
  try {
    const r = await post(`/api/sessions/${sid}/control`, body);
    toast(r.accepted ? `已切换${what}，当前这一轮立即生效` : `${what}将在下一轮生效（${r.reason}）`);
  } catch (e) { toast(`${what}将在下一轮生效`); }
}
async function modelsFor(rt) {
  if (!S.modelCat[rt]) {
    try { S.modelCat[rt] = (await api(`/api/runtimes/${encodeURIComponent(rt)}/models`)).models; }
    catch (_) { S.modelCat[rt] = [{ id: '', label: '默认', desc: '', efforts: [] }]; }
  }
  return S.modelCat[rt];
}

const PREF_KEY = 'blazar.prefs.claude';
function prefs() {
  let v = {}; try { v = JSON.parse(localStorage.getItem(PREF_KEY) || '{}'); } catch (_) {}
  return { thinking: v.thinking !== false, fast: !!v.fast, style: v.style || '' };
}
function setPrefs(patch, live) {
  const v = { ...prefs(), ...patch };
  try { localStorage.setItem(PREF_KEY, JSON.stringify(v)); } catch (_) {}
  drawModelChip();
  if (live && wsRunning() && S.session) liveControl(live.body, live.what);
}

function prefBody() {
  if (currentRuntime() !== 'claude') return {};
  const p = prefs();
  return { thinking: p.thinking ? null : false, fast_mode: p.fast || null, output_style: p.style || null };
}
const FAST_WHY = { extra_usage_disabled: 'extra usage is disabled on this account', model_not_supported: 'not supported by the current model',
  disabled_by_env: 'disabled by environment', cooldown: 'cooling down, resumes automatically' };
async function drawModelChip() {
  const chip = $('#cbModel'); if (!chip) return;
  const rt = currentRuntime(), sel = modelSel(rt);
  const list = await modelsFor(rt);
  const m = list.find(x => x.id === (sel.model || '')) || list[0];
  chip.innerHTML = `${esc((m?.label || 'Default').replace(/\s*\(recommended\)/i, ''))}${sel.effort ? ` <span class="faint">${esc(EFF_LABEL[sel.effort] || sel.effort)}</span>` : ''}`;
  chip.title = chip.textContent.trim();
  if (rt === 'claude') claudeCatalog().then(drawMcpBtn); else drawMcpBtn();
  drawAgentChip();
}
function closePop() { const p = $('#cbPop'); if (p) { p.hidden = true; p.innerHTML = ''; } }
function openPop(html) {
  const p = $('#cbPop'); if (!p) return null;
  p.innerHTML = html; p.hidden = false;
  return p;
}
async function openModelPop() {
  const rt = currentRuntime(), sel = modelSel(rt);
  const list = await modelsFor(rt);
  const cur = list.find(x => x.id === (sel.model || '')) || list[0];
  const draw = () => {
    const s2 = modelSel(rt);
    const m = list.find(x => x.id === (s2.model || '')) || list[0];
    const effs = m?.efforts || [];
    const p = openPop(`<div class="cp-h">Select a model</div>
      ${list.map(x => `<button class="cp-row" data-m="${esc(x.id)}">
        <span class="cp-t"><b>${esc(x.label)}</b>${x.desc ? `<span>${esc(x.desc)}</span>` : ''}</span>
        ${x.id === (s2.model || '') ? '<span class="cp-ok">✓</span>' : ''}</button>`).join('')}
      ${effs.length ? '<div id="mdEff"></div>' : ''}
      `);
    drawEffortInto(p.querySelector('#mdEff'));
    p.querySelectorAll('.cp-row').forEach(b => {
      b.onclick = () => {
        const next = { ...modelSel(rt), model: b.dataset.m };
        const nm = list.find(x => x.id === next.model);
        if (next.effort && nm && !(nm.efforts || []).includes(next.effort)) delete next.effort;
        setModelSel(next, rt); closePop();
      };
    });
  };
  if (!cur) return;
  draw();
}

async function openStylePop() {
  await claudeCatalog();
  const styles = (S.catalog?.output_styles || []).length ? S.catalog.output_styles : ['default', 'Explanatory', 'Learning'];
  const cur = prefs().style || S.catalog?.output_style || 'default';
  const p = openPop(`<div class="cp-sec">Output style</div>${styles.map(x =>
    `<button class="cp-row" data-st="${esc(x)}"><span class="cp-t"><b>${esc(STYLE_LABEL[x] || x)}</b></span>${x === cur ? '<span class="cp-ok">✓</span>' : ''}</button>`).join('')}`);
  p.querySelectorAll('[data-st]').forEach(b => {
    b.onclick = () => { const v = b.dataset.st; setPrefs({ style: v === 'default' ? '' : v }, { body: { output_style: v }, what: '输出风格' }); closePop(); };
  });
}
function toggleThinking() {
  const on = !prefs().thinking;
  if (wsRunning() && S.session) setPrefs({ thinking: on }, { body: { thinking: on }, what: on ? '思考（开）' : '思考（关）' });
  else { setPrefs({ thinking: on }); toast(on ? '思考已开启' : '之后的消息不做扩展思考'); }
}
function toggleFast() {
  const on = !prefs().fast;
  if (wsRunning() && S.session) setPrefs({ fast: on }, { body: { fast_mode: on }, what: on ? '快速模式（开）' : '快速模式（关）' });
  else { setPrefs({ fast: on }); toast(on ? '快速模式已开启' : '快速模式已关闭'); }
}
const STYLE_LABEL = { default: 'Default' };
function bindPrefs(p, redraw) {
  claudeCatalog();
  p.querySelectorAll('.tg[data-p]').forEach(b => {
    b.onclick = () => {
      const on = b.getAttribute('aria-checked') !== 'true';
      if (b.dataset.p === 'thinking') setPrefs({ thinking: on }, { body: { thinking: on }, what: on ? '思考（开）' : '思考（关）' });
      else setPrefs({ fast: on }, { body: { fast_mode: on }, what: on ? '快速模式（开）' : '快速模式（关）' });

      redraw();
    };
  });
  p.querySelectorAll('.cp-style').forEach(b => {
    b.onclick = () => {
      const v = b.dataset.st;
      setPrefs({ style: v === 'default' ? '' : v }, { body: { output_style: v }, what: '输出风格' });
      redraw();
    };
  });
}

const MCP_TEXT = { connected: 'connected', pending: 'connecting', failed: 'failed', 'needs-auth': 'needs auth', disabled: 'disabled' };
function mcpList() {
  if (CC.sess?.mcp_servers?.length) {

    const probe = new Map((S.catalog?.mcp_servers || []).map(m => [m.name, m.status]));
    const list = CC.sess.mcp_servers.map(m => m.status === 'pending' && probe.get(m.name) && probe.get(m.name) !== 'pending'
      ? { ...m, status: probe.get(m.name) } : m);
    return { from: '本会话', list };
  }
  return { from: '本机 Claude', list: S.catalog?.mcp_servers || [] };
}
function drawMcpBtn() {
  const b = $('#cbMcp'); if (!b) return;
  b.hidden = currentRuntime() !== 'claude';
  const { list } = mcpList();
  const bad = list.filter(m => m.status === 'failed').length;
  const ok = list.filter(m => m.status === 'connected').length;
  b.dataset.bad = String(bad > 0);
  b.title = list.length ? `MCP：${ok}/${list.length} 已连接${bad ? `，${bad} 个失败` : ''}` : 'MCP 服务器';
}
async function openMcpPop() {
  await claudeCatalog();
  const { from, list } = mcpList();
  const live = wsRunning() && S.session;
  const order = { failed: 0, 'needs-auth': 1, pending: 2, connected: 3, disabled: 4 };
  const rows = [...list].sort((a, b) => (order[a.status] ?? 5) - (order[b.status] ?? 5) || a.name.localeCompare(b.name));
  const p = openPop(`<div class="cp-h">MCP servers · ${list.filter(m => m.status === 'connected').length}/${list.length} connected</div>
    ${rows.length ? rows.map(m => `<div class="mcp-row"><span class="mcp-dot" data-s="${esc(m.status)}"></span>
      <span class="nm" title="${esc(m.name)}">${esc(m.name)}</span><span class="mcp-st">${esc(MCP_TEXT[m.status] || m.status)}</span>
      ${live && m.status === 'failed' ? `<button class="mcp-act" data-rc="${esc(m.name)}">Reconnect</button>` : ''}
      ${live && (m.status === 'connected' || m.status === 'disabled') ? `<button class="mcp-act" data-tg="${esc(m.name)}" data-en="${m.status === 'disabled'}">${m.status === 'disabled' ? 'Enable' : 'Disable'}</button>` : ''}
      </div>`).join('') : '<div class="t-caption faint" style="padding:8px 10px">No MCP servers</div>'}
    ${TIP('需要登录的服务器请在终端里运行 claude，用 /mcp 完成授权。重连、停用只对正在运行的会话有效。')}`);
  p.querySelectorAll('[data-rc]').forEach(b => { b.onclick = () => { liveControl({ mcp_reconnect: b.dataset.rc }, `重连 ${b.dataset.rc}`); closePop(); }; });
  p.querySelectorAll('[data-tg]').forEach(b => {
    b.onclick = () => {
      const en = b.dataset.en === 'true';
      liveControl({ mcp_toggle: { name: b.dataset.tg, enabled: en } }, `${en ? '启用' : '停用'} ${b.dataset.tg}`);
      const m = CC.sess?.mcp_servers?.find(x => x.name === b.dataset.tg);
      if (m) m.status = en ? 'pending' : 'disabled';
      closePop(); drawMcpBtn();
    };
  });
}

async function drawEffortInto(host) {
  if (!host) return;
  const rt = currentRuntime();
  const list = await modelsFor(rt);
  const draw = () => {
    const s2 = modelSel(rt);
    const m = list.find(x => x.id === (s2.model || '')) || list[0];
    const effs = m?.efforts || [];
    if (!effs.length) { host.innerHTML = ''; return; }
    host.innerHTML = `<div class="cp-eff"><span>Effort <span class="faint">(${esc(s2.effort ? EFF_LABEL[s2.effort] || s2.effort : 'Default')})</span></span>
      <span class="eff-track">${effs.map((e, i) => `<button class="eff-dot" data-e="${esc(e)}" title="${esc(EFF_LABEL[e] || e)}"
        data-on="${s2.effort ? effs.indexOf(s2.effort) >= i : false}" data-cur="${s2.effort === e}"></button>`).join('')}</span></div>`;
    host.querySelectorAll('.eff-dot').forEach(b => {
      b.onclick = e => {
        e.stopPropagation();
        const v = modelSel(rt).effort === b.dataset.e ? undefined : b.dataset.e;
        setModelSel({ ...modelSel(rt), effort: v }, rt); draw();
      };
    });
  };
  draw();
}

function drawAgentChip() {
  const el = $('#cbAgent'), sel = $('#agentSel');
  if (!el || !sel) return;
  const name = sel.selectedOptions[0]?.textContent || '选择 Agent';
  el.innerHTML = `${rtIcon(currentRuntime())}<span>${esc(name)}</span>`;
  el.title = name;
}
function openAgentPop() {
  const sel = $('#agentSel');
  const groups = [...sel.children];
  let html = '';
  for (const g of groups) {
    if (g.tagName === 'OPTGROUP') {
      html += `<div class="cp-sec">${esc(g.label)}</div>` + [...g.children].map(o => {
        const rt = o.value.startsWith('r:') ? o.value.slice(2) : (S.profiles || []).find(p => p.id === o.value.slice(2))?.runtime;
        return `<button class="cp-row" data-av="${esc(o.value)}">${rtMark(rt, 'sm')}<span class="cp-t"><b>${esc(o.textContent)}</b></span>${o.value === sel.value ? '<span class="cp-ok">✓</span>' : ''}</button>`;
      }).join('');
    } else if (g.value === 'new') {
      html += `<button class="cp-row" data-av="new"><span class="cp-t"><b>＋ New Agent…</b></span></button>`;
    }
  }
  if (!html) html = `<div class="t-caption faint" style="padding:8px 10px">本机还没有可用的运行时。去「运行时」页登录 Claude Code 或 Codex${
    S.ws?.node !== 'local' ? '（远端工作区只能由这两个驱动）' : ''}。</div>`;
  const p = openPop(html);
  p.querySelectorAll('[data-av]').forEach(b => { b.onclick = () => { closePop(); sel.value = b.dataset.av; sel.onchange?.(); drawAgentChip(); }; });
}

function openPlusPop() {
  const p = openPop(`<button class="cp-row" data-pl="up">${svgI('up', 'cp-ico')}<span class="cp-t"><b>Upload from computer</b></span><span class="cp-r">or paste / drop</span></button>
    <button class="cp-row" data-pl="ref">${svgI('file', 'cp-ico')}<span class="cp-t"><b>Add context</b></span><span class="cp-r">@</span></button>
    ${S.file ? `<button class="cp-row" data-pl="ctx">${svgI('file', 'cp-ico')}<span class="cp-t"><b>${ctxFile() ? 'Remove current file' : 'Add current file'}</b><span>${esc(S.file)}</span></span></button>` : ''}`);
  p.querySelector('[data-pl="up"]').onclick = () => { closePop(); $('#cbFile').click(); };
  p.querySelector('[data-pl="ref"]').onclick = () => openFilePop();
  const c = p.querySelector('[data-pl="ctx"]');
  if (c) c.onclick = () => { S.ctxOff = ctxFile() ? S.file : null; drawCtxChip(); closePop(); };
}

const ctxFile = () => (S.file && S.ctxOff !== S.file ? S.file : null);
function drawCtxChip() {
  const c = $('#cbCtx'), d = $('#cbDiv'); if (!c) return;
  const f = ctxFile();
  c.hidden = !f; if (d) d.hidden = !f;
  if (f) { c.querySelector('.nm').textContent = f.split('/').pop(); c.title = `Current file: ${f}`; }
}

const RATE_KEY = 'blazar.rate';
function saveRate(windows) {
  if (!Array.isArray(windows) || !windows.length) return;
  S.rate = windows;
  try { localStorage.setItem(RATE_KEY, JSON.stringify({ at: Date.now(), windows })); } catch (_) {}
  drawRateBanner();
}
function loadRate() {
  if (S.rate) return S.rate;
  try { S.rate = JSON.parse(localStorage.getItem(RATE_KEY) || '{}').windows || []; } catch (_) { S.rate = []; }
  return S.rate;
}
const WIN_LABEL = { five_hour: 'Session (5hr)', seven_day: 'Weekly (7 day)', seven_day_opus: 'Weekly Opus', seven_day_sonnet: 'Weekly Sonnet' };
const WIN_SHORT = { five_hour: 'session', seven_day: 'weekly', seven_day_opus: 'weekly Opus', seven_day_sonnet: 'weekly Sonnet' };
const resetIn = t => {
  if (!t) return '';
  const ms = new Date(t) - Date.now(); if (!(ms > 0)) return 'reset';
  const h = Math.floor(ms / 3600e3), m = Math.floor(ms % 3600e3 / 60e3);
  return `resets in ${h >= 24 ? `${Math.floor(h / 24)}d` : h ? `${h}h` : `${m}m`}`;
};
function drawRateBanner() {
  const el = $('#cbRate'); if (!el) return;
  const hot = loadRate().filter(w => w.utilization >= 0.8 && !(w.resets_at && new Date(w.resets_at) < Date.now()))
    .sort((a, b) => b.utilization - a.utilization)[0];
  let off = ''; try { off = localStorage.getItem(RATE_KEY + '.off') || ''; } catch (_) {}
  const key = hot ? `${hot.name}:${hot.resets_at || ''}` : '';
  if (!hot || off === key) { el.hidden = true; return; }
  el.hidden = false;
  el.innerHTML = `<span>You've used ${(hot.utilization * 100).toFixed(0)}% of your ${esc(WIN_SHORT[hot.name] || hot.name)} limit${hot.resets_at ? ` · ${resetIn(hot.resets_at)}` : ''} · <a data-use>View usage</a></span><button class="x" aria-label="Dismiss">×</button>`;
  el.querySelector('[data-use]').onclick = openUsage;
  el.querySelector('.x').onclick = () => { try { localStorage.setItem(RATE_KEY + '.off', key); } catch (_) {} el.hidden = true; };
}
function openUsage() {
  const ws = loadRate();
  openDlg(`<h3>Usage</h3>
    <div>
      ${ws.length ? ws.map(w => {
        const pct = Math.round(w.utilization * 100);
        return `<div class="use-row"><div class="h"><span>${esc(WIN_LABEL[w.name] || w.name)}</span><span class="num">${pct}%</span></div>
          <div class="use-bar"><i style="width:${Math.min(100, pct)}%" data-hot="${pct >= 80}"></i></div>
          <div class="t-caption faint">${esc(resetIn(w.resets_at))}</div></div>`;
      }).join('') : '<div class="faint">No usage data yet — it appears after the first Claude turn.</div>'}
      <div class="t-caption faint" style="margin-top:14px">来自最近一次对话里 Claude 报告的额度。${TIP('只统计本机 Claude 登录账号的额度窗口；详细用量在 claude.ai 的设置里看。')}</div>
    </div>
    <div class="dfoot"><button class="btn btn-outline" onclick="closeDlg()">Close</button></div>`);
}

function openRewindPop() {
  const us = [...document.querySelectorAll('#log .cc-user[data-cp]')].reverse();
  if (!us.length) { toast('还没有可回退的消息（工作区不是 git 仓库时没有检查点）'); return; }
  const p = openPop(`<div class="cp-sec">Rewind to before…</div>${us.slice(0, 30).map(u =>
    `<button class="cp-row" data-cp="${esc(u.dataset.cp)}"><span class="cp-t"><b>${esc(shortStr(u.querySelector('.cc-ut').textContent, 80))}</b></span></button>`).join('')}`);
  p.querySelectorAll('[data-cp]').forEach(b => { b.onclick = () => { closePop(); rewind(b.dataset.cp); }; });
}

let cpSel = 0;
async function openCmdPop() {
  const cat = currentRuntime() === 'claude' ? await claudeCatalog() : { commands: [] };
  const p = openPop(`<input class="input input-sm cp-q" id="palQ" placeholder="Filter actions…"><div id="palL"></div>`);
  const q = $('#palQ'), host = $('#palL');
  const claude = currentRuntime() === 'claude';
  const rt = currentRuntime(), sel = modelSel(rt);
  const models = await modelsFor(rt);
  const mLabel = (models.find(x => x.id === (sel.model || '')) || models[0])?.label || '默认';
  const { list: mcp } = mcpList();
  const items = [
    { sec: 'Context', name: 'Attach file…', run: () => $('#cbFile').click() },
    { sec: 'Context', name: 'Mention file from this project…', run: openFilePop, keep: true },
    { sec: 'Context', name: 'Clear conversation', run: () => setFresh(true) },
    { sec: 'Context', name: 'Rewind', run: openRewindPop, keep: true },
    { sec: 'Model', name: 'Switch model…', right: mLabel, run: openModelPop, keep: true },
    { sec: 'Model', name: 'Effort', ctrl: 'effort' },
    ...(claude ? [
      { sec: 'Model', name: 'Thinking', ctrl: 'tg', on: () => prefs().thinking, run: toggleThinking },
      { sec: 'Model', name: 'Fast mode', ctrl: 'tg', on: () => prefs().fast, run: toggleFast },
      { sec: 'Model', name: 'Output style…', right: STYLE_LABEL[prefs().style || 'default'] || prefs().style, run: openStylePop, keep: true },
    ] : []),
    { sec: 'Session', name: 'Modes…', right: PERM_TEXT[$('#permMode').value] || '', run: openModePop, keep: true },
    { sec: 'Session', name: 'Agent…', right: $('#agentSel').selectedOptions[0]?.textContent || '', run: openAgentPop, keep: true },
    ...(claude ? [{ sec: 'Session', name: 'MCP servers…', right: mcp.length ? `${mcp.filter(m => m.status === 'connected').length}/${mcp.length}` : '', run: openMcpPop, keep: true }] : []),
    { sec: 'Session', name: 'Account & usage…', run: openUsage },
    { sec: 'Session', name: 'Conversations…', run: openHistPop, keep: true },
    ...(wsRunning() ? [{ sec: 'Session', name: 'Interrupt', run: stopSession }] : []),
    { sec: 'Session', name: `Open in ${EDITORS[pickedEditor()].label}`, run: () => openInEditor(pickedEditor()) },
    ...(cat.commands || []).map(c => ({ sec: 'Claude Code', name: '/' + c.name, desc: c.description || '', hint: c.argumentHint || '', cli: c })),
  ];
  const draw = () => {
    const k = q.value.trim().toLowerCase();
    const hits = items.filter(i => !k || i.name.toLowerCase().includes(k) || (i.desc || '').toLowerCase().includes(k));
    cpSel = Math.min(cpSel, Math.max(0, hits.length - 1));
    let last = '';
    host.innerHTML = hits.length ? hits.map((i, n) => {
      const sec = i.sec !== last ? `<div class="cp-sec">${esc(i.sec)}</div>` : ''; last = i.sec;
      if (i.ctrl === 'effort') return `${sec}<div id="palEff" data-n="${n}"></div>`;
      const right = i.ctrl === 'tg' ? `<button class="tg" role="switch" aria-checked="${i.on()}" tabindex="-1"></button>`
        : i.right ? `<span class="cp-r">${esc(i.right)}</span>` : '';
      return `${sec}<button class="cp-row" data-n="${n}" data-sel="${n === cpSel}"><span class="cp-t"><b>${esc(i.name)}${
        i.hint ? ` <span class="faint mono" style="display:inline;font-size:11.5px">${esc(i.hint)}</span>` : ''}</b>${
        i.desc ? `<span>${esc(i.desc)}</span>` : ''}</span>${right}</button>`;
    }).join('') : '<div class="t-caption faint" style="padding:8px 10px">No matching actions</div>';
    drawEffortInto($('#palEff'));
    const pick = n => {
      const i = hits[n]; if (!i || i.ctrl === 'effort') return;
      if (i.cli) { closePop(); const pr = $('#prompt'); pr.value = `${i.name} `; growPrompt(); drawStatus(); pr.focus(); if (!i.hint) send(); return; }
      if (i.ctrl === 'tg') { i.run(); draw(); return; }
      if (!i.keep) closePop();
      i.run();
    };
    host.querySelectorAll('.cp-row[data-n]').forEach(b => { b.onclick = () => pick(+b.dataset.n); });
    host.querySelector('[data-sel="true"]')?.scrollIntoView({ block: 'nearest' });
    q.onkeydown = e => {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        const rows = hits.map((x, n) => n).filter(n => hits[n].ctrl !== 'effort');
        const at = Math.max(0, rows.indexOf(cpSel));
        cpSel = rows[(at + (e.key === 'ArrowDown' ? 1 : -1) + rows.length) % rows.length] ?? 0;
        draw();
      } else if (e.key === 'Enter' && !e.isComposing) { e.preventDefault(); pick(cpSel); }
      else if (e.key === 'Escape') { e.preventDefault(); closePop(); $('#prompt').focus(); }
    };
  };
  cpSel = 0;
  q.oninput = () => { cpSel = 0; draw(); };
  draw(); q.focus();
}

const IMG_TYPES = ['image/png', 'image/jpeg', 'image/gif', 'image/webp'];
function addImage(file) {
  if (!file) return;
  if (!IMG_TYPES.includes(file.type)) { toast('只支持 PNG / JPEG / GIF / WebP'); return; }
  if (file.size > 5 * 1024 * 1024) { toast('单张图片不能超过 5MB'); return; }
  if ((S.attach || []).length >= 8) { toast('一次最多 8 张'); return; }
  const r = new FileReader();
  r.onload = () => {
    const url = String(r.result);
    S.attach.push({ media_type: file.type, data: url.slice(url.indexOf(',') + 1), url });
    drawAttach();
  };
  r.readAsDataURL(file);
}
function drawAttach() {
  const host = $('#cbAtts'); if (!host) return;
  const list = S.attach || [];
  host.hidden = !list.length;
  host.innerHTML = list.map((a, i) => `<span class="cb-att"><img src="${a.url}" alt="">
    <button data-rm="${i}" aria-label="移除">×</button></span>`).join('');
  host.querySelectorAll('[data-rm]').forEach(b => { b.onclick = () => { S.attach.splice(+b.dataset.rm, 1); drawAttach(); }; });
}
function takeImages() {
  const imgs = (S.attach || []).map(a => ({ media_type: a.media_type, data: a.data }));
  return imgs;
}
function setFresh(on) {
  $('#resumeChk').checked = !on;
  const c = $('#cbFresh'); if (c) c.hidden = !on;
}

async function claudeCatalog() {
  if (!S.catalog) {
    try { S.catalog = await api('/api/runtimes/claude/catalog'); } catch (_) { S.catalog = { commands: [] }; }
  }
  return S.catalog;
}
function blazarCommands() {
  return [
    { name: 'new', desc: 'Clear conversation', run: () => setFresh(true) },
    { name: 'model', desc: 'Switch model / effort', run: () => openModelPop() },
    { name: 'modes', desc: effectiveMode()?.[1] || '—', run: () => cycleMode() },
    ...(currentRuntime() === 'claude' ? [
      { name: 'thinking', desc: prefs().thinking ? 'on → off' : 'off → on', run: toggleThinking },
      { name: 'fast', desc: prefs().fast ? 'on → off' : 'off → on', run: toggleFast },
      { name: 'output-style', desc: STYLE_LABEL[prefs().style || 'default'] || prefs().style, run: () => openStylePop() },
      { name: 'mcp', desc: 'MCP servers', run: () => openMcpPop() },
    ] : []),
    ...(wsRunning() ? [{ name: 'interrupt', desc: 'Stop the current run', run: () => stopSession() }] : []),
    { name: 'open', desc: `Open in ${EDITORS[pickedEditor()].label}`, run: () => openInEditor(pickedEditor()) },
  ];
}
let slashSel = 0;
async function openSlashPop() {
  const pr = $('#prompt');
  if (!pr.value.startsWith('/')) { pr.value = '/'; growPrompt(); }
  pr.focus();
  await drawSlash();
}
async function drawSlash() {
  const pr = $('#prompt');
  const m = pr.value.match(/^\/(\S*)$/);
  if (!m) { closePop(); return; }
  const k = m[1].toLowerCase();
  const cli = currentRuntime() === 'claude' ? (await claudeCatalog()).commands || [] : [];
  const items = [
    ...blazarCommands().map(c => ({ ...c, kind: 'blazar' })),
    ...cli.map(c => ({ name: c.name, desc: c.description || '', hint: c.argumentHint || '', kind: 'cli' })),
  ].filter(c => !k || c.name.toLowerCase().includes(k) || c.desc.toLowerCase().includes(k)).slice(0, 60);
  S.slashItems = items;
  slashSel = Math.min(slashSel, Math.max(0, items.length - 1));
  const p = openPop(`<div class="cp-h">命令${cli.length ? ` · ${cli.length} 个来自 Claude Code` : ''}</div>
    ${items.length ? items.map((c, i) => `<button class="cp-row" data-i="${i}" data-sel="${i === slashSel}">
      <span class="cp-t"><b>/${esc(c.name)}${c.hint ? ` <span class="faint mono" style="display:inline;font-size:11.5px">${esc(c.hint)}</span>` : ''}</b>
      ${c.desc ? `<span>${esc(c.desc)}</span>` : ''}</span></button>`).join('')
      : '<div class="t-caption faint" style="padding:8px 10px">没有匹配的命令</div>'}`);
  p.querySelectorAll('.cp-row').forEach(b => { b.onclick = () => runSlash(+b.dataset.i); });
  p.querySelector('[data-sel="true"]')?.scrollIntoView({ block: 'nearest' });
}
function runSlash(i) {
  const c = S.slashItems?.[i]; if (!c) return;
  const pr = $('#prompt');
  closePop();
  if (c.kind === 'blazar') { pr.value = ''; growPrompt(); c.run(); pr.focus(); return; }

  const local = { mcp: openMcpPop, fast: toggleFast,
    'output-style': openStylePop, config: openModelPop, model: openModelPop }[c.name];
  if (local) { pr.value = ''; growPrompt(); local(); return; }

  pr.value = `/${c.name} `; growPrompt(); drawStatus(); pr.focus();
  if (!c.hint) send();
}
function openFilePop() {
  const files = (S.tree || []).filter(e => !e.is_dir).map(e => e.path);
  const p = openPop(`<div class="cp-h">引用文件 / 片段<span class="grow"></span><button class="linkbtn" id="cpSnipMgr">管理片段…</button></div>
    <input class="input input-sm mono cp-q" id="cpQ" placeholder="搜索文件或片段…">
    <div class="cp-list" id="cpList"></div>`);
  const q = $('#cpQ'), host = $('#cpList');
  const draw = () => {
    const k = q.value.toLowerCase();

    const snips = (S.snippets || []).filter(x => !k || x.name.toLowerCase().includes(k) || x.body.toLowerCase().includes(k)).slice(0, 8);
    const hits = files.filter(f => !k || f.toLowerCase().includes(k)).slice(0, 40);
    host.innerHTML = (snips.map(x => `<button class="cp-row cp-snip" data-s="${esc(x.id)}"><span class="cp-t"><b>@${esc(x.name)}</b><span class="faint">${esc(shortStr(x.body.replace(/\s+/g, ' '), 70))}</span></span><span class="badge badge-muted">片段</span></button>`).join('')
      + hits.map(f => `<button class="cp-row cp-file" data-f="${esc(f)}">${esc(f)}</button>`).join(''))
      || '<div class="t-caption faint" style="padding:8px 10px">没有匹配的文件或片段</div>';
    host.querySelectorAll('.cp-file').forEach(b => { b.onclick = () => insertRef(b.dataset.f); });
    host.querySelectorAll('.cp-snip').forEach(b => { b.onclick = () => insertSnippet(b.dataset.s); });
  };
  q.oninput = draw;
  q.onkeydown = e => {
    if (e.key === 'Enter') { e.preventDefault(); host.querySelector('.cp-snip, .cp-file')?.click(); }
    if (e.key === 'Escape') { closePop(); $('#prompt').focus(); }
  };
  $('#cpSnipMgr').onclick = () => { closePop(); dlgSnippets(); };
  draw(); q.focus();

  loadSnippets().then(draw);
}
async function loadSnippets() {
  try { S.snippets = await api('/api/snippets'); } catch (_) { S.snippets = S.snippets || []; }
  return S.snippets;
}

function insertSnippet(id) {
  const x = (S.snippets || []).find(v => v.id === id); if (!x) return;
  const pr = $('#prompt'), at = pr.selectionStart ?? pr.value.length;
  const pre = pr.value.slice(0, at), post = pr.value.slice(at);
  const ins = `${pre && !/\s$/.test(pre) ? '\n' : ''}${x.body}${/^\s/.test(post) || !post ? '' : '\n'}`;
  pr.value = pre + ins + post;
  closePop(); pr.focus();
  pr.selectionStart = pr.selectionEnd = (pre + ins).length;
  pr.dispatchEvent(new Event('input'));
}
async function dlgSnippets(editId) {
  await loadSnippets();
  const cur = (S.snippets || []).find(x => x.id === editId) || null;
  openDlg(`<h3>片段</h3>
    <div class="t-caption muted" style="margin-bottom:10px">反复要说的话存一份：验收清单、代码规范、提交前检查……在输入框里敲 <b class="mono">@</b> 选名字，就地展开成全文。</div>
    <div class="snip-list">${(S.snippets || []).map(x => `<div class="snip-row" data-on="${x.id === editId}"><button class="snip-nm" data-e="${esc(x.id)}"><b>@${esc(x.name)}</b><span class="faint">${esc(shortStr(x.body.replace(/\s+/g, ' '), 60))}</span></button>
      <button class="linkbtn danger" data-d="${esc(x.id)}">删除</button></div>`).join('') || '<div class="t-caption faint">还没有片段</div>'}</div>
    <div class="field" style="margin-top:12px"><label>${cur ? '修改' : '新建'} · 名字（不带空格）</label><input id="snName" class="input mono" value="${esc(cur?.name || '')}" placeholder="验收清单"></div>
    <div class="field"><label>内容</label><textarea id="snBody" class="input" rows="7" placeholder="- 跑一遍测试&#10;- 不要改动公共接口&#10;- 完成后列出改了哪些文件">${esc(cur?.body || '')}</textarea></div>
    <div class="dfoot">${cur ? '<button class="btn btn-outline" id="snNew">改为新建</button>' : ''}<span class="grow"></span>
      <button class="btn btn-outline" onclick="closeDlg()">关闭</button><button class="btn btn-brand" id="snSave">${cur ? '保存' : '添加'}</button></div>`);
  $$('#dlgBody [data-e]').forEach(b => { b.onclick = () => dlgSnippets(b.dataset.e); });
  $$('#dlgBody [data-d]').forEach(b => { b.onclick = async () => {
    try { await api(`/api/snippets/${b.dataset.d}`, { method: 'DELETE' }); } catch (e) { toast(e.message); }
    dlgSnippets(editId === b.dataset.d ? undefined : editId);
  }; });
  if (cur) $('#snNew').onclick = () => dlgSnippets();
  $('#snSave').onclick = async () => {
    const body = { name: $('#snName').value.trim(), body: $('#snBody').value };
    try {
      await api(cur ? `/api/snippets/${cur.id}` : '/api/snippets', { method: cur ? 'PUT' : 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
      toast(cur ? '已保存' : '已添加'); dlgSnippets();
    } catch (e) { toast(e.message); }
  };
}
function insertRef(path) {
  const pr = $('#prompt'), at = pr.selectionStart ?? pr.value.length;
  const pre = pr.value.slice(0, at), post = pr.value.slice(at);
  const ins = `${pre && !/\s$/.test(pre) ? ' ' : ''}@${path} `;
  pr.value = pre + ins + post;
  closePop(); pr.focus();
  pr.selectionStart = pr.selectionEnd = (pre + ins).length;
  pr.dispatchEvent(new Event('input'));
}
document.addEventListener('mousedown', e => {
  if (!e.target.closest?.('#cbPop, #cbModel, #cbSlash, #cbPlus, #cbAgent, #ccMode')) closePop();
});
