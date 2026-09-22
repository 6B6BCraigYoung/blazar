const PV_KEY = 'blazar.preview';
const pvPrefs = () => { try { return { device: 'desktop', log: false, ...JSON.parse(localStorage.getItem(PV_KEY) || '{}') }; } catch (_) { return { device: 'desktop', log: false }; } };
const setPvPrefs = p => { try { localStorage.setItem(PV_KEY, JSON.stringify({ ...pvPrefs(), ...p })); } catch (_) {} };
const PV_DEVICE = { desktop: ['桌面', '100%', '100%'], mobile: ['手机', '390px', '844px'], fluid: ['自适应', '70%', '100%'] };

async function loadDev() {
  if (!S.ws) return;
  const ws = S.ws.id;
  try { const d = await api(`/api/workspaces/${ws}/dev`); if (S.ws?.id !== ws) return; S.dev = { ...d, ws }; }
  catch (e) { S.dev = { ws, running: false, reason: e.message }; }
  drawPreview();
}
function drawPreview() {
  const bar = $('#pvbar'), body = $('#pvbody'); if (!bar || !body) return;
  const d = S.dev?.ws === S.ws?.id ? S.dev : null, p = pvPrefs();
  const busy = S.devBusy;
  bar.innerHTML = `
    <button class="btn ${d?.running ? 'btn-outline' : 'btn-brand'} btn-xs" data-a="${d?.running ? 'stop' : 'start'}" ${busy ? 'disabled' : ''}>${busy === 'start' ? '启动中…' : busy === 'stop' ? '停止中…' : d?.running ? '停止' : '启动 dev server'}</button>
    <input class="input input-sm mono pv-url" id="pvUrl" value="${esc(S.pvUrl || d?.preview_url || '')}" placeholder="${d?.running ? '还没从日志里认出地址，可以手动填 http://localhost:端口' : '启动后这里会出现地址'}" spellcheck="false">
    <button class="laybtn" data-a="reload" title="刷新页面">${ICON.refresh}</button>
    <span class="seg">${Object.entries(PV_DEVICE).map(([k, v]) => `<button data-a="device" data-v="${k}" data-on="${p.device === k}">${v[0]}</button>`).join('')}</span>
    <span class="grow"></span>
    ${d?.tunnel ? `<span class="gchip info" title="远端工作区：hub 起了一条 ssh 隧道，把 ${esc(d.node)} 的 ${d.port} 端口接到本机 ${d.tunnel}">隧道 ${esc(d.node)}:${d.port}</span>` : ''}
    ${d?.tunnel_error ? `<span class="gchip bad" title="${esc(d.tunnel_error)}">隧道没连上</span>` : ''}
    <button class="btn btn-outline btn-xs" data-a="log" aria-pressed="${p.log}">日志</button>
    <button class="btn btn-outline btn-xs" data-a="open" ${S.pvUrl || d?.preview_url ? '' : 'disabled'}>在浏览器打开</button>
    <button class="btn btn-outline btn-xs" data-a="scripts">脚本…</button>
    <button class="laybtn" data-a="max" aria-pressed="${!!S.lay.panelMax}" title="${S.lay.panelMax ? '还原面板' : '最大化面板'}">⤢</button>`;
  const url = S.pvUrl || d?.preview_url || '';
  const [, w, h] = PV_DEVICE[p.device] || PV_DEVICE.desktop;
  const frame = $('#pvFrame');
  if (!url) {
    body.innerHTML = `<div class="empty" style="padding:30px 20px">${d?.running ? '进程在跑，但还没从日志里认出本地地址。<br><span class="t-caption faint">看一眼日志，或者直接在上面填地址。</span>'
      : esc(d?.reason || '') || '这里可以起这个工作区的 dev server，把页面直接嵌进来看。<br><span class="t-caption faint">先在「脚本…」里填启动命令，比如 npm run dev。远端工作区会自动走 ssh 隧道。</span>'}</div>
      ${p.log && d?.log ? `<pre class="pv-log">${esc(d.log.slice(-6000))}</pre>` : ''}`;
    return;
  }

  if (!frame || frame.dataset.url !== url) {
    body.innerHTML = `<div class="pv-stage" data-device="${p.device}"><iframe id="pvFrame" data-url="${esc(url)}" src="${esc(url)}" title="预览"
      sandbox="allow-scripts allow-same-origin allow-forms allow-popups allow-modals allow-downloads" style="width:${w};height:${h}"></iframe></div><pre class="pv-log" id="pvLog" hidden></pre>`;
  } else {
    frame.style.width = w; frame.style.height = h; frame.parentElement.dataset.device = p.device;
  }
  const log = $('#pvLog'); if (log) { log.hidden = !p.log; if (p.log) { log.textContent = (d?.log || '').slice(-6000); log.scrollTop = log.scrollHeight; } }
}
function wirePreview() {
  const bar = $('#pvbar'); if (!bar || bar.dataset.wired) return;
  bar.dataset.wired = '1';
  bar.addEventListener('click', async e => {
    const b = e.target.closest('[data-a]'); if (!b || b.disabled) return;
    const a = b.dataset.a;
    if (a === 'start' || a === 'stop') {
      S.devBusy = a; drawPreview();
      try {
        const d = await post(`/api/workspaces/${S.ws.id}/dev/${a}`);
        S.dev = { ...d, ws: S.ws.id }; if (a === 'stop') S.pvUrl = '';
        if (a === 'start' && !d.preview_url) pollDev(8);
      } catch (err) { toast(err.message); if (/还没配置/.test(err.message)) dlgScripts(); }
      finally { S.devBusy = null; drawPreview(); }
    }
    else if (a === 'reload') { const f = $('#pvFrame'); if (f) f.src = f.dataset.url; else loadDev(); }
    else if (a === 'device') { setPvPrefs({ device: b.dataset.v }); drawPreview(); }
    else if (a === 'log') { setPvPrefs({ log: !pvPrefs().log }); await loadDev(); }
    else if (a === 'open') window.open(S.pvUrl || S.dev?.preview_url, '_blank', 'noopener');
    else if (a === 'scripts') dlgScripts();
    else if (a === 'max') { togglePanelMax(); drawPreview(); }
  });
  bar.addEventListener('keydown', e => {
    if (e.target.id === 'pvUrl' && e.key === 'Enter') { S.pvUrl = e.target.value.trim(); const f = $('#pvFrame'); if (f) f.dataset.url = ''; drawPreview(); }
  });
}

