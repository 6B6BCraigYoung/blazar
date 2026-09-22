const DIFF_PREF_KEY = 'blazar.diffprefs';
const DIFF_BIG = 800;
function diffPrefs() {
  const d = { base: 'head', view: 'unified', w: false, wrap: true, tree: true };
  try { return { ...d, ...JSON.parse(localStorage.getItem(DIFF_PREF_KEY) || '{}') }; } catch (_) { return d; }
}
function setDiffPrefs(p) { try { localStorage.setItem(DIFF_PREF_KEY, JSON.stringify({ ...diffPrefs(), ...p })); } catch (_) {} }

function parseDiff(raw) {
  const files = []; let f = null, h = null, o = 0, n = 0;
  for (const line of raw.split('\n')) {
    if (line.startsWith('diff --git ')) {
      const m = /^diff --git "?a\/(.*?)"? "?b\/(.*?)"?$/.exec(line);
      f = { old: m?.[1] || '', path: m?.[2] || '', status: 'modified', hunks: [], add: 0, del: 0, binary: false, lines: 0 };
      files.push(f); h = null; continue;
    }
    if (!f) continue;
    const hm = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/.exec(line);
    if (hm) { h = { o: +hm[1], n: +hm[2], ctx: hm[3].trim(), lines: [] }; f.hunks.push(h); o = h.o; n = h.n; continue; }
    if (!h) {
      if (line.startsWith('new file mode')) f.status = 'added';
      else if (line.startsWith('deleted file mode')) f.status = 'deleted';
      else if (line.startsWith('rename from ')) { f.status = 'renamed'; f.old = line.slice(12); }
      else if (line.startsWith('rename to ')) f.path = line.slice(10);
      else if (line.startsWith('Binary files') || line.startsWith('GIT binary patch')) f.binary = true;
      else if (line.startsWith('+++ ') && line !== '+++ /dev/null') f.path = line.slice(4).replace(/^"?b\//, '').replace(/"$/, '');
      continue;
    }
    const c = line[0];
    if (c === '+') { h.lines.push({ t: '+', n: n++, text: line.slice(1) }); f.add++; f.lines++; }
    else if (c === '-') { h.lines.push({ t: '-', o: o++, text: line.slice(1) }); f.del++; f.lines++; }
    else if (c === ' ') { h.lines.push({ t: ' ', o: o++, n: n++, text: line.slice(1) }); f.lines++; }
    else if (c === '\\') h.lines.push({ t: '\\', text: line.slice(2) });
  }
  return files;
}

async function loadDiff() {
  if (!S.ws) return;
  const ws = S.ws.id, p = diffPrefs();
  if (S.diff?.ws !== ws) S.diff = { ws, files: [], collapsed: new Set(), expanded: new Set(), q: '' };
  const D = S.diff;
  try {

    let base = '';
    if (p.base === 'target') {
      if (S.git?.ws !== ws) await loadGit(true);
      base = S.git?.target_ok && !S.git.same ? S.git.target : '';
    }
    const qs = new URLSearchParams();
    if (base) qs.set('base', base);
    if (p.w) qs.set('w', '1');
    const r = await api(`/api/workspaces/${ws}/diff?${qs}`);
    if (S.ws?.id !== ws) return;
    Object.assign(D, { files: parseDiff(r.diff || ''), reason: r.reason || '', truncated: !!r.truncated, base, error: '' });
  } catch (e) { Object.assign(D, { files: [], error: e.message }); }
  drawDiff();
}

