const APP_LOGO = {
  lark: '<img class="app-icon" src="/vendor/brands/lark.png" alt="飞书">',

  github: '<svg class="app-icon mono" viewBox="0 0 16 16" aria-label="GitHub"><path fill="currentColor" d="M8 0c4.42 0 8 3.58 8 8a8.013 8.013 0 0 1-5.45 7.59c-.4.08-.55-.17-.55-.38 0-.27.01-1.13.01-2.2 0-.75-.25-1.23-.54-1.48 1.78-.2 3.65-.88 3.65-3.95 0-.88-.31-1.59-.82-2.15.08-.2.36-1.02-.08-2.12 0 0-.67-.22-2.2.82-.64-.18-1.32-.27-2-.27-.68 0-1.36.09-2 .27-1.53-1.03-2.2-.82-2.2-.82-.44 1.1-.16 1.92-.08 2.12-.51.56-.82 1.28-.82 2.15 0 3.06 1.86 3.75 3.64 3.95-.23.2-.44.55-.51 1.07-.46.21-1.61.55-2.33-.66-.15-.24-.6-.83-1.23-.82-.67.01-.27.38.01.53.34.19.73.9.82 1.13.16.45.68 1.31 2.69.94 0 .67.01 1.3.01 1.49 0 .21-.15.45-.55.38A7.995 7.995 0 0 1 0 8c0-4.42 3.58-8 8-8Z"/></svg>',
  obsidian: '<img class="app-icon" src="/vendor/brands/obsidian.png" alt="Obsidian">',
};
const OFFICE_APPS = [
  { id: 'lark', name: '飞书 / Lark', via: 'lark-cli', blurb: '消息、文档、多维表格、表格、日历、邮件、任务、会议纪要、知识库。智能体可以替你查和办；提醒可以发到飞书。' },
  { id: 'github', name: 'GitHub', via: 'gh', blurb: '开 PR、查 PR 状态和检查结果；SKILLs 市场在匿名额度用完时也借它。' },
  { id: 'obsidian', name: 'Obsidian', via: '本机的库文件夹', blurb: '笔记库开成工作区，智能体能读写笔记；预览认 [[双链]]；.md 一键在 Obsidian 里打开；任务一键存成笔记。不用装插件。' },
];
async function loadOffice() {
  const [lark, github, obsidian] = await Promise.all([api('/api/office/lark').catch(() => null), api('/api/office/github').catch(() => null), api('/api/office/obsidian').catch(() => null)]);
  S.office = { lark, github, obsidian };
  const n = $('#cnt-apps'); if (n) n.textContent = [lark?.installed && (lark.auth?.user?.available || lark.auth?.bot?.available), github?.authed, obsidian?.installed && obsidian.vaults?.some(v => v.exists)].filter(Boolean).length || '';
  return S.office;
}
function officeState(id) {
  const o = S.office || {};
  if (id === 'lark') { const l = o.lark; if (!l?.installed) return ['未安装', '']; if (!(l.auth?.user?.available || l.auth?.bot?.available)) return [l.configured ? '未登录' : '未配置', 'warn'];
    return [l.settings.agent_access === 'off' ? '已连接' : `已连接 · 智能体${l.settings.agent_access === 'write' ? '读写' : '只读'}`, 'ok']; }
  if (id === 'obsidian') { const ob = o.obsidian; if (!ob?.installed) return ['未安装', '']; const n = (ob.vaults || []).filter(v => v.exists).length; return n ? [`${n} 个库`, 'ok'] : ['没有库', 'warn']; }
  const g = o.github; if (!g?.installed) return ['未安装', '']; return g.authed ? ['已连接', 'ok'] : ['未登录', 'warn'];
}
async function pageApps() {
  $('#page').innerHTML = `
    <div class="phead"><button class="btn btn-ghost btn-icon menu-btn" style="display:none" aria-label="菜单"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M4 7h16M4 12h16M4 17h16"/></svg></button>
      <h1>办公</h1><div class="grow"></div></div>
    <div class="scroll">
      <div class="t-caption muted" style="margin-bottom:12px;line-height:1.7">把你平时干活用的软件接进来，让智能体能替你查和办。用的都是你本机那个软件自己的命令行和登录态 —— Blazar 不保存也不转发任何凭据，账号信息也不进这里。</div>
      <div class="ws-grid" id="appGrid" style="padding:0"><div class="empty">加载中…</div></div></div>`;
  wireHeader();
  await loadOffice();
  const grid = $('#appGrid'); if (!grid) return;
  grid.innerHTML = OFFICE_APPS.map(a => { const [t, tone] = officeState(a.id); return `<a class="app-card" href="#/apps/${a.id}">
    <div class="app-h"><span class="app-logo">${APP_LOGO[a.id]}</span><b>${esc(a.name)}</b><span class="grow"></span><span class="gchip ${tone}">${t}</span></div>
    <div class="app-b">${esc(a.blurb)}</div><div class="app-f">经 <span class="mono">${esc(a.via)}</span></div></a>`; }).join('');
}
async function pageApp(id) {
  const app = OFFICE_APPS.find(a => a.id === id); if (!app) { navigate('#/apps'); return; }
  $('#page').innerHTML = `
    <div class="phead"><button class="btn btn-ghost btn-icon menu-btn" style="display:none" aria-label="菜单"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M4 7h16M4 12h16M4 17h16"/></svg></button>
      <a class="muted" href="#/apps" style="text-decoration:none">办公</a><span class="faint" style="margin:0 8px">/</span><span class="app-logo sm">${APP_LOGO[id]}</span><h1 style="margin-left:6px">${esc(app.name)}</h1>
      <span class="st-saved" id="setSaved" data-on="false"></span><div class="grow"></div></div>
    <div class="st-body" id="setBody" style="padding-top:18px"><div class="empty">加载中…</div></div>`;
  wireHeader();
  try { await ({ lark: setLark, github: setGithub, obsidian: setObsidian }[id])($('#setBody')); } catch (e) { $('#setBody').innerHTML = `<div class="empty">${esc(e.message)}</div>`; }
  loadOffice().catch(() => {});
}

