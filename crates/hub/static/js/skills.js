async function pageLibrary() {
  const t0 = new URLSearchParams(S.route.split('?')[1] || '').get('tab');
  const tab = ['market', 'mcp'].includes(t0) ? t0 : 'skills';
  $('#page').innerHTML = `
    <div class="phead">
      <button class="btn btn-ghost btn-icon menu-btn" style="display:none" aria-label="菜单"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M4 7h16M4 12h16M4 17h16"/></svg></button>
      <h1>SKILLs</h1>
      <div class="seg" style="margin-left:10px"><button data-t="skills" data-on="${tab === 'skills'}">已安装</button><button data-t="market" data-on="${tab === 'market'}">发现</button><button data-t="mcp" data-on="${tab === 'mcp'}">MCP 服务器</button></div>
      <div class="grow"></div>
      ${tab === 'skills' ? '<button class="btn btn-outline btn-sm" id="lbImport">从本机导入…</button><button class="btn btn-brand btn-sm" id="lbNew">新建技能</button>'
        : tab === 'mcp' ? '<button class="btn btn-brand btn-sm" id="lbNew">添加 MCP 服务器</button>' : ''}
    </div>
    <div class="scroll" style="padding:14px var(--gutter)">
      ${tab === 'market' ? '' : `<div class="t-caption muted" style="margin-bottom:10px;max-width:900px">${tab === 'skills'
        ? '技能 = 一个带 SKILL.md 的目录：什么时候用、分几步做、附带哪些脚本和参考资料。存一份在这里，到智能体的「能力」页里勾上才生效；运行前物化到临时目录，不往你自己的 ~/.claude 里写东西。'
        : '存一份在这里，在智能体的「能力」页里逐个启用。密钥（环境变量 / 请求头的值）只写不读：保存后界面上只看得到键名；交给 Claude 时走进程环境变量，命令行里没有明文。'}</div>`}
      <div id="lbBody" class="ap-table"></div></div>`;
  wireHeader();
  $$('#page .phead .seg [data-t]').forEach(b => { b.onclick = () => navigate('#/skills' + (b.dataset.t === 'skills' ? '' : '?tab=' + b.dataset.t)); });
  if (tab === 'skills') { $('#lbNew').onclick = newSkill; $('#lbImport').onclick = dlgSkillImport; await drawSkills(); }
  else if (tab === 'market') await drawMarket();
  else { $('#lbNew').onclick = () => dlgMcp(); await drawMcpLib(); }
}

