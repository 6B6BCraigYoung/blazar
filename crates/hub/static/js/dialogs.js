const NW = { node: 'local', path: '', listing: null, loading: false };

function dlgNewWorkspace() {
  NW.node = S.nodes.find(n => n.name === 'local') ? 'local' : (S.nodes[0]?.name || 'local');
  NW.path = ''; NW.listing = null;
  openDlg(`
    <h3>新建工作区</h3>
    <div class="field"><label>机器</label>
      <select id="mNode" class="input">
        ${['local', ...S.nodes.map(n => n.name).filter(n => n !== 'local')]
          .map(n => `<option${n === NW.node ? ' selected' : ''}>${esc(n)}</option>`).join('')}
      </select></div>
    <div class="field">
      <label>目录 —— 点进去翻，<span style="color:var(--success)">绿色</span>的是 git 仓库</label>
      <div class="row" style="gap:6px;margin-bottom:6px">
        <button class="btn btn-outline btn-sm" id="bUp" title="上一级">↑</button>
        <input id="bPath" class="input input-sm mono" placeholder="~"
          title="也可以直接输入路径后回车">
        <button class="btn btn-outline btn-sm" id="bGo">前往</button>
      </div>
      <div id="browser" style="border:1px solid var(--border);border-radius:var(--r-lg);
        height:240px;overflow:auto;background:var(--surface)"></div>
    </div>
    <div id="isoBox" style="display:none">
      <div class="field">
        <label class="row" style="gap:6px;cursor:pointer">
          <input type="checkbox" id="mIso">
          隔离开工 —— 独立 worktree + 分支，不动这个仓库本身</label>
        <div class="hint">多个 agent 同时干活、或者要接着某个分支往下做时用。不勾就直接在这个目录里干</div>
      </div>
      <div class="field" id="brField" style="display:none">
        <label>从哪开工 <span class="faint">—— 按最近提交排序，含只在 origin 上的分支</span></label>
        <input id="brQ" class="input input-sm" placeholder="筛选分支…" style="margin-bottom:6px">
        <div id="brList" style="border:1px solid var(--border);border-radius:var(--r-lg);
          max-height:190px;overflow:auto;background:var(--surface)"></div>
      </div>
    </div>
    <div class="field"><label>名称（留空取目录名）</label><input id="mName" class="input"></div>
    <div class="field"><label>项目</label>
      <input id="mProject" class="input" placeholder="可留空">
      <div class="hint">同名项目下的多个工作区即「多机副本」，侧栏会归到一组</div></div>
    <div class="dfoot">
      <button class="btn btn-outline" onclick="closeDlg()">取消</button>
      <button class="btn btn-brand" id="mOk" disabled>选此目录并创建</button></div>`);

  $('#mNode').onchange = e => { NW.node = e.target.value; NW.path = ''; loadBrowse(); };
  $('#bUp').onclick = () => { if (NW.listing?.parent) { NW.path = NW.listing.parent; loadBrowse(); } };
  $('#bGo').onclick = () => { NW.path = $('#bPath').value.trim(); loadBrowse(); };
  $('#bPath').onkeydown = e => {
    if (e.key === 'Enter' && !e.isComposing) { NW.path = e.target.value.trim(); loadBrowse(); }
  };
  $('#mOk').onclick = createFromBrowser;
  $('#mIso').onchange = () => {
    const on = $('#mIso').checked;
    $('#brField').style.display = on ? '' : 'none';
    $('#mOk').textContent = on ? '建隔离工作区' : (NW.listing?.is_repo ? '选此仓库并创建' : '选此目录并创建');
    if (on && !NW.branches) loadBranches();
  };
  $('#brQ').oninput = drawBranches;
  loadBrowse();
}

async function loadBranches() {
  const host = $('#brList');
  host.innerHTML = '<div class="empty" style="padding:16px">读取分支…（会先 fetch 一次 origin）</div>';
  try {
    NW.branches = await api(`/api/nodes/${encodeURIComponent(NW.node)}/branches` +
      `?repo=${encodeURIComponent(NW.path)}`);
    NW.pick = '';
    drawBranches();
  } catch (e) { host.innerHTML = `<div class="empty" style="padding:16px">${esc(e.message)}</div>`; }
}
function drawBranches() {
  const host = $('#brList'); if (!host || !NW.branches) return;
  const q = ($('#brQ').value || '').toLowerCase();
  const rows = NW.branches.filter(b => !q || `${b.name} ${b.author} ${b.subject}`.toLowerCase().includes(q));
  const row = (val, title, sub, extra = '', disabled = false) => `
    <div class="lrow brrow" data-b="${esc(val)}" data-sel="${NW.pick === val}" data-dis="${disabled}"
         style="height:auto;min-height:40px;padding:5px 10px;border-bottom:0;align-items:flex-start;flex-direction:column;gap:1px">
      <div class="row" style="gap:6px;width:100%"><span class="mono t-label truncate grow">${title}</span>${extra}</div>
      <div class="t-micro faint truncate" style="width:100%">${sub}</div></div>`;
  host.innerHTML =
    row('', '＋ 从当前 HEAD 起一个新分支', '分支名取自下面填的名称') +
    rows.slice(0, 200).map(b => row(
      b.name, esc(b.name),
      `${esc(b.author)} · ${ago(b.last_commit_at)} · ${esc(b.subject)}`,
      (b.remote_only ? '<span class="badge badge-muted">仅远端</span>' : '') +
      (b.current ? '<span class="badge badge-warn" title="主仓正 checkout 着它，不能再挂到别的工作树上">主仓在用</span>' : ''),
      b.current)).join('') +
    (rows.length > 200 ? `<div class="t-micro faint" style="padding:6px 10px">还有 ${rows.length - 200} 个，请用上面的筛选</div>` : '');
  host.querySelectorAll('.brrow').forEach(el => {
    el.onclick = () => {
      if (el.dataset.dis === 'true') { toast('主仓正 checkout 着这个分支，不能再挂到别的工作树上'); return; }
      NW.pick = el.dataset.b; drawBranches();
    };
  });
}