const DF_MARK = { added: ['A', 'ok'], deleted: ['D', 'bad'], renamed: ['R', 'info'], modified: ['M', 'warn'] };
function drawDiffBar() {
  const bar = $('#diffbar'); if (!bar || !S.diff) return;
  const D = S.diff, p = diffPrefs();
  const add = D.files.reduce((a, f) => a + f.add, 0), del = D.files.reduce((a, f) => a + f.del, 0);
  const target = S.git?.ws === S.ws?.id && S.git.target_ok && !S.git.same ? S.git.target : '';
  const rv = reviewList().length;
  bar.innerHTML = `
    <button class="laybtn" data-a="tree" aria-pressed="${p.tree}" title="改动文件列表">${ICON.left}</button>
    <span class="seg" title="和谁比">
      <button data-a="base" data-v="head" data-on="${p.base === 'head'}" title="还没提交的改动（相对 HEAD）">未提交</button>
      <button data-a="base" data-v="target" data-on="${p.base === 'target'}" title="这条分支相对目标分支一共改了什么">整条分支${target ? ` · ${esc(target)}` : ''}</button>
    </span>
    <span class="seg">
      <button data-a="view" data-v="unified" data-on="${p.view === 'unified'}">统一</button>
      <button data-a="view" data-v="split" data-on="${p.view === 'split'}">并排</button>
    </span>
    <label><input type="checkbox" data-a="w" ${p.w ? 'checked' : ''}>忽略空白</label>
    <label><input type="checkbox" data-a="wrap" ${p.wrap ? 'checked' : ''}>自动换行</label>
    <span class="df-sum num">${D.files.length} 个文件 <b class="ok">+${add}</b> <b class="bad">−${del}</b></span>
    <span class="grow"></span>
    ${rv ? `<span class="badge badge-brand" title="随下一条消息一起发给 agent">${rv} 条意见待发</span>` : ''}
    <button class="btn btn-outline btn-xs" data-a="fold" ${D.files.length ? '' : 'disabled'}>${D.collapsed.size >= D.files.length && D.files.length ? '全部展开' : '全部折叠'}</button>
    <button class="btn btn-outline btn-xs" data-a="review" ${D.files.length ? '' : 'disabled'} title="开一个新对话，让 agent 只审阅不改代码">让 agent 审阅</button>
    <button class="laybtn" data-a="reload" title="刷新">${ICON.refresh}</button>
    <button class="laybtn" data-a="max" aria-pressed="${!!S.lay.panelMax}" title="${S.lay.panelMax ? '还原面板' : '最大化面板'}">⤢</button>`;
  const tab = $('#panelbar .rtab[data-p="diff"]');
  if (tab) tab.innerHTML = `差异${D.files.length ? ` <span class="tabn num">${D.files.length}</span>` : ''}`;
}
function drawDiffTree() {
  const t = $('#difftree'); if (!t || !S.diff) return;
  const D = S.diff, p = diffPrefs();
  t.hidden = !p.tree || !D.files.length;
  if (t.hidden) return;
  const q = (D.q || '').toLowerCase();
  const cm = reviewList();
  t.innerHTML = `<input class="input dt-q" placeholder="筛选文件…" value="${esc(D.q || '')}">` +
    D.files.map((f, i) => {
      if (q && !f.path.toLowerCase().includes(q)) return '';
      const [mk, tone] = DF_MARK[f.status];
      const cut = f.path.lastIndexOf('/');
      const n = cm.filter(c => c.file === f.path).length;
      return `<button class="dt-row" data-i="${i}" title="${esc(f.path)}"><b class="dt-mk ${tone}">${mk}</b>
        <span class="dt-nm">${esc(f.path.slice(cut + 1))}<span class="dt-dir">${cut > 0 ? esc(f.path.slice(0, cut)) : ''}</span></span>
        ${n ? `<span class="dt-cm num">${n}</span>` : ''}<span class="dt-st num"><i class="ok">+${f.add}</i> <i class="bad">−${f.del}</i></span></button>`;
    }).join('');
}
const dkey = (file, side, line) => `${file}|${side}|${line}`;
function commentHtml(c) {
  return `<div class="dcmt" data-id="${c.id}"><div class="dcmt-tx">${esc(c.text).replace(/\n/g, '<br>')}</div>
    <div class="dcmt-ft"><span class="faint">随下一条消息发送</span><span class="grow"></span>
    <button class="linkbtn" data-a="cm-edit">编辑</button><button class="linkbtn" data-a="cm-del">删除</button></div></div>`;
}