const MK_KEY = 'blazar.skills.market';
const mkPrefs = () => { try { return { provider: 'skills_sh', q: '', repo: 'anthropics/skills', ...JSON.parse(localStorage.getItem(MK_KEY) || '{}') }; } catch (_) { return { provider: 'skills_sh', q: '', repo: 'anthropics/skills' }; } };
const setMkPrefs = p => { try { localStorage.setItem(MK_KEY, JSON.stringify({ ...mkPrefs(), ...p })); } catch (_) {} };
const PROVIDER_LABEL = { skills_sh: 'skills.sh', skillsmp: 'SkillsMP', clawhub: 'ClawHub', github: 'GitHub' };
const fmtCount = n => n == null ? '' : n >= 1e6 ? (n / 1e6).toFixed(1) + 'M' : n >= 1e3 ? (n / 1e3).toFixed(n >= 1e4 ? 0 : 1) + 'k' : String(n);
async function drawMarket() {
  const host = $('#lbBody'); if (!host) return;
  if (!S.mkProviders) { try { S.mkProviders = await api('/api/skills/market/providers'); } catch (e) { host.innerHTML = `<div class="empty">${esc(e.message)}</div>`; return; } }
  const p = mkPrefs(), cur = S.mkProviders.find(x => x.id === p.provider) || S.mkProviders[0];
  host.className = '';
  host.innerHTML = `
    <div class="mk-bar"><div class="seg" id="mkProv">${S.mkProviders.map(x => `<button data-p="${x.id}" data-on="${x.id === cur.id}">${esc(x.label)}</button>`).join('')}</div>
      ${cur.id === 'github'
        ? `<input id="mkQ" class="input input-sm mono" style="flex:1;max-width:460px" placeholder="owner/repo 或 GitHub 地址（可带子目录、@分支）" value="${esc(p.repo)}" spellcheck="false"><button class="btn btn-brand btn-sm" id="mkGo">列出技能</button>`
        : `<input id="mkQ" class="input input-sm" style="flex:1;max-width:460px" placeholder="搜 ${esc(cur.label)}：pdf、code review、前端设计…" value="${esc(p.q)}">`}
      <span class="grow"></span>
      ${cur.id === 'skillsmp' ? `<button class="linkbtn" id="mkKey">${cur.has_key ? 'API key 已填 · 更换' : '填 API key（每天 500 次）'}</button>` : ''}
      <a class="linkbtn" href="${esc(cur.home)}" target="_blank" rel="noopener">打开 ${esc(cur.label)} ↗</a></div>
    <div class="t-caption muted" style="margin:8px 0 10px">${esc(cur.note)}</div>
    ${cur.featured ? `<div class="mk-chips">${cur.featured.map(r => `<button class="gchip" data-repo="${esc(r)}">${esc(r)}</button>`).join('')}</div>` : ''}
    <div id="mkList" class="ap-table"></div>`;
  $$('#mkProv [data-p]').forEach(b => { b.onclick = () => { setMkPrefs({ provider: b.dataset.p }); drawMarket(); }; });
  const q = $('#mkQ');
  if (cur.id === 'github') {
    const go = () => { setMkPrefs({ repo: q.value.trim() }); loadMarket(); };
    $('#mkGo').onclick = go; q.onkeydown = e => { if (e.key === 'Enter') go(); };
    $$('.mk-chips [data-repo]').forEach(b => { b.onclick = () => { q.value = b.dataset.repo; go(); }; });
  } else {
    let t; q.oninput = () => { clearTimeout(t); t = setTimeout(() => { setMkPrefs({ q: q.value.trim() }); loadMarket(); }, 450); };
  }
  if ($('#mkKey')) $('#mkKey').onclick = async () => {
    const k = await askText('SkillsMP 的 API key（sk_live_…）。只写不读：保存后这里不会再显示它；留空 = 删掉', '', { ok: '保存' });
    if (k == null) return;
    try { await jput('/api/skills/market/key', { provider: 'skillsmp', key: k.trim() }); S.mkProviders = null; toast(k.trim() ? '已保存' : '已删除'); drawMarket(); } catch (e) { toast(e.message); }
  };
  loadMarket();
}
async function loadMarket() {
  const host = $('#mkList'); if (!host) return;
  const p = mkPrefs(), cur = S.mkProviders.find(x => x.id === p.provider) || S.mkProviders[0];
  const term = cur.id === 'github' ? p.repo : p.q;
  if (!term || term.length < 2) { host.innerHTML = `<div class="empty" style="padding:40px 20px">${cur.id === 'github' ? '填一个仓库，或者点上面的官方源' : '输入关键词开始搜'}</div>`; return; }
  host.innerHTML = '<div class="empty" style="padding:40px 20px">加载中…</div>';
  const token = S.mkToken = Math.random();
  let r;
  try { r = await api(cur.id === 'github' ? `/api/skills/market/repo?source=${encodeURIComponent(term)}` : `/api/skills/market/search?provider=${cur.id}&q=${encodeURIComponent(term)}`); }
  catch (e) { if (token === S.mkToken) host.innerHTML = `<div class="empty" style="padding:40px 20px">${esc(e.message)}</div>`; return; }
  if (token !== S.mkToken || !$('#mkList')) return;
  S.mkItems = r.items || [];
  host.innerHTML = S.mkItems.length ? `<div class="ap-row mk-row ap-hd"><span>技能</span><span>说明</span><span>来自</span><span>${esc(cur.metric || '')}</span><span></span></div>` + S.mkItems.map((it, i) => `
    <div class="ap-row mk-row" data-i="${i}" tabindex="0"><span class="mono"><b>${esc(it.name)}</b>${it.title && it.title !== it.name ? ` <span class="faint">${esc(it.title)}</span>` : ''}${it.suspicious ? ' <span class="gchip bad" title="平台把它标成了可疑">可疑</span>' : ''}</span>
      <span class="muted" title="${esc(it.description || '')}">${esc(it.description || (it.path ? it.path : '—'))}</span><span class="muted">${esc(it.by || '')}</span>
      <span class="num">${fmtCount(it.count)}</span>
      <span>${it.installed ? '<span class="gchip ok">已安装</span>' : '<button class="btn btn-outline btn-xs" data-get>查看</button>'}</span></div>`).join('')
      + (r.truncated ? '<div class="t-caption faint" style="padding:8px 14px">仓库太大，只列出了一部分；可以在地址后面带上子目录再列。</div>' : '')
    : `<div class="empty" style="padding:40px 20px">${cur.id === 'github' ? '这个仓库里没找到 SKILL.md' : '没搜到'}</div>`;
  host.querySelectorAll('.mk-row[data-i]').forEach(row => { row.onclick = () => dlgMarketSkill(S.mkItems[+row.dataset.i]); });
}