async function loadBrowse() {
  const host = $('#browser');
  if (!host || NW.loading) return;
  NW.loading = true;
  host.innerHTML = '<div class="empty" style="padding:24px">读取中…</div>';
  $('#mOk').disabled = true;
  try {
    const l = await api(`/api/nodes/${encodeURIComponent(NW.node)}/browse` +
      `?path=${encodeURIComponent(NW.path)}`);
    NW.listing = l;
    NW.path = l.path;
    NW.branches = null; NW.pick = '';
    $('#isoBox').style.display = l.is_repo ? '' : 'none';
    if (!l.is_repo) $('#mIso').checked = false;
    if ($('#mIso').checked) loadBranches();
    $('#bPath').value = l.path;
    $('#bUp').disabled = !l.parent;

    $('#mOk').disabled = false;
    $('#mOk').textContent = l.is_repo ? '选此仓库并创建' : '选此目录并创建';

    host.innerHTML = l.entries.length ? l.entries.map(e => `
      <div class="lrow" style="height:30px;border-bottom:0;padding:0 10px"
           data-p="${esc(e.path)}" title="${esc(e.path)}">
        <span style="width:16px;color:${e.is_repo ? 'var(--success)' : 'var(--faint-fg)'}">
          ${e.is_repo ? '◆' : '▸'}</span>
        <span class="grow truncate mono t-label"
          ${e.is_repo ? 'style="color:var(--success)"' : ''}>${esc(e.name)}</span>
        ${e.children ? `<span class="t-micro faint num">${e.children}</span>` : ''}
      </div>`).join('')
      : '<div class="empty" style="padding:24px">这个目录下没有子目录</div>';
    host.querySelectorAll('.lrow').forEach(el => {
      el.onclick = () => { NW.path = el.dataset.p; loadBrowse(); };
    });
  } catch (e) {
    host.innerHTML = `<div class="empty" style="padding:24px">${esc(e.message)}</div>`;
    $('#mOk').disabled = true;
  } finally { NW.loading = false; }
}

async function createFromBrowser() {
  if (!NW.path) { toast('请先选一个目录'); return; }
  if ($('#mIso')?.checked) {
    const btn = $('#mOk'); btn.disabled = true; btn.textContent = '建立 worktree…';
    try {
      const r = await post('/api/workspaces/isolated', {
        node: NW.node, repo: NW.path,
        from_branch: NW.pick || null,
        name: $('#mName').value.trim() || null,
        project: $('#mProject').value.trim() || null,
      });
      closeDlg(); await refreshState(); navigate(`#/workspaces/${r.id}`);
      toast(`已在分支 ${r.branch} 上建好隔离工作区`);
    } catch (e) { toast('创建失败: ' + e.message); btn.disabled = false; btn.textContent = '建隔离工作区'; }
    return;
  }
  try {
    const r = await post('/api/workspaces', {
      node: NW.node, path: NW.path,
      name: $('#mName').value.trim() || null,
      project: $('#mProject').value.trim() || null,
    });
    closeDlg(); await refreshState(); navigate(`#/workspaces/${r.id}`);
  } catch (e) { toast('创建失败: ' + e.message); }
}

function dlgAddMachine() {
  openDlg(`
    <h3>接入新机器</h3>
    <div class="desc muted t-caption" style="margin-bottom:14px">
      Blazar 走两条路接机器，都不需要在目标机器上装 Blazar 本体。</div>
    <div class="card" style="margin-bottom:12px;box-shadow:none">
      <h3>① 已在 EasyTier mesh 里</h3>
      <div class="desc">机器加入 mesh 后点「从 mesh 发现」即可，延迟、丢包、NAT 类型都会自动带出来。</div>
      <button class="btn btn-outline btn-sm" id="dMesh">从 mesh 发现</button>
    </div>
    <div class="card" style="margin-bottom:12px;box-shadow:none">
      <h3>② 新同事的电脑 <span class="badge badge-brand">推荐</span></h3>
      <div class="desc">在「组网」页生成一个邀请文件发给 TA，TA 用 Blazar 双击打开、输一次电脑密码就进网了。
        不用装 EasyTier、不用配任何东西。</div>
      <button class="btn btn-outline btn-sm" onclick="closeDlg();navigate('#/nodes');setTimeout(()=>$('#meshIssue')?.scrollIntoView({behavior:'smooth'}),300)">去邀请</button>
    </div>
    <div class="card" style="margin-bottom:0;box-shadow:none">
      <h3>③ 只能 SSH</h3>
      <div class="desc">在 <code>~/.ssh/config</code> 里配好 Host 之后，
        直接用那个 Host 名新建工作区即可 —— 借用的、不便装服务的机器走这条路。</div>
      <div class="mono t-caption muted" style="background:var(--muted);padding:8px 10px;border-radius:var(--r-md)">
        Host gpu-new<br>&nbsp;&nbsp;HostName 10.99.0.30<br>&nbsp;&nbsp;User me</div>
    </div>
    <div class="dfoot"><button class="btn btn-outline" onclick="closeDlg()">关闭</button></div>`);
  $('#dMesh').onclick = async () => {
    try {
      const r = await post('/api/mesh/refresh');
      toast(`发现 ${r.discovered} 个节点`); closeDlg();
      await refreshState(); render();
    } catch (e) { toast('失败: ' + e.message); }
  };
}
