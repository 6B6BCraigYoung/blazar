const UI_KEY = 'blazar.ui';
const uiPrefs = () => { const d = { theme: 'system', fontSize: 12.5, minimap: true, wordWrap: false, sendKey: 'enter' };
  try { return { ...d, ...JSON.parse(localStorage.getItem(UI_KEY) || '{}') }; } catch (_) { return d; } };
function setUiPrefs(p) { try { localStorage.setItem(UI_KEY, JSON.stringify({ ...uiPrefs(), ...p })); } catch (_) {} applyUiPrefs(); }
const isDark = () => { const t = document.documentElement.dataset.theme; return t ? t === 'dark' : matchMedia('(prefers-color-scheme: dark)').matches; };
function applyUiPrefs() {
  const p = uiPrefs();
  if (p.theme === 'system') delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = p.theme;
  if (window.monaco && monacoReady) { defineMonacoTheme(); monaco.editor.setTheme('blazar'); }
  S.editor?.updateOptions({ fontSize: p.fontSize, minimap: { enabled: !!p.minimap }, wordWrap: p.wordWrap ? 'on' : 'off' });
}
async function loadServerPrefs() { try { S.prefs = await api('/api/settings'); } catch (_) { S.prefs = S.prefs || { git: { branch_prefix: 'blazar/', pr_draft: false, ai_draft: true }, inbox: { muted: [] } }; } return S.prefs; }

const SET_NAV = [
  ['这台设备', [['appearance', '外观'], ['chat', '对话与编辑'], ['notify', '提醒'], ['shortcuts', '快捷键']]],
  ['智能体', [['rules', '自动批准'], ['snippets', '片段'], ['hooks', '生命周期钩子']]],
  ['代码', [['git', 'Git 与 PR']]],
  ['连接', [['platforms', 'Skill 平台']]],
  ['关于', [['about', '数据与关于']]],
];
let setSaveTimer;
function setSaved(text = '已保存') { const el = $('#setSaved'); if (!el) return; el.textContent = text; el.dataset.on = 'true'; clearTimeout(setSaveTimer); setSaveTimer = setTimeout(() => { if ($('#setSaved')) $('#setSaved').dataset.on = 'false'; }, 1600); }
const setRow = (label, hint, control) => `<div class="st-row"><div class="st-l"><b>${label}</b>${hint ? `<span>${hint}</span>` : ''}</div><div class="st-c">${control}</div></div>`;
const setToggle = (id, on) => `<label class="st-sw"><input type="checkbox" id="${id}" ${on ? 'checked' : ''}><i></i></label>`;
const setCard = (title, desc, body) => `<section class="st-card"><h3>${title}</h3>${desc ? `<p class="st-desc">${desc}</p>` : ''}${body}</section>`;