async function dlgMarketSkill(it, asUpdate) {
  openDlg('<div class="empty" style="padding:40px">正在取这个技能的内容…</div>'); $('#dlgBody').classList.add('wide');
  let p; try { p = await post('/api/skills/market/preview', it.install); } catch (e) { openDlg(`<h3>取不到</h3><div class="ask-msg">${esc(e.message)}</div><div class="dfoot"><button class="btn btn-outline" onclick="closeDlg()">关闭</button></div>`); return; }
  openDlg(`<h3 class="mono">${esc(p.name)} <span class="badge badge-muted">${esc(PROVIDER_LABEL[p.origin.provider] || p.origin.provider)}</span></h3>
    <div class="t-caption muted" style="margin:-8px 0 10px">来自 <a href="${esc(p.origin.url)}" target="_blank" rel="noopener">${esc(p.origin.repo || `${p.origin.owner}/${p.origin.slug}`)}</a> · 版本 <span class="mono">${esc(String(p.version).slice(0, 12))}</span>${it.count != null ? ` · ${fmtCount(it.count)}` : ''}</div>
    <div class="mk-warn"><b>第三方内容。</b>技能是 agent 会照着执行的指令和脚本 —— 装之前把下面看一遍。装进来只是存进你的库，不会自动挂到任何智能体上。
      ${p.warnings.map(w => `<div>· ${esc(w)}</div>`).join('')}</div>
    <div class="sk-ed" style="height:48vh"><div class="sk-files">${p.files.map(f => `<div class="sk-f" style="cursor:default" title="${esc(f.path)}">${f.script ? '⚙ ' : ''}${esc(f.path)} <span class="faint">${f.size > 1024 ? Math.round(f.size / 1024) + 'K' : f.size + 'B'}</span></div>`).join('')}</div>
      <div class="sk-main" style="overflow:auto"><article class="md-body" style="padding:4px 10px 20px;max-width:none">${mdFull(p.skill_md, 'SKILL.md')}</article></div></div>
    <div class="dfoot">${p.exists && !asUpdate ? `<select id="mkConflict" class="input input-sm" style="width:auto"><option value="rename">库里已有同名的：改名装一份</option><option value="overwrite">覆盖库里的（保留它挂在哪些智能体上）</option></select>` : ''}
      <span class="grow"></span><a class="btn btn-outline" href="${esc(it.url || p.origin.url)}" target="_blank" rel="noopener">在网页上看</a>
      <button class="btn btn-outline" onclick="closeDlg()">取消</button><button class="btn btn-brand" id="mkInstall">${asUpdate ? '更新到这一版' : '安装到我的 SKILLs'}</button></div>`);
  $('#dlgBody').classList.add('wide');
  $('#mkInstall').onclick = async () => {
    const b = $('#mkInstall'); b.disabled = true; b.textContent = '安装中…';
    try {
      const r = await post('/api/skills/market/install', { install: it.install, conflict: asUpdate ? 'overwrite' : ($('#mkConflict')?.value || 'skip') });
      closeDlg(); toast(`已${asUpdate ? '更新' : '安装'} ${r.name}（${r.files} 个文件${r.skipped.length ? `，跳过 ${r.skipped.length} 个` : ''}）。到智能体的「能力」页里勾上才会生效`);
      it.installed = true; if ($('#mkList')) loadMarket(); else drawSkills();
    } catch (e) { toast(e.message); b.disabled = false; b.textContent = asUpdate ? '更新到这一版' : '安装到我的 SKILLs'; }
  };
}

