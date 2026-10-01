const ACC_PROVIDERS = [['claude', 'Claude Code'], ['codex', 'Codex']];
const ACC_WINDOW = { five_hour: '5 小时', seven_day: '7 天', seven_day_opus: '7 天 · Opus', seven_day_sonnet: '7 天 · Sonnet', seven_day_overage_included: '7 天 · 含超额', blocked: '已限流' };
const accWindowLabel = n => ACC_WINDOW[n] || n.replace(/_/g, ' ');
const ACC_PLAN = { pro: 'Pro', max: 'Max', max5x: 'Max 5x', max20x: 'Max 20x', team: 'Team', enterprise: 'Enterprise', prolite: 'Pro Lite', plus: 'Plus', free: 'Free', business: 'Business', edu: 'Edu' };
const accPlanLabel = p => ACC_PLAN[p] || p;
// claude-fable-5-1 → Fable 5.1
const accModelLabel = m => {
  const x = String(m).replace(/^claude-/, '').split('-');
  return x[0] ? x[0][0].toUpperCase() + x[0].slice(1) + (x.length > 1 ? ' ' + x.slice(1).join('.') : '') : m;
};
const accUntil = t => {
  if (!t) return '';
  const d = (new Date(t) - Date.now()) / 1000;
  if (d <= 0) return '已重置';
  if (d < 3600) return `${Math.ceil(d / 60)} 分钟后重置`;
  if (d < 86400) return `${Math.round(d / 3600)} 小时后重置`;
  return `${Math.round(d / 86400)} 天后重置`;
};
const CONTINUE_TEXT = '（已换账号接着做）请从刚才中断的地方继续，把没做完的工作完成。';

async function loadAccounts() {
  try { S.accounts = await api('/api/accounts'); } catch (_) { S.accounts = { accounts: [], modes: {} }; }
  return S.accounts;
}

const accSupported = rt => ACC_PROVIDERS.some(([p]) => p === rt);
const accOf = id => (S.accounts?.accounts || []).find(a => a.id === id);
const accUsable = a => a && !a.disabled && a.status !== 'logged_out';
const accModeLabel = (rt, mode = S.accounts?.modes?.[rt] || '') =>
  mode === 'auto' ? '自动选择' : (accOf(mode)?.label || '默认登录');
const accQuotaShort = a => (a.windows || []).filter(w => w.name !== 'blocked')
  .map(w => `${w.name === 'five_hour' ? '5h' : w.name === 'seven_day' ? '7d' : accWindowLabel(w.name)} ${Math.round(w.utilization * 100)}%`).join(' · ');

// 「运行时 · 账号」合成一个下拉：值是 rt（不支持多账号的运行时）或 rt|账号（空 = 跟随运行时的账号策略）。
const rtPart = v => (v || '').split('|')[0];
const accPart = v => (v || '').includes('|') ? (v.split('|')[1] || null) : null;
function runtimeAccountOpts(rts, curRt, curAcc) {
  const opt = (v, t, on) => `<option value="${esc(v)}" ${on ? 'selected' : ''}>${esc(t)}</option>`;
  return rts.map(r => {
    if (!accSupported(r.id)) return opt(r.id, `${r.label}${r.authed === false ? '（未登录）' : ''}`, r.id === curRt);
    const mine = r.id === curRt;
    const list = (S.accounts?.accounts || []).filter(a => a.provider === r.id);
    return `<optgroup label="${esc(r.label)}">
      ${opt(`${r.id}|`, `${r.label} · 跟随运行时设置（${accModeLabel(r.id)}）`, mine && !curAcc)}
      ${opt(`${r.id}|auto`, `${r.label} · 自动选择`, mine && curAcc === 'auto')}
      ${list.map(a => opt(`${r.id}|${a.id}`, `${r.label} · ${a.label}${a.plan ? ` (${accPlanLabel(a.plan)})` : ''}${a.disabled ? '（已停用）' : a.status === 'logged_out' ? '（不可用）' : ''}`, mine && curAcc === a.id)).join('')}
    </optgroup>`;
  }).join('');
}