const cmBtn = '<button class="dcm" data-a="cm-add" title="对这一行写意见" aria-label="对这一行写意见">+</button>';
function fileBodyHtml(f, fi, view, byKey) {
  if (f.binary) return '<div class="df-note">二进制文件，不显示内容</div>';
  if (!f.hunks.length) return `<div class="df-note">${f.status === 'renamed' ? `从 ${esc(f.old)} 改名，内容没变` : '没有文本改动（可能只改了权限位）'}</div>`;
  const out = [];
  const cmts = (side, line) => (byKey.get(dkey(f.path, side, line)) || []).map(commentHtml).join('');
  const code = l => esc(l.text) || ' ';
  for (const h of f.hunks) {
    out.push(`<div class="dh">@@ −${h.o} +${h.n} @@ <span>${esc(h.ctx)}</span></div>`);
    if (view === 'split') {

      let i = 0; const L = h.lines;
      const half = (l, side) => {
        if (!l) return '<div class="half" data-t="_"></div>';
        const line = side === 'old' ? l.o : l.n;
        return `<div class="half" data-t="${l.t}" data-f="${fi}" data-side="${side}" data-l="${line}"><span class="ln">${line}${cmBtn}</span><span class="dc">${code(l)}</span></div>`;
      };
      while (i < L.length) {
        const l = L[i];
        if (l.t === ' ') { out.push(`<div class="dr2">${half(l, 'old')}${half(l, 'new')}</div>${cmts('new', l.n)}`); i++; continue; }
        if (l.t === '\\') { i++; continue; }
        const dels = [], adds = [];
        while (i < L.length && L[i].t === '-') dels.push(L[i++]);
        while (i < L.length && L[i].t === '+') adds.push(L[i++]);
        for (let k = 0; k < Math.max(dels.length, adds.length); k++) {
          out.push(`<div class="dr2">${half(dels[k], 'old')}${half(adds[k], 'new')}</div>${
            dels[k] ? cmts('old', dels[k].o) : ''}${adds[k] ? cmts('new', adds[k].n) : ''}`);
        }
      }
    } else {
      for (const l of h.lines) {
        if (l.t === '\\') { out.push(`<div class="dr" data-t="n"><span class="ln"></span><span class="ln"></span><span class="dg"></span><span class="dc faint">${esc(l.text)}</span></div>`); continue; }
        const side = l.t === '-' ? 'old' : 'new', line = l.t === '-' ? l.o : l.n;
        out.push(`<div class="dr" data-t="${l.t}" data-f="${fi}" data-side="${side}" data-l="${line}"><span class="ln">${l.o ?? ''}</span><span class="ln">${l.n ?? ''}${cmBtn}</span><span class="dg">${l.t === ' ' ? '' : l.t === '-' ? '−' : '+'}</span><span class="dc">${code(l)}</span></div>${cmts(side, line)}`);
      }
    }
  }
  return out.join('');
}
function drawDiff() {
  const v = $('#diffview'); if (!v || !S.diff) return;
  const D = S.diff, p = diffPrefs();
  drawDiffBar(); drawDiffTree();
  v.dataset.wrap = String(p.wrap || p.view === 'split'); v.dataset.view = p.view;
  if (D.error) { v.innerHTML = `<div class="empty">${esc(D.error)}</div>`; return; }
  if (!D.files.length) {
    v.innerHTML = `<div class="empty">${esc(D.reason || (p.base === 'target'
      ? (D.base ? `相对 ${D.base} 没有改动` : '没找到可对比的目标分支（或当前就在目标分支上）：到 Git 面板里选一个')
      : '没有未提交的改动'))}</div>`;
    return;
  }
  const byKey = new Map();
  for (const c of reviewList()) { const k = dkey(c.file, c.side, c.line); byKey.set(k, [...(byKey.get(k) || []), c]); }
  const keep = v.scrollTop;
  v.innerHTML = (D.truncated ? '<div class="df-note warn">改动太大，只显示了前 3MB</div>' : '') + D.files.map((f, i) => {
    const [mk, tone] = DF_MARK[f.status];
    const folded = D.collapsed.has(f.path);
    const big = f.lines > DIFF_BIG && !D.expanded.has(f.path);
    return `<section class="df" data-i="${i}" data-folded="${folded}">
      <header class="df-head" data-a="toggle"><span class="df-caret">▾</span><b class="dt-mk ${tone}">${mk}</b>
        <span class="df-path">${f.status === 'renamed' ? `${esc(f.old)} → ` : ''}${esc(f.path)}</span>
        <span class="df-st num"><i class="ok">+${f.add}</i> <i class="bad">−${f.del}</i></span><span class="grow"></span>
        ${f.status === 'deleted' ? '' : '<button class="linkbtn" data-a="open" title="在编辑器里打开">打开</button>'}</header>
      ${folded ? '' : `<div class="df-body">${big
        ? `<button class="df-more" data-a="expand">这个文件改了 ${f.lines} 行，点这里展开</button>`
        : fileBodyHtml(f, i, p.view, byKey)}</div>`}</section>`;
  }).join('');
  v.scrollTop = keep;
}
function wireDiff() {
  const bar = $('#diffbar'), v = $('#diffview'), t = $('#difftree');
  if (!bar || bar.dataset.wired) return;
  bar.dataset.wired = '1';
  bar.addEventListener('click', e => {
    const b = e.target.closest('[data-a]'); if (!b || b.tagName === 'INPUT') return;
    const a = b.dataset.a, D = S.diff;
    if (a === 'base') { setDiffPrefs({ base: b.dataset.v }); loadDiff(); }
    else if (a === 'view') { setDiffPrefs({ view: b.dataset.v }); drawDiff(); }
    else if (a === 'tree') { setDiffPrefs({ tree: !diffPrefs().tree }); drawDiff(); }
    else if (a === 'fold') { D.collapsed = D.collapsed.size >= D.files.length ? new Set() : new Set(D.files.map(f => f.path)); drawDiff(); }
    else if (a === 'reload') { loadDiff(); if (S.git) loadGit(true); }
    else if (a === 'review') reviewWithAgent();
    else if (a === 'max') togglePanelMax();
  });
  bar.addEventListener('change', e => {
    const a = e.target.dataset?.a;
    if (a === 'w') { setDiffPrefs({ w: e.target.checked }); loadDiff(); }
    if (a === 'wrap') { setDiffPrefs({ wrap: e.target.checked }); drawDiff(); }
  });
  t.addEventListener('input', e => {
    if (!e.target.classList.contains('dt-q')) return;
    S.diff.q = e.target.value; const pos = e.target.selectionStart;
    drawDiffTree(); const q = $('#difftree .dt-q'); q.focus(); q.setSelectionRange(pos, pos);
  });
  t.addEventListener('click', e => {
    const r = e.target.closest('.dt-row'); if (!r) return;
    const f = S.diff.files[+r.dataset.i];
    if (S.diff.collapsed.delete(f.path)) drawDiff();
    $(`#diffview .df[data-i="${r.dataset.i}"]`)?.scrollIntoView({ block: 'start' });
  });
  v.addEventListener('click', e => {
    const b = e.target.closest('[data-a]'); if (!b) return;
    const a = b.dataset.a, D = S.diff;
    const sec = b.closest('.df'), f = sec && D.files[+sec.dataset.i];
    if (a === 'open') { e.stopPropagation(); openFile(f.path); }
    else if (a === 'toggle') { D.collapsed.has(f.path) ? D.collapsed.delete(f.path) : D.collapsed.add(f.path); drawDiff(); }
    else if (a === 'expand') { D.expanded.add(f.path); drawDiff(); }
    else if (a === 'cm-add') { e.stopPropagation(); editComment(b.closest('[data-l]'), null); }
    else if (a === 'cm-del') { saveReview(reviewList().filter(c => c.id !== b.closest('.dcmt').dataset.id)); drawDiff(); }
    else if (a === 'cm-edit') {
      const box = b.closest('.dcmt'); const c = reviewList().find(x => x.id === box.dataset.id);
      if (c) editComment(null, c, box);
    }
  });
}

