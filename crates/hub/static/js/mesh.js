async function drawMeshLocal() {
  const host = $('#meshLocal'); if (!host) return;
  let v;
  try { v = await api('/api/mesh/local'); }
  catch (e) { host.innerHTML = `<h3>本机</h3><div class="empty">${esc(e.message)}</div>`; return; }
  const st = v.status, n = st.node;

  const other = st.joined && st.other_engine ? `<div class="notice warn">本机还在运行另一个 EasyTier
    （<span class="mono">${esc(st.other_engine)}</span>）。两个实例连同一个网络会互相抢路由，建议退出它。</div>` : '';
  if (!st.joined && st.external) {
    const x = st.external, xn = x.node;
    host.innerHTML = `<h3>本机 <span class="badge badge-ok">在线 · 由 ${esc(x.label)} 提供</span></h3>
      <div class="desc">这台电脑已经通过 ${esc(x.label)} 在组网里了，Blazar 直接复用它，不再装第二个实例（两个实例会抢同一网段的路由）。</div>
      <div class="kv">
        ${xn?.network_name ? `<span>网络</span><b class="mono">${esc(xn.network_name)}</b>` : ''}
        <span>虚拟地址</span><b class="mono">${esc(x.virtual_ipv4 || '未能识别')}</b>
        ${xn?.hostname ? `<span>主机名</span><b class="mono">${esc(xn.hostname)}</b>` : ''}
        ${xn ? `<span>可见节点</span><b>${x.peer_count}</b><span>NAT</span><b>${esc(xn.nat_type || '—')}</b>` : ''}
        ${x.rpc ? `<span>RPC</span><b class="mono faint">${esc(x.rpc)}</b>` : ''}
      </div>
      ${x.rpc ? '' : `<details class="takeover" style="margin-top:10px"><summary class="t-caption muted" style="text-align:left">连接 ${esc(x.label)} 的 RPC…</summary>
        <div class="notice">在 ${esc(x.label)} 设置里把「RPC 门户」设为 <span class="mono">127.0.0.1:15888</span>，可看到本机 NAT 与可见节点数。
        <div class="row" style="margin-top:8px"><input id="extRpc" class="input input-sm mono" style="max-width:220px" placeholder="127.0.0.1:15888">
          <button class="btn btn-outline btn-sm" id="extRpcSave">连接</button></div></div></details>`}
      <div class="actions" style="margin-top:12px">
        <button class="btn btn-outline btn-sm" id="meshRefresh">刷新节点</button>
        <div class="grow"></div>
        <details class="takeover"><summary class="t-caption muted">改由 Blazar 管理…</summary>
          <div class="t-caption muted" style="margin-top:6px;max-width:420px">先退出 ${esc(x.label)}，再用邀请文件或导入配置加入。</div></details>
      </div>`;
    $('#meshRefresh').onclick = async () => {
      try { const r = await post('/api/mesh/refresh'); toast(`发现 ${r.discovered} 个节点`); await refreshState(); drawMeshLocal(); }
      catch (e) { toast('刷新失败: ' + e.message); }
    };
    $('#extRpcSave')?.addEventListener('click', async () => {
      try {
        const r = await api('/api/mesh/external-rpc', { method: 'PUT', headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ rpc: $('#extRpc').value.trim() }) });
        toast(r.status.external?.rpc ? '已连上' : '还连不上 —— 确认 GUI 里设置的地址一致');
        drawMeshLocal();
      } catch (e) { toast(e.message); }
    });
    return;
  }
  if (!st.joined) {
    host.innerHTML = `<h3>本机 <span class="badge badge-muted">未加入组网</span></h3>
      ${st.engine_bundled ? '' : '<div class="notice danger">这个安装包没有带组网引擎，请安装完整版桌面端。</div>'}
      ${other}
      <div class="joinways">
        <button class="joinway" id="wayInvite">
          <b>用邀请文件加入</b>
          <span>管理员发给你的 <span class="mono">.blazar</span> 文件。新同事走这条路，什么都不用配。</span></button>
        <button class="joinway" id="wayToml">
          <b>导入 EasyTier 配置</b>
          <span>已有的 <span class="mono">config.toml</span>（网络密钥、子网代理、端口转发、ACL……全部照用）。
            也可以<a href="#" id="wayBlank">从空白配置开始写</a>。</span></button>
      </div>
      <div class="dropzone" id="meshDrop">或者把 <span class="mono">.blazar</span> / <span class="mono">.toml</span> 文件拖到这里</div>`;
    $('#wayInvite').onclick = () => $('#inviteFile')?.click();
    $('#wayToml').onclick = e => { if (e.target.id !== 'wayBlank') $('#tomlFile')?.click(); };
    $('#wayBlank').onclick = e => { e.preventDefault(); e.stopPropagation(); dlgEditConfig(BLANK_TOML); };
    $('#meshDrop').onclick = () => $('#inviteFile')?.click();
    return;
  }
  const pill = st.running
    ? `<span class="badge badge-ok">在线 · ${st.peer_count} 个节点可见</span>`
    : '<span class="badge badge-danger">引擎未运行</span>';
  host.innerHTML = `<h3>本机 ${pill}</h3>
    <div class="kv">
      <span>网络</span><b class="mono">${esc(n?.network_name || '—')}</b>
      <span>主机名</span><b class="mono">${esc(n?.hostname || '—')}</b>
      <span>虚拟地址</span><b class="mono">${esc(n?.virtual_ipv4 || '—')}</b>
      <span>NAT</span><b>${esc(n?.nat_type || '—')}</b>
      <span>引擎</span><b class="mono">EasyTier ${esc(n?.version || st.engine_version)}</b>
    </div>${other}
    ${st.running ? '' : `<div class="notice warn">组网服务没有在运行 ${TIP('重新打开邀请文件加入一次即可修复；日志在系统组网目录的 <span class="mono">logs/</span> 下。')}</div>`}
    <div class="actions" style="margin-top:12px">
      <button class="btn btn-outline btn-sm" id="meshRefresh">刷新节点</button>
      <button class="btn btn-outline btn-sm" id="meshEdit">编辑 EasyTier 配置</button>
      <button class="btn btn-ghost btn-sm" id="meshSwitch">换一份配置 / 邀请…</button>
      <div class="grow"></div>
      <button class="btn btn-ghost btn-sm danger" id="meshLeave">退出组网</button>
    </div>`;
  $('#meshRefresh').onclick = async () => {
    try { const r = await post('/api/mesh/refresh'); toast(`发现 ${r.discovered} 个节点`); await refreshState(); }
    catch (e) { toast('刷新失败: ' + e.message); }
  };
  $('#meshEdit').onclick = async () => {
    try { dlgEditConfig((await api('/api/mesh/config')).toml); }
    catch (e) { toast('读取配置失败：' + e.message); }
  };
  $('#meshSwitch').onclick = () => $('#tomlFile')?.click();
  $('#meshLeave').onclick = async () => {
    if (!await ask('退出组网后，本机上的组网凭据会被删除，其它机器将无法再连到本机。确定？', { ok: '退出', danger: true })) return;
    toast('等待系统授权…');
    try { await post('/api/mesh/leave'); toast('已退出组网'); drawMeshLocal(); }
    catch (e) { toast('退出失败: ' + e.message); }
  };
}