function updateSkill(s) {
  const o = s.origin || {};
  const install = o.provider === 'clawhub' ? { kind: 'clawhub', owner: o.owner, slug: o.slug } : { kind: 'github', repo: o.repo, path: o.path, ref: o.ref };
  dlgMarketSkill({ install, url: o.url, name: s.name }, true);
}

async function drawSkills() {
  const host = $('#lbBody'); if (!host) return;
  let list = []; try { list = await api('/api/skills'); } catch (e) { host.innerHTML = `<div class="empty">${esc(e.message)}</div>`; return; }
  S.skills = list;
  host.innerHTML = list.length ? `<div class="ap-row lb-row ap-hd"><span>技能</span><span>说明</span><span>文件</span><span>用在</span><span></span></div>` + list.map(s => `
    <div class="ap-row lb-row" data-id="${esc(s.id)}" tabindex="0"><span class="mono"><b>${esc(s.name)}</b>${s.origin ? ` <span class="badge badge-muted" title="${esc(s.origin.url || '')} @ ${esc(s.version || '')}">${esc(PROVIDER_LABEL[s.origin.provider] || s.origin.provider)}</span>` : ''}</span><span class="muted">${esc(s.description || '—')}</span>
      <span class="num">${s.files}</span><span class="muted">${esc(s.used_by || '还没挂到智能体上')}</span>
      <span class="row" style="gap:10px;justify-content:flex-end">${s.origin ? '<button class="linkbtn" data-upd title="按记下的出处再取一次最新的，看过再覆盖">更新</button>' : ''}<button class="linkbtn danger" data-del>删除</button></span></div>`).join('')
    : '<div class="empty" style="padding:50px 20px">还没有技能<div class="t-caption faint" style="margin-top:8px">到「发现」里从 skills.sh / SkillsMP / ClawHub / GitHub 装，新建一个，或者把本机 ~/.claude/skills 里现成的导进来。</div><button class="btn btn-brand btn-sm" style="margin-top:14px" onclick="navigate(\'#/skills?tab=market\')">去发现</button></div>';
  const c = $('#cnt-skills'); if (c) c.textContent = list.length || '';
  host.querySelectorAll('.lb-row[data-id]').forEach(r => {
    r.onclick = async e => {
      const s = list.find(x => x.id === r.dataset.id);
      if (e.target.closest('[data-upd]')) { updateSkill(s); return; }
      if (e.target.closest('[data-del]')) {
        if (await ask(`删除技能「${s.name}」？${s.used_by ? `它现在挂在 ${s.used_by} 上，删掉后这些智能体就用不了了。` : ''}`, { ok: '删除', danger: true })) { await api(`/api/skills/${s.id}`, { method: 'DELETE' }); drawSkills(); }
        return;
      }
      dlgSkill(s.id);
    };
  });
}
async function newSkill() {
  const name = await askText('技能名（字母、数字、- 和 _，会成为目录名）', '', { ok: '创建' });
  if (!name) return;
  try { const r = await post('/api/skills', { name: name.trim() }); await drawSkills(); dlgSkill(r.id); } catch (e) { toast(e.message); }
}
async function dlgSkill(id, openPath) {
  let sk; try { sk = await api(`/api/skills/${id}`); } catch (e) { toast(e.message); return; }
  const cur = sk.files.find(f => f.path === openPath) || sk.files[0];
  openDlg(`<h3 class="mono">${esc(sk.name)}</h3>
    ${sk.source ? `<div class="t-micro faint" style="margin:-6px 0 8px">导入自 ${esc(sk.source)}</div>` : ''}
    <div class="sk-ed"><div class="sk-files">${sk.files.map(f => `<button class="sk-f" data-p="${esc(f.path)}" data-on="${f.path === cur?.path}" title="${esc(f.path)}">${esc(f.path)}</button>`).join('')}
        <button class="linkbtn" id="skAdd" style="margin-top:6px">+ 添加文件</button></div>
      <div class="sk-main"><textarea id="skText" class="input mono" spellcheck="false">${esc(cur?.content || '')}</textarea></div></div>
    <div class="dfoot">${cur && cur.path !== 'SKILL.md' ? '<button class="btn btn-outline" id="skDelFile">删除这个文件</button>' : ''}<span class="grow"></span>
      <button class="btn btn-outline" onclick="closeDlg()">关闭</button><button class="btn btn-brand" id="skSave">保存 ${esc(cur?.path || '')}</button></div>`);
  $('#dlgBody').classList.add('wide');
  const save = async () => { await jput(`/api/skills/${id}/files`, { path: cur.path, content: $('#skText').value }); };
  $('#skSave').onclick = async () => { try { await save(); toast('已保存'); drawSkills(); } catch (e) { toast(e.message); } };
  $$('#dlgBody .sk-f').forEach(b => { b.onclick = async () => { try { if ($('#skText').value !== (cur?.content || '')) await save(); } catch (e) { toast(e.message); return; } dlgSkill(id, b.dataset.p); }; });
  $('#skAdd').onclick = async () => {
    const p = await askText('文件路径（技能目录里的相对路径）', 'reference.md', { ok: '添加' });
    if (!p) { dlgSkill(id, cur?.path); return; }
    try { await jput(`/api/skills/${id}/files`, { path: p.trim(), content: '' }); dlgSkill(id, p.trim()); } catch (e) { toast(e.message); dlgSkill(id, cur?.path); }
  };
  if ($('#skDelFile')) $('#skDelFile').onclick = async () => { await api(`/api/skills/${id}/files?path=${encodeURIComponent(cur.path)}`, { method: 'DELETE' }); dlgSkill(id); drawSkills(); };
}
async function dlgSkillImport() {
  let found = []; try { found = await api('/api/skills/import'); } catch (e) { toast(e.message); return; }
  openDlg(`<h3>从本机导入技能</h3>
    <div class="t-caption muted" style="margin-bottom:10px">只读取 ~/.claude/skills 和 ~/.codex/skills 里的文本文件（不跟符号链接，单个文件 ≤256KB），复制一份进库；原目录不动。</div>
    <div class="snip-list" style="max-height:300px">${found.map((f, i) => `<label class="snip-row" style="cursor:pointer"><input type="checkbox" data-i="${i}" ${f.exists ? '' : 'checked'}>
      <span style="flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap"><b class="mono">${esc(f.name)}</b> <span class="faint">${esc(f.description)}</span></span>
      <span class="faint t-micro">${esc(f.source.replace('/' + f.name, ''))}${f.exists ? ' · 库里已有' : ''}</span></label>`).join('') || '<div class="t-caption faint" style="padding:8px">这台机器上没找到技能目录</div>'}</div>
    <div class="field" style="margin-top:12px"><label>库里已有同名的</label><select id="siConflict" class="input"><option value="skip">跳过</option><option value="rename">改名导入（加 -2、-3…）</option><option value="overwrite">覆盖库里的</option></select></div>
    <div class="dfoot"><button class="btn btn-outline" onclick="closeDlg()">取消</button><button class="btn btn-brand" id="siGo" ${found.length ? '' : 'disabled'}>导入</button></div>`);
  $('#siGo').onclick = async () => {
    const sources = [...$$('#dlgBody [data-i]:checked')].map(c => found[+c.dataset.i].source);
    if (!sources.length) { toast('先勾上要导入的'); return; }
    try {
      const rep = await post('/api/skills/import', { sources, conflict: $('#siConflict').value });
      closeDlg(); toast(`导入完成：${rep.filter(r => /已导入/.test(r.result)).length} 个成功，${rep.filter(r => !/已导入/.test(r.result)).length} 个跳过`); drawSkills();
    } catch (e) { toast(e.message); }
  };
}
async function drawMcpLib() {
  const host = $('#lbBody'); if (!host) return;
  let list = []; try { list = await api('/api/mcp-servers'); } catch (e) { host.innerHTML = `<div class="empty">${esc(e.message)}</div>`; return; }
  S.mcpLib = list;
  host.innerHTML = list.length ? `<div class="ap-row lb-row ap-hd"><span>名字</span><span>怎么连</span><span>密钥</span><span>用在</span><span></span></div>` + list.map(m => `
    <div class="ap-row lb-row" data-id="${esc(m.id)}" tabindex="0"><span class="mono"><b>${esc(m.name)}</b> <span class="badge badge-muted">${m.transport === 'http' ? 'HTTP' : 'STDIO'}</span></span>
      <span class="mono muted" title="${esc(m.transport === 'http' ? m.url : [m.command, ...m.args].join(' '))}">${esc(m.transport === 'http' ? m.url : [m.command, ...m.args].join(' '))}</span>
      <span class="num">${m.env_keys.length + m.header_keys.length || '—'}</span><span class="muted">${esc(m.used_by || '还没启用')}</span>
      <span><button class="linkbtn danger" data-del>删除</button></span></div>`).join('')
    : '<div class="empty" style="padding:50px 20px">还没有 MCP 服务器<div class="t-caption faint" style="margin-top:8px">添加之后，到智能体的「能力 → MCP」里启用。</div></div>';
  host.querySelectorAll('.lb-row[data-id]').forEach(r => {
    r.onclick = async e => {
      const m = list.find(x => x.id === r.dataset.id);
      if (e.target.closest('[data-del]')) { if (await ask(`删除 MCP 服务器「${m.name}」？`, { ok: '删除', danger: true })) { await api(`/api/mcp-servers/${m.id}`, { method: 'DELETE' }); drawMcpLib(); } return; }
      dlgMcp(m);
    };
  });
}