const CN_STEP = { install: '安装', init: '创建应用', login: '登录', logout: '退出登录' };
const LARK_DOMAINS = [['im', '消息'], ['docs', '文档'], ['drive', '云盘'], ['base', '多维表格'], ['sheets', '表格'], ['slides', '幻灯片'], ['calendar', '日历'], ['mail', '邮箱'],
  ['task', '任务'], ['vc', '会议'], ['minutes', '妙记'], ['wiki', '知识库'], ['contact', '通讯录'], ['approval', '审批'], ['okr', 'OKR'], ['attendance', '考勤']];
const cnNum = (n, state) => `<span class="cn-n" data-s="${state}">${state === 'done' ? '✓' : n}</span>`;

const cnRow = (n, state, title, hint, control) => `<div class="st-row cn-row" data-s="${state}"><div class="st-l"><b>${cnNum(n, state)}${title}</b>${hint ? `<span>${hint}</span>` : ''}</div><div class="st-c">${control}</div></div>`;
function cnJobHtml(j) {
  if (!j) return '';
  const name = CN_STEP[j.step] || j.step;
  if (j.status !== 'running') {
    const tone = j.status === 'done' ? 'ok' : j.status === 'cancelled' ? '' : 'bad';
    return `<div class="cn-job" data-s="${j.status}"><div class="cn-head"><span class="gchip ${tone}">${name}</span><span>${esc(j.message)}</span><span class="grow"></span><button class="linkbtn" id="cnHide">收起</button></div>
      ${j.status === 'failed' && j.lines.length ? `<pre class="st-out" style="margin:8px 0 0">${esc(j.lines.join('\n'))}</pre>` : ''}</div>`;
  }
  const wait = j.step === 'install' ? '正在下载并安装，通常一两分钟…' : j.url ? '等你在浏览器里完成，这里会自动继续…' : '正在向服务器要授权链接…';
  return `<div class="cn-job" data-s="running"><div class="cn-head"><svg class="spin" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" aria-hidden="true"><path d="M21 12a9 9 0 1 1-6.2-8.6"/></svg>
      <b>${name}</b><span class="muted">${wait}</span><span class="grow"></span><button class="btn btn-outline btn-xs" id="cnCancel">取消</button></div>
    ${j.url ? `<div class="cn-auth">${j.qr ? `<img class="cn-qr" src="${esc(j.qr)}" alt="授权链接的二维码" width="132" height="132">` : ''}<div class="cn-auth-r">
        ${j.code ? `<div class="t-caption muted">① 先复制这个一次性验证码</div><div class="row" style="gap:8px"><code class="cn-code">${esc(j.code)}</code><button class="linkbtn" data-copy="${esc(j.code)}">复制</button></div><div class="t-caption muted" style="margin-top:6px">② 再打开链接，把它填进去</div>`
          : `<div class="t-caption muted">${j.qr ? '用手机扫码，或者在这台电脑上打开链接：' : '在浏览器里打开这个链接：'}</div>`}
        <div class="row" style="gap:8px;flex-wrap:wrap"><button class="btn btn-brand btn-sm" id="cnOpen">在浏览器里打开</button><button class="linkbtn" data-copy="${esc(j.url)}">复制链接</button></div>
        <div class="cn-url mono">${esc(j.url)}</div>
        ${j.url_trusted ? '' : '<div class="t-caption" style="color:var(--warning)">这个链接的域名不是这个软件官方的，打开前先看一眼。</div>'}</div></div>` : ''}
    ${j.lines.length ? `<pre class="st-out cn-log" style="margin:8px 0 0">${esc(j.lines.slice(-14).join('\n'))}</pre>` : ''}</div>`;
}

