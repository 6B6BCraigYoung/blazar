const EDITORS = {
  vscode:   { label: 'VS Code',  scheme: 'vscode' },
  cursor:   { label: 'Cursor',   scheme: 'cursor' },
  windsurf: { label: 'Windsurf', scheme: 'windsurf' },
  insiders: { label: 'VS Code Insiders', scheme: 'vscode-insiders' },
};
function pickedEditor() {
  let k = 'vscode';
  try { k = localStorage.getItem('blazar.editor') || 'vscode'; } catch (_) {}
  return EDITORS[k] ? k : 'vscode';
}

function editorUrl(node, path, line) {
  const ed = EDITORS[pickedEditor()];

  const p = path.split('/').map(encodeURIComponent).join('/');
  const tail = line ? `:${line}` : '';
  return node === 'local'
    ? `${ed.scheme}://file${p}${tail}`
    : `${ed.scheme}://vscode-remote/ssh-remote+${encodeURIComponent(node)}${p}${tail}`;
}

function openInEditor(key) {
  if (!S.ws) return;
  try { localStorage.setItem('blazar.editor', key); } catch (_) {}
  const target = S.file ? `${S.ws.path.replace(/\/$/, '')}/${S.file}` : S.ws.path;
  const line = S.file ? S.editor?.getPosition()?.lineNumber : null;
  location.href = editorUrl(S.ws.node, target, line);
}

const STATUS_TEXT = { ...ACT, online: '在线', offline: '离线' };

const statusPill = st => `<span class="pill ${esc(st)}"><span class="dot ${esc(st)}"></span>${
  esc(STATUS_TEXT[st] || st)}</span>`;

const latency = ms => ms == null ? '<span class="faint">—</span>'
  : `<span class="lat ${ms < 60 ? 'good' : ms < 150 ? 'mid' : 'bad'}">${ms.toFixed(0)} ms</span>`;

function diffBadge(stat, at) {
  if (!stat) return '<span class="faint t-caption">—</span>';
  if (!stat.files) return `<span class="faint t-caption" title="${at ? '统计于 ' + ago(at) : ''}">无改动</span>`;
  return `<span class="dstat" title="${stat.files} 个文件${at ? ' · 统计于 ' + ago(at) : ''}">` +
    `<span class="dadd">+${fmt(stat.added)}</span> <span class="ddel">−${fmt(stat.removed)}</span></span>`;
}

function inspectorHtml(d) {
  const kv = (k, v, mono) =>
    `<div class="kv"><span class="k">${k}</span><span class="v ${mono ? 'mono t-caption' : ''}">${v}</span></div>`;
  return `
    <div class="insp">
      <h4>属性</h4>
      ${kv('机器', `<a href="#/nodes/${encodeURIComponent(d.node)}">${esc(d.node)}</a>`)}
      ${kv('路径', esc(d.path), true)}
      ${kv('项目', esc(d.project || '—'))}
      ${d.isolated ? kv('分支', esc(d.branch), true) : ''}
      ${d.isolated ? kv('基线', esc((d.base_commit || '').slice(0, 10)), true) : ''}
      ${kv('来源', d.origin === 'task' ? '批量派发' : '手动创建')}
      ${kv('状态', d.status === 'paused' ? '<span class="badge badge-warn">已冻结</span>' : '活跃')}
      ${kv('创建', ago(d.created_at))}
    </div>
    <div class="insp">
      <h4>操作</h4>
      <div class="actions" style="flex-wrap:wrap;gap:6px">
        ${d.isolated ? (d.status === 'paused'
          ? '<button class="btn btn-outline btn-sm" data-act="resume">解冻</button>'
          : '<button class="btn btn-outline btn-sm" data-act="pause" title="提交→删 worktree→保留分支">冻结</button>') : ''}
        ${d.isolated ? '<button class="btn btn-outline btn-sm" data-act="commit">提交</button>' : ''}
        ${d.isolated && d.status !== 'paused' ? '<button class="btn btn-brand btn-sm" data-act="push" title="先提交没提交的改动，再推到 origin">推送分支</button>' : ''}
        <button class="btn btn-danger btn-sm" data-act="destroy">${d.isolated ? '销毁' : '移除'}</button>
      </div>
      ${d.isolated ? '' : '<div class="t-micro faint" style="margin-top:6px">非隔离工作区：移除只删记录，不动你的目录</div>'}
    </div>
    <div class="insp">
      <h4>会话 ${d.sessions.length ? `<span class="badge badge-muted num">${d.sessions.length}</span>` : ''}</h4>
      ${d.sessions.length ? d.sessions.slice(0, 8).map(s => `
        <div class="kv"><span class="k">${esc(AGENT_LABEL[s.runtime] || s.runtime)}</span>
          <span class="v t-caption muted">${esc(s.status)} · ${s.events} 条事件 · ${ago(s.created_at)}</span></div>`).join('')
        : '<div class="t-caption faint">还没有会话</div>'}
    </div>`;
}