async function pageSettings() {
  const sec = new URLSearchParams(S.route.split('?')[1] || '').get('s') || 'appearance';
  $('#page').innerHTML = `
    <div class="phead"><button class="btn btn-ghost btn-icon menu-btn" style="display:none" aria-label="菜单"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M4 7h16M4 12h16M4 17h16"/></svg></button>
      <h1>设置</h1><span class="st-saved" id="setSaved" data-on="false"></span><div class="grow"></div></div>
    <div class="st-wrap"><nav class="st-nav">${SET_NAV.map(([g, items]) => `<div class="st-g">${g}</div>${items.map(([k, t]) =>
      `<a class="st-i" href="#/settings?s=${k}" data-on="${k === sec}">${t}</a>`).join('')}`).join('')}</nav>
      <div class="st-body" id="setBody"><div class="empty">加载中…</div></div></div>`;
  wireHeader();
  const draw = { appearance: setAppearance, chat: setChat, notify: setNotify, shortcuts: setShortcuts, rules: setRules, snippets: setSnippets,
    hooks: setHooks, git: setGit, platforms: setPlatforms, about: setAbout }[sec] || setAppearance;
  if (sec === 'lark') { navigate('#/apps/lark'); return; }
  try { await draw($('#setBody')); } catch (e) { $('#setBody').innerHTML = `<div class="empty">${esc(e.message)}</div>`; }
}
function setAppearance(host) {
  const p = uiPrefs();
  host.innerHTML = setCard('外观', '存在这台设备上。', [
    setRow('主题', '跟随系统时，系统切深浅色这里跟着切', `<div class="seg" id="stTheme">${[['system', '跟随系统'], ['light', '浅色'], ['dark', '深色']].map(([k, t]) => `<button data-v="${k}" data-on="${p.theme === k}">${t}</button>`).join('')}</div>`),
  ].join('')) + setCard('编辑器', '', [
    setRow('字号', '', `<input id="stFont" type="range" min="11" max="18" step="0.5" value="${p.fontSize}" style="width:160px"> <span class="num" id="stFontV">${p.fontSize}</span>`),
    setRow('小地图', '右侧的代码缩略图', setToggle('stMini', p.minimap)),
    setRow('自动换行', '长行折到下一行，而不是横向滚动', setToggle('stWrap', p.wordWrap)),
  ].join(''));
  $$('#stTheme [data-v]').forEach(b => { b.onclick = () => { setUiPrefs({ theme: b.dataset.v }); setSaved(); setAppearance(host); }; });
  $('#stFont').oninput = e => { $('#stFontV').textContent = e.target.value; setUiPrefs({ fontSize: +e.target.value }); setSaved(); };
  $('#stMini').onchange = e => { setUiPrefs({ minimap: e.target.checked }); setSaved(); };
  $('#stWrap').onchange = e => { setUiPrefs({ wordWrap: e.target.checked }); setSaved(); };
}
function setChat(host) {
  const p = uiPrefs(), d = diffPrefs();
  host.innerHTML = setCard('对话', '', [
    setRow('发送键', '另一个组合用来换行', `<select id="stSend" class="input input-sm" style="width:auto"><option value="enter" ${p.sendKey === 'enter' ? 'selected' : ''}>Enter 发送，Shift+Enter 换行</option><option value="mod" ${p.sendKey === 'mod' ? 'selected' : ''}>⌘ / Ctrl + Enter 发送，Enter 换行</option></select>`),
  ].join('')) + setCard('文档与差异', '', [
    setRow('打开 Markdown 时', '', `<div class="seg" id="stMd">${[['preview', '先看预览'], ['source', '先看源码']].map(([k, t]) => `<button data-v="${k}" data-on="${mdMode() === k}">${t}</button>`).join('')}</div>`),
    setRow('差异默认视图', '', `<div class="seg" id="stDiff">${[['unified', '统一'], ['split', '并排']].map(([k, t]) => `<button data-v="${k}" data-on="${d.view === k}">${t}</button>`).join('')}</div>`),
    setRow('差异里忽略空白', '只改了缩进的行不算改动', setToggle('stDiffW', d.w)),
  ].join(''));
  $('#stSend').onchange = e => { setUiPrefs({ sendKey: e.target.value }); setSaved(); };
  $$('#stMd [data-v]').forEach(b => { b.onclick = () => { try { localStorage.setItem(MD_MODE_KEY, b.dataset.v); } catch (_) {} setSaved(); setChat(host); }; });
  $$('#stDiff [data-v]').forEach(b => { b.onclick = () => { setDiffPrefs({ view: b.dataset.v }); setSaved(); setChat(host); }; });
  $('#stDiffW').onchange = e => { setDiffPrefs({ w: e.target.checked }); setSaved(); };
}
async function setNotify(host) {
  const s = soundPrefs(), pf = await loadServerPrefs();
  host.innerHTML = setCard('这台设备上怎么提醒', '', [
    setRow('系统通知', '跑完、出错、等你裁决时弹一条，窗口在后台也看得到', setToggle('stSys', notifyOn())),
    setRow('提示音', '跑完一种声音，需要你处理另一种', setToggle('stSnd', s.on)),
    setRow('音色', '', `<select id="stTone" class="input input-sm" style="width:auto">${Object.entries(TONES).map(([k, v]) => `<option value="${k}" ${s.tone === k ? 'selected' : ''}>${v[0]}</option>`).join('')}</select>
      <button class="btn btn-outline btn-xs" id="stTry1">试听：跑完</button><button class="btn btn-outline btn-xs" id="stTry2">试听：需要处理</button>`),
    setRow('音量', '', `<input id="stVol" type="range" min="0.05" max="1" step="0.05" value="${s.volume}" style="width:160px">`),
  ].join('')) + setCard('收件箱里哪些算未读', '静音的类型照样记进收件箱（回头能查），但不算未读、不响、也不转发到飞书。', Object.entries(IN_KIND).map(([k, v]) =>
    setRow(v[0], '', setToggle('stK-' + k, !pf.inbox.muted.includes(k)))).join('')) +
    setCard('转发到飞书', '', setRow('把提醒发到飞书的群或你自己', '在侧栏「办公 → 飞书 / Lark」里配置', '<a class="btn btn-outline btn-xs" href="#/apps/lark">去配置</a>'));
  $('#stSys').onchange = async e => { if (e.target.checked !== notifyOn()) await toggleNotify(); e.target.checked = notifyOn(); setSaved(); };
  $('#stSnd').onchange = e => { setSoundPrefs({ on: e.target.checked }); if (e.target.checked) playSound('done', true); renderSidebar(); setSaved(); };
  $('#stTone').onchange = e => { setSoundPrefs({ tone: e.target.value }); playSound('done', true); setSaved(); };
  $('#stVol').onchange = e => { setSoundPrefs({ volume: +e.target.value }); playSound('done', true); setSaved(); };
  $('#stTry1').onclick = () => playSound('done', true); $('#stTry2').onclick = () => playSound('attention', true);
  Object.keys(IN_KIND).forEach(k => { $('#stK-' + k).onchange = async () => {
    const muted = Object.keys(IN_KIND).filter(x => !$('#stK-' + x).checked);
    try { S.prefs = await jput('/api/settings', { inbox: { muted } }); setSaved(); } catch (e) { toast(e.message); }
  }; });
}
function setShortcuts(host) {
  const map = shortcutMap();
  host.innerHTML = setCard('快捷键', '点「改键」后按下新的组合。存在这台设备上；不带 ⌘ / Ctrl 的组合在输入框里不生效。',
    `<input id="stScQ" class="input input-sm" placeholder="搜索动作…" style="max-width:260px;margin-bottom:8px">
     <div id="stScList">${SHORTCUTS.map(s => `<div class="st-row st-sc" data-t="${esc(s.label)}"><div class="st-l"><b>${esc(s.label)}</b></div>
       <div class="st-c"><kbd class="sc-kbd" data-k="${s.id}">${esc(comboText(map[s.id]))}</kbd><button class="linkbtn" data-rec="${s.id}">改键</button><button class="linkbtn" data-clr="${s.id}">清除</button></div></div>`).join('')}</div>
     <div class="row" style="margin-top:10px"><button class="btn btn-outline btn-xs" id="stScReset">全部恢复默认</button></div>`);
  const save = (id, combo) => { let v = {}; try { v = JSON.parse(localStorage.getItem(SHORTCUT_KEY) || '{}'); } catch (_) {} v[id] = combo; try { localStorage.setItem(SHORTCUT_KEY, JSON.stringify(v)); } catch (_) {} setSaved(); };
  const stop = () => { S.recordingShortcut = null; document.removeEventListener('keydown', onKey, true); };
  const onKey = e => {
    const id = S.recordingShortcut; if (!id) return;
    e.preventDefault(); e.stopPropagation();
    if (e.key === 'Escape') { stop(); setShortcuts(host); return; }
    const combo = comboOf(e); if (!combo) return;
    const clash = SHORTCUTS.find(s => s.id !== id && shortcutMap()[s.id] === combo);
    if (clash) { toast(`${comboText(combo)} 已经是「${clash.label}」了`); return; }
    if (!combo.includes('+') && !/^F\d/.test(combo)) { toast('单个字母会和打字冲突，加上 ⌘ / Ctrl / ⌥'); return; }
    save(id, combo); stop(); setShortcuts(host);
  };
  $('#stScQ').oninput = e => { const q = e.target.value.trim().toLowerCase(); $$('#stScList .st-sc').forEach(r => { r.hidden = !!q && !r.dataset.t.toLowerCase().includes(q); }); };
  host.querySelectorAll('[data-rec]').forEach(b => { b.onclick = () => { stop(); S.recordingShortcut = b.dataset.rec; host.querySelector(`[data-k="${b.dataset.rec}"]`).textContent = '按下新的组合…（Esc 取消）'; document.addEventListener('keydown', onKey, true); }; });
  host.querySelectorAll('[data-clr]').forEach(b => { b.onclick = () => { save(b.dataset.clr, ''); setShortcuts(host); }; });
  $('#stScReset').onclick = () => { try { localStorage.removeItem(SHORTCUT_KEY); } catch (_) {} setSaved('已恢复默认'); setShortcuts(host); };
}
async function setRules(host) {
  const rules = await api('/api/approval-rules');
  host.innerHTML = setCard('自动批准规则', '命中规则的操作不再弹卡，直接放行，并在对话里注明「自动批准」。默认一条都没有。带 <span class="mono">; & | ` $( > <</span> 的命令永远不会被自动批准。',
    `<div class="st-list">${rules.map(r => `<div class="st-li"><label class="st-sw"><input type="checkbox" data-en="${esc(r.id)}" ${r.enabled ? 'checked' : ''}><i></i></label>
      <span class="st-li-m"><b class="mono">${esc(r.tool)}</b>${r.pattern ? ` <span class="mono muted">${esc(r.pattern)}</span>` : ''}<span class="faint t-micro"> · ${r.workspace_id ? esc(r.workspace_name || '（工作区已删除）') : '全局'} · 命中 ${r.hits} 次</span></span>
      <button class="linkbtn danger" data-del="${esc(r.id)}">删除</button></div>`).join('') || '<div class="t-caption faint">还没有规则。审批卡上点「这个工作区里以后都允许」也会在这里加一条。</div>'}</div>
     <div class="st-add"><input id="rlTool" class="input input-sm mono" placeholder="工具：Read / Edit / Bash / mcp__x__*"><input id="rlPat" class="input input-sm mono" placeholder="路径或命令前缀（可空）">
       <select id="rlWs" class="input input-sm"><option value="">全局</option>${(S.workspaces || []).map(w => `<option value="${esc(w.id)}">只在 ${esc(w.name)}</option>`).join('')}</select>
       <button class="btn btn-brand btn-xs" id="rlAdd">添加</button></div>`);
  host.querySelectorAll('[data-en]').forEach(c => { c.onchange = () => jput(`/api/approval-rules/${c.dataset.en}`, { enabled: c.checked }).then(() => setSaved()).catch(e => toast(e.message)); });
  host.querySelectorAll('[data-del]').forEach(b => { b.onclick = async () => { await api(`/api/approval-rules/${b.dataset.del}`, { method: 'DELETE' }).catch(e => toast(e.message)); setSaved('已删除'); setRules(host); }; });
  $('#rlAdd').onclick = async () => { try { await post('/api/approval-rules', { tool: $('#rlTool').value.trim(), pattern: $('#rlPat').value.trim(), workspace_id: $('#rlWs').value || null }); setSaved('已添加'); setRules(host); } catch (e) { toast(e.message); } };
}
async function setSnippets(host) {
  const list = await loadSnippets();
  host.innerHTML = setCard('片段', '反复要说的话存一份：验收清单、代码规范、提交前检查……在输入框里敲 <b class="mono">@</b> 选名字，就地展开成全文。',
    `<div class="st-list">${list.map(x => `<div class="st-li"><span class="st-li-m"><b class="mono">@${esc(x.name)}</b> <span class="faint">${esc(shortStr(x.body.replace(/\s+/g, ' '), 90))}</span></span>
      <button class="linkbtn" data-e="${esc(x.id)}">编辑</button><button class="linkbtn danger" data-d="${esc(x.id)}">删除</button></div>`).join('') || '<div class="t-caption faint">还没有片段</div>'}</div>
     <div class="row" style="margin-top:10px"><button class="btn btn-brand btn-xs" id="snAdd">新建片段</button></div>`);
  const reopen = () => { const t = setInterval(() => { if ($('#dlg').dataset.open !== 'true') { clearInterval(t); if ($('#setBody')) setSnippets(host); } }, 400); };
  $('#snAdd').onclick = () => { dlgSnippets(); reopen(); };
  host.querySelectorAll('[data-e]').forEach(b => { b.onclick = () => { dlgSnippets(b.dataset.e); reopen(); }; });
  host.querySelectorAll('[data-d]').forEach(b => { b.onclick = async () => { await api(`/api/snippets/${b.dataset.d}`, { method: 'DELETE' }).catch(e => toast(e.message)); setSaved('已删除'); setSnippets(host); }; });
}
async function setHooks(host) {
  const hooks = await api('/api/hooks');
  host.innerHTML = setCard('生命周期钩子', '在 agent 的关键时点执行脚本。钩子带目标机器：发通知在中心跑，编译检查在干活那台机器上跑。「一轮开始前」的钩子退出码非 0 可以否决本次执行。可用变量：BLAZAR_EVENT · BLAZAR_WORKSPACE · BLAZAR_NODE · BLAZAR_CWD · BLAZAR_SESSION · BLAZAR_TOOL',
    `<div class="st-list">${hooks.map(h => `<div class="st-li"><span class="gchip">${esc({ turn_start: '一轮开始前', turn_end: '一轮结束', session_start: '会话建立', session_end: '会话结束', error: '出错' }[h.event] || h.event)}</span>
      <span class="st-li-m"><span class="mono">${esc(h.command)}</span><span class="faint t-micro"> · ${h.target === 'node' ? '干活那台机器' : '中心'}${h.blocking ? ' · 失败则否决' : ''}</span></span>
      <button class="linkbtn danger" data-h="${esc(h.id)}">删除</button></div>`).join('') || '<div class="t-caption faint">还没有钩子</div>'}</div>
     <div class="st-add"><select id="hEvent" class="input input-sm"><option value="turn_start">一轮开始前（可否决）</option><option value="turn_end">一轮结束</option><option value="session_start">会话建立</option><option value="session_end">会话结束</option><option value="error">出错</option></select>
       <select id="hTarget" class="input input-sm"><option value="hub">中心</option><option value="node">干活那台机器</option></select>
       <input id="hCmd" class="input input-sm mono" style="flex:1;min-width:220px" placeholder="cd $BLAZAR_CWD && cargo check">
       <input id="hTimeout" class="input input-sm" type="number" min="1" max="600" value="30" style="width:70px" title="超时（秒）">
       <label class="row t-caption" style="gap:4px"><input type="checkbox" id="hBlocking">失败则否决</label><button class="btn btn-brand btn-xs" id="hAdd">添加</button></div>`);
  host.querySelectorAll('[data-h]').forEach(b => { b.onclick = async () => { await api(`/api/hooks/${b.dataset.h}`, { method: 'DELETE' }).catch(e => toast(e.message)); setSaved('已删除'); setHooks(host); }; });
  $('#hAdd').onclick = async () => {
    const cmd = $('#hCmd').value.trim(); if (!cmd) { toast('请填写命令'); return; }
    try { await post('/api/hooks', { event: $('#hEvent').value, command: cmd, target: $('#hTarget').value, blocking: $('#hBlocking').checked, timeout_secs: +$('#hTimeout').value || 30 }); setSaved('已添加'); setHooks(host); }
    catch (e) { toast('添加失败: ' + e.message); }
  };
}
async function setGit(host) {
  const g = (await loadServerPrefs()).git;
  host.innerHTML = setCard('隔离工作区', '', setRow('分支前缀', `新建隔离工作区时的分支名：<span class="mono" id="stBpEx">${esc(g.branch_prefix)}fix-login</span>`,
    `<input id="stBp" class="input input-sm mono" style="width:180px" value="${esc(g.branch_prefix)}" placeholder="blazar/">`)) +
    setCard('Pull Request 与提交', '', [
      setRow('PR 默认建成草稿', '开 PR 的对话框里那个勾默认勾上', setToggle('stDraft', g.pr_draft)),
      setRow('AI 起草', '提交信息和 PR 描述旁边的「AI 起草」按钮（用本机 Claude 的 Haiku，看一眼 diff 替你写）', setToggle('stAi', g.ai_draft)),
    ].join(''));
  const put = async patch => { try { S.prefs = await jput('/api/settings', { git: patch }); setSaved(); } catch (e) { toast(e.message); } };
  let t; $('#stBp').oninput = e => { $('#stBpEx').textContent = e.target.value + 'fix-login'; clearTimeout(t); t = setTimeout(() => put({ branch_prefix: e.target.value.trim() }), 600); };
  $('#stDraft').onchange = e => put({ pr_draft: e.target.checked });
  $('#stAi').onchange = e => put({ ai_draft: e.target.checked });
}
async function setLark(host) {
  const L = await api('/api/office/lark'), s = L.settings, au = L.auth || {};
  const idChip = (k, label) => { const i = au[k] || {}; return `<span class="gchip ${i.available ? 'ok' : 'warn'}" title="${esc(i.message || '')}">${label}：${i.available ? '可用' : '不可用'}</span>`; };
  const copyBtn = cmd => `<code class="st-cmd">${esc(cmd)}</code><button class="linkbtn" data-copy="${esc(cmd)}">复制</button>`;
  const authed = !!(au.user?.available || au.bot?.available), configured = !!L.configured || authed;
  const later = '<span class="faint t-caption">先完成上一步</span>';
  const scopeCtl = label => `<select id="cnScope" class="input input-sm" style="width:auto"><option value="recommend">推荐权限</option><option value="all">全部权限</option><option value="domains">自选业务域…</option></select> <button class="btn btn-brand btn-sm" id="cnLogin">${label}</button>`;
  host.innerHTML = setCard('连接', '经飞书官方的命令行 <b class="mono">lark-cli</b> 接入：消息、文档、多维表格、表格、日历、邮件、任务、会议纪要、知识库。三步都可以在这里点完：要你确认的部分在浏览器里做，令牌由 lark-cli 自己存进系统钥匙串 —— 不经过 Blazar，账号信息也不进这里。',
    cnRow(1, L.installed ? 'done' : 'now', '安装 lark-cli', L.installed ? '' : '从 npm 下载官方的 <span class="mono">@larksuite/cli</span>，顺带装好教智能体用飞书的技能包',
      L.installed ? `<span class="gchip ok">已安装 · ${esc(L.version)}</span>` : L.can_install ? '<button class="btn btn-brand btn-sm" id="cnInstall">安装</button>' : '<span class="t-caption muted">需要先装 Node.js</span> <a class="linkbtn" href="https://nodejs.org" target="_blank" rel="noopener">nodejs.org</a>') +
    cnRow(2, !L.installed ? 'later' : configured ? 'done' : 'now', '创建飞书应用', configured ? '' : '在开放平台建一个属于你自己的应用，lark-cli 用它的身份调接口。点了以后去浏览器里确认，几下就好',
      !L.installed ? later : configured ? `<span class="gchip ok">已配置${au.brand ? ' · ' + (au.brand === 'lark' ? 'Lark' : '飞书') : ''}</span> <button class="linkbtn" id="cnReinit">重新创建…</button>`
        : '<div class="seg" id="cnBrand"><button data-v="feishu" data-on="true">飞书</button><button data-v="lark" data-on="false">Lark</button></div> <button class="btn btn-brand btn-sm" id="cnInit">创建应用</button>') +
    cnRow(3, !L.installed || !configured ? 'later' : authed ? 'done' : 'now', '登录授权', authed ? '' : '用你自己的飞书账号授权。权限以后随时可以再加',
      !L.installed || !configured ? later : authed ? `${idChip('user', '用户身份')} ${idChip('bot', '机器人身份')} <button class="linkbtn" id="cnMore">加权限…</button> <button class="linkbtn" id="cnLogout">退出登录</button>` : scopeCtl('登录')) +
    (authed ? `<div class="st-row" id="cnMoreRow" hidden><div class="st-l"><b>再授权一次</b><span>新选的权限会加到现有的授权上</span></div><div class="st-c">${scopeCtl('去授权')}</div></div>` : '') +
    `<div class="st-kinds cn-domains" id="cnDomains" hidden>${LARK_DOMAINS.map(([k, t]) => `<label><input type="checkbox" data-domain="${k}"> ${t}</label>`).join('')}</div>` +
    '<div id="cnJob"></div>' +
    `<details class="cn-term"><summary>也可以在终端里做</summary>${setRow('安装', '', copyBtn(L.install))}${setRow('绑定已有的应用', '要填 App ID 和 App Secret —— 密钥这种东西 Blazar 不经手，所以这条只能在终端里做', copyBtn(L.init || 'lark-cli config init'))}${setRow('登录', '', copyBtn('lark-cli auth login --recommend'))}</details>`) +
  (L.installed ? setCard('让智能体用飞书', '打开后，每个智能体都会多一个本机工具 <b class="mono">lark</b>。飞书操作始终在你这台机器上执行，远端工作区的智能体也一样能用。执行前先看那条命令自己标的风险级别。', [
    setRow('权限', '高风险写操作（删除、移除成员、批量变更）和登录 / 配置类命令，任何一档都不放', `<div class="seg" id="lkAccess">${[['off', '关'], ['read', '只读'], ['write', '读写']].map(([k, t]) => `<button data-v="${k}" data-on="${s.agent_access === k}">${t}</button>`).join('')}</div>`),
    setRow('飞书技能包', `教智能体怎么用 lark-cli 的 ${L.skills_local} 个技能（本机 ~/.claude/skills/lark-*）。Claude 在本机会自动加载；导进 SKILLs 后可以挂给任意智能体`, `<span class="faint t-caption">已导入 ${L.skills_imported} 个</span> <button class="btn btn-outline btn-xs" id="lkSkills" ${L.skills_local ? '' : 'disabled'}>${L.skills_imported ? '重新导入' : '导入 SKILLs'}</button>`),
  ].join('')) + setCard('把提醒转发到飞书', '收件箱里新来的提醒同时发一条飞书消息。内容是标题加一小段正文，可能带着工作区名和 agent 回复的开头。', [
    setRow('转发', '', setToggle('lkOn', s.notify.enabled)),
    setRow('发给谁', '群的 chat_id（oc_ 开头），或你自己的 open_id（ou_ 开头）', `<input id="lkTarget" class="input input-sm mono" style="width:240px" value="${esc(s.notify.target)}" placeholder="oc_xxxxxxxx">`),
    setRow('用哪个身份发', '机器人要先被拉进那个群', `<select id="lkAs" class="input input-sm" style="width:auto"><option value="">lark-cli 的默认</option><option value="bot" ${s.notify.as === 'bot' ? 'selected' : ''}>机器人</option><option value="user" ${s.notify.as === 'user' ? 'selected' : ''}>我自己</option></select>`),
    setRow('转发哪些', '', `<div class="st-kinds">${Object.entries(IN_KIND).map(([k, v]) => `<label><input type="checkbox" data-kind="${k}" ${s.notify.kinds.includes(k) ? 'checked' : ''}> ${v[0]}</label>`).join('')}</div>`),
    setRow('试一下', '「只预览」不会真的发出去，只看 lark-cli 会发什么请求', '<button class="btn btn-outline btn-xs" id="lkDry">只预览</button> <button class="btn btn-brand btn-xs" id="lkTest">发一条测试消息</button>'),
  ].join('') + '<pre class="st-out" id="lkOut" hidden></pre>') + await larkCallsHtml() : '');
  host.querySelectorAll('[data-copy]').forEach(b => { b.onclick = () => copyText(b.dataset.copy, '已复制，粘到终端里执行'); });
  const redraw = () => setLark(host), on = (id, fn) => { const b = host.querySelector(id); if (b) b.onclick = fn; };
  on('#cnInstall', () => cnStart('lark', { step: 'install' }, host, redraw));
  $$('#cnBrand [data-v]').forEach(b => { b.onclick = () => $$('#cnBrand [data-v]').forEach(x => { x.dataset.on = String(x === b); }); });
  on('#cnInit', () => cnStart('lark', { step: 'init', brand: host.querySelector('#cnBrand [data-on="true"]')?.dataset.v || 'feishu' }, host, redraw));
  on('#cnReinit', async () => { if (await ask('重新创建一个飞书应用？\n\n会换掉这台机器上 lark-cli 现在的应用配置，登录也要重来；原来那个应用还在开放平台上，不会被删。', { ok: '重新创建', danger: true })) cnStart('lark', { step: 'init', brand: au.brand === 'lark' ? 'lark' : 'feishu', force: true }, host, redraw); });
  on('#cnMore', () => { const r = host.querySelector('#cnMoreRow'); r.hidden = !r.hidden; if (r.hidden) host.querySelector('#cnDomains').hidden = true; });
  const scopeSel = host.querySelector('#cnScope');
  if (scopeSel) scopeSel.onchange = () => { host.querySelector('#cnDomains').hidden = scopeSel.value !== 'domains'; };
  on('#cnLogin', () => {
    const domains = [...host.querySelectorAll('[data-domain]:checked')].map(c => c.dataset.domain);
    if (scopeSel.value === 'domains' && !domains.length) { toast('至少选一个业务域'); return; }
    cnStart('lark', { step: 'login', scope: scopeSel.value, domains }, host, redraw);
  });
  on('#cnLogout', async () => { if (await ask('退出这台机器上 lark-cli 的飞书登录？智能体和提醒转发会用不了飞书，直到重新登录。', { ok: '退出登录', danger: true })) cnStart('lark', { step: 'logout' }, host, redraw); });
  cnWatch('lark', host, redraw);
  if (!L.installed) return;
  const put = async patch => { try { await jput('/api/office/lark/settings', patch); setSaved(); return true; } catch (e) { toast(e.message); return false; } };
  $$('#lkAccess [data-v]').forEach(b => { b.onclick = async () => {
    if (b.dataset.v === 'write' && !await ask('改成「读写」后，智能体可以替你发消息、建文档、改日程（高风险操作仍然不放）。\n\n在需要逐步批准的权限模式下，每次调用还是会先弹卡问你；「Bypass permissions」模式下则不会再问。', { ok: '改成读写' })) return;
    if (await put({ agent_access: b.dataset.v })) setLark(host);
  }; });
  $('#lkSkills').onclick = async () => {
    try {
      const found = (await api('/api/skills/import')).filter(f => f.name.startsWith('lark-'));
      const rep = await post('/api/skills/import', { sources: found.map(f => f.source), conflict: 'overwrite' });
      toast(`导入了 ${rep.filter(r => /已导入/.test(r.result)).length} 个飞书技能，到智能体的「能力」页里勾上`); setLark(host);
    } catch (e) { toast(e.message); }
  };
  const notifyPatch = () => ({ notify: { enabled: $('#lkOn').checked, target: $('#lkTarget').value.trim(), as: $('#lkAs').value, kinds: [...host.querySelectorAll('[data-kind]:checked')].map(c => c.dataset.kind) } });
  $('#lkOn').onchange = async e => { if (!await put(notifyPatch())) e.target.checked = !e.target.checked; };
  $('#lkAs').onchange = () => put(notifyPatch()); host.querySelectorAll('[data-kind]').forEach(c => { c.onchange = () => put(notifyPatch()); });
  let t; $('#lkTarget').oninput = () => { clearTimeout(t); t = setTimeout(() => put({ notify: { target: $('#lkTarget').value.trim() } }), 700); };
  const test = async dry => { const out = $('#lkOut'); out.hidden = false; out.textContent = '…';
    try { await put({ notify: { target: $('#lkTarget').value.trim(), as: $('#lkAs').value } }); const r = await post('/api/office/lark/test-notify', { dry_run: dry });
      out.textContent = r.ok ? (dry ? '预览（没有真的发）：\n' + r.detail : '已发出，去飞书里看一眼。') : '没发成：\n' + r.detail; } catch (e) { out.textContent = e.message; } };
  $('#lkDry').onclick = () => test(true); $('#lkTest').onclick = () => test(false);
}
async function setPlatforms(host) {
  const ps = await api('/api/skills/market/providers'), mp = ps.find(p => p.id === 'skillsmp');
  host.innerHTML = setCard('Skill 平台', '在 <a href="#/skills?tab=market">SKILLs → 发现</a> 里搜索和安装。', ps.map(p => setRow(esc(p.label), esc(p.note),
    p.id === 'skillsmp' ? `<span class="gchip ${mp.has_key ? 'ok' : ''}">${mp.has_key ? 'API key 已填' : '匿名'}</span> <button class="btn btn-outline btn-xs" id="stMpKey">${mp.has_key ? '更换 / 删除' : '填 API key'}</button>`
      : `<a class="linkbtn" href="${esc(p.home)}" target="_blank" rel="noopener">打开 ↗</a>`)).join(''));
  $('#stMpKey').onclick = async () => {
    const k = await askText('SkillsMP 的 API key（sk_live_…）。只写不读；留空 = 删掉', '', { ok: '保存' }); if (k == null) return;
    try { await jput('/api/skills/market/key', { provider: 'skillsmp', key: k.trim() }); S.mkProviders = null; setSaved(); setPlatforms(host); } catch (e) { toast(e.message); }
  };
}
async function setAbout(host) {
  const a = await api('/api/about'), c = a.counts;
  host.innerHTML = setCard('Blazar', '', [
    setRow('版本', '', `<span class="mono">${esc(a.version)}</span>`),
    setRow('Hub 地址', 'CLI 和外部脚本连这里（BLAZAR_HUB）', `<span class="mono">${esc(a.hub_url || location.origin)}</span>`),
    setRow('文档', '', '<a class="linkbtn" href="https://github.com/" target="_blank" rel="noopener" hidden></a><span class="faint t-caption">仓库里的 README.md</span>'),
  ].join('')) + setCard('许可证', 'Blazar 自己的代码是 MIT 许可证。随包分发的第三方组件及其许可证如下，完整声明在仓库的 THIRD-PARTY-NOTICES.md。', [
    setRow(`组网引擎：${esc(a.engine.name)} ${esc(a.engine.version)}`, `${esc(a.engine.license)}。随包分发的是上游官方的原版二进制（按 SHA-256 校验），作为独立进程运行，没有链接进 Blazar；许可证全文在安装包的 engine/LICENSE.txt，想换成自己编译的版本替换那两个文件即可`, `<a class="linkbtn" href="https://github.com/EasyTier/EasyTier" target="_blank" rel="noopener">GitHub 主页 ↗</a> <a class="linkbtn" href="${esc(a.engine.source)}" target="_blank" rel="noopener">对应版本的源码</a>`),
    setRow('编辑器与终端', 'Monaco Editor 0.52.2、xterm.js —— MIT', ''),
    setRow('字体', 'IBM Plex Sans、JetBrains Mono —— SIL OFL 1.1', ''),
  ].join('')) + setCard('数据', '都在本机的 SQLite 里，不上传。', [
    setRow('数据库大小', '', `<span class="num">${(a.db_bytes / 1048576).toFixed(1)} MB</span>`),
    setRow('内容', '', `<span class="faint t-caption num">${c.workspaces} 个工作区 · ${c.sessions} 段会话 · ${fmt(c.events)} 条事件 · ${c.tasks} 个任务 · ${c.skills} 个技能 · ${c.inbox} 条提醒</span>`),
  ].join('')) + setCard('这台设备上的偏好', '主题、布局、快捷键、模型选择这些存在浏览器里。', setRow('全部恢复默认', '不影响工作区、对话、任务这些数据', '<button class="btn btn-danger btn-xs" id="stReset">恢复默认</button>'));
  $('#stReset').onclick = async () => {
    if (!await ask('把这台设备上的界面偏好全部恢复默认？（主题、布局、快捷键、提醒方式、各面板的视图选项）', { ok: '恢复默认', danger: true })) return;
    try { Object.keys(localStorage).filter(k => k.startsWith('blazar.') && !k.startsWith('blazar.seen') && !k.startsWith('blazar.review.')).forEach(k => localStorage.removeItem(k)); } catch (_) {}
    applyUiPrefs(); toast('已恢复默认'); setAbout(host);
  };
}
