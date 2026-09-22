async function loadGit(quiet) {
  if (!S.ws) return;
  const ws = S.ws.id;
  try { const r = await api(`/api/workspaces/${ws}/git`); if (S.ws?.id !== ws) return; S.git = { ...r, ws }; }
  catch (e) { S.git = { ws, repo: false, reason: e.message }; }
  if (S.gitPr?.ws !== ws) S.gitPr = null;
  drawGitTab(); if (!quiet) drawGit();
}
function drawGitTab() {
  const tab = $('#panelbar .rtab[data-p="git"]'); if (!tab) return;
  const g = S.git?.ws === S.ws?.id ? S.git : null;
  const mark = g?.op ? '<span class="tabn bad">!</span>' : g?.uncommitted ? `<span class="tabn num">${g.uncommitted}</span>` : '';
  tab.innerHTML = `Git ${mark}`;
}
const GIT_OP_TEXT = { rebase: '变基', merge: '合并', 'cherry-pick': 'cherry-pick' };
const GIT_CODE = { M: ['M', 'warn'], A: ['A', 'ok'], D: ['D', 'bad'], R: ['R', 'info'], '??': ['U', 'ok'], UU: ['!', 'bad'], AA: ['!', 'bad'], DD: ['!', 'bad'], AU: ['!', 'bad'], UA: ['!', 'bad'], DU: ['!', 'bad'], UD: ['!', 'bad'] };
function drawGit() {
  const el = $('#gitview'); if (!el) return;
  const g = S.git;
  if (!g || g.ws !== S.ws?.id) { el.innerHTML = '<div class="empty">加载中…</div>'; return; }
  if (!g.repo) { el.innerHTML = `<div class="empty">${esc(g.reason || '这个目录不是 git 仓库')}</div>`; return; }
  const busy = S.gitBusy, dis = (cond, why) => cond || busy ? `disabled title="${esc(busy ? '正在执行…' : why || '')}"` : '';
  const hasUp = !!g.upstream, inOp = !!g.op, tgt = g.target_ok && !g.same;
  const pr = g.pr, prd = S.gitPr?.pr;
  const chips = [
    tgt ? `<span class="gchip" title="这条分支有、${esc(g.target)} 没有的提交">↑ ${g.ahead} 领先</span>` : '',
    tgt ? `<span class="gchip ${g.behind ? 'warn' : ''}" title="${esc(g.target)} 有、这条分支没有的提交">↓ ${g.behind} 落后</span>` : '',
    `<span class="gchip ${g.uncommitted ? 'warn' : ''}">${g.uncommitted ? `${g.uncommitted} 个未提交` : '工作区干净'}</span>`,
    g.remote ? (hasUp
      ? `<span class="gchip ${g.up_behind ? 'bad' : g.up_ahead ? 'warn' : ''}" title="相对 ${esc(g.upstream)}">${g.up_ahead || g.up_behind ? `远端 ↑${g.up_ahead} ↓${g.up_behind}` : '已与远端同步'}</span>`
      : '<span class="gchip warn">还没推送过</span>') : '<span class="gchip">没有 origin 远端</span>',
    g.running ? '<span class="gchip info">agent 运行中</span>' : '',
  ].join('');
  const out = S.gitOut;
  el.innerHTML = `
    <div class="gp-head">
      <span class="gp-br" title="当前分支"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" aria-hidden="true"><circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="8" r="2"/><path d="M6 7v10M18 10c0 5-12 3-12 7"/></svg>
        <b>${g.detached ? `游离 HEAD · ${esc(g.head)}` : esc(g.branch)}</b>
        ${g.detached || inOp ? '' : `<button class="linkbtn" data-g="rename" ${dis(false)}>改名</button>`}</span>
      <span class="faint">→</span>
      <button class="gp-target" data-g="target" title="这条分支最终要合到哪里。领先 / 落后、变基、合并和 PR 都以它为准">${g.target ? esc(g.target) : '选择目标分支'}${g.target_guess && g.target ? ' <i class="faint">自动</i>' : ''} ▾</button>
      ${chips}<span class="grow"></span>
      <button class="btn btn-outline btn-xs" data-g="fetch" ${dis(!g.remote, '没有 origin 远端')}>${busy === 'fetch' ? '拉取中…' : 'Fetch'}</button>
      <button class="laybtn" data-g="reload" title="刷新">${ICON.refresh}</button>
    </div>
    ${inOp ? `<div class="gp-op"><b>${GIT_OP_TEXT[g.op]}进行到一半</b>
        <span>${g.conflicts.length ? `${g.conflicts.length} 个文件有冲突，解决后点「继续」` : '冲突都解决了，可以继续'}</span><span class="grow"></span>
        ${g.conflicts.length ? '<button class="btn btn-brand btn-xs" data-g="resolve">让 agent 解决</button>' : ''}
        <button class="btn btn-outline btn-xs" data-g="continue" ${dis(false)}>${busy === 'continue' ? '继续中…' : '继续'}</button>
        <button class="btn btn-danger btn-xs" data-g="abort" ${dis(false)}>放弃${GIT_OP_TEXT[g.op]}</button></div>` : ''}
    <div class="gp-acts">
      <button class="btn btn-outline btn-sm" data-g="commit" ${dis(!g.uncommitted || inOp, inOp ? '先把进行到一半的操作收尾' : '没有改动可提交')}>提交…</button>
      <button class="btn btn-outline btn-sm" data-g="rebase" ${dis(!tgt || inOp || g.detached || !g.behind, !tgt ? '先选一个目标分支' : !g.behind ? `没有落后 ${g.target}，不用变基` : '')}>${busy === 'rebase' ? '变基中…' : `变基到 ${esc(g.target || '…')}`}</button>
      <button class="btn btn-outline btn-sm" data-g="merge" ${dis(!tgt || inOp || g.detached || !g.ahead || !g.target_local, !tgt ? '先选一个目标分支' : !g.target_local ? '目标是远端分支：推送后走 PR' : !g.ahead ? '没有可合并的提交' : '')}>${busy === 'merge' ? '合并中…' : `合并进 ${esc(g.target || '…')}`}</button>
      <button class="btn btn-outline btn-sm" data-g="push" ${dis(!g.remote || g.detached || inOp || (hasUp && !g.up_ahead && !g.up_behind), !g.remote ? '没有 origin 远端' : '远端已经是最新的')}>${busy === 'push' ? '推送中…' : `推送${g.up_ahead && hasUp ? ` ↑${g.up_ahead}` : ''}`}</button>
      ${pr ? '' : `<button class="btn btn-brand btn-sm" data-g="pr" ${dis(!g.remote || !tgt || g.detached || inOp || !g.ahead, !g.remote ? '没有 origin 远端' : !g.ahead ? '没有领先目标分支的提交' : '')}>开 PR…</button>`}
    </div>
    ${pr ? `<div class="gp-pr" data-s="${esc((prd?.state || pr.state || '').toLowerCase())}">
        <b>PR #${pr.number ?? ''}</b><span class="gchip">${esc({ OPEN: '开着', MERGED: '已合并', CLOSED: '已关闭' }[prd?.state || pr.state] || pr.state || '')}</span>
        ${prd ? `${prd.draft ? '<span class="gchip">草稿</span>' : ''}
          ${prd.checks.failed ? `<span class="gchip bad">${prd.checks.failed} 项检查失败</span>` : prd.checks.pending ? `<span class="gchip warn">${prd.checks.pending} 项检查进行中</span>` : prd.checks.pass ? `<span class="gchip ok">${prd.checks.pass} 项检查通过</span>` : ''}
          ${prd.review === 'APPROVED' ? '<span class="gchip ok">已批准</span>' : prd.review === 'CHANGES_REQUESTED' ? '<span class="gchip bad">要求修改</span>' : ''}
          ${prd.mergeable === 'CONFLICTING' ? '<span class="gchip bad">与目标分支冲突</span>' : ''}
          <span class="gp-prt">${esc(prd.title)}</span>` : ''}
        <span class="grow"></span><a class="linkbtn" href="${esc(pr.url)}" target="_blank" rel="noopener">在网页上打开</a>
        <button class="linkbtn" data-g="pr-refresh">${busy === 'pr-refresh' ? '查询中…' : '刷新状态'}</button></div>` : ''}
    <div class="gp-cols">
      <div class="gp-col"><h5>未提交的改动 <span class="num faint">${g.uncommitted}</span></h5>
        ${g.files.length ? g.files.map(f => { const [mk, tone] = GIT_CODE[f.code] || GIT_CODE[f.code[0]] || ['M', 'warn'];
          return `<button class="gp-file" data-path="${esc(f.path)}" title="${esc(f.path)}"><b class="dt-mk ${tone}">${mk}</b><span>${esc(f.path)}</span></button>`; }).join('')
          : '<div class="faint t-caption">没有</div>'}</div>
      <div class="gp-col"><h5>${tgt ? `领先 ${esc(g.target)} 的提交` : '提交'} <span class="num faint">${g.commits.length}${g.commits.length >= 40 ? '+' : ''}</span></h5>
        ${g.commits.length ? g.commits.map(c => `<div class="gp-commit"><code>${esc(c.sha)}</code><span class="sj" title="${esc(c.subject)}">${esc(c.subject)}</span><span class="faint t-micro">${esc(c.author)} · ${ago(new Date(c.at * 1000).toISOString())}</span></div>`).join('')
          : `<div class="faint t-caption">${tgt ? '还没有' : '选了目标分支才知道哪些提交是这条分支的'}</div>`}</div>
    </div>
    ${out?.text ? `<details class="gp-out" ${out.ok ? '' : 'open'}><summary>${out.ok ? '上一次操作的输出' : '上一次操作没成功'} · ${esc(out.op)}</summary><pre>${esc(out.text)}</pre></details>` : ''}`;
}
async function gitOp(op, body, okMsg) {
  if (S.gitBusy || !S.ws) return null;
  const ws = S.ws.id;
  S.gitBusy = op; drawGit();
  let r = null;
  try {
    r = await post(`/api/workspaces/${ws}/git/${op}`, body || {});
    if (S.ws?.id !== ws) return r;
    if (r.status) S.git = { ...r.status, ws };
    S.gitOut = { op, ok: !!r.ok, text: r.output || '' };
    if (r.ok && okMsg) toast(okMsg);
  } catch (e) { S.gitOut = { op, ok: false, text: e.message }; toast(e.message); }
  finally {
    S.gitBusy = null; drawGit(); drawGitTab();

    if (['rebase', 'continue', 'abort', 'update'].includes(op)) reloadWorkspaceFiles();
    else if (['commit', 'merge'].includes(op)) { loadTree(); loadDiff(); }
  }
  return r;
}
function wireGit() {
  const el = $('#gitview'); if (!el || el.dataset.wired) return;
  el.dataset.wired = '1';
  el.addEventListener('click', async e => {
    const f = e.target.closest('.gp-file'); if (f) { openFile(f.dataset.path); return; }
    const b = e.target.closest('[data-g]'); if (!b || b.disabled) return;
    const g = S.git, a = b.dataset.g;
    if (a === 'reload') loadGit();
    else if (a === 'fetch') gitOp('fetch', {}, '已拉取远端的最新状态');
    else if (a === 'commit') dlgCommit();
    else if (a === 'pr') dlgPr();
    else if (a === 'resolve') agentResolve();
    else if (a === 'continue') { const r = await gitOp('continue'); if (r?.ok && !S.git.op) toast('已完成'); else if (r && !r.ok) toast('还不能继续，看下面的输出'); }
    else if (a === 'abort') {
      if (await ask(`放弃这次${GIT_OP_TEXT[g.op]}？分支会回到开始之前的样子，已经解决的冲突也不保留。`, { ok: '放弃', danger: true })) gitOp('abort', {}, '已放弃，分支回到之前的状态');
    }
    else if (a === 'rebase') {
      const r = await gitOp('rebase');
      if (r?.ok) toast(`已变基到 ${g.target}`);
      else if (r && S.git.op === 'rebase') toast(`变基遇到冲突：${S.git.conflicts.length} 个文件要解决`);
      else if (r) toast('变基没成功，看下面的输出');
    }
    else if (a === 'merge') {
      if (!await ask(`把 ${g.branch} 合并进 ${g.target}？\n\n${g.behind ? `${g.target} 上有 ${g.behind} 个这条分支没有的提交，会产生一个合并提交（有冲突则不合并，得先变基）。` : '可以直接快进，不会产生合并提交。'}\n挂在这个工作区上、处于「待审阅」的任务会自动标成完成。`, { ok: '合并' })) return;
      const r = await gitOp('merge');
      if (r?.ok) toast(`已合并进 ${g.target}${r.tasks_done ? `，${r.tasks_done} 个任务已完成` : ''}`);
      else if (r) toast('没有合并，看下面的输出');
    }
    else if (a === 'push') {
      const r = await gitOp('push');
      if (r?.ok) { toast('已推送'); return; }
      if (!r?.needs_force) { if (r) toast('推送失败，看下面的输出'); return; }

      if (await ask(`远端的 ${g.branch} 上有本地没有的提交 —— 通常是因为刚变基过，本地历史被改写了。\n\n强制推送会用本地的历史覆盖远端：远端那 ${S.git.up_behind || '几'} 个提交会从分支上消失；已经基于旧历史拉过代码的人需要重新对齐。\n\n这里用的是 --force-with-lease：如果在你上次拉取之后别人又推过新提交，这次强推会被拒绝，不会悄悄盖掉别人的活。`, { ok: '强制推送', danger: true })) {
        const f = await gitOp('push', { force: true });
        toast(f?.ok ? '已强制推送' : '强制推送也被拒绝了，看下面的输出');
      }
    }
    else if (a === 'rename') {
      const name = await askText('新的分支名', g.branch, { ok: '改名' });
      if (name && name.trim() !== g.branch) { const r = await gitOp('rename-branch', { name: name.trim() }); toast(r?.ok ? '已改名（远端的旧分支不会自动删除）' : '改名失败'); }
    }
    else if (a === 'target') {
      try {
        const { branches } = await api(`/api/workspaces/${S.ws.id}/git/branches`);
        openMenu(b, [
          { label: '自动判断（main / master / origin 默认分支）', run: () => setTarget('') }, '-',
          ...branches.filter(x => x !== g.branch).slice(0, 40).map(x => ({ label: x === g.target ? `✓ ${x}` : x, run: () => setTarget(x) })),
        ]);
      } catch (err) { toast(err.message); }
    }
    else if (a === 'pr-refresh') refreshPr();
  });
}
async function setTarget(branch) {
  const r = await gitOp('set-target', { branch });
  if (r?.ok) { toast(branch ? `目标分支：${branch}` : '目标分支改回自动判断'); if (diffPrefs().base === 'target') loadDiff(); }
}
async function refreshPr() {
  if (S.gitBusy) return;
  const ws = S.ws.id; S.gitBusy = 'pr-refresh'; drawGit();
  try {
    const r = await api(`/api/workspaces/${ws}/pr`);
    S.gitPr = { ws, pr: r.pr };
    if (r.pr && S.git?.ws === ws) S.git.pr = { url: r.pr.url, number: r.pr.number, state: r.pr.state };
    if (!r.pr) toast(r.reason || '这条分支上没找到 PR');
  } catch (e) { toast(e.message); }
  finally { S.gitBusy = null; drawGit(); }
}
function agentResolve() {
  const g = S.git;
  const what = { rebase: `变基（rebase）到 ${g.target}`, merge: '合并（merge）', 'cherry-pick': 'cherry-pick' }[g.op];
  const cont = { rebase: 'git rebase --continue', merge: 'git commit --no-edit', 'cherry-pick': 'git cherry-pick --continue' }[g.op];
  draftChat(`这个工作区正在${what}，进行到一半，下面这些文件有冲突：\n${g.conflicts.map(f => `- ${f}`).join('\n')}\n\n请逐个解决：先弄清两边各自想做什么，把两边的意图都保留下来，不要整段只选一边。解决完 git add，再执行 ${cont}（设置 GIT_EDITOR=true 免得卡在编辑器上）；后面的提交又冲突就接着解决，直到整个过程结束。\n不要 abort，不要动和冲突无关的代码。完成后用几句话说明每处冲突是怎么取舍的。`);
  toast('已在新对话里写好请求：选好 agent 再发送');
}
function dlgCommit() {
  const g = S.git;
  openDlg(`<h3>提交改动</h3>
    <div class="t-caption muted" style="margin-bottom:10px">${g.uncommitted} 个文件的改动会全部提交到 <b class="mono">${esc(g.branch || g.head)}</b>（含未跟踪的新文件）</div>
    <div class="field"><label>提交信息</label><textarea id="gcMsg" class="input" rows="5" placeholder="改了什么、为什么"></textarea></div>
    <div class="dfoot"><button class="btn btn-outline" id="gcAi" ${S.prefs?.git?.ai_draft === false ? 'hidden' : ''} title="让本机的 Claude（Haiku）看一眼改动，替你起草">AI 起草</button><span class="grow"></span>
      <button class="btn btn-outline" onclick="closeDlg()">取消</button><button class="btn btn-brand" id="gcOk">提交</button></div>`);
  const msg = $('#gcMsg'); msg.focus();
  $('#gcAi').onclick = () => aiDraft('commit', $('#gcAi'), d => { msg.value = d.text; });
  const go = async () => {
    const message = msg.value.trim(); if (!message) { toast('写一句提交信息'); return; }
    closeDlg();
    const r = await gitOp('commit', { message });
    if (r?.ok) toast(r.changed ? `已提交 ${r.commit || ''}` : '没有改动可提交'); else if (r) toast('提交失败，看下面的输出');
  };
  $('#gcOk').onclick = go;
  msg.onkeydown = e => { if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) { e.preventDefault(); go(); } };
}
async function aiDraft(kind, btn, apply) {
  const old = btn.textContent; btn.disabled = true; btn.textContent = '起草中…';
  try { apply(await post(`/api/workspaces/${S.ws.id}/git/describe`, { kind })); }
  catch (e) { toast(e.message); }
  finally { btn.disabled = false; btn.textContent = old; }
}
function dlgPr() {
  const g = S.git, base = g.target.replace(/^origin\//, '');
  if (!g.gh) {

    openDlg(`<h3>开 PR</h3>
      <div class="ask-msg">工作区所在的机器上没有装 GitHub CLI（gh），没法直接创建 PR。<br>可以先把分支推上去，再到网页上开。</div>
      <div class="dfoot"><button class="btn btn-outline" onclick="closeDlg()">关闭</button>
      <button class="btn btn-brand" id="prWeb">推送并打开网页</button></div>`);
    $('#prWeb').onclick = async () => {
      closeDlg(); const r = await gitOp('push');
      const url = S.git.web?.new_pr;
      if (r?.ok && url) window.open(url, '_blank', 'noopener'); else toast(r?.ok ? '认不出托管平台，没法给网页链接' : '推送失败，看下面的输出');
    };
    return;
  }
  openDlg(`<h3>开 Pull Request</h3>
    <div class="t-caption muted" style="margin-bottom:10px"><b class="mono">${esc(g.branch)}</b> → <b class="mono">${esc(base)}</b> · ${g.ahead} 个提交${g.uncommitted ? ` · <span class="warn-tx">还有 ${g.uncommitted} 个没提交的改动不会进 PR</span>` : ''}</div>
    <div class="field"><label>标题</label><input id="prTitle" class="input" value="${esc(g.commits.length === 1 ? g.commits[0].subject : '')}" placeholder="这个 PR 做了什么"></div>
    <div class="field"><label>描述（Markdown）</label><textarea id="prBody" class="input" rows="9" placeholder="改了什么、为什么、怎么验证"></textarea></div>
    <label class="row" style="gap:6px;margin-bottom:6px"><input type="checkbox" id="prDraft" ${S.prefs?.git?.pr_draft ? 'checked' : ''}> 作为草稿创建</label>
    <div class="t-micro faint">会先把分支推到 origin，再用那台机器上 gh 的登录态创建 PR。</div>
    <div class="dfoot"><button class="btn btn-outline" id="prAi" ${S.prefs?.git?.ai_draft === false ? 'hidden' : ''} title="让本机的 Claude（Haiku）根据提交和 diff 起草">AI 起草</button><span class="grow"></span>
      <button class="btn btn-outline" onclick="closeDlg()">取消</button><button class="btn btn-brand" id="prOk">创建 PR</button></div>`);
  $('#prTitle').focus();
  $('#prAi').onclick = () => aiDraft('pr', $('#prAi'), d => { $('#prTitle').value = d.title; $('#prBody').value = d.body; });
  $('#prOk').onclick = async () => {
    const title = $('#prTitle').value.trim(); if (!title) { toast('写一个标题'); return; }
    const body = $('#prBody').value, draft = $('#prDraft').checked;
    closeDlg();
    const r = await gitOp('pr-create', { title, body, draft });
    if (r?.ok && r.url) { toastAction(r.existing ? '这条分支已经有 PR 了' : 'PR 已创建', '打开', () => window.open(r.url, '_blank', 'noopener')); refreshPr(); }
    else if (r) toast('PR 没开成，看下面的输出');
  };
}