function openInspector(d) {
  openDlg(`<h3>${esc(d.name)}</h3>${inspectorHtml(d)}
    <div class="dfoot"><button class="btn btn-outline" onclick="closeDlg()">关闭</button></div>`);
  wireInspector(d);
}
function wireInspector(d) {
  $$('#dlgBody [data-act]').forEach(b => {
    b.onclick = async () => {
      const act = b.dataset.act;
      if (act === 'destroy' && !await ask(d.isolated
        ? `销毁工作区「${d.name}」？会删除远端 worktree、分支与私有目录，不可撤销。`
        : `从 Blazar 移除「${d.name}」？只删记录，不会动机器上的目录。`, { ok: d.isolated ? '销毁' : '移除', danger: true })) return;
      try {
        if (act === 'push') {
          b.disabled = true; b.textContent = '推送中…';
          try {
            const r = await post(`/api/workspaces/${d.id}/push`);
            if (r.error) { toast(`推送失败：${r.error}`); return; }
            const links = r.web
              ? `<div class="row" style="gap:8px;margin-top:10px">
                   <a class="btn btn-brand btn-sm" href="${esc(r.web.new_pr)}" target="_blank" rel="noopener">开 PR / MR</a>
                   <a class="btn btn-outline btn-sm" href="${esc(r.web.branch)}" target="_blank" rel="noopener">在网页上看分支</a></div>`
              : '<div class="t-caption faint" style="margin-top:8px">认不出托管平台，没法给网页链接</div>';
            openDlg(`<h3>已推送</h3>
              <div class="kv"><span class="k">分支</span><span class="v mono">${esc(r.branch)}</span></div>
              <div class="kv"><span class="k">远端</span><span class="v mono t-caption">${esc(r.remote)}</span></div>
              ${r.committed ? `<div class="kv"><span class="k">自动提交</span><span class="v mono">${esc(r.committed.slice(0, 10))}</span></div>` : ''}
              ${links}
              <div class="dfoot"><button class="btn btn-outline" onclick="closeDlg()">关闭</button></div>`);
          } finally { b.disabled = false; b.textContent = '推送分支'; }
          return;
        }
        if (act === 'commit') {
          const msg = await askText('提交信息', `blazar: ${d.name}`, { ok: '提交' });
          if (!msg) return;
          const r = await post(`/api/workspaces/${d.id}/commit`, { message: msg });
          toast(r.changed ? `已提交 ${r.commit.slice(0, 8)}` : '没有改动可提交');
          return;
        }
        const r = await post(`/api/workspaces/${d.id}/${act}`);

        const notes = [];
        if (r?.stopped_sessions) notes.push(`停掉了 ${r.stopped_sessions} 个正在跑的会话`);
        if (r?.worktree_was_missing) notes.push('工作目录冻结前就已经不在了，没提交的改动（如果有）已丢失');
        if (r?.already_present) notes.push('工作目录本来就在，保持原样');
        if (r?.moved_aside) notes.push(`原目录已损坏，挪到了 ${r.moved_aside}`);
        if (r?.branch_kept) notes.push(`分支没删掉：${r.branch_kept}`);
        const head = { pause: '已冻结', resume: '已解冻', destroy: d.isolated ? '已销毁' : '已移除' }[act];
        toast(notes.length ? `${head}。${notes.join('；')}` : head);
        closeDlg();
        if (act === 'destroy') { await refreshState(); navigate('#/workspaces'); return; }
        await refreshState(); render();
      } catch (e) { toast('操作失败: ' + e.message); }
    };
  });
}

const CH = { modified: 'M', added: 'A', deleted: 'D', untracked: '?' };
const SVG_DIR = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7"
  stroke-linejoin="round"><path d="M3 7a2 2 0 0 1 2-2h3.5l2 2H19a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>`;
const SVG_FILE = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7"
  stroke-linejoin="round"><path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/></svg>`;
const SVG_CHEV = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4"
  stroke-linecap="round" stroke-linejoin="round"><path d="m9 6 6 6-6 6"/></svg>`;

const LCOLOR = {
  rust: '#d38b5d', typescript: '#3178c6', javascript: '#c9a227', python: '#3a7cb8',
  go: '#00a5c4', java: '#9a6236', c: '#7a869a', cpp: '#d1497c', ruby: '#a1303a',
  php: '#6b73b3', swift: '#e8703a', kotlin: '#9b6dd6', lua: '#3b5bbf', sql: '#8a7fb5',
  shell: '#5c9c4a', markdown: '#7c8794', json: '#b3963a', ini: '#7a869a', yaml: '#b35a4a',
  html: '#d05a35', css: '#6a4fb0', xml: '#7a869a',
};