async function drawIssuer() {
  const host = $('#meshIssue'); if (!host) return;
  let v;
  try { v = await api('/api/mesh/issuer'); }
  catch (e) { host.innerHTML = `<h3>邀请同事</h3><div class="empty">${esc(e.message)}</div>`; return; }
  if (!v.configured) {
    host.innerHTML = `<h3>邀请同事 <span class="badge badge-muted">未配置签发节点</span>
      <div class="grow"></div><button class="btn btn-brand btn-sm" id="issCfg">签发设置</button></h3>
      <div class="desc">给同事签发邀请文件，需要一台<strong>持有网络密钥</strong>的机器（通常是枢纽，EasyTier 的 <span class="mono">network_secret</span> 在它的配置里）。
        在「签发设置」里填上它的 SSH Host（或 <span class="mono">local</span>），Blazar 会经 SSH 在那台机器上执行 <span class="mono">easytier-cli credential</span> 来签发和吊销。<br>
        只是想加入别人的组网，不用配这个：用邀请文件或导入 EasyTier 配置即可。</div>`;
    $('#issCfg').onclick = () => dlgIssuer(v);
    return;
  }
  const cfg = v.config;
  const where = `${esc(cfg.via)}${cfg.container ? ` · 容器 ${esc(cfg.container)}` : ''}`;
  const head = `<h3>邀请同事
      <span class="badge ${v.reachable ? 'badge-ok' : 'badge-danger'}">签发节点 ${where}${v.reachable ? '' : ' · 不可达'}</span>
      <div class="grow"></div><button class="btn btn-ghost btn-sm" id="issCfg">签发设置</button></h3>`;
  if (!v.reachable) {
    host.innerHTML = head + `<div class="notice danger keep">连不上签发节点 ${TIP('签发邀请需要连上持有网络密钥的节点（通常是枢纽）。')}
      <div class="mono t-caption" style="margin-top:4px">${esc(v.error || '')}</div></div>`;
    $('#issCfg').onclick = () => dlgIssuer(v);
    return;
  }
  const noEntry = !v.entry_points.length;
  host.innerHTML = head + `
    <div class="desc">生成一个邀请文件发给同事。文件里是<strong>只属于 TA 的</strong>入网凭据：
      可以单独吊销，到期自动失效，不会泄露整个网络的密钥。</div>
    <div class="kv" style="margin-bottom:14px">
      <span>网络</span><b class="mono">${esc(v.network_name || '—')}</b>
      <span>网段</span><b class="mono">${esc(v.subnet || '—')}</b>
      <span>入网地址</span><b class="mono">${v.entry_points.map(esc).join('<br>') ||
        '<span style="color:var(--destructive)">未设置 —— 点「签发设置」填写</span>'}</b>
    </div>
    <div class="grid3">
      <div class="field"><label>同事称呼</label><input id="invMember" class="input" placeholder="张三"></div>
      <div class="field"><label>主机名</label><input id="invHost" class="input mono" placeholder="zhangsan-mbp">
        <div class="hint">网内唯一；称呼是英文时可留空</div></div>
      <div class="field"><label>有效期</label><select id="invDays" class="input">
        <option value="30">30 天</option><option value="90">90 天</option>
        <option value="180" selected>180 天</option><option value="365">1 年</option></select></div>
    </div>
    <div class="grid3">
      <div class="field"><label>地址</label><select id="invIp" class="input">
        <option value="auto">自动分配固定地址</option><option value="dhcp">由网络动态分配</option></select></div>
      <div class="field" style="grid-column:span 2"><label>备注</label>
        <input id="invNote" class="input" placeholder="可选，如：算法组 · 实习"></div>
    </div>
    <div class="actions"><button class="btn btn-brand btn-sm" id="invGo" ${noEntry ? 'disabled' : ''}>生成邀请文件</button>
      <span class="t-caption faint" id="invMsg"></span></div>`;
  $('#issCfg').onclick = () => dlgIssuer(v);
  $('#invGo').onclick = async () => {
    const member = $('#invMember').value.trim();
    if (!member) { toast('请填写同事称呼'); $('#invMember').focus(); return; }
    $('#invGo').disabled = true;
    try {
      const r = await post('/api/mesh/invites', {
        member, hostname: $('#invHost').value.trim() || null,
        days: +$('#invDays').value, ipv4: $('#invIp').value, note: $('#invNote').value.trim(),
      });
      downloadText(r.file_name, r.content);
      $('#invMsg').innerHTML = `已生成 <span class="mono">${esc(r.file_name)}</span>
        （${esc(r.summary.ipv4 || '动态地址')}）—— 发给 ${esc(member)}，TA 用 Blazar 打开即可加入。
        <strong>文件即凭据，请私下发送。</strong>`;
      ['#invMember', '#invHost', '#invNote'].forEach(k => { $(k).value = ''; });
      drawInvites();
    } catch (e) { toast('生成失败: ' + e.message); }
    finally { $('#invGo').disabled = false; }
  };
}