async function pollDev(times) {
  for (let i = 0; i < times; i++) {
    await new Promise(r => setTimeout(r, 1200));
    if (!S.ws || S.lay.panelTab !== 'preview') return;
    await loadDev();
    if (S.dev?.preview_url || !S.dev?.running) return;
  }
}

const SCRIPT_KIND = { setup: 'Setup', cleanup: 'Cleanup', copy: '拷贝文件' };
const SCRIPT_STATUS = { ok: ['成功', 'ok'], failed: ['失败', 'bad'], timeout: ['超时', 'bad'], running: ['运行中', 'info'] };
const SCRIPT_TRIGGER = { manual: '手动', create: '建工作区时', turn_end: '一轮结束后' };
async function dlgScripts() {
  if (!S.ws) return;
  const ws = S.ws.id;
  let cfg = {}, runs = [];
  try { [cfg, runs] = await Promise.all([api(`/api/workspaces/${ws}/scripts`), api(`/api/workspaces/${ws}/script-runs`)]); } catch (e) { toast(e.message); }
  const isolated = !!S.ws.isolated;
  openDlg(`<h3>脚本 · ${esc(S.ws.name)}</h3>
    <div class="t-caption muted" style="margin-bottom:10px">都在 <b class="mono">${esc(S.ws.node)}</b> 上、工作区目录里执行，和在终端里手敲一样。同一个源仓库新建的隔离工作区会沿用这份配置。</div>
    <div class="field"><label>Setup —— 隔离工作区建好后、agent 开工前跑一次（装依赖、生成代码…）</label><textarea id="scSetup" class="input mono" rows="3" placeholder="npm ci">${esc(cfg.setup || '')}</textarea></div>
    <div class="field"><label>Cleanup —— 每轮正常结束且有改动时跑（格式化、lint --fix…）</label><textarea id="scCleanup" class="input mono" rows="3" placeholder="npm run format">${esc(cfg.cleanup || '')}</textarea></div>
    <div class="field"><label>Dev server —— 预览面板用它起服务</label><textarea id="scDev" class="input mono" rows="2" placeholder="npm run dev">${esc(cfg.dev || '')}</textarea></div>
    <div class="field"><label>拷贝文件 —— 建隔离工作区时从源仓库带进来的、被 git 忽略的文件，一行一个（支持 glob）</label><textarea id="scCopy" class="input mono" rows="3" placeholder=".env&#10;config/*.local.json">${esc(cfg.copy_files || '')}</textarea></div>
    <div class="row" style="gap:6px;flex-wrap:wrap"><span class="faint t-caption">保存并测试：</span>
      <button class="btn btn-outline btn-xs" data-run="setup">Setup</button><button class="btn btn-outline btn-xs" data-run="cleanup">Cleanup</button>
      <button class="btn btn-outline btn-xs" data-run="copy" ${isolated ? '' : 'disabled title="只有隔离工作区才有源仓库可拷"'}>拷贝文件</button></div>
    <div class="ap-sec"><h5>最近的运行</h5><div id="scRuns">${scriptRunsHtml(runs)}</div></div>
    <div class="dfoot"><button class="btn btn-outline" onclick="closeDlg()">关闭</button><button class="btn btn-brand" id="scSave">保存</button></div>`);
  const save = async () => {
    await api(`/api/workspaces/${ws}/scripts`, { method: 'PUT', headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ setup: $('#scSetup').value, cleanup: $('#scCleanup').value, dev: $('#scDev').value, copy_files: $('#scCopy').value }) });
  };
  $('#scSave').onclick = async () => { try { await save(); toast('已保存'); closeDlg(); } catch (e) { toast(e.message); } };
  $$('#dlgBody [data-run]').forEach(b => { b.onclick = async () => {
    try { await save(); await post(`/api/workspaces/${ws}/scripts/${b.dataset.run}/run`); toast('开始跑了，结果会出现在下面'); refreshScriptRuns(); }
    catch (e) { toast(e.message); }
  }; });
}
function scriptRunsHtml(runs) {
  return runs.length ? runs.map(r => { const [t, tone] = SCRIPT_STATUS[r.status] || [r.status, '']; return `<details class="sc-run"><summary><span class="gchip ${tone}">${t}</span>
    <b>${SCRIPT_KIND[r.kind] || esc(r.kind)}</b><span class="faint">${SCRIPT_TRIGGER[r.trigger] || esc(r.trigger)}${r.exit_code != null && r.exit_code !== 0 ? ` · 退出码 ${r.exit_code}` : ''}</span>
    <span class="grow"></span><span class="faint t-micro">${ago(r.started_at)}</span></summary><pre>${esc(r.output || '（没有输出）')}</pre></details>`; }).join('')
    : '<div class="t-caption faint">还没跑过</div>';
}
async function refreshScriptRuns() {
  const host = $('#scRuns'); if (!host || !S.ws) return;
  try { host.innerHTML = scriptRunsHtml(await api(`/api/workspaces/${S.ws.id}/script-runs`)); } catch (_) {}
}