function buildTree(entries) {
  const root = { name: '', path: '', dir: true, kids: new Map() };
  for (const e of entries) {
    const parts = e.path.split('/').filter(Boolean);
    let cur = root;
    for (let i = 0; i < parts.length; i++) {
      const last = i === parts.length - 1;
      let n = cur.kids.get(parts[i]);
      if (!n) {

        n = { name: parts[i], path: parts.slice(0, i + 1).join('/'), dir: true, kids: new Map() };
        cur.kids.set(parts[i], n);
      }
      if (last) { n.dir = !!e.is_dir; if (e.change) n.change = e.change; }
      cur = n;
    }
  }
  const prep = n => {
    n.sorted = [...n.kids.values()].sort((a, b) =>
      (b.dir ? 1 : 0) - (a.dir ? 1 : 0) || a.name.localeCompare(b.name, 'zh', { numeric: true }));
    let any = !!n.change;
    for (const k of n.sorted) if (prep(k)) any = true;
    n.hasChange = any;
    return any;
  };
  prep(root);
  return root;
}

function expandChanged(node, into) {
  for (const k of node.sorted) {
    if (k.dir && k.hasChange) { into.add(k.path); expandChanged(k, into); }
  }
}
async function loadTree() {
  const host = $('#tree');
  if (!host) return;
  let truncated = false;
  try {
    const r = await api(`/api/workspaces/${S.ws.id}/tree`);
    S.tree = r.entries; truncated = !!r.truncated; S.stat = r.stat;
  } catch (e) { host.innerHTML = `<div class="empty">${esc(e.message)}</div>`; return; }
  S.treeRoot = buildTree(S.tree);
  S.changeOf = new Map(S.tree.filter(e => e.change).map(e => [e.path, e.change]));
  S.opened = new Set();

  const AUTO_EXPAND_MAX = 40;
  if (S.changeOf.size && S.changeOf.size <= AUTO_EXPAND_MAX) {
    expandChanged(S.treeRoot, S.opened);
  }
  const files = S.tree.filter(e => !e.is_dir).length;
  const c = $('#treeCount');
  if (c) {

    const n = truncated ? `${files}+ 个文件（已截断）` : `${files} 个文件`;
    c.innerHTML = S.stat && S.stat.files
      ? diffBadge(S.stat, null)
      : esc(S.changeOf.size ? `${S.changeOf.size} 处改动 / ${files}` : n);
    c.title = truncated
      ? '这个目录不是 git 仓库，只列出了前 20000 个文件。选一个具体的仓库目录能列全。'
      : '';
  }
  renderTree($('#treeFilter')?.value || '');
  renderTabs();
}
function renderTree(filter) {
  const host = $('#tree');
  if (!host || !S.treeRoot) return;
  const f = (filter || '').trim().toLowerCase();

  let keep = null;
  if (f) {
    keep = new Set();
    const walk = n => {
      let hit = !!n.path && n.path.toLowerCase().includes(f);
      for (const k of n.sorted) if (walk(k)) hit = true;
      if (hit && n.path) keep.add(n.path);
      return hit;
    };
    walk(S.treeRoot);
  }
  const rows = [];
  const emit = (node, depth) => {
    for (const n of node.sorted) {
      if (keep && !keep.has(n.path)) continue;
      const open = n.dir && (keep ? true : S.opened.has(n.path));
      const color = n.dir ? '' : `color:${LCOLOR[langOf(n.name)] || 'var(--faint-fg)'}`;
      rows.push(
        `<div class="tn" data-p="${esc(n.path)}" data-dir="${n.dir ? 1 : 0}" data-open="${open}"` +
        ` data-sel="${!n.dir && S.file === n.path}" title="${esc(n.path)}">` +
        `<span class="ind" style="width:${depth * 14}px"></span>` +
        `<span class="chev">${n.dir ? SVG_CHEV : ''}</span>` +
        `<span class="ico" style="${color}">${n.dir ? SVG_DIR : SVG_FILE}</span>` +
        `<span class="nm">${esc(n.name)}</span>` +
        `<span class="ch ${esc(n.change || '')}">${n.change ? CH[n.change] : ''}</span></div>`);
      if (n.dir && open) emit(n, depth + 1);
    }
  };
  emit(S.treeRoot, 0);
  host.innerHTML = rows.length ? rows.join('') : '<div class="empty">无匹配</div>';
}
function onTreeClick(e) {
  const row = e.target.closest('.tn');
  if (!row) return;
  const p = row.dataset.p;
  if (row.dataset.dir === '1') {
    if (S.opened.has(p)) S.opened.delete(p); else S.opened.add(p);
    renderTree($('#treeFilter')?.value || '');
  } else openFile(p);
}