function downloadText(name, text) {
  const url = URL.createObjectURL(new Blob([text], { type: 'application/x-blazar-invite' }));
  const a = Object.assign(document.createElement('a'), { href: url, download: name });
  document.body.appendChild(a); a.click(); a.remove();

  setTimeout(() => URL.revokeObjectURL(url), 2000);
}

function dlgIssuer(v) {
  const c = v.config || { via: '', container: '', entry_points: [] };
  openDlg(`<h3>签发设置</h3>
    <div class="desc muted t-caption" style="margin-bottom:14px">签发节点需在配置里设置 <span class="mono">credential_file</span>，否则它重启后签过的凭据全部失效。<br><br>签发节点是持有网络密钥（network_secret）的那台，
      通常是枢纽。Blazar 经 SSH 在它上面执行 <span class="mono">easytier-cli credential</span>。</div>
    <div class="grid2">
      <div class="field"><label>签发节点（SSH Host 或 local）</label>
        <input id="isVia" class="input mono" value="${esc(c.via)}" placeholder="hub-host"></div>
      <div class="field"><label>容器名（EasyTier 跑在 docker 里时）</label>
        <input id="isCt" class="input mono" value="${esc(c.container || '')}" placeholder="easytier"></div>
    </div>
    <div class="field"><label>入网地址（每行一个；留空则从签发节点配置推导）</label>
      <textarea id="isEp" class="input mono" rows="3" placeholder="tcp://203.0.113.10:11010">${
        esc((c.entry_points || []).join('\n'))}</textarea>
      <div class="hint">同事的电脑会连这些地址入网，必须公网可达。当前推导：${
        esc(v.entry_points.join('，') || '无')}</div></div>
    <div class="dfoot">${v.configured ? '<button class="btn btn-ghost" id="isClear" style="color:var(--destructive)">清除设置</button>' : ''}<span class="grow"></span><button class="btn btn-outline" onclick="closeDlg()">取消</button>
      <button class="btn btn-brand" id="isSave">保存</button></div>`);
  const clr = $('#isClear'); if (clr) clr.onclick = async () => {
    closeDlg();
    if (!await ask('清除签发节点设置？回到「未配置」，已签发的邀请记录不动。', { ok: '清除', danger: true })) return;
    try { await api('/api/mesh/issuer', { method: 'DELETE' }); toast('已清除'); drawIssuer(); } catch (e) { toast('清除失败: ' + e.message); }
  };
  $('#isSave').onclick = async () => {
    try {
      await api('/api/mesh/issuer', { method: 'PUT', headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          via: $('#isVia').value.trim(), container: $('#isCt').value.trim() || null,
          entry_points: $('#isEp').value.split('\n').map(x => x.trim()).filter(Boolean),
        }) });
      closeDlg(); toast('已保存'); drawIssuer();
    } catch (e) { toast('保存失败: ' + e.message); }
  };
}