function cnWatch(app, host, redraw) {
  const box = host.querySelector('#cnJob'); if (!box) return;
  clearTimeout(cnWatch.t);
  const paint = j => {
    const log = box.querySelector('.cn-log'), stick = log && log.scrollTop + log.clientHeight >= log.scrollHeight - 4;
    box.innerHTML = cnJobHtml(j);
    host.querySelectorAll('#cnInstall,#cnInit,#cnReinit,#cnLogin,#cnMore,#cnLogout').forEach(b => { b.disabled = j?.status === 'running'; });
    const l2 = box.querySelector('.cn-log'); if (l2 && (stick || !log)) l2.scrollTop = l2.scrollHeight;
    box.querySelectorAll('[data-copy]').forEach(b => { b.onclick = () => copyText(b.dataset.copy, '已复制'); });
    const open = box.querySelector('#cnOpen'); if (open) open.onclick = () => window.open(j.url, '_blank', 'noopener');
    const cancel = box.querySelector('#cnCancel'); if (cancel) cancel.onclick = async () => { cancel.disabled = true; try { await post(`/api/office/${app}/job/cancel`, {}); } catch (e) { toast(e.message); } };
    const hide = box.querySelector('#cnHide'); if (hide) hide.onclick = () => { cnWatch.hidden[app] = j.id; box.innerHTML = ''; };
  };
  const tick = async () => {
    if (!document.body.contains(box)) return;
    let j = null; try { j = await api(`/api/office/${app}/job`); } catch (_) {}
    if (!document.body.contains(box)) return;
    if (j && j.status !== 'running' && cnWatch.running[app] === j.id) {
      delete cnWatch.running[app];
      if (j.status === 'done') { cnWatch.hidden[app] = j.id; toast(`${CN_STEP[j.step] || j.step}：${j.message}`); }
      loadOffice().catch(() => {}); redraw(); return;
    }
    if (j && cnWatch.hidden[app] === j.id) j = null;
    paint(j);
    if (j && j.status === 'running') { cnWatch.running[app] = j.id; cnWatch.t = setTimeout(tick, 1100); }
  };
  tick();
}
cnWatch.running = {}; cnWatch.hidden = {};
async function cnStart(app, body, host, redraw) {
  try { await post(`/api/office/${app}/connect`, body); cnWatch(app, host, redraw); }
  catch (e) { toast(e.message); cnWatch(app, host, redraw); }
}
async function setGithub(host) {
  const g = await api('/api/office/github');
  const copyBtn = cmd => `<code class="st-cmd">${esc(cmd)}</code><button class="linkbtn" data-copy="${esc(cmd)}">复制</button>`;
  const later = '<span class="faint t-caption">先完成上一步</span>';
  host.innerHTML = setCard('连接', '经 GitHub 官方命令行 <b class="mono">gh</b>。两步都可以在这里点完：授权在浏览器里做，令牌由 gh 自己存进系统钥匙串 —— 不经过 Blazar；这里只看它登没登录，账号名不读、不显示。',
    cnRow(1, g.installed ? 'done' : 'now', '安装 gh', g.installed ? '' : '用 Homebrew 装（<span class="mono">brew install gh</span>），要几分钟',
      g.installed ? `<span class="gchip ok">已安装 · ${esc(g.version)}</span>` : g.can_install ? '<button class="btn btn-brand btn-sm" id="cnInstall">安装</button>' : '<span class="t-caption muted">这台机器没有 Homebrew</span> <a class="linkbtn" href="https://cli.github.com" target="_blank" rel="noopener">安装说明</a>') +
    cnRow(2, !g.installed ? 'later' : g.authed ? 'done' : 'now', '登录', g.authed ? '' : '设备授权：这里给你一个一次性验证码，去 github.com 填进去',
      !g.installed ? later : g.authed ? '<span class="gchip ok">已登录</span> <button class="linkbtn" id="cnLogout">退出登录</button>' : '<button class="btn btn-brand btn-sm" id="cnLogin">登录</button>') +
    '<div id="cnJob"></div>' +
    `<details class="cn-term"><summary>也可以在终端里做</summary>${setRow('安装', '', copyBtn(g.install))}${setRow('登录', '', copyBtn(g.login))}</details>`) +
    setCard('用在哪', '', [
      setRow('开 Pull Request', '工作区底部的 Git 面板 → 开 PR…（标题和描述可以让 AI 起草）', '<a class="linkbtn" href="#/settings?s=git">PR 默认设置</a>'),
      setRow('PR 状态与检查结果', '开过 PR 的工作区会定期问一次；合并后挂在上面的任务自动完成', ''),
      setRow('SKILLs 市场', 'GitHub 匿名额度（每小时 60 次）用完时，借 gh 的登录态继续列仓库', '<a class="linkbtn" href="#/skills?tab=market">去发现</a>'),
    ].join('')) + '<div class="t-caption faint" style="max-width:820px">工作区在远端机器上时，开 PR 用的是<b>那台机器</b>上的 gh 登录态；这里显示的是本机的。</div>';
  host.querySelectorAll('[data-copy]').forEach(b => { b.onclick = () => copyText(b.dataset.copy, '已复制，粘到终端里执行'); });
  const redraw = () => setGithub(host), on = (id, fn) => { const b = host.querySelector(id); if (b) b.onclick = fn; };
  on('#cnInstall', () => cnStart('github', { step: 'install' }, host, redraw));
  on('#cnLogin', () => cnStart('github', { step: 'login' }, host, redraw));
  on('#cnLogout', async () => { if (await ask('退出这台机器上 gh 的 GitHub 登录？开 PR、查 PR 状态会用不了，直到重新登录。', { ok: '退出登录', danger: true })) cnStart('github', { step: 'logout' }, host, redraw); });
  cnWatch('github', host, redraw);
}