function renderTabs() {
  setTimeout(drawCtxChip, 0);
  const bar = $('#tabbar');
  if (!bar) return;
  bar.innerHTML = S.open.map(p => {
    const ch = S.changeOf.get(p);
    return `<div class="etab" data-p="${esc(p)}" data-active="${S.file === p}" data-dirty="${isDirty(p)}" title="${esc(p)}${isDirty(p) ? '（未保存）' : ''}">` +
      `<span class="ch ${esc(ch || '')}">${ch ? CH[ch] : ''}</span>` +
      `<span class="nm">${esc(p.split('/').pop())}</span>` +
      `<button class="x" data-close="${esc(p)}" title="关闭">×</button></div>`;
  }).join('');
  const w = $('#welcome');
  if (w) w.style.display = S.open.length ? 'none' : '';
  drawEdBar();
}
function onTabbarClick(e) {
  const x = e.target.closest('[data-close]');
  if (x) { closeFile(x.dataset.close); return; }
  const t = e.target.closest('.etab');
  if (t && t.dataset.p !== S.file) openFile(t.dataset.p);
}
async function closeFile(path, force) {
  const i = S.open.indexOf(path);
  if (i < 0) return;
  if (!force && isDirty(path)) {
    if (!await ask(`「${path.split('/').pop()}」还没保存，关掉的话改动就丢了。`, { ok: '不保存，关掉', danger: true })) return;
    if (!S.open.includes(path)) return;
  }
  S.open.splice(S.open.indexOf(path), 1);

  if (S.file === path) S.editor?.setModel(null);
  try { S.models.get(path)?.dispose(); } catch (_) {  }
  S.models.delete(path); S.fileMeta.delete(path); S.dirtyShown.delete(path);
  if (S.file === path) {
    const next = S.open[i] || S.open[i - 1];
    S.file = null;
    if (next) { openFile(next); return; }
    S.editor?.setModel(null);
  }
  renderTabs();
  renderTree($('#treeFilter')?.value || '');
}

const MD_MODE_KEY = 'blazar.md.mode';
const isMd = p => /\.(md|markdown|mdx)$/i.test(p || '');
const mdMode = () => { try { return localStorage.getItem(MD_MODE_KEY) === 'source' ? 'source' : 'preview'; } catch (_) { return 'preview'; } };
const fileMeta = p => S.fileMeta.get(p);
const isDirty = p => { const m = S.models.get(p), f = fileMeta(p); return !!m && !!f && !f.readonly && m.getAlternativeVersionId() !== f.savedVersion; };
const dirtyFiles = () => S.open.filter(isDirty);

function trackModel(path, model, r) {
  S.fileMeta.set(path, { mtime: r.mtime || 0, savedVersion: model.getAlternativeVersionId(), readonly: !!(r.binary || r.too_large) });
  model.onDidChangeContent(() => {
    const was = S.dirtyShown.has(path), now = isDirty(path);
    if (was !== now) { now ? S.dirtyShown.add(path) : S.dirtyShown.delete(path); renderTabs(); }
    if (S.file === path) { drawEdBar(); if (isMd(path) && mdMode() === 'preview') renderMdSoon(); }
  });
}
function drawEdBar() {
  const bar = $('#edbar'); if (!bar) return;
  const p = S.file;
  bar.hidden = !p;
  if (!p) { showMdView(false); return; }
  const parts = p.split('/'), f = fileMeta(p), dirty = isDirty(p);
  bar.innerHTML = `<span class="ed-crumb">${parts.map((x, i) => `<span class="${i === parts.length - 1 ? 'cur' : ''}">${esc(x)}</span>`).join('<i>›</i>')}</span>
    <span class="grow"></span>
    ${f?.readonly ? '<span class="faint">只读</span>' : dirty ? `<button class="linkbtn" data-a="save" title="保存（⌘S）">● 未保存 · 保存</button>` : '<span class="faint">已保存</span>'}
    ${isMd(p) && obsidianVaultOf(S.ws) ? `<a class="linkbtn" href="obsidian://open?vault=${encodeURIComponent(obsidianVaultOf(S.ws).name)}&file=${encodeURIComponent(p.replace(/\.md$/, ''))}" title="在 Obsidian 应用里打开这篇笔记">在 Obsidian 里打开</a>` : ''}
    ${isMd(p) ? `<span class="seg ed-md"><button data-a="md" data-v="preview" data-on="${mdMode() === 'preview'}">Preview</button><button data-a="md" data-v="source" data-on="${mdMode() === 'source'}">Markdown</button></span>` : ''}`;
  showMdView(isMd(p) && mdMode() === 'preview');
}
function wireEdBar() {
  const bar = $('#edbar'); if (!bar || bar.dataset.wired) return;
  bar.dataset.wired = '1';
  bar.addEventListener('click', e => {
    const b = e.target.closest('[data-a]'); if (!b) return;
    if (b.dataset.a === 'save') saveFile();
    if (b.dataset.a === 'md') { try { localStorage.setItem(MD_MODE_KEY, b.dataset.v); } catch (_) {} drawEdBar(); if (b.dataset.v === 'source') S.editor?.focus(); }
  });
  $('#mdview').addEventListener('click', e => {
    const a = e.target.closest('a[data-file]'); if (!a) return;
    e.preventDefault(); openFile(a.dataset.file);
  });
}
async function saveFile(path = S.file, force = false) {
  const model = S.models.get(path), f = fileMeta(path);
  if (!model || !f || f.readonly || !S.ws) return false;
  if (!isDirty(path) && !force) return true;
  const ws = S.ws.id, version = model.getAlternativeVersionId();
  try {
    const r = await api(`/api/workspaces/${ws}/file`, { method: 'PUT', headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ path, content: model.getValue(), expect_mtime: force ? null : (f.mtime || null) }) });
    if (!r.saved) { conflictDlg(path, r); return false; }
    f.mtime = r.mtime; f.savedVersion = version;
    S.dirtyShown.delete(path); if (isDirty(path)) S.dirtyShown.add(path);
    renderTabs(); if (S.file === path) drawEdBar();

    refreshChangesSoon();
    return true;
  } catch (e) { toast('保存失败: ' + e.message); return false; }
}
function conflictDlg(path, r) {
  openDlg(`<h3>这个文件被别人改过了</h3>
    <div class="ask-msg"><b class="mono">${esc(path)}</b> 在你打开之后又被改过（多半是 agent）。直接保存会把那边的改动盖掉。</div>
    <div class="dfoot"><button class="btn btn-outline" id="cfCancel">先不保存</button><span class="grow"></span>
      <button class="btn btn-outline" id="cfReload">丢掉我的改动，载入最新的</button><button class="btn btn-danger" id="cfForce">用我的覆盖</button></div>`);
  $('#cfCancel').onclick = closeDlg;
  $('#cfForce').onclick = () => { closeDlg(); saveFile(path, true); };
  $('#cfReload').onclick = () => { closeDlg(); reloadFile(path, true); };
}