async function drawInvites() {
  const host = $('#meshInvites'); if (!host) return;
  let v;
  try { v = await api('/api/mesh/invites'); }
  catch (e) { host.innerHTML = `<h3>已签发的邀请</h3><div class="empty">${esc(e.message)}</div>`; return; }
  const list = v.invites;
  const lost = list.filter(i => i.state === 'lost').length;
  const active = list.filter(i => ['online', 'waiting', 'offline'].includes(i.state)).length;
  const cnt = $('#cnt-mesh'); if (cnt) cnt.textContent = active || '';
  host.innerHTML = `<h3>已签发的邀请 <span class="badge badge-muted num">${list.length}</span></h3>
    ${lost ? `<div class="notice danger">${lost} 份邀请的凭据已丢失，需要重新签发 ${TIP('通常是签发节点重启、且没有配置 <span class="mono">credential_file</span>。')}</div>` : ''}
    ${v.issuer_checked ? '' : '<div class="t-caption faint" style="margin-bottom:8px">签发节点连不上，状态未核对</div>'}
    ${list.length ? `<table class="tb"><thead><tr>
        <th>同事</th><th>主机名</th><th>地址</th><th>状态</th><th>到期</th><th></th></tr></thead><tbody>
      ${list.map(i => {
        const [cls, text] = INV_STATE[i.state] || ['badge-muted', i.state];
        const live = ['online', 'waiting', 'offline', 'lost'].includes(i.state);
        return `<tr>
          <td title="${esc(i.note || '')}">${esc(i.member)}</td>
          <td class="m">${esc(i.hostname)}</td>
          <td class="m">${esc(i.ipv4 || '动态')}</td>
          <td><span class="badge ${cls}">${text}</span></td>
          <td class="t-caption muted" title="签发于 ${dateOf(i.issued_at)}${i.issued_by ? ' · ' + esc(i.issued_by) : ''}">${
            i.state === 'revoked' ? '—' : `${dateOf(i.expires_at)}${live ? `<span class="faint"> · ${daysLeft(i.expires_at)} 天</span>` : ''}`}</td>
          <td style="text-align:right">${live
            ? `<button class="btn btn-ghost btn-sm danger" data-revoke="${esc(i.id)}" data-who="${esc(i.member)}">吊销</button>` : ''}</td>
        </tr>`;
      }).join('')}</tbody></table>`
      : '<div class="t-caption faint">还没有签发过邀请。</div>'}`;
  $$('#meshInvites [data-revoke]').forEach(b => {
    b.onclick = async () => {
      if (!await ask(`吊销 ${b.dataset.who} 的邀请？TA 的电脑会立即断开组网。`, { ok: '吊销', danger: true })) return;
      try { await api(`/api/mesh/invites/${encodeURIComponent(b.dataset.revoke)}`, { method: 'DELETE' });
        toast('已吊销'); drawInvites(); }
      catch (e) { toast('吊销失败: ' + e.message); }
    };
  });
}