function editComment(row, cur, box) {
  $('#diffview .dcmt-edit')?.remove();
  $$('#diffview .dcmt[hidden]').forEach(x => { x.hidden = false; });
  const ed = document.createElement('div'); ed.className = 'dcmt-edit';
  ed.innerHTML = `<textarea class="input" rows="3" placeholder="对这一行的意见：哪里不对、希望怎么改…（⌘↩ 保存）">${esc(cur?.text || '')}</textarea>
    <div class="row"><span class="faint t-micro">意见会攒着，随你的下一条消息一起发给 agent</span><span class="grow"></span>
    <button class="btn btn-outline btn-xs" data-x="no">取消</button><button class="btn btn-brand btn-xs" data-x="ok">${cur ? '保存' : '添加意见'}</button></div>`;
  if (box) { box.after(ed); box.hidden = true; }
  else {

    let at = row.closest('.dr2') || row;
    while (at.nextElementSibling?.classList.contains('dcmt')) at = at.nextElementSibling;
    at.after(ed);
  }
  const ta = ed.querySelector('textarea'); ta.focus();
  const done = ok => {
    const text = ta.value.trim();
    if (ok && text) {
      const list = reviewList();
      if (cur) { const c = list.find(x => x.id === cur.id); if (c) c.text = text; }
      else {
        const f = S.diff.files[+row.dataset.f];
        list.push({ id: Math.random().toString(36).slice(2, 10), file: f.path, side: row.dataset.side, line: +row.dataset.l,
          code: row.querySelector('.dc')?.textContent || '', text });
      }
      saveReview(list);
    }
    drawDiff();
  };
  ed.querySelector('[data-x="no"]').onclick = () => done(false);
  ed.querySelector('[data-x="ok"]').onclick = () => done(true);
  ta.onkeydown = e => {
    if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); done(false); }
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) { e.preventDefault(); done(true); }
  };
}
function togglePanelMax() {
  S.lay.panelMax = !S.lay.panelMax; saveLayout();
  const c = $('#ws-center'); if (c) c.dataset.max = String(!!S.lay.panelMax);
  drawDiffBar(); S.termResize?.();
}