function accBars(a) {
  if (!a.windows.length) return `<span class="faint t-caption">${a.kind === 'token' || a.provider === 'codex' ? '还没查过 —— 「⋯」里点「刷新额度」' : '还没有数据 —— 用它跑一次会话就有了'}</span>`;
  return a.windows.map(w => {
    const pct = Math.round(w.utilization * 100);
    const lvl = pct >= 90 ? 'bad' : pct >= 70 ? 'warn' : 'ok';
    return `<div class="qbar" data-lvl="${lvl}" title="${esc(`观察于 ${ago(w.observed_at)}`)}">
      <span class="ql">${esc(accWindowLabel(w.name))}</span><span class="trk"><span class="fill" style="width:${pct}%"></span></span>
      <span class="qv num">${pct}%</span><span class="qr faint">${esc(accUntil(w.resets_at))}</span></div>`;
  }).join('');
}

function accStatus(a) {
  const tok = a.kind === 'token';
  return a.disabled ? '<span class="badge badge-muted">已停用</span>'
    : a.status === 'ok' ? `<span class="rt-ok"${tok ? ' title="CLI 只能确认 token 在；是否有效要等第一次运行，认证失败会自动标成无效"' : ''}><span class="dot"></span>${tok ? '已配置 token' : '已登录'}</span>`
    : a.status === 'logged_out' ? `<span class="rt-bad"><span class="dot"></span>${tok ? 'token 无效' : '未登录'}</span>`
    : '<span class="faint">无法判断</span>';
}

// 这个运行时现在用哪个账号（没在对话里单独选过的会话都用它）。策略是旧版的「自动选择」时没有哪个算在用。
function activeAccountId(v, p) {
  const mode = v.modes[p] || '';
  return mode === '' ? `${p}-default` : mode === 'auto' ? null : mode;
}

// 一个运行时的账号区：一行一个账号，点「使用」就换成它；在用的那行高亮。
function accountsSection(v, p) {
  const list = v.accounts.filter(a => a.provider === p);
  const on = activeAccountId(v, p);
  return `<div class="acc-sec">
    <table class="tb acc-tb"><thead><tr><th>账号</th><th>状态</th><th>额度</th><th>最近使用</th>
      <th style="text-align:right"><button class="btn btn-outline btn-xs" data-add-acc="${p}">＋ 添加账号</button></th></tr></thead><tbody>
    ${list.map(a => {
      const tok = a.kind === 'token', active = a.id === on;
      return `<tr data-acc="${esc(a.id)}" class="${active ? 'acc-on' : ''}">
      <td><b title="${esc([a.email || (a.builtin ? (p === 'claude' ? '~/.claude' : '~/.codex') : ''), a.plan && accPlanLabel(a.plan), tok ? '长期 token' : '浏览器登录'].filter(Boolean).join(' · '))}">${esc(a.label)}</b>
        ${(a.model_blocks || []).length ? `<div class="t-caption acc-mb">用不了：${a.model_blocks.map(b => `<span class="badge badge-warn" title="${esc(`${ago(b.observed_at)}被拒，7 天后自动重试`)}">${esc(accModelLabel(b.model))}<button class="x" data-unblock="${esc(b.model)}" title="清除，下次照常尝试">×</button></span>`).join(' ')}</div>` : ''}</td>
      <td>${accStatus(a)}</td>
      <td class="acc-q">${accBars(a)}</td>
      <td class="t-caption">${a.last_used_at ? ago(a.last_used_at) : '<span class="faint">—</span>'}</td>
      <td style="text-align:right;white-space:nowrap">
        ${active ? '<span class="acc-use-on">使用中</span>'
          : `<button class="btn btn-outline btn-xs" data-use ${accUsable(a) ? '' : 'disabled title="这个账号现在不可用"'}>使用</button>`}
        <button class="btn btn-ghost btn-xs acc-more" data-more title="更多" aria-label="更多">⋯</button></td></tr>`;
    }).join('')}
    </tbody></table></div>`;
}

function dlgAccountPlan(a) {
  openDlg(`<h3>订阅类型 · ${esc(a.label)}</h3>
    <div class="field"><select id="apSel" class="input"><option value="">不标</option>
      ${['pro', 'max5x', 'max20x', 'team', 'enterprise'].map(x => `<option value="${x}" ${a.plan === x ? 'selected' : ''}>${esc(accPlanLabel(x))}</option>`).join('')}</select></div>
    <div class="t-caption faint">只用于展示；能用哪些模型以实际运行结果为准。</div>
    <div class="dfoot"><button class="btn btn-outline" id="apNo">取消</button><button class="btn btn-brand" id="apOk">保存</button></div>`);
  $('#apNo').onclick = closeDlg;
  $('#apOk').onclick = async () => {
    try { await accPatch(a, { plan: $('#apSel').value }); closeDlg(); redrawAccounts(); } catch (e) { toast(e.message); }
  };
}