const BLANK_TOML = `# EasyTier 配置（完整字段见 https://easytier.cn/guide/network/configurations.html）
hostname = "my-machine"
dhcp = true

[network_identity]
network_name = "my-mesh"
network_secret = ""

# 枢纽（公网可达的那台）的入网地址
[[peer]]
uri = "tcp://203.0.113.10:11010"

# 子网代理：把本机所在的局域网暴露给组网
# [[proxy_network]]
# cidr = "192.168.1.0/24"

# 端口转发
# [[port_forward]]
# bind_addr = "0.0.0.0:8080"
# dst_addr = "10.99.0.11:80"
# proto = "tcp"

[flags]
# latency_first = true
`;

async function openMeshFile(file) {
  if (/\.toml$/i.test(file.name)) {
    if (file.size > 262144) { toast('配置文件过大'); return; }
    return openConfigText(await file.text());
  }
  return openInviteFile(file);
}

async function openConfigText(text) {
  try { showConfigDialog(await post('/api/mesh/config/preview', { toml: text }), text); }
  catch (e) { dlgEditConfig(text, e.message); }
}

function dlgEditConfig(text, error) {
  openDlg(`<h3>EasyTier 配置</h3>
    <div class="desc muted t-caption" style="margin-bottom:10px">应用时会弹一次系统授权，替换本机组网服务的配置并重启引擎。
      配置含组网密钥，只保存在本机 root 可读的位置。</div>
    <textarea id="cfgText" class="input mono cfgedit" spellcheck="false"></textarea>
    <div id="cfgErr" class="notice danger" style="display:${error ? 'block' : 'none'}">${esc(error || '')}</div>
    <div class="dfoot"><button class="btn btn-outline" onclick="closeDlg()">取消</button>
      <button class="btn btn-brand" id="cfgCheck">校验并预览</button></div>`);
  $('#dlgBody').classList.add('wide');
  $('#cfgText').value = text;
  $('#cfgCheck').onclick = async () => {
    const t = $('#cfgText').value;
    try { showConfigDialog(await post('/api/mesh/config/preview', { toml: t }), t); }
    catch (e) { $('#cfgErr').textContent = e.message; $('#cfgErr').style.display = 'block'; }
  };
}