const reviewKey = () => `blazar.review.${S.ws?.id || ''}`;
function reviewList() { try { return JSON.parse(localStorage.getItem(reviewKey()) || '[]'); } catch (_) { return []; } }
function saveReview(list) {
  try { list.length ? localStorage.setItem(reviewKey(), JSON.stringify(list)) : localStorage.removeItem(reviewKey()); } catch (_) {}
  drawReviewBanner(); drawDiffBar(); drawDiffTree();
}
function reviewBlock(list) {
  if (!list.length) return '';
  const tick = '`';
  return `\n\n---\n审阅意见（${list.length} 条），请逐条处理：\n\n` + list.map((c, i) =>
    `${i + 1}. ${tick}${c.file}${tick} 第 ${c.line} 行${c.side === 'old' ? '（被删掉的那一行）' : ''}\n   > ${c.code.trim().slice(0, 200)}\n   ${c.text.replace(/\n/g, '\n   ')}`).join('\n\n');
}
function drawReviewBanner() {
  const el = $('#cbReview'); if (!el) return;
  const n = reviewList().length;
  el.hidden = !n;
  if (!n) return;
  el.innerHTML = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round" aria-hidden="true"><path d="M4 5h16v11H9l-5 4z"/></svg>
    <span>将附带 <b class="num">${n}</b> 条审阅意见</span><span class="grow"></span>
    <button class="linkbtn" data-a="view">查看</button><button class="linkbtn" data-a="clear">清除</button>`;
  el.querySelector('[data-a="view"]').onclick = () => { if (S.lay.hidePanel) toggleRegion('panel'); showPanelTab('diff'); };
  el.querySelector('[data-a="clear"]').onclick = async () => {
    if (await ask(`丢掉这 ${n} 条还没发出去的审阅意见？`, { ok: '丢掉', danger: true })) { saveReview([]); drawDiff(); }
  };
}

function draftChat(text) {
  if (S.lay.hideAux) toggleRegion('aux');
  showAuxTab('chat'); newChat();
  const p = $('#prompt'); if (!p) return;
  p.value = text; growPrompt(); p.focus();
}
function reviewWithAgent() {
  const D = S.diff, p = diffPrefs();
  const scope = p.base === 'target' && D.base
    ? `这条分支相对 ${D.base} 的全部改动（git diff ${D.base}...HEAD，加上还没提交的部分）`
    : '这个工作区里还没提交的改动（git diff HEAD，包括新建的文件）';
  draftChat(`请审阅${scope}，一共 ${D.files.length} 个文件。\n\n重点看：正确性和边界情况、有没有引入回归、错误处理、命名和可读性、该有而没有的测试。\n按严重程度从高到低列出问题，每条给出文件和行号、问题是什么、建议怎么改；没问题的地方不用说。\n只审阅，不要改任何代码。`);
  toast('已在新对话里写好审阅请求：选好 agent 再发送');
}