const accPatch = (a, body) => api(`/api/accounts/${encodeURIComponent(a.id)}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });

function wireAccounts(host, v) {
  const q = s => [...host.querySelectorAll(s)];
  const find = el => v.accounts.find(a => a.id === el.closest('[data-acc]').dataset.acc);
  q('[data-add-acc]').forEach(b => { b.onclick = () => dlgAddAccount(b.dataset.addAcc); });
  q('[data-use]').forEach(b => {
    b.onclick = async () => {
      const a = find(b);
      try {
        await api('/api/accounts/mode', { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ provider: a.provider, mode: a.builtin ? '' : a.id }) });
        toast(`${a.provider === 'claude' ? 'Claude Code' : 'Codex'} 现在用「${a.label}」`);
      } catch (e) { toast(e.message); }
      redrawAccounts();
    };
  });
  q('[data-unblock]').forEach(b => {
    b.onclick = async () => {
      try { await api(`/api/accounts/${encodeURIComponent(find(b).id)}/models/${encodeURIComponent(b.dataset.unblock)}`, { method: 'DELETE' }); redrawAccounts(); }
      catch (e) { toast(e.message); }
    };
  });
  q('[data-more]').forEach(b => {
    b.onclick = () => {
      const a = find(b), tok = a.kind === 'token';
      openMenu(b, [
        !tok && { label: a.status === 'ok' ? '重新登录' : '登录', run: () => dlgAccountLogin(a) },
        (tok || a.provider === 'codex') && { label: '刷新额度', run: async () => {
          try { await post(`/api/accounts/${encodeURIComponent(a.id)}/quota`); } catch (e) { toast(e.message); }
          redrawAccounts();
        } },
        a.provider === 'claude' && !a.builtin && { label: tok ? '更换 token' : '改用长期 token', run: () => dlgAccountToken(a) },
        tok && { label: a.plan ? '改订阅类型' : '标订阅类型', run: () => dlgAccountPlan(a) },
        { label: '改名', run: async () => {
          const name = await askText('新的名字', a.label, { ok: '保存' });
          if (name == null || !name.trim()) return;
          try { await accPatch(a, { label: name }); redrawAccounts(); } catch (e) { toast(e.message); }
        } },
        { label: a.disabled ? '启用' : '停用', run: async () => {
          try { await accPatch(a, { disabled: !a.disabled }); redrawAccounts(); } catch (e) { toast(e.message); }
        } },
        !a.builtin && '-',
        !a.builtin && { label: '删除', danger: true, run: async () => {
          const how = tok ? '会删掉保存的 token 和它的配置目录。token 本身在 Anthropic 那边仍然有效，不用了请到 claude.ai 的设置里吊销。' : '会先让 CLI 退出登录，再删掉它的配置目录。';
          if (!await ask(`删除账号「${a.label}」？\n${how}共享的设置和会话历史不受影响。`, { ok: '删除', danger: true })) return;
          try { await api(`/api/accounts/${encodeURIComponent(a.id)}`, { method: 'DELETE' }); toast('已删除'); redrawAccounts(); }
          catch (e) { toast(e.message); }
        } },
      ]);
    };
  });
}

// 账号数据变了：重画当前页面上显示账号的那一块。
async function redrawAccounts() {
  const path = (S.route || '').split('?')[0];
  if (path === '#/runtimes') return drawRuntimes();
  const m = path.match(/^#\/runtimes\/(.+)$/);
  if (m && accSupported(decodeURIComponent(m[1])) && $('#accTab')) return drawAccountsTab(decodeURIComponent(m[1]));
  await loadAccounts();
  if (S.ws) drawAccChip();
}

async function drawAccountsTab(rt) {
  const host = $('#agentBody'); if (!host) return;
  const v = await loadAccounts();
  if (!$('#agentBody')) return;
  host.innerHTML = `<div class="card" id="accTab">
    <h3>账号</h3>
    <div class="desc">每个账号是一个独立的 CLI 配置目录。浏览器登录的账号由官方 CLI 自己保管凭据，Blazar 不读取；长期 token 存在账号目录里只有你能读的文件中，启动时才交给 CLI。
      点「使用」切换这个运行时用的账号；对话里也可以从运行时选择器单独换。设置、技能和会话历史在各账号之间共享，换账号后上下文不变。${TIP(`账号目录在 <span class="mono">${esc(v.root || '')}</span>。只对在本机运行的会话生效。`)}</div>
    ${accountsSection(v, rt)}</div>`;
  wireAccounts(host, v);
}

const TOKEN_HELP = `在浏览器里登录<b>这个</b>账号，然后在终端运行 <span class="mono">claude setup-token</span>，把最后输出的
  <span class="mono">sk-ant-oat01-…</span> 整段复制过来。每个账号各生成一次即可，有效期约一年。`;

function dlgAddAccount(provider = 'claude') {
  const label = ACC_PROVIDERS.find(([p]) => p === provider)?.[1] || provider;
  openDlg(`<h3>给 ${esc(label)} 添加账号</h3>
    <div class="field"><label>名字</label><input id="naL" class="input" maxlength="40" placeholder="例如：工作号、个人 Max"></div>
    <div class="field" id="naHowF"><label>接入方式</label><select id="naHow" class="input">
      <option value="token">粘贴长期 token（claude setup-token）</option><option value="login">在浏览器里登录</option></select></div>
    <div class="field" id="naTokF"><label>长期 token</label><input id="naTok" class="input mono" type="password" autocomplete="off" spellcheck="false" placeholder="sk-ant-oat01-…">
      <div class="t-caption faint" style="margin-top:6px">${TOKEN_HELP}</div></div>
    <div class="t-caption faint" id="naLoginHint">创建后会打开一个终端运行官方登录命令，按提示在浏览器里登录这个账号即可。</div>
    <div class="dfoot"><button class="btn btn-outline" id="naNo">取消</button><button class="btn btn-brand" id="naOk">创建</button></div>`);
  const sync = () => {
    const claude = provider === 'claude', token = claude && $('#naHow').value === 'token';
    $('#naHowF').hidden = !claude; $('#naTokF').hidden = !token; $('#naLoginHint').hidden = token;
    $('#naOk').textContent = token ? '创建' : '创建并登录';
  };
  $('#naHow').onchange = sync; sync();
  $('#naL').focus();
  $('#naNo').onclick = closeDlg;
  $('#naOk').onclick = async () => {
    const name = $('#naL').value.trim();
    if (!name) { toast('名字必填'); $('#naL').focus(); return; }
    const token = !$('#naTokF').hidden ? $('#naTok').value.trim() : '';
    if (!$('#naTokF').hidden && !token) { toast('把 claude setup-token 生成的 token 粘贴进来'); $('#naTok').focus(); return; }
    try {
      const a = await post('/api/accounts', { provider, label: name, token: token || null });
      if (token) {
        closeDlg();
        toast(a.status === 'ok' ? `「${a.label}」已添加，第一次运行时会验证 token` : `「${a.label}」已添加，但 CLI 没认出这个 token`);
        redrawAccounts();
      } else { await redrawAccounts(); dlgAccountLogin(a); }
    } catch (e) { toast(e.message); }
  };
}

function dlgAccountToken(a) {
  openDlg(`<h3>${a.kind === 'token' ? '更换' : '改用'}长期 token · ${esc(a.label)}</h3>
    <div class="field"><input id="atTok" class="input mono" type="password" autocomplete="off" spellcheck="false" placeholder="sk-ant-oat01-…"></div>
    <div class="t-caption faint">${TOKEN_HELP}${a.kind === 'token' ? '' : '<br>改用 token 后，这个账号原来的浏览器登录就不再使用了。'}</div>
    <div class="dfoot"><button class="btn btn-outline" id="atNo">取消</button><button class="btn btn-brand" id="atOk">保存</button></div>`);
  $('#atTok').focus();
  $('#atNo').onclick = closeDlg;
  $('#atOk').onclick = async () => {
    const token = $('#atTok').value.trim();
    if (!token) { $('#atTok').focus(); return; }
    try {
      const r = await api(`/api/accounts/${encodeURIComponent(a.id)}/token`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ token }) });
      closeDlg(); toast(r.status === 'ok' ? '已保存，第一次运行时会验证 token' : '已保存，但 CLI 没认出这个 token'); redrawAccounts();
    } catch (e) { toast(e.message); }
  };
}

function dlgAccountLogin(a) {
  dlgTermLogin({
    title: `登录 ${a.label}`,
    hint: `正在运行 <span class="mono">${esc(a.provider === 'claude' ? 'claude auth login' : 'codex login')}</span>${a.config_dir ? '（独立配置目录）' : '（默认登录）'}。浏览器没自动打开的话，复制终端里的链接去登录。`,
    path: `/api/accounts/${encodeURIComponent(a.id)}/login/ws`,
    after: async () => {
      try {
        const r = await post(`/api/accounts/${encodeURIComponent(a.id)}/check`);
        toast(r.status === 'ok' ? `「${r.label}」已登录${r.email ? '：' + r.email : ''}` : `「${r.label}」还没有登录成功`);
      } catch (e) { toast(e.message); }
      redrawAccounts();
    },
  });
}

// 在远端机器上登录 Codex：设备码方式，链接和验证码在下面的终端里，用本地浏览器打开完成验证。
function dlgNodeLogin(node, rt = 'codex') {
  dlgTermLogin({
    title: `在 ${node} 上登录 Codex`,
    hint: `正在 ${esc(node)} 上运行 <span class="mono">codex login --device-auth</span>。在本地浏览器打开下面的链接、输入验证码即可，凭据只保存在 ${esc(node)} 上。`,
    path: `/api/nodes/${encodeURIComponent(node)}/login/${encodeURIComponent(rt)}/ws`,
    after: async () => {
      try {
        const found = await api(`/api/nodes/${encodeURIComponent(node)}/agents`);
        S.scan = S.scan || {}; S.scan[node] = found;
        const c = found.find(x => x.id === rt);
        toast(c?.authed ? `${node} 上的 Codex 已登录` : `${node} 上的 Codex 还没有登录成功`);
      } catch (e) { toast(e.message); }
    },
  });
}

function dlgTermLogin({ title, hint, path, after }) {
  openDlg(`<h3>${esc(title)}</h3>
    <div class="t-caption faint" style="margin-bottom:8px">${hint}</div>
    <div id="accTerm" style="height:320px;background:var(--muted);border-radius:var(--r-md);padding:6px"></div>
    <div class="dfoot"><span class="t-caption faint grow" id="accTermState">登录中…</span><button class="btn btn-brand" id="accTermDone">完成</button></div>`);
  $('#dlgBody').classList.add('wide');
  const term = new Terminal({ fontSize: 12.5, cursorBlink: true, scrollback: 2000, allowTransparency: true,
    fontFamily: 'JetBrains Mono, ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
    theme: isDark() ? { background: '#00000000', foreground: '#e8edf5', cursor: '#8b7dff' } : { background: '#00000000', foreground: '#0b1220', cursor: '#5b4fe0' } });
  const fit = new FitAddon.FitAddon();
  term.loadAddon(fit); term.open($('#accTerm')); fit.fit();
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const sock = new WebSocket(`${proto}://${location.host}${path}?cols=${term.cols}&rows=${term.rows}`);
  sock.binaryType = 'arraybuffer';
  sock.onmessage = e => term.write(e.data instanceof ArrayBuffer ? new Uint8Array(e.data) : e.data);
  sock.onclose = () => { const s = $('#accTermState'); if (s) s.textContent = '登录流程已结束'; };
  term.onData(d => { if (sock.readyState === 1) sock.send(JSON.stringify({ type: 'input', data: d })); });
  let done = false;
  const finish = async () => {
    if (done) return; done = true;
    clearInterval(watch);
    try { sock.close(); } catch (_) {}
    term.dispose(); closeDlg();
    await after();
  };
  const watch = setInterval(() => { if ($('#dlg').dataset.open !== 'true') finish(); }, 400);
  $('#accTermDone').onclick = finish;
}

