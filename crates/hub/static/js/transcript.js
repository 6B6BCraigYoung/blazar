const decisionText = d => d?.kind === 'auto_allowed' ? `自动批准${d.rule ? ' · ' + d.rule : ''}` : (DECISION[d?.kind] || '已处理');
const DECISION = { allow: '已允许', deny: '已拒绝', cancelled: 'agent 撤回了这次询问', aborted: '会话已结束，作废' };
const shortStr = (s, n = 80) => { s = String(s ?? ''); return s.length > n ? s.slice(0, n - 1) + '…' : s; };
const relPath = p => {
  const root = S.ws?.path;
  return p && root && String(p).startsWith(root + '/') ? String(p).slice(root.length + 1) : (p || '');
};
const CC = { toolIn: new Map(), full: new Map(), fullSeq: 0, lastText: '', usage: null, model: '', rate: '', userSeen: new Set(), ask: new Map(), apprReq: new Map() };

function mdInline(escaped) {
  return escaped
    .replace(/`([^`\n]+)`/g, '<code>$1</code>')
    .replace(/\*\*([^*\n]+)\*\*/g, '<b>$1</b>')
    .replace(/\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)/g,
      '<a href="$2" target="_blank" rel="noopener noreferrer">$1</a>');
}
function md(text) {
  const out = [], lines = String(text ?? '').split('\n');
  let para = [], list = null, i = 0;
  const flushPara = () => { if (para.length) { out.push(`<p>${mdInline(esc(para.join('\n')))}</p>`); para = []; } };
  const flushList = () => {
    if (!list) return;
    out.push(`<${list.tag}>${list.items.map(x => `<li>${mdInline(esc(x))}</li>`).join('')}</${list.tag}>`);
    list = null;
  };
  while (i < lines.length) {
    const l = lines[i];
    if (/^\s*```/.test(l)) {
      flushPara(); flushList();
      const buf = []; i++;
      while (i < lines.length && !/^\s*```/.test(lines[i])) buf.push(lines[i++]);
      i++;
      out.push(`<pre class="cb">${esc(buf.join('\n'))}</pre>`);
      continue;
    }
    const h = l.match(/^#{1,4}\s+(.*)/);
    if (h) { flushPara(); flushList(); out.push(`<div class="mh">${mdInline(esc(h[1]))}</div>`); i++; continue; }
    const li = l.match(/^\s*([-*]|\d+\.)\s+(.*)/);
    if (li) {
      flushPara();
      const tag = /\d/.test(li[1]) ? 'ol' : 'ul';
      if (!list || list.tag !== tag) { flushList(); list = { tag, items: [] }; }
      list.items.push(li[2]); i++; continue;
    }
    if (!l.trim()) { flushPara(); flushList(); i++; continue; }
    if (list && /^\s{2,}\S/.test(l)) { list.items[list.items.length - 1] += ' ' + l.trim(); i++; continue; }
    flushList(); para.push(l); i++;
  }
  flushPara(); flushList();
  return out.join('');
}

const toolName = n => String(n || '').replace(/^mcp__blazar__/, '');
const SHELL_TOOLS = new Set(['Bash', 'Shell', 'shell', 'exec', 'command_execution']);

function toolTitle(name, inp) {
  inp = inp && typeof inp === 'object' ? inp : {};
  const cmd = Array.isArray(inp.command) ? inp.command.join(' ') : inp.command;
  const f = {
    Read: () => relPath(inp.file_path) + (inp.offset ? `:${inp.offset}` : ''),
    Write: () => relPath(inp.file_path),
    Edit: () => relPath(inp.file_path) || (Array.isArray(inp) ? '' : ''),
    MultiEdit: () => relPath(inp.file_path),
    NotebookEdit: () => relPath(inp.notebook_path),
    Glob: () => inp.pattern + (inp.path ? ` in ${relPath(inp.path)}` : ''),
    Grep: () => `"${shortStr(inp.pattern, 60)}"` + (inp.path ? ` in ${relPath(inp.path)}` : ''),
    LS: () => relPath(inp.path),
    WebFetch: () => inp.url,
    WebSearch: () => inp.query,
    Task: () => inp.description,
    Agent: () => inp.description,
    TodoWrite: () => '',
    ExitPlanMode: () => '',
  }[name];
  const arg = SHELL_TOOLS.has(name) ? shortStr(String(cmd || '').split('\n')[0], 140)
    : f ? f() : shortStr(JSON.stringify(inp), 100);
  const label = { TodoWrite: 'Update Todos', Shell: 'Bash', Task: 'Task', Agent: 'Task', ExitPlanMode: '计划' }[name] || name;
  return { label, arg };
}

function todoList(todos) {
  if (!Array.isArray(todos) || !todos.length) return '';
  return `<div class="cc-todos">${todos.map(t => {
    const st = t.status || 'pending';
    const box = st === 'completed' ? '☒' : '☐';
    return `<div class="cc-todo ${esc(st)}"><span>${box}</span><span>${esc(t.content || t.activeForm || '')}</span></div>`;
  }).join('')}</div>`;
}

function editDiff(inp) {
  const edits = Array.isArray(inp?.edits) ? inp.edits
    : inp && (inp.old_string != null || inp.new_string != null) ? [inp] : [];
  if (!edits.length) {

    const files = Array.isArray(inp) ? inp.map(c => c.path || c.file || '').filter(Boolean) : [];
    return files.length ? `<div class="cc-diff">${files.map(f =>
      `<div class="dh">${esc(relPath(f))}</div>`).join('')}</div>` : '';
  }
  const MAX = 14;
  let rows = [];
  for (const e of edits) {
    const del = String(e.old_string ?? '').split('\n'), add = String(e.new_string ?? '').split('\n');
    if (e.old_string) rows.push(...del.map(l => ['del', l]));
    if (e.new_string) rows.push(...add.map(l => ['add', l]));
    if (edits.length > 1) rows.push(['gap', '⋯']);
  }
  const more = rows.length - MAX;
  const body = rows.slice(0, MAX).map(([c, l]) =>
    `<div class="d${c}"><span class="dm">${c === 'add' ? '+' : c === 'del' ? '-' : ' '}</span>${esc(l)}</div>`).join('');
  return `<div class="cc-diff">${body}${more > 0 ? `<div class="dgap">… 还有 ${more} 行</div>` : ''}</div>`;
}

function resLines(text, cls = '', max = 4) {
  const lines = String(text).split('\n');
  const more = lines.length - max;
  let btn = '';
  if (more > 0) {
    const key = ++CC.fullSeq;
    CC.full.set(key, text);
    btn = `<button class="cc-more" data-full="${key}">… +${more} 行（点击展开）</button>`;
  }
  return `<div class="cc-out ${cls}"><span class="cc-elbow">⎿</span><div class="cc-pre">${
    esc(lines.slice(0, max).join('\n'))}${btn}</div></div>`;
}

function resultHtml(name, k, inp) {
  const text = String(k.content ?? '').replace(/\s+$/, '');
  const n = text ? text.split('\n').length : 0;
  if (!k.ok) return resLines(text || '失败', 'err', 6);
  if (name === 'Read') return resLines(`读取了 ${n} 行`, 'sum');
  if (name === 'TodoWrite') return '';
  if (name === 'ExitPlanMode') return resLines('计划已批准', 'sum');
  if (name === 'Edit' || name === 'MultiEdit') return resLines(`已修改 ${relPath(inp?.file_path) || '文件'}`, 'sum');
  if (name === 'Write') {
    const w = String(inp?.content ?? '').split('\n').length;
    return resLines(`写入 ${w} 行 → ${relPath(inp?.file_path)}`, 'sum');
  }
  if (name === 'Glob' || name === 'LS') return resLines(n ? `找到 ${n} 项\n${text}` : '没有匹配', 'sum', 1) ;
  if (name === 'Grep') return resLines(n ? `找到 ${n} 处\n${text}` : '没有匹配', 'sum', 1);
  if (name === 'Task' || name === 'Agent') return resLines(text || '完成', '', 3);
  return resLines(text || '(无输出)', text ? '' : 'sum');
}

function toolBlock(k) {
  const t = toolTitle(k.name, k.input);
  const extra = k.name === 'TodoWrite' ? todoList(k.input?.todos)
    : (k.name === 'Edit' || k.name === 'MultiEdit') ? editDiff(k.input)
    : k.name === 'ExitPlanMode' && k.input?.plan ? `<details class="cc-planbody"><summary>计划内容</summary><div class="cc-md">${md(k.input.plan)}</div></details>` : '';
  return `<div class="cc-row cc-tool" data-tool="${esc(k.id)}" data-st="run" data-name="${esc(k.name)}">
    <span class="cc-dot"></span><div class="cc-main">
      <div class="cc-head"><b>${esc(t.label)}</b>${t.arg ? `<span class="cc-arg">(${esc(t.arg)})</span>` : ''}</div>
      ${extra}<div class="cc-res"></div></div></div>`;
}

function approvalCard(k, resolved) {
  const r = k.request || {};
  const inp = r.input || {};
  const tn = toolName(r.tool_name);
  const tool = r.display_name || tn || '工具';
  const title = SHELL_TOOLS.has(tn) ? `Bash 命令${r.tool_name?.startsWith('mcp__blazar__') && S.ws ? ` · 在 ${S.ws.node} 上` : ''}` : tool;
  const what = inp.command ? (Array.isArray(inp.command) ? inp.command.join(' ') : inp.command)
    : inp.file_path ? relPath(inp.file_path)
    : JSON.stringify(inp, null, 2).slice(0, 800);
  if (tn === 'ExitPlanMode') {

    return `<div class="cc-appr cc-plan" data-appr="${esc(k.id)}" data-done="${resolved ? 'true' : 'false'}">
    <div class="ah">计划</div>
    <div class="cc-md aplan">${md(String(inp.plan || ''))}</div>
    <div class="aq">按这个计划开始？</div>
    <div class="aopts">
      <button class="aopt" data-ok data-mode="acceptEdits"><span class="ak">1</span>是，并自动批准改动</button>
      <button class="aopt" data-ok data-mode="default"><span class="ak">2</span>是，每次改动前问我</button>
      <button class="aopt" data-no><span class="ak">3</span>否，继续规划 <span class="faint">(esc)</span></button>
    </div>
    <div class="adeny"><input class="input input-sm" placeholder="要怎么改计划，回车发送（可留空）"></div>
    <div class="apstate">${resolved ? esc(resolved) : ''}</div>
  </div>`;
  }
  const diff = (tn === 'Edit' || tn === 'MultiEdit') ? editDiff(inp) : '';
  CC.apprReq.set(k.id, r);
  return `<div class="cc-appr" data-appr="${esc(k.id)}" data-done="${resolved ? 'true' : 'false'}">
    <div class="ah">${esc(title)}</div>
    <pre class="acmd">${esc(what)}</pre>${diff}
    ${r.description ? `<div class="adesc">${esc(r.description)}</div>` : ''}
    ${r.blocked_path ? `<div class="adesc">涉及路径：${esc(r.blocked_path)}</div>` : ''}
    <div class="aq">要继续吗？</div>
    <div class="aopts">
      <button class="aopt" data-ok><span class="ak">1</span>是</button>
      <button class="aopt" data-always><span class="ak">2</span>是，这个工作区里以后都允许</button>
      <button class="aopt" data-no><span class="ak">3</span>否，告诉 agent 该怎么做 <span class="faint">(esc)</span></button>
    </div>
    <div class="adeny"><input class="input input-sm" placeholder="换个做法的说明，回车发送（可留空）"></div>
    <div class="apstate">${resolved ? esc(resolved) : ''}</div>
  </div>`;
}

const SUB_KINDS = new Set(['user_message', 'assistant_message', 'thinking', 'tool_use', 'tool_result']);

function subBox(log, parent) {
  const host = log.querySelector(`.cc-tool[data-tool="${CSS.escape(parent)}"]`);
  if (!host) return null;
  let d = host.querySelector(':scope > .cc-main > .cc-sub');
  if (!d) {
    host.querySelector(':scope > .cc-main > .cc-res').insertAdjacentHTML('beforebegin',
      '<details class="cc-sub"><summary>子 agent 工作中…</summary><div class="cc-subl"></div></details>');
    d = host.querySelector(':scope > .cc-main > .cc-sub');
  }
  return d;
}
function subCount(d, last) {
  d.dataset.n = String((+d.dataset.n || 0) + 1);
  d.querySelector('summary').textContent = `${d.dataset.n} 次工具调用 · 最近：${last}`;
}

const TODO_KEY = 'blazar.todosOpen';
function drawTodos() {
  const el = $('#ccTodos'); if (!el) return;
  const t = Array.isArray(S.todos) ? S.todos : [];
  const done = t.filter(x => x.status === 'completed').length;
  if (!t.length || (done === t.length && !wsRunning())) { el.hidden = true; el.innerHTML = ''; return; }
  let open = true; try { open = localStorage.getItem(TODO_KEY) !== '0'; } catch (_) {}
  const cur = t.find(x => x.status === 'in_progress');
  el.hidden = false;
  el.innerHTML = `<button class="td-h" data-td>${open ? '▾' : '▸'} <b>Todos ${done}/${t.length}</b>${
    cur ? `<span class="td-cur">${esc(cur.activeForm || cur.content || '')}</span>` : ''}</button>${open ? todoList(t) : ''}`;
}

async function refreshCheckpoints() {
  if (!S.ws) return;
  const ws = S.ws.id;
  try {
    const cps = await api(`/api/workspaces/${ws}/checkpoints`);
    if (S.ws?.id !== ws) return;
    const by = new Map(cps.filter(c => c.session_id && c.seq != null).map(c => [`${c.session_id}:${c.seq}`, c.id]));
    document.querySelectorAll('#log .cc-user[data-sid]').forEach(u => {
      const id = by.get(`${u.dataset.sid}:${u.dataset.seq}`);
      if (id) u.dataset.cp = id;
    });
  } catch (_) {  }
}
let cpTimer;
const refreshCpSoon = () => { clearTimeout(cpTimer); cpTimer = setTimeout(refreshCheckpoints, 800); };
let changesTimer;
function refreshChangesSoon() {
  clearTimeout(changesTimer);
  changesTimer = setTimeout(() => { if (S.ws) { loadTree(); loadDiff(); loadGit(S.lay.panelTab !== 'git'); if (S.file) reloadFile(S.file); } }, 700);
}

function reloadWorkspaceFiles() {
  loadTree(); loadDiff();

  for (const p of S.open) reloadFile(p);
}
async function rewind(cp, isUndo = false) {
  if (!isUndo && !await ask('把工作区的文件恢复到这条消息发出之前？\n对话记录不变；回退前的状态会先存一份，可以撤销。', { ok: '回退' })) return;
  try {
    const r = await post(`/api/checkpoints/${cp}/restore`);
    reloadWorkspaceFiles();
    if (isUndo) toast('已撤销回退');
    else if (r.undo) toastAction('已回退到这条消息之前', '撤销', () => rewind(r.undo, true));
    else toast('已回退');
  } catch (e) { toast('回退失败: ' + e.message); }
}

function renderKind(log, k, resolved, meta = {}) {

  const sub = meta.parent && SUB_KINDS.has(k.type) ? subBox(log, meta.parent) : null;
  if (sub && k.type === 'user_message') return;
  const into = sub ? sub.querySelector('.cc-subl') : log;
  const add = html => into.insertAdjacentHTML('beforeend', html);
  const prevTs = CC.lastTs;
  if (meta.ts) { const t = Date.parse(meta.ts); if (t) CC.lastTs = t; }
  if (k.type === 'thinking') CC.lastTs = prevTs;
  switch (k.type) {
    case 'user_message':
    {

      const first = !!meta.sid && !CC.userSeen.has(meta.sid) && !meta.rewound;
      if (meta.sid) CC.userSeen.add(meta.sid);
      add(`<div class="cc-user"${meta.sid ? ` data-sid="${esc(meta.sid)}" data-seq="${meta.seq}"` : ''} data-first="${first}"><span class="cc-gt">&gt;</span><div class="cc-ut">${esc(k.text)}</div><span class="cc-acts">${
        first ? '<button class="cc-rw" data-edit title="改一改这条消息，从这里重来">✎ 编辑并重试</button><button class="cc-rw" data-regen title="原话不变，让 agent 重新回答这一轮">⟳ 重新生成</button>' : ''
      }<button class="cc-rw" data-rw title="把工作区的文件恢复到这条消息发出之前">↺ 回退到这里</button></span></div>`);
    }
      break;
    case 'assistant_message':
      if (!k.text?.trim()) break;
      CC.lastText = k.text.trim();
      add(`<div class="cc-row cc-msg"><span class="cc-dot"></span><div class="cc-main cc-md">${md(k.text)}</div></div>`);
      break;
    case 'thinking': {

      const secs = meta.ts && CC.lastTs ? Math.max(1, Math.round((Date.parse(meta.ts) - CC.lastTs) / 1000)) : 0;
      const head = `✻ Thought${secs ? ` for ${secs}s` : ''}`;
      add(k.text?.trim()
        ? `<details class="cc-think"><summary>${head}</summary><div class="cc-md">${md(k.text)}</div></details>`
        : `<div class="cc-thinkmark">${head}</div>`);
      break;
    }
    case 'tool_use': {

      const t = { ...k, name: toolName(k.name) };
      CC.toolIn.set(k.id, { name: t.name, input: k.input });
      add(toolBlock(t));
      if (sub) { const tt = toolTitle(t.name, t.input); subCount(sub, tt.arg ? `${tt.label} ${shortStr(tt.arg, 40)}` : tt.label); }
      if (t.name === 'TodoWrite' && !sub && Array.isArray(k.input?.todos)) { S.todos = k.input.todos; drawTodos(); }
      break;
    }
    case 'tool_result': {
      const el = log.querySelector(`[data-tool="${CSS.escape(k.id)}"]`);
      const t = CC.toolIn.get(k.id);
      if (el) {
        el.dataset.st = k.ok ? 'ok' : 'err';
        el.querySelector('.cc-res').innerHTML = resultHtml(el.dataset.name, k, t?.input);
      } else {
        add(`<div class="cc-row cc-orphan">${resultHtml(t?.name || '', k, t?.input)}</div>`);
      }
      break;
    }
    case 'approval':
      add((toolName(k.request?.tool_name) === 'AskUserQuestion' ? askCard : approvalCard)(k, resolved?.get(k.id)).replace('<div class="cc-appr', `<div${meta.sid ? ` data-sid="${esc(meta.sid)}"` : ''} class="cc-appr`));
      break;
    case 'approval_resolved':
      markApproval(k.id, decisionText(k.decision));
      break;
    case 'input_consumed':
      log.querySelector('.cc-user:not(.ack)')?.classList.add('ack');
      break;
    case 'session_started':
      CC.model = k.model || CC.model;
      CC.sess = { output_style: k.output_style, fast_mode: k.fast_mode, fast_mode_reason: k.fast_mode_reason,
        mcp_servers: k.mcp_servers || [] };
      drawMcpBtn();
      add(`<div class="cc-sep" title="${esc([k.model, PERM_TEXT[k.permission_mode] || k.permission_mode].filter(Boolean).join(' · '))}">新会话${
        /\/brain\//.test(k.cwd || '') && S.ws ? ` · ${esc(S.ws.node)}` : ''}</div>`);
      break;
    case 'token_usage':
      CC.usage = k;
      break;
    case 'rate_limit':
      CC.rate = (k.windows || []).map(w => `${w.name} ${(w.utilization * 100).toFixed(0)}%`).join(' · ');
      saveRate(k.windows);
      break;
    case 'error':
      add(`<div class="cc-row cc-err"><span class="cc-dot"></span><div class="cc-main">${esc(k.message)}</div></div>`);
      break;
    case 'background_task': {
      const bad = ['killed', 'stopped', 'failed'].includes(k.status);
      const what = k.description || k.task_id;
      if (k.status === 'started') {
        add(`<div class="cc-row cc-bg"><span class="cc-dot"></span><div class="cc-main"><b>后台任务</b><span class="cc-arg">(${esc(shortStr(what, 100))})</span></div></div>`);
      } else {
        add(`<div class="cc-row cc-bg">${resLines(bad
          ? `后台任务「${what}」随这一轮结束被终止了（${k.status}），没有跑完。需要长时间跑的，让 agent 用 tmux new -d / setsid nohup 脱离，或在终端面板里跑。`
          : `后台任务「${what}」：${k.status}`, bad ? 'err' : 'sum', 6)}</div>`);
      }
      break;
    }
    case 'finished': {

      log.querySelectorAll('.cc-tool[data-st="run"]').forEach(e => { e.dataset.st = 'ok'; });
      setTimeout(drawTodos, 0);
      if (k.usage) CC.usage = k.usage;
      if (k.status !== 'success') {
        const why = k.status === 'interrupted' ? '已中断' : (k.message || k.status);
        add(`<div class="cc-row cc-err"><span class="cc-dot"></span><div class="cc-main">${esc(why)}</div></div>`);
      } else {

        if (k.text?.trim() && k.text.trim() !== CC.lastText) {
          add(`<div class="cc-row cc-msg"><span class="cc-dot"></span><div class="cc-main cc-md">${md(k.text)}</div></div>`);
        }
        if (k.denied?.length) {
          add(`<div class="cc-row cc-warn"><span class="cc-dot"></span><div class="cc-main">${k.denied.length} 次操作被权限拦下，没有执行：${
            k.denied.map(d => `<div class="faint">· ${esc(d)}</div>`).join('')}
            ${TIP('无头模式下 agent 不会停下来问你，而是直接拒绝。需要放行就把权限模式换成「自动批准改动」。')}</div></div>`);
        }
      }
      const u = CC.usage;
      add(`<div class="cc-sep" title="${u ? `↑${fmt(u.input)} ↓${fmt(u.output)}` : ''}">${k.status === 'success' ? '本轮完成' : '本轮结束'}${
        u?.cost_usd ? ` · $${u.cost_usd.toFixed(2)}` : ''}</div>`);
      CC.lastText = '';
      break;
    }
    default: break;
  }
  drawStatus();
}

