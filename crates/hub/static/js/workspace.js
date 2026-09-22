const LAY_KEY = 'blazar.ws.layout.v1';

function loadLayout() {
  const def = {
    ex: 260, aux: 380, panel: 240,
    hideEx: false, hideAux: false, hidePanel: false,
    panelTab: 'term', auxTab: 'chat',
  };
  let raw = null;
  try { raw = localStorage.getItem(LAY_KEY); } catch (_) {  }
  const s = { ...def };
  try { if (raw) Object.assign(s, JSON.parse(raw)); } catch (_) {  }

  if (!raw && innerWidth <= 1180) s.hideAux = true;
  if (!raw && innerWidth <= 860) s.hideEx = true;
  return s;
}
function saveLayout() {
  try { localStorage.setItem(LAY_KEY, JSON.stringify(S.lay)); } catch (_) {  }
}

const LIM = { ex: [180, 480], aux: [300, 1200] };
const CODE_MIN = 360;
const clampN = (v, lo, hi) => Math.max(lo, Math.min(hi, v));
function effectiveSizes() {
  const l = S.lay;
  const g = $('#wsgrid');
  const W = g ? g.getBoundingClientRect().width : innerWidth;
  let ex = l.hideEx ? 0 : clampN(l.ex, ...LIM.ex);
  let aux = l.hideAux ? 0 : clampN(l.aux, ...LIM.aux);

  let over = ex + aux + CODE_MIN + 20 - W;
  if (over > 0 && aux) { const d = Math.min(over, aux - LIM.aux[0]); aux -= d; over -= d; }
  if (over > 0 && ex) { const d = Math.min(over, ex - LIM.ex[0]); ex -= d; }
  const H = g ? g.getBoundingClientRect().height : innerHeight;
  const panel = clampN(l.panel, 90, Math.max(90, Math.min(H * 0.7, H - 160)));
  return { ex, aux, panel };
}
function applyLayout() {
  const g = $('#wsgrid');
  if (!g) return;
  const l = S.lay;
  const e = effectiveSizes();
  g.style.setProperty('--explorer-w', e.ex + 'px');
  g.style.setProperty('--aux-w', e.aux + 'px');
  g.style.setProperty('--panel-h', e.panel + 'px');
  const hide = (sel, v) => { const e = $(sel); if (e) e.dataset.collapsed = String(!!v); };
  hide('#ws-explorer', l.hideEx);  hide('#sp-ex', l.hideEx);
  hide('#ws-aux', l.hideAux);      hide('#sp-aux', l.hideAux);
  hide('#ws-panel', l.hidePanel);  hide('#sp-panel', l.hidePanel);
  const press = (sel, v) => $(sel)?.setAttribute('aria-pressed', String(v));
  press('#tgEx', !l.hideEx); press('#tgPanel', !l.hidePanel); press('#tgAux', !l.hideAux);

  S.termResize?.();
}
function toggleRegion(which) {
  if (!$('#wsgrid')) return;
  const key = { ex: 'hideEx', panel: 'hidePanel', aux: 'hideAux' }[which];
  S.lay[key] = !S.lay[key];
  applyLayout(); saveLayout();

  if (which === 'panel' && !S.lay.hidePanel) showPanelTab(S.lay.panelTab);
}
function wireSplits() {
  const clamp = (v, lo, hi) => Math.max(lo, Math.min(Math.max(lo, hi), v));
  const drag = (sel, dir, apply) => {
    const el = $(sel);
    if (!el) return;
    el.onpointerdown = e => {
      e.preventDefault();
      const p0 = dir === 'x' ? e.clientX : e.clientY;
      const base = effectiveSizes();
      el.dataset.drag = 'true';
      document.body.dataset.dragging = dir;
      let raf = 0;
      const move = ev => {
        apply(base, (dir === 'x' ? ev.clientX : ev.clientY) - p0);

        if (!raf) raf = requestAnimationFrame(() => { raf = 0; applyLayout(); });
      };
      const up = () => {
        el.dataset.drag = 'false';
        delete document.body.dataset.dragging;
        removeEventListener('pointermove', move);
        removeEventListener('pointerup', up);
        applyLayout(); saveLayout();
      };
      addEventListener('pointermove', move);
      addEventListener('pointerup', up);
    };
    el.ondblclick = () => {
      toggleRegion({ '#sp-ex': 'ex', '#sp-aux': 'aux', '#sp-panel': 'panel' }[sel]);
    };
  };

  const room = () => ($('#wsgrid')?.getBoundingClientRect().width || innerWidth) - CODE_MIN - 20;
  drag('#sp-ex', 'x', (b, d) => {
    const other = S.lay.hideAux ? 0 : effectiveSizes().aux;
    S.lay.ex = clamp(b.ex + d, LIM.ex[0], Math.min(LIM.ex[1], room() - other));
  });
  drag('#sp-aux', 'x', (b, d) => {
    const other = S.lay.hideEx ? 0 : effectiveSizes().ex;
    S.lay.aux = clamp(b.aux - d, LIM.aux[0], Math.min(LIM.aux[1], room() - other));
  });
  drag('#sp-panel', 'y', (b, d) => {
    const H = $('#wsgrid')?.getBoundingClientRect().height || innerHeight;
    S.lay.panel = clamp(b.panel - d, 90, Math.max(90, Math.min(H * 0.7, H - 160)));
  });
}
function showPanelTab(name) {
  S.lay.panelTab = name; saveLayout();
  drawTermTabs();
  $$('#panelbar .rtab').forEach(t => { t.dataset.active = String(t.dataset.p === name); });
  $$('#panelbody .ppane').forEach(p => { p.dataset.active = String(p.id === 'pp-' + name); });

  if (name === 'term') {
    if (!S.termWs) openTerm(); else setTimeout(() => S.termResize?.(), 0);
  }
  if (name === 'git') { drawGit(); loadGit(); }
  if (name === 'preview') { drawPreview(); loadDev(); }
  if (name === 'diff' && diffPrefs().base === 'target') loadDiff();
}
function showAuxTab(name) {
  S.lay.auxTab = name; saveLayout();
  $$('#auxbar .rtab').forEach(t => { t.dataset.active = String(t.dataset.a === name); });
  $$('#auxbody .apane').forEach(p => { p.dataset.active = String(p.id === 'ap-' + name); });
}