// ── 输入框里的账号：按对话记住，下一条消息起生效；换号续接同一个会话，上下文不变 ──

const accSelKey = (thread = S.viewSession) => `blazar.acc.${S.ws?.id}.${thread || 'new'}`;
function accSel(thread) { try { return localStorage.getItem(accSelKey(thread)) || ''; } catch (_) { return ''; } }
function setAccSel(v, thread) {
  try { if (v) localStorage.setItem(accSelKey(thread), v); else localStorage.removeItem(accSelKey(thread)); } catch (_) {}
  drawAccChip();
}
// 新对话发出去拿到 thread id 之后，把「新对话」上选的账号挪过去。
function adoptAccSel(thread) {
  const v = accSel(null);
  if (v && thread) { setAccSel(v, thread); setAccSel('', null); }
}
function threadAccount() { return (S.threads || []).find(t => t.id === S.viewSession)?.account || null; }

function selectedProfileAccount() {
  const v = $('#agentSel')?.value || '';
  return v.startsWith('p:') ? (S.profiles || []).find(p => p.id === v.slice(2))?.account || '' : '';
}

function drawAccChip() { drawAgentChip(); }

// 这个对话接下来用哪个账号：对话里选过就用选的，否则按运行时的账号策略；策略是自动选择时看上一轮用的哪个。
function currentAccount(rt) {
  const sel = accSel();
  if (sel && sel !== 'auto') return sel;
  const policy = S.accounts?.modes?.[rt] || '';
  if (policy && policy !== 'auto') return policy;
  if (policy === 'auto' || sel === 'auto') return accOf(threadAccount())?.provider === rt ? threadAccount() : null;
  return `${rt}-default`;
}