const parseKv = text => Object.fromEntries(text.split('\n').map(l => l.trim()).filter(l => l && l.includes('=')).map(l => [l.slice(0, l.indexOf('=')).trim(), l.slice(l.indexOf('=') + 1)]));
function dlgMcp(m) {
  const e = m || { transport: 'stdio', args: [], env_keys: [], header_keys: [] };
  openDlg(`<h3>${m ? '编辑' : '添加'} MCP 服务器</h3>
    <div class="grid2"><div class="field"><label>名字（字母、数字、- 和 _）</label><input id="mcName" class="input mono" value="${esc(e.name || '')}" placeholder="docs"></div>
      <div class="field"><label>类型</label><select id="mcType" class="input"><option value="stdio" ${e.transport !== 'http' ? 'selected' : ''}>STDIO（本地起一个进程）</option><option value="http" ${e.transport === 'http' ? 'selected' : ''}>Streamable HTTP</option></select></div></div>
    <div class="field"><label>说明</label><input id="mcDesc" class="input" value="${esc(e.description || '')}" placeholder="查内部文档"></div>
    <div id="mcStdio"><div class="field"><label>启动命令</label><input id="mcCmd" class="input mono" value="${esc(e.command || '')}" placeholder="npx"></div>
      <div class="field"><label>参数（一行一个）</label><textarea id="mcArgs" class="input mono" rows="3" placeholder="-y&#10;@acme/docs-mcp">${esc((e.args || []).join('\n'))}</textarea></div>
      <div class="field"><label>环境变量（KEY=VALUE，一行一个）${m ? ` · 现有：${e.env_keys.map(k => `<span class="mono">${esc(k)}</span>`).join('、') || '无'}` : ''}</label>
        <textarea id="mcEnv" class="input mono" rows="2" placeholder="${m ? '留空 = 保留原来的密钥不动；填了就整体替换' : 'API_KEY=sk-…'}"></textarea></div></div>
    <div id="mcHttp"><div class="field"><label>地址</label><input id="mcUrl" class="input mono" value="${esc(e.url || '')}" placeholder="https://mcp.example.com/mcp"></div>
      <div class="field"><label>请求头（KEY=VALUE，一行一个）${m ? ` · 现有：${e.header_keys.map(k => `<span class="mono">${esc(k)}</span>`).join('、') || '无'}` : ''}</label>
        <textarea id="mcHdr" class="input mono" rows="2" placeholder="${m ? '留空 = 保留原来的不动' : 'Authorization=Bearer …'}"></textarea></div></div>
    <details style="margin-top:4px"><summary class="t-caption muted" style="cursor:pointer">从 JSON 粘贴（.mcp.json / claude mcp 的那种格式）</summary>
      <textarea id="mcJson" class="input mono" rows="4" style="margin-top:6px" placeholder='{"mcpServers":{"docs":{"command":"npx","args":["-y","@acme/docs-mcp"],"env":{"API_KEY":"…"}}}}'></textarea>
      <button class="btn btn-outline btn-xs" id="mcFill" style="margin-top:6px">填进表单</button></details>
    <div class="dfoot"><button class="btn btn-outline" onclick="closeDlg()">取消</button><button class="btn btn-brand" id="mcSave">${m ? '保存' : '添加'}</button></div>`);
  const sync = () => { const http = $('#mcType').value === 'http'; $('#mcStdio').hidden = http; $('#mcHttp').hidden = !http; };
  $('#mcType').onchange = sync; sync();
  $('#mcFill').onclick = () => {
    try {
      let j = JSON.parse($('#mcJson').value); let name = '';
      if (j.mcpServers) { [name, j] = Object.entries(j.mcpServers)[0]; }
      if (name && !$('#mcName').value) $('#mcName').value = name;
      const http = !!j.url; $('#mcType').value = http ? 'http' : 'stdio'; sync();
      if (http) { $('#mcUrl').value = j.url; $('#mcHdr').value = Object.entries(j.headers || {}).map(([k, v]) => `${k}=${v}`).join('\n'); }
      else { $('#mcCmd').value = j.command || ''; $('#mcArgs').value = (j.args || []).join('\n'); $('#mcEnv').value = Object.entries(j.env || {}).map(([k, v]) => `${k}=${v}`).join('\n'); }
      toast('已填进表单，检查一下再保存');
    } catch (err) { toast('JSON 解析不了：' + err.message); }
  };
  $('#mcSave').onclick = async () => {
    const http = $('#mcType').value === 'http';
    const envText = $('#mcEnv').value.trim(), hdrText = $('#mcHdr').value.trim();
    const body = { name: $('#mcName').value.trim(), description: $('#mcDesc').value.trim(), transport: http ? 'http' : 'stdio',
      command: $('#mcCmd').value.trim(), args: $('#mcArgs').value.split('\n').map(x => x.trim()).filter(Boolean), url: $('#mcUrl').value.trim(),

      env: envText ? parseKv(envText) : (m ? null : {}), headers: hdrText ? parseKv(hdrText) : (m ? null : {}) };
    try { await jput(m ? `/api/mcp-servers/${m.id}` : '/api/mcp-servers', body, m ? 'PUT' : 'POST'); closeDlg(); toast(m ? '已保存' : '已添加'); drawMcpLib(); }
    catch (err) { toast(err.message); }
  };
}