function showConfigDialog(p, text) {
  const s = p.summary;
  openDlg(`<h3>应用 EasyTier 配置</h3>
    <div class="kv">
      <span>网络</span><b class="mono">${esc(s.network_name)}</b>
      <span>身份</span><b>${s.auth === 'network_secret' ? '网络密钥（管理员 / 枢纽）' : '个人凭据'}</b>
      <span>主机名</span><b class="mono">${esc(s.hostname || '（系统主机名）')}</b>
      <span>地址</span><b class="mono">${esc(s.ipv4 || (s.dhcp ? '自动分配' : '无'))}</b>
      <span>对端</span><b class="mono">${s.peers.map(esc).join('<br>') || '—'}</b>
      ${s.listeners.length ? `<span>监听</span><b class="mono">${s.listeners.map(esc).join('<br>')}</b>` : ''}
      ${s.features.length ? `<span>功能</span><b>${s.features.map(f => `<span class="badge badge-brand">${esc(f)}</span>`).join(' ')}</b>` : ''}
    </div>
    ${s.warnings.map(w => `<div class="notice warn">${esc(w)}</div>`).join('')}
    <div class="dfoot">
      <button class="btn btn-ghost" id="cfgBack">返回修改</button>
      <div class="grow"></div>
      <button class="btn btn-outline" onclick="closeDlg()">取消</button>
      <button class="btn btn-brand" id="cfgGo" ${p.can_join ? '' : 'disabled title="安装包没有带组网引擎"'}>应用</button>
    </div>`);
  $('#dlgBody').classList.remove('wide');
  $('#cfgBack').onclick = () => dlgEditConfig(text);
  $('#cfgGo').onclick = async () => {
    const btn = $('#cfgGo'); btn.disabled = true; btn.textContent = '等待系统授权…';
    try {
      const st = await api('/api/mesh/config', { method: 'PUT',
        headers: { 'content-type': 'application/json' }, body: JSON.stringify({ toml: text }) });
      closeDlg();
      toast(`已应用，${s.network_name} · 本机地址 ${st.node?.virtual_ipv4 || '分配中'}`);
      navigate('#/nodes'); render();
    } catch (e) {
      btn.disabled = false; btn.textContent = '应用';
      toast('应用失败：' + e.message);
    }
  };
}

async function openInviteFile(file) {
  if (file.size > 16384) { toast('这不是 Blazar 邀请文件'); return; }
  const text = await file.text();
  try { showJoinDialog(await post('/api/mesh/join/preview', { invite: text }), text); }
  catch (e) { toast('无法使用这个邀请：' + e.message); }
}

async function checkPendingInvite() {
  try {
    const v = await api('/api/mesh/local');
    if (v.pending) showJoinDialog(v.pending, null);
    else if (v.pending_error) toast('无法使用这个邀请：' + v.pending_error);
  } catch (_) {  }
}
window.blazarInvite = checkPendingInvite;

function showJoinDialog(p, text) {
  const s = p.summary;
  openDlg(`<h3>加入团队组网</h3>
    <div class="desc muted t-caption" style="margin-bottom:14px">${esc(s.issued_by || '管理员')} 邀请这台电脑加入组网。
      加入后，团队里的机器可以通过虚拟地址访问它，它也能访问团队的机器。</div>
    <div class="kv">
      <span>网络</span><b class="mono">${esc(s.network_name)}</b>
      <span>本机主机名</span><b class="mono">${esc(s.hostname)}</b>
      <span>本机地址</span><b class="mono">${esc(s.ipv4 || '自动分配')}</b>
      <span>入网地址</span><b class="mono">${s.peers.map(esc).join('<br>')}</b>
      <span>有效期至</span><b>${dateOf(s.expires_at)}（${daysLeft(s.expires_at)} 天）</b>
      ${s.note ? `<span>备注</span><b>${esc(s.note)}</b>` : ''}
    </div>
    ${p.warnings.map(w => `<div class="notice warn">${esc(w)}</div>`).join('')}
    <div class="t-caption muted" style="margin-top:12px">需要输入这台电脑的登录密码 ${TIP('组网要创建虚拟网卡，并把引擎装成开机自启的系统服务（关掉 Blazar 也保持在线）。')}</div>
    <div class="dfoot">
      <button class="btn btn-outline" id="jnNo">暂不加入</button>
      <button class="btn btn-brand" id="jnGo" ${p.can_join ? '' : 'disabled title="安装包没有带组网引擎"'}>加入</button>
    </div>`);
  $('#jnNo').onclick = async () => {
    closeDlg();
    if (!text) { try { await api('/api/mesh/join', { method: 'DELETE' }); } catch (_) {} }
  };
  $('#jnGo').onclick = async () => {
    const btn = $('#jnGo'); btn.disabled = true; btn.textContent = '等待系统授权…';
    try {
      const st = await post('/api/mesh/join', text ? { invite: text } : {});
      closeDlg();
      toast(`已加入 ${s.network_name}，本机地址 ${st.node?.virtual_ipv4 || s.ipv4 || ''}`);
      navigate('#/nodes'); render();

    } catch (e) {
      btn.disabled = false; btn.textContent = '加入';
      toast('加入失败：' + e.message);
    }
  };
}