// 运行时选择器里，某个运行时下面的账号行。
const wsRemote = () => !!S.ws && S.ws.node !== 'local';
function accountRows(rt, current) {
  // 远端工作区：Codex 用那台机器自己的登录；Claude 只能用长期 token（浏览器登录的凭据带不过去）。
  if (wsRemote() && rt === 'codex') {
    const authed = S.scan?.[S.ws.node]?.find(x => x.id === 'codex')?.authed;
    return `<button class="cp-row cp-sub" data-node-login="${esc(S.ws.node)}"><span class="cp-t"><b>${esc(S.ws.node)} 上的 Codex 登录</b></span>
      <span class="cp-r">${authed === true ? '已登录 · 点击重新登录' : authed === false ? '未登录 · 点击登录' : '点击登录'}</span></button>`;
  }
  const list = (S.accounts?.accounts || []).filter(a => a.provider === rt);
  const cur = current ? currentAccount(rt) : null;
  return list.map(a => {
    const away = wsRemote() && a.kind !== 'token';
    const off = !accUsable(a) || away;
    const note = away ? '远端需长期 token' : off ? (a.disabled ? '已停用' : a.kind === 'token' ? 'token 无效' : '未登录') : accQuotaShort(a);
    return `<button class="cp-row cp-sub" data-av="r:${esc(rt)}" data-acc="${esc(a.id)}" ${off ? 'disabled' : ''}>
      <span class="cp-t"><b>${esc(a.label)}</b></span>${note ? `<span class="cp-r">${esc(note)}</span>` : ''}${a.id === cur ? '<span class="cp-ok">✓</span>' : ''}</button>`;
  }).join('');
}