async function drawAgentCaps(pane, a, kind) {
  pane.innerHTML = '<div class="empty">加载中…</div>';
  let lib = [], caps = { skills: [], mcp: [] };
  try { [lib, caps] = await Promise.all([api(kind === 'skills' ? '/api/skills' : '/api/mcp-servers'), api(`/api/agent-profiles/${a.id}/capabilities`)]); }
  catch (e) { pane.innerHTML = `<div class="empty">${esc(e.message)}</div>`; return; }
  const on = new Set(caps[kind]);
  const claudeOnly = kind === 'skills' && a.runtime !== 'claude';
  pane.innerHTML = `<div class="set-sec"><div class="set-h"><h3>${kind === 'skills' ? '技能' : 'MCP 服务器'}</h3>
      <a class="linkbtn" href="#/skills${kind === 'mcp' ? '?tab=mcp' : ''}" data-leave>${kind === 'mcp' ? '管理 MCP 服务器' : '管理 SKILLs'}</a>${kind === 'skills' ? ' <a class="linkbtn" href="#/skills?tab=market" data-leave>去发现更多</a>' : ''}</div>
    <div class="t-caption muted" style="margin-bottom:10px">${kind === 'skills'
      ? (claudeOnly ? `${esc(AGENT_LABEL[a.runtime] || a.runtime)} 没有原生的技能机制：勾上的技能会以「清单 + SKILL.md 路径」的形式写进角色说明，它需要时自己去读。` : '勾上的技能在每轮开始前物化到临时目录，用 --add-dir 交给 Claude；不改动你自己的 ~/.claude。')
      : (['claude', 'codex'].includes(a.runtime) ? '勾上的服务器在每轮启动时合并进运行时的 MCP 配置。' : `${esc(AGENT_LABEL[a.runtime] || a.runtime)} 暂不支持从这里挂 MCP 服务器（只有 Claude Code 和 Codex 支持）。`)}</div>
    <div class="card set-card">${lib.length ? lib.map(x => `<label class="cap-row"><input type="checkbox" data-id="${esc(x.id)}" ${on.has(x.id) ? 'checked' : ''}>
      <span><b class="mono">${esc(x.name)}</b><span class="faint t-caption">${esc(x.description || (kind === 'mcp' ? (x.transport === 'http' ? x.url : [x.command, ...x.args].join(' ')) : ''))}</span></span></label>`).join('')
      : `<div class="t-caption faint" style="padding:6px 0">SKILLs 里还没有${kind === 'skills' ? '技能' : ' MCP 服务器'}。<a href="#/skills${kind === 'mcp' ? '?tab=mcp' : '?tab=market'}">去添加</a></div>`}</div>
    <div class="t-micro faint" id="capSaved" style="height:16px;margin-top:6px"></div></div>`;
  pane.querySelectorAll('input[data-id]').forEach(c => { c.onchange = async () => {
    const ids = [...pane.querySelectorAll('input[data-id]:checked')].map(x => x.dataset.id);
    try { await jput(`/api/agent-profiles/${a.id}/capabilities`, { [kind]: ids }); $('#capSaved').textContent = '已保存，下一轮起生效'; }
    catch (e) { toast(e.message); c.checked = !c.checked; }
  }; });
}