addEventListener('dragover', e => {
  if ([...(e.dataTransfer?.items || [])].some(i => i.kind === 'file')) {
    e.preventDefault(); document.body.dataset.dropping = 'true';
  }
});
addEventListener('dragleave', e => { if (!e.relatedTarget) delete document.body.dataset.dropping; });
addEventListener('drop', e => {
  delete document.body.dataset.dropping;
  const files = [...(e.dataTransfer?.files || [])];

  const f = files.find(f => /\.blazar$/i.test(f.name)) ||
    (S.route === '#/nodes' && files.find(f => /\.toml$/i.test(f.name)));
  if (!f) return;
  e.preventDefault();
  openMeshFile(f);
});

const TIP = html => `<span class="tip" tabindex="0" role="button" aria-label="说明"><span class="tip-q">?</span><span class="tip-pop" role="tooltip">${html}</span></span>`;
function tipNode(html) {
  const t = document.createElement('template');
  t.innerHTML = TIP(html);
  return t.content.firstElementChild;
}
function tipify(root) {
  if (!root) return;
  root.querySelectorAll('.desc:not(.keep)').forEach(d => {
    const h = d.previousElementSibling;
    if (!h || !/^H[1-4]$/.test(h.tagName) || !d.textContent.trim()) return;
    const tip = tipNode(d.innerHTML);
    const grow = h.querySelector(':scope > .grow');
    if (grow) h.insertBefore(tip, grow); else h.append(tip);
    d.remove();
  });
  root.querySelectorAll('.field > .hint:not(.keep)').forEach(hn => {
    const lab = hn.parentElement.querySelector(':scope > label');
    if (!lab) return;
    lab.append(tipNode(hn.innerHTML));
    hn.remove();
  });
}
let tipQueued = false;
const tipObserver = new MutationObserver(() => {
  if (tipQueued) return;
  tipQueued = true;
  queueMicrotask(() => { tipQueued = false; tipify($('#main')); tipify($('#dlgBody')); });
});
tipObserver.observe($('#main'), { childList: true, subtree: true });
tipObserver.observe($('#dlgBody'), { childList: true, subtree: true });

const tipFloat = Object.assign(document.createElement('div'), { id: 'tipFloat', role: 'tooltip' });
document.body.appendChild(tipFloat);
let tipPinned = null;
function showTip(tip) {
  const pop = tip.querySelector('.tip-pop'); if (!pop) return;
  tipFloat.innerHTML = pop.innerHTML;
  tipFloat.dataset.show = 'true';
  const r = tip.getBoundingClientRect();
  const w = tipFloat.offsetWidth, h = tipFloat.offsetHeight;
  tipFloat.style.left = Math.max(12, Math.min(r.left - 10, innerWidth - w - 12)) + 'px';
  const below = r.bottom + 8 + h < innerHeight - 8;
  tipFloat.style.top = (below ? r.bottom + 8 : Math.max(8, r.top - h - 8)) + 'px';
}
function hideTip() { if (!tipPinned) tipFloat.dataset.show = 'false'; }
document.addEventListener('mouseover', e => {
  const t = e.target.closest?.('.tip');
  if (t) showTip(t);
  else if (!e.target.closest?.('#tipFloat')) hideTip();
});
document.addEventListener('focusin', e => { const t = e.target.closest?.('.tip'); if (t) showTip(t); });
document.addEventListener('focusout', e => { if (e.target.closest?.('.tip')) hideTip(); });

document.addEventListener('click', e => {
  const t = e.target.closest?.('.tip');
  if (t) {
    e.preventDefault(); e.stopPropagation();
    tipPinned = tipPinned === t ? null : t;
    if (tipPinned) showTip(t); else tipFloat.dataset.show = 'false';
    return;
  }
  if (!e.target.closest?.('#tipFloat') && tipPinned) { tipPinned = null; tipFloat.dataset.show = 'false'; }
}, true);
addEventListener('scroll', () => { tipPinned = null; tipFloat.dataset.show = 'false'; }, true);