const chatKey = () => 'blazar.chats.' + S.ws.id;
function threadTitle(id) {
  if (!id) return 'New chat';
  const t = (S.threads || []).find(x => x.id === id);
  return t?.title || '对话';
}
function saveTabs() {
  try { localStorage.setItem(chatKey(), JSON.stringify({ tabs: S.tabs.map(t => t.id), active: S.viewSession })); } catch (_) {}
}

let threadsTimer = 0;
function refreshThreadsSoon() {
  if (threadsTimer) return;
  threadsTimer = setTimeout(() => { threadsTimer = 0; if (S.ws && S.tabs) loadThreads().then(drawChatTabs); }, 2500);
}
async function loadThreads() {
  try { S.threads = await api(`/api/workspaces/${S.ws.id}/sessions`); } catch (_) { S.threads = S.threads || []; }
  return S.threads;
}
async function initChatTabs() {
  const ws = S.ws.id;
  await loadThreads();
  if (S.ws?.id !== ws) return;
  let saved = null; try { saved = JSON.parse(localStorage.getItem(chatKey()) || 'null'); } catch (_) {}
  const known = new Set(S.threads.map(t => t.id));
  let ids = (saved?.tabs || []).filter(id => id === null || known.has(id));
  if (!ids.length) ids = S.threads.length ? [S.threads[0].id] : [null];
  S.tabs = ids.map(id => ({ id }));
  S.viewSession = ids.includes(saved?.active ?? '__') ? saved.active : ids[0];
  let fresh = false; try { fresh = localStorage.getItem(chatKey() + '.newchat') === '1'; localStorage.removeItem(chatKey() + '.newchat'); } catch (_) {}
  if (fresh) { if (!S.tabs.some(t => t.id === null)) S.tabs.push({ id: null }); S.viewSession = null; }
  S.threadSessions = new Set();
  drawChatTabs();
  await loadHistory();
}
function drawChatTabs() {
  const host = $('#chatTabs'); if (!host || !S.tabs) return;
  const runningThread = id => (S.threads || []).find(t => t.id === id)?.status === 'running';
  host.innerHTML = S.tabs.map(t => `<button class="ctab" data-tid="${esc(t.id || '')}" data-active="${t.id === S.viewSession}" title="${esc(threadTitle(t.id))}（双击改名）">${
    t.id ? rtIcon((S.threads || []).find(x => x.id === t.id)?.runtime) : ''}<span class="t">${esc(threadTitle(t.id))}</span>${
    runningThread(t.id) ? '<span class="live"></span>' : ''}<span class="x" data-close="${esc(t.id || '')}" title="关闭标签">×</span></button>`).join('')
    + '<button class="ctab add" data-add title="新对话">＋</button>';
  host.querySelector('[data-active="true"]')?.scrollIntoView({ inline: 'nearest', block: 'nearest' });
}
async function activateTab(id) {
  if (!S.tabs.some(t => t.id === id)) S.tabs.push({ id });
  S.viewSession = id;
  setFresh(!id);
  saveTabs(); drawChatTabs();
  await loadHistory();
}
function closeChatTab(idRaw) {
  const id = idRaw === '' ? null : idRaw;
  const i = S.tabs.findIndex(t => t.id === id); if (i < 0) return;
  S.tabs.splice(i, 1);
  if (!S.tabs.length) S.tabs.push({ id: null });
  if (S.viewSession === id) activateTab(S.tabs[Math.min(i, S.tabs.length - 1)].id);
  else { saveTabs(); drawChatTabs(); }
}