async function reloadFile(path, discard) {
  const model = S.models.get(path), f = fileMeta(path);
  if (!model || !f || !S.ws || (isDirty(path) && !discard)) return;
  try {
    const r = await api(`/api/workspaces/${S.ws.id}/file?path=${encodeURIComponent(path)}`);
    if (r.binary || r.too_large || (r.mtime === f.mtime && !discard)) return;
    if (model.getValue() !== r.content) {
      const view = S.file === path ? S.editor?.saveViewState() : null;
      model.setValue(r.content);
      if (view) S.editor.restoreViewState(view);
    }
    f.mtime = r.mtime; f.savedVersion = model.getAlternativeVersionId();
    S.dirtyShown.delete(path); renderTabs();
    if (S.file === path) { drawEdBar(); if (isMd(path)) renderMd(); }
  } catch (_) {  }
}

function mdLink(href, base) {
  const h = href.trim();
  if (/^(https?:|mailto:)/i.test(h)) return { href: h, ext: true };
  if (h.startsWith('#')) return { href: h };
  if (/^[a-z][a-z0-9+.-]*:/i.test(h) || h.startsWith('//')) return null;

  const dir = base.includes('/') ? base.slice(0, base.lastIndexOf('/')) : '';
  const segs = (h.startsWith('/') ? h.slice(1) : (dir ? dir + '/' : '') + h).split('#')[0].split('/');
  const out = [];
  for (const s of segs) { if (s === '..') { if (!out.length) return null; out.pop(); } else if (s && s !== '.') out.push(s); }
  return out.length ? { file: out.join('/') } : null;
}