// 失败的那一轮下面的「换个账号继续」：在同一个对话里续接，只换账号。
async function continueOnAnotherAccount() {
  const rt = currentRuntime();
  const last = threadAccount();
  const list = (S.accounts?.accounts || []).filter(a => a.provider === rt && a.id !== last && accUsable(a));
  if (!list.length) { toast('没有别的可用账号了，先在「运行时」页添加或登录一个'); return; }
  openDlg(`<h3>换个账号继续</h3>
    <div class="t-caption faint" style="margin-bottom:10px">接着这个对话做下去，上下文不变，只是换个账号。</div>
    ${list.map(a => `<button class="cp-row acc-pick" data-pick="${esc(a.id)}"><span class="cp-t"><b>${esc(a.label)}${a.plan ? ` · ${esc(accPlanLabel(a.plan))}` : ''}</b>
      <span>${esc(accQuotaShort(a) || '还没有额度数据')}</span></span></button>`).join('')}
    <div class="dfoot"><button class="btn btn-outline" onclick="closeDlg()">取消</button></div>`);
  $$('#dlgBody [data-pick]').forEach(b => {
    b.onclick = async () => {
      closeDlg();
      setAccSel(b.dataset.pick);
      try {
        const r = await post(`/api/workspaces/${S.ws.id}/prompt`, { ...sendOptions(), text: CONTINUE_TEXT, resume: true,
          resume_session: S.viewSession || null, account: b.dataset.pick, wait_secs: 20 });
        if (r.admitted === false) { toast(r.reason || '工作区正忙'); return; }
        if (r.session_id) { S.session = r.session_id; noteSent(r); }
        toast(`已换到「${accOf(b.dataset.pick)?.label || ''}」接着做`);
      } catch (e) { toast('没发出去：' + e.message); }
    };
  });
}
document.addEventListener('click', e => {
  if (e.target.closest?.('[data-acc-continue]')) continueOnAnotherAccount();
  const nl = e.target.closest?.('[data-node-login]');
  if (nl) { e.stopPropagation(); closePop(); dlgNodeLogin(nl.dataset.nodeLogin); }
}, true);