function obsidianVaultOf(ws) {
  if (!ws || ws.node !== 'local') return null;
  const want = (ws.path || '').replace(/\/$/, '');
  return (S.office?.obsidian?.vaults || []).find(v => v.path === want) || null;
}
async function setObsidian(host) {
  const ob = await api('/api/office/obsidian'); S.office = { ...(S.office || {}), obsidian: ob };
  const vaults = ob.vaults || [], live = vaults.filter(v => v.exists);
  const vaultRow = v => setRow(esc(v.name), `<span class="mono">${esc(v.path)}</span>${v.exists ? ` · ${v.notes} 篇笔记` : ' · <span style="color:var(--destructive)">文件夹不在了</span>'}`,
    v.exists ? (v.workspace_id ? `<a class="btn btn-outline btn-xs" href="#/workspaces/${v.workspace_id}">打开工作区</a>` : `<button class="btn btn-brand btn-xs" data-open-vault="${esc(v.path)}">开成工作区</button>`) : '');
  host.innerHTML = setCard('Obsidian', '笔记库就是本机的一个 Markdown 文件夹，所以不需要插件、接口或登录。库的清单来自 Obsidian 自己的记录；Blazar 只数文件，不读笔记。',
    ob.installed
      ? (vaults.length ? vaults.map(vaultRow).join('') : setRow('还没有库', '在 Obsidian 里新建或打开一个库，回来刷新这页', ''))
      : setRow('还没安装', '', '<a class="linkbtn" href="https://obsidian.md" target="_blank" rel="noopener">obsidian.md ↗</a>')) +
  (live.length ? setCard('存到 Obsidian', '任务详情菜单里的「存到 Obsidian」和智能体写的笔记都放这里。', [
    setRow('默认的库', '', `<select id="obVault" class="input input-sm" style="width:auto;max-width:320px">${live.map(v => `<option value="${esc(v.path)}" ${ob.prefs.vault === v.path ? 'selected' : ''}>${esc(v.name)}</option>`).join('')}</select>`),
    setRow('文件夹', '库内的相对路径；不存在会自动建', `<input id="obFolder" class="input input-sm mono" style="width:200px" value="${esc(ob.prefs.folder || '')}" placeholder="Blazar">`),
    setRow('试一下', '写一篇测试笔记到上面的位置', '<button class="btn btn-outline btn-xs" id="obTest">写一篇测试笔记</button>'),
  ].join('') + '<div class="t-caption faint" id="obOut" style="padding-bottom:10px"></div>') : '') +
  setCard('库开成工作区之后', '', [
    setRow('浏览与编辑', '和别的工作区一样：文件树、Monaco、Markdown 预览；预览里 [[双链]] 和 ![[图片]] 按 Obsidian 的规则解析，点了直接跳', ''),
    setRow('在 Obsidian 里打开', '编辑器顶栏多一个按钮，用 obsidian:// 链接唤起应用', ''),
    setRow('智能体', '在这个工作区里对话，智能体就能读写你的笔记 —— 受同一套权限模式约束；不开成工作区它碰不到', ''),
  ].join(''));
  host.querySelectorAll('[data-open-vault]').forEach(b => { b.onclick = async () => {
    b.disabled = true;
    try { const r = await post('/api/workspaces', { node: 'local', path: b.dataset.openVault, name: null, project: null }); await refreshState(); navigate(`#/workspaces/${r.id}`); }
    catch (e) { toast('开不了：' + e.message); b.disabled = false; }
  }; });
  const put = async patch => { try { await jput('/api/office/obsidian/settings', patch); setSaved(); return true; } catch (e) { toast(e.message); return false; } };
  const sel = $('#obVault'); if (sel) { if (!ob.prefs.vault && live.length) put({ vault: live[0].path }); sel.onchange = () => put({ vault: sel.value }); }
  let t; const fo = $('#obFolder'); if (fo) fo.oninput = () => { clearTimeout(t); t = setTimeout(() => put({ folder: fo.value.trim() }), 700); };
  const test = $('#obTest'); if (test) test.onclick = async () => {
    const out = $('#obOut'); out.textContent = '…';
    try { await put({ vault: sel.value, folder: fo.value.trim() });
      const r = await post('/api/office/obsidian/note', { title: 'Blazar 测试笔记', content: `# Blazar 测试笔记\n\n写于 ${new Date().toLocaleString()}。这篇可以删。\n`, overwrite: true });
      out.innerHTML = `已写到 <span class="mono">${esc(r.path)}</span> <a class="linkbtn" href="${esc(r.open)}">在 Obsidian 里打开</a>`; }
    catch (e) { out.textContent = e.message; }
  };
}
const CALL_VERDICT = { allowed: ['执行了', 'ok'], dry_run: ['只预览', 'info'], refused: ['拒绝', 'bad'] };
const CALL_RISK = { read: '读', write: '写', 'high-risk-write': '高风险写', denied: '不可用' };
async function larkCallsHtml() {
  let calls = []; try { calls = await api('/api/office/lark/calls'); } catch (_) {}
  return setCard('智能体的调用记录', '只记命令、不记参数（参数里是消息正文、文档内容）。', calls.length
    ? `<div class="st-list">${calls.map(c => { const [t, tone] = CALL_VERDICT[c.verdict] || [c.verdict, '']; return `<div class="st-li"><span class="gchip ${tone}">${t}</span>
        <span class="st-li-m mono">lark-cli ${esc(c.command)}</span><span class="faint t-micro">${CALL_RISK[c.risk] || esc(c.risk)}${c.exit_code != null && c.exit_code !== 0 ? ` · 退出码 ${c.exit_code}` : ''} · ${ago(c.created_at)}</span></div>`; }).join('')}</div>`
    : '<div class="t-caption faint" style="padding-bottom:10px">还没有。打开上面的权限后，智能体每用一次飞书，这里记一条。</div>');
}