function wikiTarget(name) {
  const n = name.trim().replace(/\\/g, '/').replace(/^\//, '');
  if (!n || !Array.isArray(S.tree)) return null;
  const files = S.tree.filter(e => !e.is_dir).map(e => e.path);
  const has = p => files.includes(p);
  if (has(n)) return n;
  if (has(n + '.md')) return n + '.md';
  const base = n.toLowerCase(), baseMd = base + '.md';
  return files.find(p => { const b = p.toLowerCase().slice(p.lastIndexOf('/') + 1); return b === baseMd || b === base; }) || null;
}
const unesc = t => t.replace(/&quot;/g, '"').replace(/&#39;/g, "'").replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&amp;/g, '&');
const MD_TAGS = new Set('div p center br hr b strong i em s del u sub sup kbd code mark small span details summary img a ul ol li table thead tbody tr th td h1 h2 h3 h4 h5 h6 blockquote'.split(' '));
const MD_BLOCK_TAGS = new Set('div p center details summary table thead tbody tr th td ul ol li h1 h2 h3 h4 h5 h6 blockquote hr'.split(' '));
const MD_TAG_RE = /<!--[\s\S]*?-->|<(\/?)([a-zA-Z][a-zA-Z0-9]*)((?:\s+[a-zA-Z-]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s"'<>=`]+))?)*)\s*\/?>/g;
const MD_IMG_RE = /\.(png|jpe?g|gif|webp|svg|ico|bmp|avif)$/i;
function mdImage(alt, href, base, dim = '') {
  const l = mdLink(href, base);
  const ws = typeof S !== 'undefined' && S.ws ? S.ws.id : '';
  if (l?.file && ws && MD_IMG_RE.test(l.file))
    return `<img class="md-image" loading="lazy" alt="${esc(alt)}" src="/api/workspaces/${ws}/raw?path=${encodeURIComponent(l.file)}"${dim}>`;
  const label = esc(alt) || '图片';
  return l?.ext ? `<a class="md-img" href="${esc(l.href)}" target="_blank" rel="noopener noreferrer">🖼 ${label}（外链，点开看）</a>` : `<span class="md-img">🖼 ${label}</span>`;
}
function mdAnchor(l, extra = '') {
  if (!l) return '<a>';
  if (l.file) return `<a href="#" data-file="${esc(l.file)}" title="在编辑器里打开 ${esc(l.file)}">`;
  return `<a href="${esc(l.href)}"${l.ext ? ' target="_blank" rel="noopener noreferrer"' : ''}${extra}>`;
}
function mdTag(closing, tag, attrText, base) {
  const t = tag === 'center' ? 'div' : tag;
  if (closing) return tag === 'br' || tag === 'hr' || tag === 'img' ? '' : `</${t}>`;
  const at = {};
  attrText.replace(/([a-zA-Z-]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'<>=`]+)))?/g, (_, k, a, b, c) => { at[k.toLowerCase()] = unesc(a ?? b ?? c ?? ''); return ''; });
  const al = (at.align || (tag === 'center' ? 'center' : '')).toLowerCase();
  const align = ['left', 'center', 'right'].includes(al) ? ` style="text-align:${al}"` : '';
  const dim = ['width', 'height'].map(k => /^\d{1,4}%?$/.test(at[k] || '') ? ` ${k}="${at[k]}"` : '').join('');
  switch (tag) {
    case 'img': return mdImage(at.alt || '', at.src || '', base, dim);
    case 'a': return mdAnchor(mdLink(at.href || '', base), at.title ? ` title="${esc(at.title)}"` : '');
    case 'details': return `<details${'open' in at ? ' open' : ''}>`;
    case 'div': case 'p': case 'center': case 'th': case 'td': return `<${t}${align}>`;
    default: return `<${t}>`;
  }
}
function mdHtml(raw, base) {
  let out = '', i = 0, m;
  MD_TAG_RE.lastIndex = 0;
  while ((m = MD_TAG_RE.exec(raw))) {
    out += esc(raw.slice(i, m.index)); i = MD_TAG_RE.lastIndex;
    if (!m[2]) continue;
    const tag = m[2].toLowerCase();
    out += MD_TAGS.has(tag) ? mdTag(!!m[1], tag, m[3], base) : esc(m[0]);
  }
  return out + esc(raw.slice(i));
}
function mdHtmlStart(l, interrupt = false) {
  const m = /^\s*<(?:(!--)|\/?([a-zA-Z][a-zA-Z0-9]*))/.exec(l);
  if (!m) return false;
  if (m[1]) return true;
  const t = m[2].toLowerCase();
  return MD_BLOCK_TAGS.has(t) || (!interrupt && MD_TAGS.has(t) && /^\s*(?:<[^<>]*>\s*)+$/.test(l));
}
function mdSpan(text, base) {
  const keep = [];
  const stash = html => { keep.push(html); return `${keep.length - 1}`; };
  let s = esc(text.replace(MD_TAG_RE, m => { const h = mdHtml(m, base); return h.startsWith('&lt;') ? m : stash(h); }));
  s = s.replace(/`([^`\n]+)`/g, (_, c) => stash(`<code>${c}</code>`));

  s = s.replace(/!\[\[([^\]|#]+)(?:#[^\]|]*)?(?:\|([^\]]*))?\]\]/g, (_, target, alias) => {
    const f = wikiTarget(unesc(target)), ws = S.ws?.id;
    if (f && ws && /\.(png|jpe?g|gif|webp|svg|ico|bmp|avif)$/i.test(f)) return stash(`<img class="md-image" loading="lazy" alt="${alias || target}" src="/api/workspaces/${ws}/raw?path=${encodeURIComponent(f)}">`);
    return stash(f ? `<a href="#" data-file="${esc(f)}" title="在编辑器里打开 ${esc(f)}">📎 ${alias || target}</a>` : `<span class="md-img">📎 ${target}</span>`);
  });
  s = s.replace(/\[\[([^\]|#]+)(#[^\]|]*)?(?:\|([^\]]*))?\]\]/g, (_, target, hash, alias) => {
    const f = wikiTarget(unesc(target)), label = alias || target + (hash || '');
    return stash(f ? `<a href="#" data-file="${esc(f)}" title="在编辑器里打开 ${esc(f)}">${label}</a>` : `<span class="md-dead" title="工作区里没有这篇笔记">${label}</span>`);
  });
  s = s.replace(/==([^=\n]+)==/g, '<mark>$1</mark>');

  s = s.replace(/!\[([^\]]*)\]\(([^)\s]+)(?:\s+&quot;[^&]*&quot;)?\)/g, (_, alt, href) => stash(mdImage(unesc(alt), unesc(href), base)));
  s = s.replace(/\[([^\]]+)\]\(([^)\s]+)(?:\s+&quot;[^&]*&quot;)?\)/g, (m, label, href) => {
    const l = mdLink(unesc(href), base); if (!l) return label;
    return stash(`${mdAnchor(l)}${label}</a>`);
  });

  s = s.replace(/(^|[\s(（])(https?:\/\/[^\s<)）]+)/g, (_, pre, raw) => {
    const u = raw.replace(/[.,;:!?。，；：！？、]+$/, '');
    return `${pre}${stash(`<a href="${u}" target="_blank" rel="noopener noreferrer">${u}</a>`)}${raw.slice(u.length)}`;
  });
  s = s.replace(/\*\*([^*\n]+)\*\*/g, '<strong>$1</strong>').replace(/__([^_\n]+)__/g, '<strong>$1</strong>')
    .replace(/(^|[^*])\*([^*\n]+)\*(?!\*)/g, '$1<em>$2</em>').replace(/(^|[^\w_])_([^_\n]+)_(?![\w_])/g, '$1<em>$2</em>')
    .replace(/~~([^~\n]+)~~/g, '<del>$1</del>');
  const restore = t => t.replace(/(\d+)/g, (_, i) => restore(keep[+i]));
  return restore(s);
}
function mdFull(src, base = '', opts = {}) {
  const lines = src.replace(/\r\n?/g, '\n').split('\n');
  const breaks = opts.breaks ?? !!(typeof obsidianVaultOf === 'function' && typeof S !== 'undefined' && S.ws && obsidianVaultOf(S.ws));
  const slugs = new Map();
  const slug = t => { let k = t.toLowerCase().replace(/[^\p{L}\p{N}\s-]/gu, '').trim().replace(/\s+/g, '-') || 'section'; const n = slugs.get(k) || 0; slugs.set(k, n + 1); return n ? `${k}-${n}` : k; };
  const cells = l => l.trim().replace(/^\|/, '').replace(/\|$/, '').split(/(?<!\\)\|/).map(c => c.trim().replace(/\\\|/g, '|'));
  const isSep = l => /^\s*\|?\s*:?-{1,}:?\s*(\|\s*:?-{1,}:?\s*)*\|?\s*$/.test(l) && l.includes('-');
  const listRe = /^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$/;
  const mdPara = xs => xs.map((x, k) => { const hard = /(?: {2,}|\\)$/.test(x); return mdSpan(x.replace(/\\$/, '').trim(), base) + (k === xs.length - 1 ? '' : hard || breaks ? '<br>' : ' '); }).join('');
  function block(ls) {
    const out = []; let i = 0;
    while (i < ls.length) {
      const l = ls[i];
      if (!l.trim()) { i++; continue; }
      let m;
      if ((m = /^(\s*)(```+|~~~+)\s*([\w+#.-]*)/.exec(l))) {
        const fence = m[2], lang = m[3], ind = m[1].length, body = []; i++;
        while (i < ls.length && !new RegExp(`^\\s*${fence[0]}{${fence.length},}\\s*$`).test(ls[i])) body.push(ls[i++].slice(Math.min(ind, ls[i - 1].search(/\S|$/)))); i++;
        out.push(`<pre class="md-pre"><code data-lang="${esc(lang)}">${esc(body.join('\n'))}</code></pre>`); continue;
      }
      if (mdHtmlStart(l)) { const h = []; while (i < ls.length && ls[i].trim()) h.push(ls[i++]); out.push(mdHtml(h.join('\n'), base)); continue; }
      if ((m = /^(#{1,6})\s+(.*?)\s*#*\s*$/.exec(l))) { out.push(`<h${m[1].length} id="${esc(slug(m[2]))}">${mdSpan(m[2], base)}</h${m[1].length}>`); i++; continue; }
      if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(l)) { out.push('<hr>'); i++; continue; }
      if (/^\s*>/.test(l)) { const q = []; while (i < ls.length && /^\s*>/.test(ls[i])) q.push(ls[i++].replace(/^\s*>\s?/, '')); out.push(`<blockquote>${block(q)}</blockquote>`); continue; }
      if (l.includes('|') && i + 1 < ls.length && isSep(ls[i + 1])) {
        const head = cells(l), align = cells(ls[i + 1]).map(c => c.startsWith(':') && c.endsWith(':') ? 'center' : c.endsWith(':') ? 'right' : c.startsWith(':') ? 'left' : ''); i += 2;
        const rows = []; while (i < ls.length && ls[i].includes('|') && ls[i].trim()) rows.push(cells(ls[i++]));
        const td = (tag, c, k) => `<${tag}${align[k] ? ` style="text-align:${align[k]}"` : ''}>${mdSpan(c || '', base)}</${tag}>`;
        out.push(`<div class="md-table"><table><thead><tr>${head.map((c, k) => td('th', c, k)).join('')}</tr></thead><tbody>${rows.map(r => `<tr>${head.map((_, k) => td('td', r[k], k)).join('')}</tr>`).join('')}</tbody></table></div>`); continue;
      }
      if ((m = listRe.exec(l))) {
        const ind = m[1].length, ordered = /\d/.test(m[2]), items = [];
        while (i < ls.length) {
          const mm = listRe.exec(ls[i]);
          if (mm && mm[1].length === ind && /\d/.test(mm[2]) === ordered) { items.push([mm[3]]); i++; }
          else if (items.length && (!ls[i].trim() ? (i + 1 < ls.length && (/^\s+\S/.test(ls[i + 1]) || listRe.test(ls[i + 1]) && listRe.exec(ls[i + 1])[1].length >= ind)) : ls[i].search(/\S/) > ind)) { items[items.length - 1].push(ls[i].slice(Math.min(ls[i].search(/\S|$/), ind + 2))); i++; }
          else break;
        }
        const li = it => { const t = /^\[([ xX])\]\s+(.*)$/.exec(it[0]); const first = t ? `<input type="checkbox" disabled ${t[1] !== ' ' ? 'checked' : ''}> ${mdSpan(t[2], base)}` : mdSpan(it[0], base);
          const rest = it.slice(1); return `<li${t ? ' class="md-task"' : ''}>${first}${rest.some(x => x.trim()) ? block(rest) : ''}</li>`; };
        out.push(`<${ordered ? `ol start="${parseInt(m[2], 10)}"` : 'ul'}>${items.map(li).join('')}</${ordered ? 'ol' : 'ul'}>`); continue;
      }
      const para = [];
      while (i < ls.length && ls[i].trim() && !/^(#{1,6}\s|\s*(```|~~~)|\s*>)/.test(ls[i]) && !listRe.test(ls[i]) && !(ls[i].includes('|') && i + 1 < ls.length && isSep(ls[i + 1])) && !mdHtmlStart(ls[i], true)) para.push(ls[i++]);
      out.push(`<p>${mdPara(para)}</p>`);
    }
    return out.join('\n');
  }

  let fm = '';
  if (lines[0] === '---') { const end = lines.indexOf('---', 1); if (end > 0) { fm = `<pre class="md-pre md-fm"><code>${esc(lines.slice(1, end).join('\n'))}</code></pre>`; lines.splice(0, end + 1); } }
  return fm + block(lines);
}
let mdTimer;
const renderMdSoon = () => { clearTimeout(mdTimer); mdTimer = setTimeout(renderMd, 180); };
function renderMd() {
  const v = $('#mdview'), m = S.models.get(S.file); if (!v || !m || !isMd(S.file)) return;
  const keep = v.dataset.file === S.file ? v.scrollTop : 0;
  v.dataset.file = S.file;
  v.innerHTML = `<article class="md-body">${mdFull(m.getValue(), S.file)}</article>`;
  v.scrollTop = keep;

  if (monacoReady) v.querySelectorAll('pre code[data-lang]').forEach(c => {
    const lang = { js: 'javascript', ts: 'typescript', sh: 'shell', bash: 'shell', zsh: 'shell', rs: 'rust', py: 'python', yml: 'yaml', md: 'markdown' }[c.dataset.lang] || c.dataset.lang;
    if (!lang || c.textContent.length > 20000) return;
    monaco.editor.colorize(c.textContent, lang, { tabSize: 2 }).then(html => { if (c.isConnected) c.innerHTML = html; }).catch(() => {});
  });
}
function showMdView(on) {
  const v = $('#mdview'); if (!v) return;
  v.hidden = !on;
  if (on) renderMd();
}

const mb = n => (n / 1048576).toFixed(1) + ' MB';
async function openFile(path, opts) {
  if (!S.ws) return;
  if (!S.open.includes(path)) S.open.push(path);
  S.file = path;
  renderTabs();
  renderTree($('#treeFilter')?.value || '');
  const reveal = () => {
    if (opts?.line && S.editor) {
      S.editor.revealLineInCenter(opts.line);
      S.editor.setPosition({ lineNumber: opts.line, column: 1 });
    }
  };

  const cached = S.models.get(path);
  if (cached) {
    S.editor?.setModel(cached); S.editor?.updateOptions({ readOnly: !!fileMeta(path)?.readonly });
    reveal(); drawEdBar();
    reloadFile(path);
    return;
  }
  try {
    const r = await api(`/api/workspaces/${S.ws.id}/file?path=${encodeURIComponent(path)}`);

    const text = r.binary
      ? `// ${path}\n// 二进制文件（${mb(r.size)}），不在编辑器里显示。\n// 要看内容请用下方「终端」面板。`
      : r.too_large
        ? `// ${path}\n// 文件过大（${mb(r.size)}），超出 2 MB 上限未加载。\n// 远程读是整文件传输，大文件会把链路占满。`
        : r.content;
    if (S.editor && monacoReady) {
      const m = monaco.editor.createModel(text, r.binary || r.too_large ? 'plaintext' : langOf(path));
      S.models.set(path, m);
      trackModel(path, m, r);
      S.editor.setModel(m);
      S.editor.updateOptions({ readOnly: !!(r.binary || r.too_large) });
    }
    reveal(); drawEdBar();
    fetch(`/api/workspaces/${S.ws.id}/context/editor`, {
      method: 'PUT', headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ file: path }) }).catch(() => {});
  } catch (e) { toast('打开失败: ' + e.message); }
}