function openThread(id) { activateTab(id); }

function noteSent(r) {
  if (!r?.session_id) return;
  S.threadSessions.add(r.session_id);
  if (r.thread_id && r.thread_id !== S.viewSession) {
    const cur = S.tabs.find(t => t.id === S.viewSession);
    if (cur && S.viewSession === null) cur.id = r.thread_id; else if (!S.tabs.some(t => t.id === r.thread_id)) S.tabs.push({ id: r.thread_id });
    S.viewSession = r.thread_id;
    setFresh(false);
    loadThreads().then(drawChatTabs);
    saveTabs(); drawChatTabs();
  }

  loadHistory();
}

async function openHistPop() {
  const list = await loadThreads();
  const when = t => { try { return new Date(t).toLocaleString('zh-CN', { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' }); } catch (_) { return ''; } };
  const rtName = r => (S.profiles || []).find(p => p.id === r.profile)?.name || AGENT_LABEL[r.runtime] || r.runtime;
  const p = openPop(`<div class="cp-sec">Conversations</div>
    <button class="cp-row" data-h="new"><span class="cp-t"><b>＋ New conversation</b></span></button>
    ${list.map(r => `<button class="cp-row hist-row" data-h="${esc(r.id)}" data-sel="${S.viewSession === r.id}">${rtMark(r.runtime, 'sm')}
      <span class="cp-t"><b>${esc(r.title || '(empty)')}</b><span>${esc(rtName(r))} · ${esc(when(r.last_at || r.created_at))}${r.status === 'running' ? ' · running' : ''}</span></span>
      ${S.viewSession === r.id ? '<span class="cp-ok">✓</span>' : ''}</button>`).join('')}`);
  p.querySelectorAll('[data-h]').forEach(b => {
    b.onclick = () => {
      closePop();
      if (b.dataset.h === 'new') { newChat(); return; }
      openThread(b.dataset.h);
    };
  });
}

function newChat() {
  if (!S.tabs) S.tabs = [];
  if (!S.tabs.some(t => t.id === null)) S.tabs.push({ id: null });
  activateTab(null);
  $('#prompt')?.focus();
}

function pinCurrentQuestion() {
  const log = $('#log'); if (!log) return;
  const us = log.querySelectorAll(':scope > .cc-user');
  let cur = null;
  for (const u of us) {

    const top = u.dataset.pin === 'true' ? +u.dataset.top : u.offsetTop;
    u.dataset.top = top;
    if (top <= log.scrollTop + 2) cur = u; else break;
  }
  us.forEach(u => { if (u !== cur && u.dataset.pin === 'true') delete u.dataset.pin; });
  if (cur && cur.dataset.pin !== 'true') cur.dataset.pin = 'true';
}
let pinRaf = 0;
document.addEventListener('scroll', e => {
  if (e.target?.id !== 'log' || pinRaf) return;
  pinRaf = requestAnimationFrame(() => { pinRaf = 0; pinCurrentQuestion(); });
}, true);
async function loadHistory() {
  try {
    S.threadSessions = new Set();
    if (!S.viewSession) {

      S.session = null; S.todos = null; drawTodos();
      CC.toolIn.clear(); CC.full.clear(); CC.lastText = ''; CC.usage = null; CC.model = ''; CC.sess = null; CC.lastTs = 0; CC.userSeen.clear();
      drawQueue();
      S.seen = new Set();
      const empty = $('#log');
      if (empty) empty.innerHTML = emptyChatHtml();
      drawStatus();
      return;
    }
    const rows = await api(`/api/workspaces/${S.ws.id}/history?session=${encodeURIComponent(S.viewSession)}`);
    if (S.inboxUnread) seenThread(S.viewSession);
    rows.forEach(r => S.threadSessions.add(r.session_id));

    S.session = rows.length ? rows[rows.length - 1].session_id : S.viewSession;
    S.seen = new Set(rows.map(r => r.session_id + ':' + r.seq));
    const log = $('#log');
    if (!log) return;
    CC.toolIn.clear(); CC.full.clear(); CC.lastText = ''; CC.usage = null; CC.model = ''; CC.rate = ''; CC.sess = null; CC.lastTs = 0; CC.userSeen.clear();
    drawQueue();

    const resolved = new Map(rows.filter(r => r.kind.type === 'approval_resolved')
      .map(r => [r.kind.id, decisionText(r.kind.decision)]));
    log.innerHTML = '';
    S.todos = null;
    let rwBox = null;
    for (const r of rows) {
      let into = log;
      if (r.rewound) {
        if (!rwBox) {
          rwBox = document.createElement('details'); rwBox.className = 'cc-rewound';
          rwBox.innerHTML = '<summary></summary><div class="cc-rwl"></div>'; rwBox.turns = 0;
          log.appendChild(rwBox);
        }
        if (r.kind.type === 'user_message' && !r.parent) rwBox.turns += 1;
        rwBox.firstChild.textContent = `已回退的历史 · ${rwBox.turns} 条消息（编辑并重试时丢弃，agent 不再记得）`;
        into = rwBox.lastChild;
      } else rwBox = null;
      renderKind(into, r.kind, resolved, { parent: r.parent, sid: r.session_id, seq: r.seq, ts: r.ts, rewound: r.rewound });
    }
    drawTodos(); refreshCheckpoints();
    if (!log.children.length) log.innerHTML = emptyChatHtml();
    log.scrollTop = log.scrollHeight;
    pinCurrentQuestion();
  } catch (e) { const l = $('#log'); if (l) l.innerHTML = `<div class="empty">${esc(e.message)}</div>`; }
}
function appendEvent(sid, entry) {
  const key = sid + ':' + entry.seq;
  if (S.seen.has(key)) return;
  S.seen.add(key);
  const log = $('#log');
  if (!log) return;
  log.querySelector('.cc-empty')?.remove();
  const atBottom = log.scrollHeight - log.scrollTop - log.clientHeight < 80;
  renderKind(log, entry.kind, null, { parent: entry.parent_tool_use_id, sid, seq: entry.seq, ts: entry.ts });
  if (entry.kind.type === 'user_message') refreshCpSoon();
  if (atBottom) log.scrollTop = log.scrollHeight;
  pinCurrentQuestion();
}
function markApproval(id, text) {
  document.querySelectorAll(`[data-appr="${CSS.escape(id)}"]`).forEach(c => {
    c.dataset.done = 'true'; c.dataset.deny = 'false';
    const st = c.querySelector('.apstate'); if (st) st.textContent = text;
  });
}

async function decide(id, allow, message = '') {
  try {
    const r = await post(`/api/approvals/${id}`, { allow, message });
    if (r.delivered) { markApproval(id, allow ? '已允许' : '已拒绝'); return true; }
    toast(r.reason || '没能送达');
    markApproval(id, r.reason || '没能送达');
  } catch (e) { toast('裁决失败: ' + e.message); }
  return false;
}
function openDeny(card) {
  card.dataset.deny = 'true';
  const inp = card.querySelector('.adeny input');
  inp.focus();
  inp.onkeydown = e => {
    if (e.key === 'Enter' && !e.isComposing) { e.preventDefault(); decide(card.dataset.appr, false, inp.value.trim()); }
    if (e.key === 'Escape') { e.stopPropagation(); card.dataset.deny = 'false'; }
  };
}
document.addEventListener('click', e => {
  const more = e.target.closest('.cc-more');
  if (more) {
    const full = CC.full.get(+more.dataset.full);
    if (full != null) more.parentElement.textContent = full;
    return;
  }
  const rw = e.target.closest('[data-rw]');
  if (rw) { const u = rw.closest('.cc-user'); if (u?.dataset.cp) rewind(u.dataset.cp); return; }
  const ed = e.target.closest('[data-edit]');
  if (ed) { editUserMessage(ed.closest('.cc-user')); return; }
  const rg = e.target.closest('[data-regen]');
  if (rg) { retryMessage(rg.closest('.cc-user'), null); return; }
  const ao = e.target.closest('.askopt');
  if (ao && ao.closest('[data-appr]')?.dataset.done !== 'true') {
    const box = ao.closest('.askq'), on = ao.getAttribute('aria-pressed') === 'true';
    if (box.dataset.multi !== 'true') box.querySelectorAll('.askopt').forEach(x => x.setAttribute('aria-pressed', 'false'));
    ao.setAttribute('aria-pressed', String(!on));
    return;
  }
  const ask1 = e.target.closest('[data-ask-ok]');
  if (ask1) { const c = ask1.closest('[data-appr]'); if (c?.dataset.done !== 'true') submitAsk(c); return; }
  if (e.target.closest('[data-td]')) {
    const el = $('#ccTodos'); const open = !!el?.querySelector('.cc-todos');
    try { localStorage.setItem(TODO_KEY, open ? '0' : '1'); } catch (_) {}
    drawTodos(); return;
  }
  const card = e.target.closest('[data-appr]');
  if (!card || card.dataset.done === 'true') return;
  const ok = e.target.closest('[data-ok]');
  if (ok) {
    const mode = ok.dataset.mode;
    if (mode) {

      const sid = card.dataset.sid || S.session;
      const m = $('#permMode'); if (m) { m.value = mode; drawMode(); }
      liveControl({ permission_mode: mode }, '权限模式', sid).finally(() => decide(card.dataset.appr, true));
    } else decide(card.dataset.appr, true);
  }
  else if (e.target.closest('[data-always]')) allowAlways(card);
  else if (e.target.closest('[data-no]')) openDeny(card);
});

document.addEventListener('keydown', e => {
  if (e.metaKey || e.ctrlKey || e.altKey || $('#dlg')?.dataset.open === 'true') return;
  if (e.target.closest?.('input, textarea, select, [contenteditable]')) return;
  const card = document.querySelector('#log [data-appr][data-done="false"], #apprAll [data-appr][data-done="false"]');
  if (!card) return;
  const opts = card.querySelectorAll('.aopt');
  const n = /^[1-9]$/.test(e.key) ? +e.key : 0;
  if (n && n <= opts.length) { e.preventDefault(); opts[n - 1].click(); }
  else if (e.key === 'Escape') { e.preventDefault(); openDeny(card); }
});

const PERM_TEXT = { '': 'Manual', acceptEdits: 'Edit automatically', default: 'Manual',
  bypassPermissions: 'Bypass permissions', plan: 'Plan', auto: 'Auto',
  'read-only': 'Read Only', 'workspace-write': 'Auto', 'danger-full-access': 'Full Access' };
const IC = {
  hand: '<path d="M8 12V6.5a1.5 1.5 0 0 1 3 0V11M11 10V5a1.5 1.5 0 0 1 3 0v6M14 10V6.5a1.5 1.5 0 0 1 3 0V14a6 6 0 0 1-6 6h-.5a6 6 0 0 1-4.9-2.5L3.8 14a1.5 1.5 0 0 1 2.4-1.8L8 14"/>',
  code: '<path d="M9 8l-4 4 4 4M15 8l4 4-4 4"/>',
  plan: '<path d="M4 5h16v14H4z"/><path d="M8 15l3-3 2 2 3-4"/>',
  bolt: '<path d="M13 3L5 14h6l-1 7 8-11h-6z"/>',
  gear: '<circle cx="12" cy="12" r="3"/><path d="M12 3v3M12 18v3M3 12h3M18 12h3M5.6 5.6l2.1 2.1M16.3 16.3l2.1 2.1M5.6 18.4l2.1-2.1M16.3 7.7l2.1-2.1"/>',
  warn: '<path d="M12 4l9 16H3z"/><path d="M12 10v4M12 17v.5"/>',
  up: '<path d="M12 16V4M7 9l5-5 5 5"/><path d="M4 16v4h16v-4"/>',
  file: '<path d="M6 3h8l4 4v14H6z"/><path d="M14 3v4h4M9 13h6M9 17h4"/>',
  globe: '<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c3 3.5 3 14.5 0 18M12 3c-3 3.5-3 14.5 0 18"/>',
  user: '<circle cx="12" cy="8" r="4"/><path d="M4 21c1.5-4 4.5-6 8-6s6.5 2 8 6"/>',
  cpu: '<rect x="6" y="6" width="12" height="12" rx="2"/><path d="M9 3v3M15 3v3M9 18v3M15 18v3M3 9h3M3 15h3M18 9h3M18 15h3"/>',
};
const svgI = (k, cls = '') => `<svg class="${cls}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${IC[k]}</svg>`;

const MODES_CLAUDE = [
  ['default', 'Manual', 'Claude will ask for approval before making each edit', 'hand'],
  ['acceptEdits', 'Edit automatically', 'Claude will edit your selected text or the whole file', 'code'],
  ['plan', 'Plan', 'Claude will explore the code and present a plan before editing', 'plan'],
  ['auto', 'Auto', 'Claude will approve actions that pass a safety check and pause for anything risky', 'bolt'],
  ['bypassPermissions', 'Bypass permissions', 'Claude will not ask about anything. Use with care.', 'warn'],
];
const MODES_CODEX = [
  ['read-only', 'Read Only', 'Codex can read files and answer questions. Codex requires approval to make edits, run commands, or access network.', 'plan'],
  ['workspace-write', 'Auto', 'Codex can read files, make edits, and run commands in the workspace. Codex requires approval to work outside the workspace or access network.', 'bolt'],
  ['danger-full-access', 'Full Access', 'Codex can read files, make edits, and run commands with network access, without approval. Exercise caution.', 'warn'],
];
const modeList = () => modesFor(currentRuntime());
const SPIN = ['·', '✢', '✳', '✶', '✻', '✽', '✻', '✶', '✳', '✢'];
const WORD = ['思考中', '推敲中', '干活中', '琢磨中', '处理中'];
let runSince = null, spinI = 0;
function wsRunning() {
  const w = S.ws && S.workspaces.find(x => x.id === S.ws.id);
  return w?.activity === 'running';
}
const fmtDur = sec => sec < 60 ? `${sec}s` : sec < 3600 ? `${Math.floor(sec / 60)}m ${sec % 60}s`
  : `${Math.floor(sec / 3600)}h ${Math.floor(sec % 3600 / 60)}m`;
function drawStatus() {
  const ring = $('#cbRing'); if (!ring) return;
  const running = wsRunning();
  if (running && !runSince) runSince = Date.now();
  if (!running) runSince = null;
  ring.dataset.run = String(running);
  const u = CC.usage;
  const used = u ? (u.input || 0) + (u.cache_read || 0) + (u.cache_creation || 0) : 0;
  const win = /\[1m\]/i.test(CC.model) ? 1_000_000 : 200_000;
  const pct = Math.min(100, used / win * 100);
  ring.style.setProperty('--p', pct.toFixed(1));
  ring.title = running ? 'Running' : u ? `Context ${pct.toFixed(0)}% · ${fmt(used)} / ${fmt(win)}${
    u.cost_usd ? ` · $${u.cost_usd.toFixed(2)}` : ''}` : 'Context';
  const t = $('#cbTime');
  t.textContent = running ? fmtDur(Math.floor((Date.now() - runSince) / 1000)) : '';
  const pr = $('#prompt'), btn = $('#btnSend');
  const stop = running && !pr.value.trim();
  btn.dataset.mode = stop ? 'stop' : 'send';
  btn.title = stop ? 'Stop (esc)' : running ? 'Queue (enter)' : 'Send (enter)';
  pr.placeholder = running ? 'Queue a message…' : 'Message the agent…';
}
setInterval(() => { spinI++; if (runSince || wsRunning()) drawStatus(); }, 120);

function emptyChatHtml() {
  const p = currentProfile();
  const st = p?.starters?.length ? p.starters : [];
  return `<div class="cc-empty">${p ? `<div style="display:flex;align-items:center;gap:10px;margin-bottom:4px">${avatarHtml(p)}<div><b style="color:var(--fg)">和 ${esc(p.name)} 对话</b><div class="t-caption">${esc(p.description || '')}</div></div></div>` : 'New conversation.'}
    ${st.length ? `<div class="starters">${st.map((x, i) => `<button class="starter" data-starter="${i}"><b>${esc(x.label)}</b><span>${esc(x.prompt)}</span></button>`).join('')}</div>` : ''}
    <div class="t-caption">Type below, Enter to send.${p ? ` <a href="#/agent/${encodeURIComponent(p.id)}?view=capabilities">编辑开场白</a>` : ''}</div></div>`;
}
document.addEventListener('click', e => {
  const b = e.target.closest('[data-starter]'); if (!b) return;
  const p = currentProfile(); const st = p?.starters?.[+b.dataset.starter]; if (!st) return;
  const pr = $('#prompt'); pr.value = st.prompt; growPrompt(); drawStatus(); pr.focus();
});

function currentProfile() {
  const v = $('#agentSel')?.value || '';
  return v.startsWith('p:') ? (S.profiles || []).find(p => p.id === v.slice(2)) || null : null;
}

function effectiveMode() {
  const list = modeList(); if (!list.length) return null;
  const v = $('#permMode')?.value;
  const pick = x => list.find(y => y[0] === x);
  return (v && pick(v)) || pick(currentProfile()?.permission_mode) || list[0];
}

function syncModeForRuntime() {
  const m = $('#permMode'); if (!m) return;
  const list = modeList();
  [...m.options].forEach(o => { o.hidden = !!o.value && !list.some(x => x[0] === o.value); });
  m.value = '';
  drawMode();
}
function drawMode() {
  const m = $('#permMode'), el = $('#ccMode');
  if (!m || !el) return;
  const md = effectiveMode();
  el.hidden = !md;
  if (!md) return;
  const from = m.value ? '' : currentProfile()?.permission_mode ? ' (from Agent)' : ' (default)';
  el.innerHTML = `${svgI(md[3])}<span>${esc(md[1])}</span>`;
  el.title = `${md[2]}${from}`;
}
function setMode(v) {
  const m = $('#permMode'); if (!m) return;
  m.value = v; drawMode();
  if (wsRunning() && S.session && v) liveControl({ permission_mode: v }, '权限模式');
}

async function openModePop() {
  if (!modeList().length) return;
  const cur = effectiveMode()?.[0];
  const p = openPop(`<div class="cp-sec" style="display:flex">Modes<span class="cp-hk"><kbd>⇧</kbd> + <kbd>tab</kbd> to switch</span></div>
    ${modeList().map(([v, t, d, ic]) => `<button class="cp-row" data-mv="${esc(v)}" data-sel="${v === cur}">${svgI(ic, 'cp-ico')}
      <span class="cp-t"><b>${esc(t)}</b><span>${esc(d)}</span></span>${v === cur ? '<span class="cp-ok">✓</span>' : ''}</button>`).join('')}
    <div id="mpEff"></div>`);
  p.querySelectorAll('[data-mv]').forEach(b => { b.onclick = () => { setMode(b.dataset.mv); closePop(); }; });
  await drawEffortInto($('#mpEff'));
}

function growPrompt() {
  const pr = $('#prompt'); if (!pr) return;
  pr.style.height = 'auto';
  pr.style.height = Math.min(pr.scrollHeight, 220) + 'px';
}

function restorePrompt(text) {
  const pr = $('#prompt'); if (!pr) return;
  pr.value = text; growPrompt(); drawStatus();
}
