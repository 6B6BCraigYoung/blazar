function closeTerm() {
  if (S.termResize) { removeEventListener('resize', S.termResize); S.termResize = null; }
  if (S.termWs) { try { S.termWs.close(); } catch (_) {} S.termWs = null; }
  if (S.term) { S.term.dispose(); S.term = null; S.fit = null; }
}

function termKey() { return 'blazar.terms.' + S.ws.id; }
function termTabs() {
  let v = null; try { v = JSON.parse(localStorage.getItem(termKey()) || 'null'); } catch (_) {}
  return Array.isArray(v) && v.length ? v : [0];
}
function saveTermTabs(list, cur) {
  try { localStorage.setItem(termKey(), JSON.stringify(list)); } catch (_) {}
  if (cur != null) S.termTab = cur;
  drawTermTabs();
}
function drawTermTabs() {
  const host = $('#termTabs'); if (!host) return;
  const list = termTabs();
  host.hidden = S.lay.panelTab !== 'term';
  host.innerHTML = list.map((n, i) => `<button class="tmux-tab" data-t="${n}" data-on="${n === S.termTab}">终端 ${i + 1}${
    list.length > 1 ? ` <span class="x" data-close="${n}">×</span>` : ''}</button>`).join('')
    + '<button class="tmux-tab" data-add title="新建终端">＋</button>';
  host.querySelectorAll('[data-t]').forEach(b => {
    b.onclick = e => {
      const c = e.target.closest('[data-close]');
      if (c) {
        const n = +c.dataset.close;
        const left = list.filter(x => x !== n);
        saveTermTabs(left, S.termTab === n ? left[0] : S.termTab);
        if (S.termTab === left[0]) openTerm();
        return;
      }
      if (+b.dataset.t === S.termTab) return;
      S.termTab = +b.dataset.t; drawTermTabs(); openTerm();
    };
  });
  host.querySelector('[data-add]').onclick = () => {
    const list2 = termTabs();
    const n = Math.max(...list2) + 1;
    saveTermTabs([...list2, n], n);
    openTerm();
  };
}
function openTerm() {
  if (!S.ws || typeof Terminal === 'undefined' || !$('#term')) return;
  closeTerm();
  if (S.termTab == null || !termTabs().includes(S.termTab)) S.termTab = termTabs()[0];
  drawTermTabs();
  const dark = isDark();
  S.term = new Terminal({
    fontSize: 12.5, cursorBlink: true, scrollback: 10000, allowTransparency: true,
    fontFamily: 'JetBrains Mono, ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
    theme: dark ? { background: '#00000000', foreground: '#e8edf5', cursor: '#8b7dff', selectionBackground: '#7667ff55' }
                : { background: '#00000000', foreground: '#0b1220', cursor: '#5b4fe0', selectionBackground: '#5b4fe033' },
  });
  S.fit = new FitAddon.FitAddon();
  S.term.loadAddon(S.fit);
  const host = $('#term'); host.innerHTML = '';
  S.term.open(host); S.fit.fit();

  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const sock = new WebSocket(`${proto}://${location.host}/api/workspaces/${S.ws.id}` +
    `/terminal/ws?cols=${S.term.cols}&rows=${S.term.rows}&tab=${S.termTab || 0}`);
  sock.binaryType = 'arraybuffer';
  S.termWs = sock;
  const tab = $('#panelbar .rtab[data-p="term"]');
  if (tab) tab.title = `${S.ws.node} · 会话常驻（断线重连回到原处）`;

  sock.onmessage = e => S.term?.write(e.data instanceof ArrayBuffer ? new Uint8Array(e.data) : e.data);
  sock.onclose = () => S.term?.write('\r\n\x1b[90m[已断开 —— 远端会话仍在运行，重开即可回到原处]\x1b[0m\r\n');
  S.term.onData(d => { if (sock.readyState === 1) sock.send(JSON.stringify({ type: 'input', data: d })); });

  const sync = () => {
    if (!S.fit || !S.term) return;
    try { S.fit.fit(); } catch (_) { return; }
    if (sock.readyState === 1) {
      sock.send(JSON.stringify({ type: 'resize', cols: S.term.cols, rows: S.term.rows }));
    }
  };
  S.termResize = sync;
  addEventListener('resize', sync);
}

function resetWorkspace() {
  closeTerm();
  for (const m of S.models.values()) { try { m.dispose(); } catch (_) {} }
  S.models.clear(); S.fileMeta.clear(); S.dirtyShown.clear();
  try { S.editor?.dispose(); } catch (_) {}
  S.editor = null;
  S.open = []; S.file = null; S.ws = null; S.session = null; S.viewSession = null; S.termTab = null;
  S.tabs = null; S.threads = null; S.threadSessions = new Set();
  S.tree = []; S.treeRoot = null; S.opened = new Set(); S.changeOf = new Map();
  S.seen = new Set();
}