const ICON = {

  left: `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.1">
    <rect class="fill" x="2" y="3" width="4" height="10" fill="currentColor" stroke="none"/>
    <rect x="1.5" y="2.5" width="13" height="11" rx="1.5"/><path d="M6 2.5v11"/></svg>`,
  bottom: `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.1">
    <rect class="fill" x="2" y="10" width="12" height="3.5" fill="currentColor" stroke="none"/>
    <rect x="1.5" y="2.5" width="13" height="11" rx="1.5"/><path d="M1.5 10h13"/></svg>`,
  right: `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.1">
    <rect class="fill" x="10" y="3" width="4" height="10" fill="currentColor" stroke="none"/>
    <rect x="1.5" y="2.5" width="13" height="11" rx="1.5"/><path d="M10 2.5v11"/></svg>`,
  refresh: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"
    stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-3-6.7"/>
    <path d="M21 3v6h-6"/></svg>`,
};

async function pageWorkspace(id) {

  const token = ++S.navToken;
  const d = await api(`/api/workspaces/${id}/detail`);
  if (token !== S.navToken) return;
  S.ws = { ...d, id };
  markSeen(id);
  S.lay = loadLayout();

  $('#page').innerHTML = `
    <div class="phead">
      <button class="btn btn-ghost btn-icon menu-btn" style="display:none" aria-label="菜单"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M4 7h16M4 12h16M4 17h16"/></svg></button>
      <a href="#/workspaces" class="muted t-label">工作区</a><span class="faint">/</span>
      <h1 class="truncate">${esc(d.name)}</h1>
      ${statusPill(d.activity)}
      <span class="t-caption faint mono truncate" style="max-width:280px"
        title="${esc(d.node)}:${esc(d.path)}">${esc(d.node)}:${esc(d.path)}</span>
      <div class="grow"></div>
      <button class="laybtn" id="tgEx" title="资源管理器 ⌘B">${ICON.left}</button>
      <button class="laybtn" id="tgPanel" title="面板 ⌘J">${ICON.bottom}</button>
      <button class="laybtn" id="tgAux" title="对话 ⌘⌥B">${ICON.right}</button>
    </div>
    <div id="wsgrid">

      <div class="region" id="ws-explorer">
        <div class="rhead">
          <span>资源管理器</span><div class="grow"></div>
          <span class="t-micro faint" id="treeCount"
            style="text-transform:none;letter-spacing:0;font-weight:400"></span>
          <button class="laybtn" id="btnTreeReload" title="重新扫描">${ICON.refresh}</button>
        </div>
        <input id="treeFilter" class="input input-sm" placeholder="筛选文件…">
        <div id="tree"><div class="empty">加载中…</div></div>
      </div>
      <div class="split" data-dir="x" id="sp-ex" title="拖动调宽度，双击收起"></div>

      <div class="region" id="ws-center">
        <div id="ws-editor-area">
          <div id="tabbar"></div>
          <div id="edbar" hidden></div>
          <div id="editorstack">
            <div id="editor"></div>
            <div id="mdview" hidden></div>
            <div id="welcome"><div class="empty">
              从左边选一个文件<br>
              <span class="t-caption">⌘B 资源管理器 · ⌘J 面板 · ⌘⌥B 对话</span>
            </div></div>
          </div>
        </div>
        <div class="split" data-dir="y" id="sp-panel" title="拖动调高度，双击收起"></div>
        <div class="region" id="ws-panel">
          <div class="rhead" id="panelbar">
            <div class="rtabs">
              <button class="rtab" data-p="term">终端</button>
              <button class="rtab" data-p="diff">差异</button>
              <button class="rtab" data-p="git">Git</button>
              <button class="rtab" data-p="preview">预览</button>
            </div>
            <div class="tmux-tabs" id="termTabs"></div>
            <div class="grow"></div>
            <button class="laybtn" id="btnTermNew" title="重开这个终端（丢掉它的 tmux 会话）">${ICON.refresh}</button>
            <button class="laybtn" id="btnPanelHide" title="收起面板 ⌘J">▾</button>
          </div>
          <div id="panelbody">
            <div class="ppane" id="pp-term"><div id="term"></div></div>
            <div class="ppane" id="pp-diff"><div id="diffbar"></div>
              <div id="diffwrap"><div id="difftree" hidden></div><div id="diffview"></div></div></div>
            <div class="ppane" id="pp-git"><div id="gitview"></div></div>
            <div class="ppane" id="pp-preview"><div id="pvbar"></div><div id="pvbody"></div></div>
          </div>
        </div>
      </div>
      <div class="split" data-dir="x" id="sp-aux" title="拖动调宽度，双击收起"></div>

      <aside class="region" id="ws-aux">
        <div class="rhead" id="auxbar">
          <div class="chat-tabs" id="chatTabs"></div>
          <button class="laybtn" id="btnHist" title="对话历史" aria-label="对话历史"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 12a9 9 0 1 0 3-6.7M3 4v4h4"/><path d="M12 7v5l3 2"/></svg></button>
          <button class="laybtn" id="btnNewChat" title="新对话" aria-label="新对话"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14"/></svg></button>
          <button class="laybtn" id="btnAuxHide" title="收起 ⌘⌥B" aria-label="收起对话栏"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M6 6l12 12M18 6 6 18"/></svg></button>
        </div>
        <div id="auxbody">
          <div class="apane" id="ap-chat">
            <div id="log"><div class="empty">加载中…</div></div>
            <div id="composer">
              <div class="cb-pop" id="cbPop" hidden></div>
              <div id="ccTodos" hidden></div>
              <div id="cbRate" hidden></div>
              <div id="cbReview" hidden></div>
              <div id="cbQueue" hidden></div>
              <div class="cc-box">
                <div class="cb-atts" id="cbAtts" hidden></div>
                <textarea id="prompt" rows="1" placeholder="Message the agent…"></textarea>
                <div class="cb-bar">
                  <button class="cb-ic" id="cbPlus" title="Attach" aria-label="Attach">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg></button>
                  <button class="cb-ic" id="cbSlash" title="Commands (/)" aria-label="Commands">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="3.5" y="3.5" width="17" height="17" rx="3"/><path d="M14 8l-4 8"/></svg></button>
                  <span class="cb-ring" id="cbRing"></span>
                  <span class="cb-time num" id="cbTime"></span>
                  <button class="cb-chip" id="cbModel" title="Model"></button>
                  <span class="cb-div" id="cbDiv" hidden></span>
                  <span class="cb-chip cb-ctx" id="cbCtx" hidden><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"><path d="M6 3h8l4 4v14H6z"/><path d="M14 3v4h4"/></svg><span class="nm"></span><button class="x" id="cbCtxX" title="Don't include the current file" aria-label="Don't include the current file">×</button></span>
                  <span class="cb-chip cb-fresh" id="cbFresh" hidden title="Next message starts a new conversation">New conversation ×</span>
                  <span class="cb-grow"></span>
                  <button class="cb-chip cb-plain" id="cbAgent" title="Agent"></button>
                  <button class="cb-chip cb-plain" id="ccMode" title="Modes (shift+tab)"></button>
                  <select id="agentSel" class="cb-sel"></select>
                  <select id="permMode" hidden>
                    <option value="">Default</option>
                    <option value="default">Manual</option>
                    <option value="acceptEdits">Edit automatically</option>
                    <option value="plan">Plan</option>
                    <option value="auto">Auto</option>
                    <option value="bypassPermissions">Bypass permissions</option>
                    <option value="read-only">Read Only</option>
                    <option value="workspace-write">Auto</option>
                    <option value="danger-full-access">Full Access</option>
                  </select>
                  <input type="checkbox" id="resumeChk" checked hidden>
                  <input type="file" id="cbFile" accept="image/png,image/jpeg,image/gif,image/webp" multiple hidden>
                  <button class="cb-send" id="btnSend" title="Send (enter)" aria-label="Send">
                    <svg class="i-send" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 19V5M5.5 11.5 12 5l6.5 6.5"/></svg>
                    <svg class="i-stop" viewBox="0 0 24 24" fill="currentColor"><rect x="6.5" y="6.5" width="11" height="11" rx="2"/></svg></button>
                </div>
              </div>
            </div>
          </div>
        </div>
      </aside>
    </div>`;

  mountEditor();
  fillAgentSel();

  $('#tgEx').onclick = () => toggleRegion('ex');
  $('#tgPanel').onclick = () => toggleRegion('panel');
  $('#tgAux').onclick = () => toggleRegion('aux');
  $('#btnPanelHide').onclick = () => toggleRegion('panel');
  $('#btnAuxHide').onclick = () => toggleRegion('aux');
  $('#btnHist').onclick = openHistPop;
  $('#btnNewChat').onclick = newChat;
  $('#chatTabs').onclick = e => {
    const x = e.target.closest('[data-close]');
    if (x) { e.stopPropagation(); closeChatTab(x.dataset.close); return; }
    if (e.target.closest('[data-add]')) { newChat(); return; }
    const t = e.target.closest('.ctab[data-tid]'); if (t) activateTab(t.dataset.tid === '' ? null : t.dataset.tid);
  };
  $('#chatTabs').ondblclick = async e => {
    const t = e.target.closest('.ctab[data-tid]'); if (!t || !t.dataset.tid) return;
    const id = t.dataset.tid;
    const name = await askText('对话标题', threadTitle(id), { ok: '改名' });
    if (name == null || !name.trim()) return;
    try { const r = await api(`/api/sessions/${id}/title`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ title: name.trim() }) });
      if (r.ok) { (S.threads.find(x => x.id === id) || {}).title = r.title; drawChatTabs(); } else toast(r.reason || '改名失败'); }
    catch (err) { toast('改名失败: ' + err.message); }
  };
  $('#panelbar').onclick = e => {
    const t = e.target.closest('.rtab'); if (t) showPanelTab(t.dataset.p);
  };

  $('#treeFilter').oninput = e => renderTree(e.target.value);
  $('#btnTreeReload').onclick = () => loadTree();
  $('#tree').onclick = onTreeClick;
  $('#tabbar').onclick = onTabbarClick;
  $('#btnSend').onclick = () => {

    if (wsRunning() && !$('#prompt').value.trim()) stopSession(); else send();
  };
  const pr = $('#prompt');
  const grow = growPrompt;
  pr.oninput = () => {
    grow(); drawStatus();

    if (/^\/\S*$/.test(pr.value)) { slashSel = 0; drawSlash(); }
    else if (S.slashItems && !$('#cbPop').hidden && !pr.value.startsWith('/')) { S.slashItems = null; closePop(); }
    else if (/(^|\s)@$/.test(pr.value.slice(0, pr.selectionStart))) {
      pr.value = pr.value.slice(0, pr.selectionStart - 1) + pr.value.slice(pr.selectionStart);
      openFilePop();
    }
  };
  pr.onkeydown = e => {

    if (S.slashItems && !$('#cbPop').hidden && /^\/\S*$/.test(pr.value)) {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        slashSel = (slashSel + (e.key === 'ArrowDown' ? 1 : -1) + S.slashItems.length) % Math.max(1, S.slashItems.length);
        drawSlash(); return;
      }
      if ((e.key === 'Enter' && !e.isComposing) || e.key === 'Tab') { e.preventDefault(); runSlash(slashSel); return; }
    }

    if (e.key === 'Enter' && !e.isComposing && (uiPrefs().sendKey === 'mod' ? (e.metaKey || e.ctrlKey) : !e.shiftKey)) { e.preventDefault(); send().then(grow); }
    else if (e.key === 'Tab' && e.shiftKey) { e.preventDefault(); cycleMode(); }
    else if (e.key === 'Escape' && !$('#cbPop').hidden) { e.preventDefault(); closePop(); }
    else if (e.key === 'Escape' && wsRunning()) { e.preventDefault(); e.stopPropagation(); stopSession(); }
  };
  $('#ccMode').onclick = openModePop;
  $('#cbPlus').onclick = openPlusPop;
  $('#cbAgent').onclick = openAgentPop;
  $('#cbCtxX').onclick = e => { e.stopPropagation(); S.ctxOff = S.file; drawCtxChip(); };
  $('#cbFile').onchange = e => { [...e.target.files].forEach(addImage); e.target.value = ''; };
  S.attach = [];
  pr.onpaste = e => {
    const imgs = [...(e.clipboardData?.items || [])].filter(i => i.kind === 'file' && i.type.startsWith('image/'));
    if (!imgs.length) return;
    e.preventDefault();
    imgs.forEach(i => addImage(i.getAsFile()));
  };
  const box = $('#composer .cc-box');
  box.ondragover = e => { if ([...(e.dataTransfer?.items || [])].some(i => i.type.startsWith('image/'))) e.preventDefault(); };
  box.ondrop = e => {
    const imgs = [...(e.dataTransfer?.files || [])].filter(f => f.type.startsWith('image/'));
    if (!imgs.length) return;
    e.preventDefault(); e.stopPropagation();
    imgs.forEach(addImage);
  };
  $('#cbSlash').onclick = openCmdPop;
  $('#cbModel').onclick = openModelPop;
  drawCtxChip(); drawRateBanner();
  $('#cbFresh').onclick = () => setFresh(false);
  syncModeForRuntime(); drawStatus(); drawModelChip();

  $('#btnTermNew').onclick = () => { closeTerm(); openTerm(); };
  wireSplits();
  wireInspector(d);
  wireHeader();
  applyLayout();
  showAuxTab('chat');
  initChatTabs();
  wireDiff(); wireGit(); wirePreview(); wireEdBar();
  S.dev = null; S.pvUrl = ''; S.devBusy = null;
  S.git = null; S.gitOut = null; S.gitPr = null;
  { const c = $('#ws-center'); if (c) c.dataset.max = String(!!S.lay.panelMax); }
  if (!S.lay.hidePanel) showPanelTab(S.lay.panelTab);
  renderTabs();
  drawReviewBanner();

  S.queue = []; S.running = null;
  loadTree(); loadHistory(); loadDiff(); loadGit(true); loadQueue();
  try {
    const ctx = await api(`/api/workspaces/${id}/context`);
    if (token !== S.navToken) return;
    if (ctx?.editor?.file) await openFile(ctx.editor.file, { line: ctx.editor.line });
  } catch (_) {  }
}
